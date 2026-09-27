//! The NVIDIA GPU of the Recoil 16: runtime power state, driver settings, which
//! processes use it, and PRIME render offload.
//!
//! Every file read here was checked on the Recoil 16 not to wake the GPU
//! (`power/runtime_active_time` unchanged across the reads). Nothing opens
//! `/dev/nvidia*` or runs `nvidia-smi`, which would.

use std::fmt;

use anyhow::{Result, bail};

use crate::sys::Sys;

pub const PCI: &str = "/sys/bus/pci/devices";
const NVIDIA: &str = "0x10de";
const NV_PROC: &str = "/proc/driver/nvidia";
const UNREADABLE: &str = "unreadable";

/// Environment for PRIME render offload to the NVIDIA GPU (`OpenGL` via GLX/EGL,
/// and Vulkan); the set switcheroo-control uses.
pub const OFFLOAD_ENV: [(&str, &str); 4] = [
  ("__NV_PRIME_RENDER_OFFLOAD", "1"),
  ("__GLX_VENDOR_LIBRARY_NAME", "nvidia"),
  ("__VK_LAYER_NV_optimus", "NVIDIA_only"),
  ("VK_LOADER_DRIVERS_SELECT", "*nvidia*"),
];

/// Runtime power state of one PCI function; `None` where the attribute
/// couldn't be read.
#[derive(Debug, PartialEq, Eq)]
pub struct Power {
  pub addr: String,
  /// `power/runtime_status`: suspended, active, suspending, resuming, ...
  pub status: Option<String>,
  /// `power/control`: `auto` (may suspend) or `on` (never suspends).
  pub control: Option<String>,
}

impl Power {
  /// `power/runtime_status` for display: "unreadable" if it couldn't be read.
  pub fn shown_status(&self) -> &str {
    self.status.as_deref().unwrap_or(UNREADABLE)
  }

  /// `power/control` for display: "unreadable" if it couldn't be read.
  pub fn shown_control(&self) -> &str {
    self.control.as_deref().unwrap_or(UNREADABLE)
  }
}

/// "suspended (runtime PM auto)"
impl fmt::Display for Power {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(
      f,
      "{} (runtime PM {})",
      self.shown_status(),
      self.shown_control()
    )
  }
}

/// The NVIDIA display controller, its HDMI audio function and driver settings.
#[derive(Debug)]
pub struct Gpu {
  /// Power state of the display controller itself (as opposed to its audio function).
  pub controller: Power,
  pub audio: Option<Power>,
  pub model: Option<String>,
  pub driver_version: Option<String>,
  /// Driver's `Runtime D3 status`, e.g. "Enabled (fine-grained)".
  pub d3: Option<String>,
  /// The `DynamicPowerManagement` module parameter.
  pub dynamic_pm: Option<u32>,
}

impl Gpu {
  /// One line for `recoil16ctl` status: "suspended (runtime PM auto)".
  pub fn summary(&self) -> String {
    self.controller.to_string()
  }
}

impl fmt::Display for Gpu {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let c = &self.controller;
    let d3 = self
      .d3
      .as_deref()
      .map(|d| format!(", {}", d3_text(d)))
      .unwrap_or_default();
    let model = self.model.as_deref().unwrap_or("NVIDIA GPU");
    write!(
      f,
      "{}",
      row(
        "dgpu",
        format_args!(
          "{} (runtime PM {}{d3})  {model} {}",
          c.shown_status(),
          c.shown_control(),
          c.addr
        )
      )
    )?;
    if let Some(a) = &self.audio {
      write!(f, "\n{}", row("audio fn", format_args!("{a}  {}", a.addr)))?;
    }
    let version = self.driver_version.as_deref().unwrap_or("?");
    let dynamic_pm = self
      .dynamic_pm
      .map(|d| format!(", DynamicPowerManagement={d}"))
      .unwrap_or_default();
    write!(
      f,
      "\n{}",
      row("driver", format_args!("nvidia {version}{dynamic_pm}"))
    )
  }
}

