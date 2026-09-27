//! `recoil16ctl check`, power group: settings that decide whether the hybrid
//! graphics and the rest of the laptop reach low power at idle. Advice only:
//! items are ok or warn, never fail. The fixes are in the README section
//! "Power on the hybrid GPU".

use std::{path::Path, thread, time::Duration};

use crate::{
  check::{Item, ok, warn},
  gpu::{self, Gpu, PCI},
  sys::Sys,
};

const README: &str = "see README \"Power on the hybrid GPU\"";
const NOT_READABLE: &str = "power/control not readable";
const ICD: &str = "/usr/share/vulkan/icd.d";
/// PCI classes worth runtime-suspending: Ethernet, Wi-Fi, SD host, `NVMe`.
const PM_CLASSES: [(&str, &str); 4] = [
  ("0x0200", "ethernet"),
  ("0x0280", "wi-fi"),
  ("0x0805", "card reader"),
  ("0x0108", "nvme"),
];

/// All power items, in display order.
pub fn run(sys: &Sys) -> Vec<Item> {
  let mut items = Vec::new();
  match gpu::find(sys) {
    Ok(g) => {
      items.push(dgpu_runtime_pm(&g));
      items.push(audio_runtime_pm(&g));
      items.push(dynamic_pm(&g));
      items.push(dgpu_idle(sys, &g, || {
        thread::sleep(Duration::from_secs(3));
        gpu::find(sys)
          .ok()
          .and_then(|g| g.controller.status)
          .unwrap_or_default()
      }));
    }
    Err(e) => items.push(warn("dgpu", format!("{e:#}"))),
  }
  items.extend(early_loaded(sys));
  items.push(vulkan_igpu(sys));
  items.push(pci_runtime_pm(sys));
  items
}

fn dgpu_runtime_pm(g: &Gpu) -> Item {
  let c = &g.controller;
  match c.control.as_deref() {
    Some("auto") => ok(
      "dgpu runtime",
      format!("may suspend (now {})", c.shown_status()),
    ),
    Some(control) => warn(
      "dgpu runtime",
      format!(
        "power/control is {control}: the NVIDIA GPU never suspends (~9 W at idle); add the udev rule, {README}"
      ),
    ),
    None => warn("dgpu runtime", format!("{} {NOT_READABLE}", c.addr)),
  }
}

fn audio_runtime_pm(g: &Gpu) -> Item {
  let Some(a) = &g.audio else {
    return ok("dgpu audio", "no HDMI audio function");
  };
  match a.control.as_deref() {
    Some("auto") => ok(
      "dgpu audio",
      format!("may suspend (now {})", a.shown_status()),
    ),
    Some(control) => warn(
      "dgpu audio",
      format!(
        "{} power/control is {control}: it keeps the GPU from suspending; add the udev rule, {README}",
        a.addr
      ),
    ),
    None => warn("dgpu audio", format!("{} {NOT_READABLE}", a.addr)),
  }
}

fn dynamic_pm(g: &Gpu) -> Item {
  match g.dynamic_pm {
    Some(0) => warn(
      "nvidia dyn pm",
      format!(
        "DynamicPowerManagement=0: runtime D3 is off; set options nvidia NVreg_DynamicPowerManagement=0x02, {README}"
      ),
    ),
    Some(v) => ok(
      "nvidia dyn pm",
      format!(
        "DynamicPowerManagement={v}, {}",
        g.d3
          .as_deref()
          .map_or("D3 status not readable".into(), gpu::d3_text)
      ),
    ),
    None => warn("nvidia dyn pm", "DynamicPowerManagement not readable"),
  }
}

/// Active with none of the user's apps holding it, twice 3 s apart. A
/// process that only holds the NVIDIA card node (typically the compositor,
/// Xwayland, or `systemd-logind`, which hands the device to the session)
/// doesn't count: holding it doesn't keep the GPU awake. `again` re-reads
/// the runtime status (injected so tests don't sleep).
fn dgpu_idle(sys: &Sys, g: &Gpu, again: impl FnOnce() -> String) -> Item {
  match g.controller.status.as_deref() {
    Some("active") => {}
    Some(status) => return ok("dgpu idle", status),
    None => {
      return warn(
        "dgpu idle",
        format!("{} power/runtime_status not readable", g.controller.addr),
      );
    }
  }
  if gpu::users(sys, g).iter().any(|u| u.role == gpu::Role::App) {
    return ok("dgpu idle", "active: in use by your apps (recoil16ctl gpu)");
  }
  let second = again();
  if second == "active" {
    warn(
      "dgpu idle",
      "active with none of your apps using it; something keeps it awake \
       (brief wakes are normal, re-run in a minute): sudo recoil16ctl gpu",
    )
  } else {
    ok("dgpu idle", format!("{second} (was briefly active)"))
  }
}

