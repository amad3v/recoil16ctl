//! `recoil16ctl gpu test`: wake the NVIDIA GPU on purpose and check that
//! offloaded apps land on it and other apps don't, for Vulkan and `OpenGL`.
//!
//! Needs the desktop session: `OpenGL` is tested on the Wayland display, the way
//! real apps are routed. EGL without a display can't prove anything: glvnd
//! tries the NVIDIA vendor first there, with or without the offload variables
//! (measured on the Recoil 16).

use std::{env, ffi::OsStr, process::Command};

use anyhow::{Result, bail};

use crate::{
  gpu::{self, OFFLOAD_ENV},
  sys::Sys,
};

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
  Pass(String),
  Fail(String),
  Skipped(String),
}

/// One test: a tool run twice, without and with the offload variables.
struct Probe {
  name: &'static str,
  tool: &'static str,
  args: &'static [&'static str],
  package: &'static str,
  judge: fn(&str, &str) -> Verdict,
}

const PROBES: [Probe; 2] = [
  Probe {
    name: "vulkan",
    tool: "vulkaninfo",
    args: &["--summary"],
    package: "vulkan-tools",
    judge: vulkan_verdict,
  },
  Probe {
    name: "opengl",
    tool: "eglinfo",
    args: &["-B", "-p", "wayland"],
    package: "mesa-utils",
    judge: opengl_verdict,
  },
];

/// Run every test on the NVIDIA GPU, in the desktop session.
pub fn run() -> Result<Vec<(&'static str, Verdict)>> {
  preflight(&Sys::new(), env::var_os("WAYLAND_DISPLAY").as_deref())?;
  Ok(PROBES.iter().map(|p| (p.name, p.run())).collect())
}

/// The NVIDIA GPU exists and this runs in the Wayland session (`wayland_display`
/// is `WAYLAND_DISPLAY`, which sudo, SSH and X11 sessions don't have).
fn preflight(sys: &Sys, wayland_display: Option<&OsStr>) -> Result<()> {
  gpu::find(sys)?;
  if wayland_display.is_none() {
    bail!(
      "needs your Wayland desktop session: run it as your user in a terminal on the desktop, \
       not with sudo or over SSH"
    );
  }
  Ok(())
}

/// Fails if no test could run (every tool missing), naming what to install.
pub fn ensure_ran(results: &[(&str, Verdict)]) -> Result<()> {
  if results
    .iter()
    .all(|(_, v)| matches!(v, Verdict::Skipped(_)))
  {
    let packages: Vec<&str> = PROBES.iter().map(|p| p.package).collect();
    bail!(
      "nothing could be tested: install {}",
      packages.join(" and ")
    );
  }
  Ok(())
}

impl Probe {
  fn run(&self) -> Verdict {
    let output = |offload: bool| -> Option<String> {
      let mut cmd = Command::new(self.tool);
      cmd.args(self.args);
      for (k, v) in OFFLOAD_ENV {
        if offload {
          cmd.env(k, v);
        } else {
          cmd.env_remove(k);
        }
      }
      cmd
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
    };
    match (output(false), output(true)) {
      (Some(plain), Some(offload)) => (self.judge)(&plain, &offload),
      _ => Verdict::Skipped(format!(
        "{} not found (install {})",
        self.tool, self.package
      )),
    }
  }
}

/// `vulkaninfo --summary`: another GPU visible by default, only NVIDIA offloaded.
/// The CPU device (lavapipe) doesn't count as another GPU: apps pick a real one.
fn vulkan_verdict(plain: &str, offload: &str) -> Verdict {
  let plain = values(plain, "deviceName");
  let offload = values(offload, "deviceName");
  if plain.is_empty() {
    return Verdict::Fail(
      "no Vulkan device listed: the Vulkan loader or drivers are missing or broken".to_owned(),
    );
  }
  let plain: Vec<String> = plain.into_iter().filter(|d| !is_cpu(d)).collect();
  if !plain.iter().any(|d| !is_nvidia(d)) {
    return Verdict::Fail(format!(
      "only NVIDIA is visible by default ({}): every Vulkan app wakes it; install vulkan-radeon",
      plain.join(", ")
    ));
  }
  if offload.is_empty() || !offload.iter().all(|d| is_nvidia(d)) {
    return Verdict::Fail(format!(
      "offloaded apps see: {}",
      if offload.is_empty() {
        "no GPU".to_owned()
      } else {
        offload.join(", ")
      }
    ));
  }
  Verdict::Pass(format!(
    "default {}; offloaded {}",
    plain.join(" + "),
    offload.join(", ")
  ))
}

/// `eglinfo -B`: the default renderer isn't NVIDIA, the offloaded one is.
fn opengl_verdict(plain: &str, offload: &str) -> Verdict {
  let renderer = |out: &str| {
    values(out, "OpenGL core profile renderer")
      .into_iter()
      .next()
  };
  match (renderer(plain), renderer(offload)) {
    (Some(p), Some(o)) if !is_nvidia(&p) && is_nvidia(&o) => {
      Verdict::Pass(format!("default {p}, offloaded {o}"))
    }
    (Some(p), Some(o)) => Verdict::Fail(format!("default {p}, offloaded {o}")),
    _ => Verdict::Fail("no OpenGL renderer reported".to_owned()),
  }
}

/// Values of `key = value` / `key: value` lines.
fn values(text: &str, key: &str) -> Vec<String> {
  text
    .lines()
    .filter_map(|l| {
      let rest = l.trim().strip_prefix(key)?.trim_start();
      let value = rest.strip_prefix('=').or_else(|| rest.strip_prefix(':'))?;
      Some(value.trim().to_owned())
    })
    .collect()
}