/// The driver's `Runtime D3 status` in words: "Enabled (fine-grained)" ->
/// "fine-grained D3", anything else -> "D3 <status>" (lower case).
pub(crate) fn d3_text(d3: &str) -> String {
  d3.strip_prefix("Enabled (")
    .and_then(|grain| grain.strip_suffix(')'))
    .map_or_else(
      || format!("D3 {}", d3.to_lowercase()),
      |grain| format!("{grain} D3"),
    )
}

/// One row of the `gpu` and `power` reports: `label` in a 12-column field, then `value`.
pub fn row(label: &str, value: impl fmt::Display) -> String {
  format!("{label:<12}{value}")
}

/// Find the NVIDIA GPU (vendor `0x10de`, display class `0x03xxxx`) and read its state.
pub fn find(sys: &Sys) -> Result<Gpu> {
  let devices = sys.list(PCI);
  let Some(addr) = devices.iter().find(|a| is_nvidia(sys, a, "0x03")) else {
    bail!("no NVIDIA GPU found");
  };
  let driver = sys.read_link(&format!("{PCI}/{addr}/driver"));
  if !driver.is_some_and(|d| d.ends_with("/nvidia")) {
    bail!("nvidia driver not loaded for {addr}");
  }
  // the HDMI audio function sits in the same slot: 0000:01:00.0 -> 0000:01:00.1
  let slot = addr.rsplit_once('.').map_or(addr.as_str(), |(s, _)| s);
  let audio = devices
    .iter()
    .find(|a| a.starts_with(&format!("{slot}.")) && is_nvidia(sys, a, "0x0403"))
    .map(|a| power(sys, a));
  let nv = format!("{NV_PROC}/gpus/{addr}");
  Ok(Gpu {
    controller: power(sys, addr),
    audio,
    model: sys
      .get(&format!("{nv}/information"))
      .and_then(|t| field(&t, "Model")),
    driver_version: sys.get("/sys/module/nvidia/version"),
    d3: sys
      .get(&format!("{nv}/power"))
      .and_then(|t| field(&t, "Runtime D3 status")),
    dynamic_pm: sys
      .get(&format!("{NV_PROC}/params"))
      .and_then(|t| field(&t, "DynamicPowerManagement"))
      .and_then(|v| v.parse().ok()),
  })
}

/// Attribute `attr` (e.g. `class`, `power/control`) of the PCI function at `addr`;
/// `None` if it can't be read.
pub fn pci_attr(sys: &Sys, addr: &str, attr: &str) -> Option<String> {
  sys.get(&format!("{PCI}/{addr}/{attr}"))
}

/// An NVIDIA PCI function whose class starts with `class` ("0x03" display, "0x0403" audio).
fn is_nvidia(sys: &Sys, addr: &str, class: &str) -> bool {
  pci_attr(sys, addr, "vendor").as_deref() == Some(NVIDIA)
    && pci_attr(sys, addr, "class").is_some_and(|c| c.starts_with(class))
}

fn power(sys: &Sys, addr: &str) -> Power {
  Power {
    addr: addr.to_owned(),
    status: pci_attr(sys, addr, "power/runtime_status"),
    control: pci_attr(sys, addr, "power/control"),
  }
}

/// Value of the `key: value` line in `text`.
fn field(text: &str, key: &str) -> Option<String> {
  text.lines().find_map(|l| {
    let (k, v) = l.split_once(':')?;
    (k.trim() == key).then(|| v.trim().to_owned())
  })
}

/// Role implied by which NVIDIA device file a process holds open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
  /// Holds the NVIDIA GPU's DRM card node: typically the compositor,
  /// Xwayland, or `systemd-logind` (which hands the device to the
  /// session). None of them keep the GPU awake by holding it, and holding
  /// it doesn't mean rendering anything on it.
  Display,
  /// Holds `/dev/nvidiactl`, `/dev/nvidia<N>` or the GPU's DRM render node
  /// (and not the card node): it uses the NVIDIA GPU.
  App,
}

/// A process with one of the GPU's device files open.
#[derive(Debug, PartialEq, Eq)]
pub struct User {
  pub pid: u32,
  pub name: String,
  pub role: Role,
}