/// `None` when there is no `/etc/mkinitcpio.conf` (other initramfs tools).
fn early_loaded(sys: &Sys) -> Option<Item> {
  let conf = sys.get("/etc/mkinitcpio.conf")?;
  let modules = conf
    .lines()
    .map(str::trim)
    .filter(|l| !l.starts_with('#'))
    .find_map(|l| l.strip_prefix("MODULES="))
    .unwrap_or_default();
  Some(if modules.contains("nvidia") {
    warn(
      "nvidia initrd",
      "nvidia in MODULES= of /etc/mkinitcpio.conf: ~110 MB of GPU firmware in every boot image, \
       and the integrated GPU drives the panel anyway; remove it and run sudo mkinitcpio -P",
    )
  } else {
    ok("nvidia initrd", "not early-loaded")
  })
}

fn vulkan_igpu(sys: &Sys) -> Item {
  if sys.list(ICD).iter().any(|f| {
    f.starts_with("radeon_icd")
      && Path::new(f)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
  }) {
    ok("vulkan igpu", "radeon (RADV) installed")
  } else {
    warn(
      "vulkan igpu",
      "no Vulkan driver for the AMD GPU: every Vulkan app runs on (and wakes) the NVIDIA GPU; \
       install vulkan-radeon",
    )
  }
}