fn is_nvidia(name: &str) -> bool {
  name.contains("NVIDIA")
}

/// Mesa's software rasteriser (lavapipe's Vulkan device is named after it).
fn is_cpu(name: &str) -> bool {
  name.contains("llvmpipe")
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::{gpu::testutil::fake_gpu, sys::testutil::tempdir};

  // `vulkaninfo --summary` / `eglinfo -B -p wayland` on the Recoil 16 (trimmed)
  const VK_BOTH: &str = "Devices:\n========\nGPU0:\n\tdeviceName         = AMD Ryzen 9 9955HX3D 16-Core Processor (RADV RAPHAEL_MENDOCINO)\nGPU1:\n\tdeviceName         = NVIDIA GeForce RTX 5070 Ti Laptop GPU\n";
  const VK_NVIDIA: &str =
    "Devices:\n========\nGPU0:\n\tdeviceName         = NVIDIA GeForce RTX 5070 Ti Laptop GPU\n";
  const LLVMPIPE: &str = "llvmpipe (LLVM 21.1.8, 256 bits)";
  const EGL_AMD: &str = "EGL vendor string: Mesa Project\nOpenGL core profile renderer: AMD Ryzen 9 9955HX3D 16-Core Processor (radeonsi, raphael_mendocino, ACO)\nOpenGL compatibility profile renderer: AMD Ryzen 9 9955HX3D\n";
  const EGL_NVIDIA: &str = "EGL vendor string: NVIDIA\nOpenGL core profile renderer: NVIDIA GeForce RTX 5070 Ti Laptop GPU/PCIe/SSE2\n";

  #[test]
  fn needs_the_nvidia_gpu_then_the_wayland_session() {
    let root = tempdir();
    let err = preflight(&Sys::with_root(&root), Some("wayland-0".as_ref())).unwrap_err();
    assert_eq!(err.to_string(), "no NVIDIA GPU found");
    fake_gpu(&root, "suspended", "auto");
    let sys = Sys::with_root(&root);
    let err = preflight(&sys, None).unwrap_err().to_string();
    assert!(err.contains("Wayland desktop session"), "{err}");
    assert!(err.contains("not with sudo"), "{err}");
    assert!(preflight(&sys, Some("wayland-0".as_ref())).is_ok());
  }

  #[test]
  fn fails_when_no_probe_could_run() {
    let skipped = |t: &str| Verdict::Skipped(t.to_owned());
    let err = ensure_ran(&[("vulkan", skipped("x")), ("opengl", skipped("y"))])
      .unwrap_err()
      .to_string();
    assert!(
      err.contains("vulkan-tools") && err.contains("mesa-utils"),
      "{err}"
    );
    assert!(
      ensure_ran(&[
        ("vulkan", Verdict::Pass("x".into())),
        ("opengl", skipped("y"))
      ])
      .is_ok()
    );
  }

  #[test]
  fn vulkan_passes_when_offload_selects_only_nvidia() {
    assert!(matches!(
      vulkan_verdict(VK_BOTH, VK_NVIDIA),
      Verdict::Pass(_)
    ));
  }

  #[test]
  fn vulkan_fails_without_an_igpu_driver() {
    let Verdict::Fail(msg) = vulkan_verdict(VK_NVIDIA, VK_NVIDIA) else {
      panic!("expected Fail");
    };
    assert!(msg.contains("vulkan-radeon"), "{msg}");
  }

  #[test]
  fn vulkan_fails_when_offload_still_sees_amd() {
    assert!(matches!(vulkan_verdict(VK_BOTH, VK_BOTH), Verdict::Fail(_)));
  }

  #[test]
  fn vulkan_ignores_the_cpu_device() {
    let lavapipe = format!("{VK_NVIDIA}GPU1:\n\tdeviceName         = {LLVMPIPE}\n");
    let Verdict::Fail(msg) = vulkan_verdict(&lavapipe, VK_NVIDIA) else {
      panic!("expected Fail");
    };
    assert!(msg.contains("vulkan-radeon"), "{msg}");
    let all = format!("{VK_BOTH}GPU2:\n\tdeviceName         = {LLVMPIPE}\n");
    assert!(matches!(vulkan_verdict(&all, VK_NVIDIA), Verdict::Pass(_)));
  }

  #[test]
  fn vulkan_fails_when_no_device_is_listed() {
    let Verdict::Fail(msg) = vulkan_verdict("", VK_NVIDIA) else {
      panic!("expected Fail");
    };
    assert!(msg.contains("no Vulkan device"), "{msg}");
  }

  #[test]
  fn opengl_passes_amd_by_default_nvidia_offloaded() {
    assert_eq!(
      opengl_verdict(EGL_AMD, EGL_NVIDIA),
      Verdict::Pass(
        "default AMD Ryzen 9 9955HX3D 16-Core Processor (radeonsi, raphael_mendocino, ACO), \
         offloaded NVIDIA GeForce RTX 5070 Ti Laptop GPU/PCIe/SSE2"
          .into()
      )
    );
  }

  #[test]
  fn opengl_fails_when_offload_is_ignored_or_nothing_is_reported() {
    assert!(matches!(opengl_verdict(EGL_AMD, EGL_AMD), Verdict::Fail(_)));
    assert!(matches!(opengl_verdict("", ""), Verdict::Fail(_)));
  }
}