/// Processes holding the NVIDIA GPU open, classified by which device node:
/// holding the GPU's DRM card node (via `/dev/dri/by-path`, resolved the same
/// way as the render node) makes a process [`Role::Display`] — typically the
/// compositor, Xwayland, or `systemd-logind` (which hands the device to the
/// session), whether or not it also holds other NVIDIA device files. Holding
/// `/dev/nvidiactl`, `/dev/nvidia<N>` or the GPU's DRM render node without
/// the card node makes it [`Role::App`]. Only processes whose
/// `/proc/<pid>/fd` is readable are seen (the caller's own; all of them
/// with sudo). Reading fds doesn't touch the GPU.
pub fn users(sys: &Sys, gpu: &Gpu) -> Vec<User> {
  let addr = &gpu.controller.addr;
  let render = dri_node(sys, addr, "render");
  let card = dri_node(sys, addr, "card");
  let mut found: Vec<User> = sys
    .list("/proc")
    .into_iter()
    .filter_map(|p| p.parse::<u32>().ok())
    .filter_map(|pid| {
      let fd = format!("/proc/{pid}/fd");
      let mut role = None;
      for n in sys.list(&fd) {
        let Some(target) = sys.read_link(&format!("{fd}/{n}")) else {
          continue;
        };
        match node_role(&target, card.as_deref(), render.as_deref()) {
          Some(Role::Display) => role = Some(Role::Display),
          Some(Role::App) if role.is_none() => role = Some(Role::App),
          _ => {}
        }
      }
      role.map(|role| User {
        pid,
        name: sys.get(&format!("/proc/{pid}/comm")).unwrap_or_default(),
        role,
      })
    })
    .collect();
  found.sort_by_key(|u| u.pid);
  found
}

/// The GPU's DRM node of `kind` ("render" or "card") as an absolute path, via
/// `/dev/dri/by-path` (render/card numbers change between boots, the PCI
/// address doesn't).
fn dri_node(sys: &Sys, addr: &str, kind: &str) -> Option<String> {
  sys
    .read_link(&format!("/dev/dri/by-path/pci-{addr}-{kind}"))
    .map(|t| resolve("/dev/dri/by-path", &t))
}

/// The role implied by one open device-file `target`, or `None` if it isn't
/// one of the GPU's device files. The card node wins over anything else the
/// same fd could match, since it alone means [`Role::Display`].
fn node_role(target: &str, card: Option<&str>, render: Option<&str>) -> Option<Role> {
  if card == Some(target) {
    Some(Role::Display)
  } else if is_gpu_node(target, render) {
    Some(Role::App)
  } else {
    None
  }
}

fn is_gpu_node(target: &str, render: Option<&str>) -> bool {
  target == "/dev/nvidiactl"
    || target
      .strip_prefix("/dev/nvidia")
      .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    || render == Some(target)
}

/// `target` of a symlink located in `dir`, as a normalised absolute path.
fn resolve(dir: &str, target: &str) -> String {
  if target.starts_with('/') {
    return target.to_owned();
  }
  let mut parts: Vec<&str> = dir.split('/').filter(|p| !p.is_empty()).collect();
  for part in target.split('/') {
    match part {
      "" | "." => {}
      ".." => {
        parts.pop();
      }
      p => parts.push(p),
    }
  }
  format!("/{}", parts.join("/"))
}

#[cfg(test)]
pub mod testutil {
  use std::path::Path;

  use crate::sys::testutil::{link, put};