fn pci_runtime_pm(sys: &Sys) -> Item {
  let stuck: Vec<String> = sys
    .list(PCI)
    .into_iter()
    .filter_map(|addr| {
      let class = gpu::pci_attr(sys, &addr, "class")?;
      let (_, kind) = PM_CLASSES.iter().find(|(p, _)| class.starts_with(p))?;
      (gpu::pci_attr(sys, &addr, "power/control").as_deref() == Some("on"))
        .then(|| format!("{kind} {addr}"))
    })
    .collect();
  if stuck.is_empty() {
    ok("pci runtime", "network, card reader and NVMe may suspend")
  } else {
    warn(
      "pci runtime",
      format!(
        "never suspend: {}; add the PCI runtime PM udev rule, {README}",
        stuck.join(", ")
      ),
    )
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::{
    check::Level,
    gpu::testutil::fake_gpu,
    sys::testutil::{link, put, tempdir},
  };

  fn gpu_in(status: &str, control: &str) -> (std::path::PathBuf, Gpu) {
    let root = tempdir();
    fake_gpu(&root, status, control);
    let g = gpu::find(&Sys::with_root(&root)).unwrap();
    (root, g)
  }

  #[test]
  fn runtime_pm_items() {
    let (_, auto) = gpu_in("suspended", "auto");
    assert_eq!(dgpu_runtime_pm(&auto).level, Level::Pass);
    assert_eq!(audio_runtime_pm(&auto).level, Level::Pass);
    assert_eq!(dynamic_pm(&auto).level, Level::Pass);
    let (_, on) = gpu_in("active", "on");
    let item = dgpu_runtime_pm(&on);
    assert_eq!(item.level, Level::Warn);
    assert!(item.detail.contains("never suspends"), "{}", item.detail);
  }

  #[test]
  fn unreadable_power_control_warns_with_the_reason() {
    let (root, _) = gpu_in("suspended", "auto");
    std::fs::remove_file(root.join("sys/bus/pci/devices/0000:01:00.0/power/control")).unwrap();
    let g = gpu::find(&Sys::with_root(&root)).unwrap();
    let item = dgpu_runtime_pm(&g);
    assert_eq!(item.level, Level::Warn);
    assert!(
      item.detail.contains("power/control not readable"),
      "{}",
      item.detail
    );
    assert!(!item.detail.contains("never suspends"), "{}", item.detail);
  }

  #[test]
  fn audio_function_items() {
    let (root, _) = gpu_in("suspended", "auto");
    put(
      &root,
      "/sys/bus/pci/devices/0000:01:00.1/power/control",
      "on\n",
    );
    let g = gpu::find(&Sys::with_root(&root)).unwrap();
    let item = audio_runtime_pm(&g);
    assert_eq!(item.level, Level::Warn);
    assert!(item.detail.contains("0000:01:00.1"), "{}", item.detail);
    std::fs::remove_dir_all(root.join("sys/bus/pci/devices/0000:01:00.1")).unwrap();
    let g = gpu::find(&Sys::with_root(&root)).unwrap();
    assert!(g.audio.is_none());
    assert_eq!(audio_runtime_pm(&g).level, Level::Pass);
  }

  #[test]
  fn dynamic_pm_off_warns() {
    let (root, _) = gpu_in("suspended", "auto");
    put(
      &root,
      "/proc/driver/nvidia/params",
      "DynamicPowerManagement: 0\n",
    );
    let g = gpu::find(&Sys::with_root(&root)).unwrap();
    assert_eq!(dynamic_pm(&g).level, Level::Warn);
  }

  #[test]
  fn dynamic_pm_ok_shows_formatted_d3_status() {
    let (_, g) = gpu_in("suspended", "auto");
    let item = dynamic_pm(&g);
    assert_eq!(item.level, Level::Pass);
    assert_eq!(item.detail, "DynamicPowerManagement=2, fine-grained D3");
  }

  #[test]
  fn dynamic_pm_ok_with_unreadable_d3() {
    let (root, _) = gpu_in("suspended", "auto");
    std::fs::remove_file(root.join("proc/driver/nvidia/gpus/0000:01:00.0/power")).unwrap();
    let g = gpu::find(&Sys::with_root(&root)).unwrap();
    let item = dynamic_pm(&g);
    assert_eq!(item.level, Level::Pass);
    assert_eq!(
      item.detail,
      "DynamicPowerManagement=2, D3 status not readable"
    );
  }

  #[test]
  fn idle_warns_only_when_active_twice_and_unused() {
    let (root, g) = gpu_in("active", "auto");
    let sys = Sys::with_root(&root);
    assert_eq!(dgpu_idle(&sys, &g, || "active".into()).level, Level::Warn);
    assert_eq!(
      dgpu_idle(&sys, &g, || "suspended".into()).level,
      Level::Pass
    );
    link(&root, "/proc/100/fd/3", "/dev/nvidia0");
    put(&root, "/proc/100/comm", "freecad\n");
    assert_eq!(dgpu_idle(&sys, &g, || "active".into()).level, Level::Pass);
    let (root2, asleep) = gpu_in("suspended", "auto");
    let never = || -> String { panic!("no second sample when suspended") };
    assert_eq!(
      dgpu_idle(&Sys::with_root(&root2), &asleep, never).level,
      Level::Pass
    );
  }

  #[test]
  fn idle_warns_when_only_a_display_server_holds_it() {
    let (root, g) = gpu_in("active", "auto");
    let sys = Sys::with_root(&root);
    link(
      &root,
      "/dev/dri/by-path/pci-0000:01:00.0-render",
      "../renderD129",
    );
    link(&root, "/dev/dri/by-path/pci-0000:01:00.0-card", "../card0");
    // kwin_wayland as measured: card node, render node and nvidia0 all open.
    // Holding the render/nvidia nodes too must not make it count as an app.
    link(&root, "/proc/1426/fd/7", "/dev/dri/card0");
    link(&root, "/proc/1426/fd/8", "/dev/dri/renderD129");
    link(&root, "/proc/1426/fd/9", "/dev/nvidia0");
    put(&root, "/proc/1426/comm", "kwin_wayland\n");
    // a display server holding the GPU must not make the item ok
    assert_eq!(dgpu_idle(&sys, &g, || "active".into()).level, Level::Warn);
  }

  #[test]
  fn early_loading_in_mkinitcpio() {
    let root = tempdir();
    let sys = Sys::with_root(&root);
    assert!(early_loaded(&sys).is_none());
    put(
      &root,
      "/etc/mkinitcpio.conf",
      "#MODULES=(nvidia)\nMODULES=()\n",
    );
    assert_eq!(early_loaded(&sys).unwrap().level, Level::Pass);
    put(
      &root,
      "/etc/mkinitcpio.conf",
      "MODULES=(nvidia nvidia_modeset nvidia_uvm nvidia_drm)\n",
    );
    assert_eq!(early_loaded(&sys).unwrap().level, Level::Warn);
  }

  #[test]
  fn vulkan_driver_for_the_integrated_gpu() {
    let root = tempdir();
    put(&root, "/usr/share/vulkan/icd.d/nvidia_icd.json", "{}");
    let sys = Sys::with_root(&root);
    assert_eq!(vulkan_igpu(&sys).level, Level::Warn);
    put(&root, "/usr/share/vulkan/icd.d/radeon_icd.json", "{}");
    assert_eq!(vulkan_igpu(&sys).level, Level::Pass);
  }

  #[test]
  fn pci_devices_that_never_suspend_are_listed() {
    let root = tempdir();
    let dev = |addr: &str, class: &str, control: &str| {
      put(
        &root,
        &format!("/sys/bus/pci/devices/{addr}/class"),
        &format!("{class}\n"),
      );
      put(
        &root,
        &format!("/sys/bus/pci/devices/{addr}/power/control"),
        &format!("{control}\n"),
      );
    };
    dev("0000:03:00.0", "0x028000", "auto");
    dev("0000:05:00.0", "0x020000", "auto");
    dev("0000:00:00.0", "0x060000", "on"); // host bridge: not our business
    let sys = Sys::with_root(&root);
    assert_eq!(pci_runtime_pm(&sys).level, Level::Pass);
    dev("0000:06:00.0", "0x080501", "on");
    let item = pci_runtime_pm(&sys);
    assert_eq!(item.level, Level::Warn);
    assert!(
      item.detail.contains("card reader 0000:06:00.0"),
      "{}",
      item.detail
    );
  }
}