  /// The Recoil 16's GPUs as fake sysfs/proc: NVIDIA at 0000:01:00.0 (+ audio .1)
  /// in the given runtime `status` and `control`, and the AMD one at 0000:07:00.0.
  pub fn fake_gpu(root: &Path, status: &str, control: &str) {
    let d = "/sys/bus/pci/devices/0000:01:00.0";
    put(root, &format!("{d}/vendor"), "0x10de\n");
    put(root, &format!("{d}/class"), "0x030000\n");
    put(
      root,
      &format!("{d}/power/runtime_status"),
      &format!("{status}\n"),
    );
    put(root, &format!("{d}/power/control"), &format!("{control}\n"));
    link(
      root,
      &format!("{d}/driver"),
      "../../../bus/pci/drivers/nvidia",
    );
    let a = "/sys/bus/pci/devices/0000:01:00.1";
    put(root, &format!("{a}/vendor"), "0x10de\n");
    put(root, &format!("{a}/class"), "0x040300\n");
    put(root, &format!("{a}/power/runtime_status"), "suspended\n");
    put(root, &format!("{a}/power/control"), "auto\n");
    let i = "/sys/bus/pci/devices/0000:07:00.0";
    put(root, &format!("{i}/vendor"), "0x1002\n");
    put(root, &format!("{i}/class"), "0x030000\n");
    let p = "/proc/driver/nvidia/gpus/0000:01:00.0";
    put(
      root,
      &format!("{p}/information"),
      "Model: \t\t NVIDIA GeForce RTX 5070 Ti Laptop GPU\nIRQ:   \t\t 162\n",
    );
    put(
      root,
      &format!("{p}/power"),
      "Runtime D3 status:          Enabled (fine-grained)\nVideo Memory:               Off\n",
    );
    put(
      root,
      "/proc/driver/nvidia/params",
      "ResmanDebugLevel: 4294967295\nDynamicPowerManagement: 2\n",
    );
    put(root, "/sys/module/nvidia/version", "615.71.09\n");
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::sys::testutil::{put, tempdir};
  use testutil::fake_gpu;

  #[test]
  fn finds_the_nvidia_gpu_and_its_audio_function() {
    let root = tempdir();
    fake_gpu(&root, "suspended", "auto");
    let g = find(&Sys::with_root(&root)).unwrap();
    assert_eq!(
      g.controller,
      Power {
        addr: "0000:01:00.0".into(),
        status: Some("suspended".into()),
        control: Some("auto".into())
      }
    );
    assert_eq!(g.audio.as_ref().unwrap().addr, "0000:01:00.1");
    assert_eq!(
      g.model.as_deref(),
      Some("NVIDIA GeForce RTX 5070 Ti Laptop GPU")
    );
    assert_eq!(g.driver_version.as_deref(), Some("615.71.09"));
    assert_eq!(g.d3.as_deref(), Some("Enabled (fine-grained)"));
    assert_eq!(g.dynamic_pm, Some(2));
  }

  #[test]
  fn shows_state_driver_and_summary() {
    let root = tempdir();
    fake_gpu(&root, "active", "on");
    let g = find(&Sys::with_root(&root)).unwrap();
    assert_eq!(g.summary(), "active (runtime PM on)");
    assert_eq!(
      g.to_string(),
      "dgpu        active (runtime PM on, fine-grained D3)  \
       NVIDIA GeForce RTX 5070 Ti Laptop GPU 0000:01:00.0\n\
       audio fn    suspended (runtime PM auto)  0000:01:00.1\n\
       driver      nvidia 615.71.09, DynamicPowerManagement=2"
    );
  }

  #[test]
  fn unreadable_power_attributes_are_shown_as_such() {
    let root = tempdir();
    fake_gpu(&root, "active", "on");
    let d = root.join("sys/bus/pci/devices/0000:01:00.0/power");
    std::fs::remove_file(d.join("control")).unwrap();
    std::fs::remove_file(d.join("runtime_status")).unwrap();
    let g = find(&Sys::with_root(&root)).unwrap();
    assert_eq!(g.controller.status, None);
    assert_eq!(g.controller.control, None);
    assert_eq!(g.summary(), "unreadable (runtime PM unreadable)");
  }

  #[test]
  fn d3_status_without_nested_parentheses() {
    assert_eq!(d3_text("Enabled (fine-grained)"), "fine-grained D3");
    assert_eq!(d3_text("Not supported"), "D3 not supported");
  }

  #[test]
  fn no_nvidia_gpu_is_an_error() {
    let root = tempdir();
    put(
      &root,
      "/sys/bus/pci/devices/0000:07:00.0/vendor",
      "0x1002\n",
    );
    put(
      &root,
      "/sys/bus/pci/devices/0000:07:00.0/class",
      "0x030000\n",
    );
    let err = find(&Sys::with_root(&root)).unwrap_err();
    assert_eq!(err.to_string(), "no NVIDIA GPU found");
  }

  #[test]
  fn other_driver_is_an_error() {
    let root = tempdir();
    fake_gpu(&root, "active", "on");
    let d = root.join("sys/bus/pci/devices/0000:01:00.0/driver");
    std::fs::remove_file(&d).unwrap();
    std::os::unix::fs::symlink("../../../bus/pci/drivers/nouveau", d).unwrap();
    let err = find(&Sys::with_root(&root)).unwrap_err();
    assert_eq!(err.to_string(), "nvidia driver not loaded for 0000:01:00.0");
  }

  #[test]
  fn users_are_processes_with_nvidia_device_files_open() {
    use crate::sys::testutil::link;
    let root = tempdir();
    fake_gpu(&root, "active", "auto");
    link(
      &root,
      "/dev/dri/by-path/pci-0000:01:00.0-render",
      "../renderD129",
    );
    link(&root, "/dev/dri/by-path/pci-0000:01:00.0-card", "../card0");
    // offloaded app: /dev/nvidia0
    link(&root, "/proc/100/fd/3", "/dev/nvidia0");
    put(&root, "/proc/100/comm", "freecad\n");
    // Vulkan app on the NVIDIA render node
    link(&root, "/proc/200/fd/7", "/dev/dri/renderD129");
    put(&root, "/proc/200/comm", "vkcube\n");
    // app on the AMD render node: not a user
    link(&root, "/proc/300/fd/7", "/dev/dri/renderD128");
    put(&root, "/proc/300/comm", "firefox\n");
    // only nvidia-uvm open: not counted
    link(&root, "/proc/400/fd/1", "/dev/nvidia-uvm");
    put(&root, "/proc/400/comm", "cudaprobe\n");
    // compositor holding only the NVIDIA card node: a display server
    link(&root, "/proc/250/fd/9", "/dev/dri/card0");
    put(&root, "/proc/250/comm", "kwin_wayland\n");
    // compositor holding the card node plus nvidia0 and the render node
    // (like the real kwin_wayland): still a display server, card node wins
    link(&root, "/proc/260/fd/1", "/dev/dri/card0");
    link(&root, "/proc/260/fd/2", "/dev/nvidia0");
    link(&root, "/proc/260/fd/3", "/dev/dri/renderD129");
    put(&root, "/proc/260/comm", "kwin_wayland\n");
    // app holding both the render node and nvidia0, no card node: an app
    link(&root, "/proc/270/fd/1", "/dev/dri/renderD129");
    link(&root, "/proc/270/fd/2", "/dev/nvidia0");
    put(&root, "/proc/270/comm", "some_app\n");
    // systemd-logind: also holds only the card node (hands the device to
    // the session), classified the same as any other card-node holder -
    // no process-name special-casing
    link(&root, "/proc/912/fd/3", "/dev/dri/card0");
    put(&root, "/proc/912/comm", "systemd-logind\n");
    // a process without a readable fd dir, and a non-pid entry
    put(&root, "/proc/500/comm", "kwin_wayland\n");
    put(&root, "/proc/self/comm", "x\n");
    let sys = Sys::with_root(&root);
    let g = find(&sys).unwrap();
    assert_eq!(
      users(&sys, &g),
      [
        User {
          pid: 100,
          name: "freecad".into(),
          role: Role::App,
        },
        User {
          pid: 200,
          name: "vkcube".into(),
          role: Role::App,
        },
        User {
          pid: 250,
          name: "kwin_wayland".into(),
          role: Role::Display,
        },
        User {
          pid: 260,
          name: "kwin_wayland".into(),
          role: Role::Display,
        },
        User {
          pid: 270,
          name: "some_app".into(),
          role: Role::App,
        },
        User {
          pid: 912,
          name: "systemd-logind".into(),
          role: Role::Display,
        },
      ]
    );
  }

  #[test]
  fn resolves_relative_links() {
    assert_eq!(
      resolve("/dev/dri/by-path", "../renderD129"),
      "/dev/dri/renderD129"
    );
    assert_eq!(resolve("/dev/dri/by-path", "/dev/x"), "/dev/x");
  }
}
