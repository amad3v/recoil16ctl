//! `recoil16ctl check`: automated installation and health checks.
//!
//! Everything here only reads, so it runs without sudo. Checks that need a
//! person (pressing keys, watching lights, suspending) are listed by
//! [`MANUAL`].

use std::{fmt, process::Command};

use anyhow::{Result, anyhow};

use crate::{
  battery, lightbar,
  locks::{self, Lock},
  profile, sensors,
  sys::Sys,
};

/// Modules the package installs, in `/sys/module` spelling.
const MODULES: [&str; 4] = [
  "uniwill_laptop",
  "ite8291_mono",
  "ite8233_lightbar",
  "copilot_rctrl",
];
/// First kernel the bundled uniwill-laptop builds for.
const UNIWILL_MIN_KERNEL: (u32, u32) = (7, 2);
const KDE_SHORTCUT: [&str; 2] = [
  "/usr/share/kglobalaccel/recoil16.desktop",
  "/usr/local/share/kglobalaccel/recoil16.desktop",
];

/// Things to try by hand after the automated checks.
pub const MANUAL: [&str; 6] = [
  "Fn+F6 / Fn+F7: keyboard brightness changes and KDE shows its indicator",
  "Mode button: light cycles green -> blue -> purple, KDE battery popup follows",
  "Sc: screen flips 180°, again flips back",
  "Copilot+C / Copilot+V: copy / paste (Copilot is Right Ctrl)",
  "Fn+F2 then Super: Super locked; Fn+F2 again unlocks it",
  "systemctl suspend, wake: keyboard, lightbar and mode light come back as before",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
  Pass,
  Warn,
  Fail,
}

pub struct Item {
  pub name: &'static str,
  pub level: Level,
  pub detail: String,
}

impl fmt::Display for Item {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let mark = match self.level {
      Level::Pass => "ok  ",
      Level::Warn => "warn",
      Level::Fail => "FAIL",
    };
    write!(f, "[{mark}] {:<14} {}", self.name, self.detail)
  }
}

/// Output of the external tools the checks read; `None` if a tool is missing.
struct Probes {
  dkms_status: Option<String>,
  powerprofilesctl: Option<String>,
}

impl Probes {
  fn collect(release: &str) -> Self {
    let output = |program: &str, args: &[&str]| {
      Command::new(program)
        .args(args)
        .output()
        .ok()
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    Self {
      dkms_status: output("dkms", &["status", "recoil16", "-k", release]),
      powerprofilesctl: output("powerprofilesctl", &[]),
    }
  }
}

/// Run every automated check.
pub fn run(sys: &Sys) -> Vec<Item> {
  let release = sys.read("/proc/sys/kernel/osrelease").unwrap_or_default();
  let probes = Probes::collect(&release);
  evaluate(sys, &release, &probes)
}

/// The checks themselves; pure apart from reading `sys`.
fn evaluate(sys: &Sys, release: &str, probes: &Probes) -> Vec<Item> {
  let full = kernel_at_least(release, UNIWILL_MIN_KERNEL);
  // On older kernels the uniwill-laptop features are expected to be missing.
  let uniwill = if full { Level::Fail } else { Level::Warn };

  vec![
    kernel(release, full),
    modules(sys, full),
    dkms(release, probes.dkms_status.as_deref()),
    item("battery", battery_check(sys), uniwill),
    item(
      "profile",
      profile::current(sys).map(|p| p.to_string()),
      uniwill,
    ),
    power_daemon(probes.powerprofilesctl.as_deref()),
    item("keyboard", keyboard(sys), Level::Fail),
    item(
      "lightbar",
      lightbar::current(sys).map(|s| s.to_string()),
      Level::Fail,
    ),
    copilot(sys),
    item("sensors", sensors::fans_and_temps(sys), uniwill),
    kde_shortcut(sys),
  ]
}

fn item(name: &'static str, result: Result<String>, on_error: Level) -> Item {
  match result {
    Ok(detail) => Item {
      name,
      level: Level::Pass,
      detail,
    },
    Err(e) => Item {
      name,
      level: on_error,
      detail: format!("{e:#}"),
    },
  }
}

fn warn(name: &'static str, detail: impl Into<String>) -> Item {
  Item {
    name,
    level: Level::Warn,
    detail: detail.into(),
  }
}

fn kernel(release: &str, full: bool) -> Item {
  if full {
    item("kernel", Ok(release.to_owned()), Level::Fail)
  } else {
    let (major, minor) = UNIWILL_MIN_KERNEL;
    warn(
      "kernel",
      format!(
        "{release}: older than {major}.{minor}, so no Fn keys, power profiles, charge limit or Sc"
      ),
    )
  }
}

fn modules(sys: &Sys, full: bool) -> Item {
  let missing: Vec<&str> = MODULES
    .into_iter()
    .filter(|m| !sys.path(&format!("/sys/module/{m}")).exists())
    .filter(|m| full || *m != "uniwill_laptop")
    .collect();
  if missing.is_empty() {
    item("modules", Ok("all loaded".to_owned()), Level::Fail)
  } else {
    item(
      "modules",
      Err(anyhow!("not loaded: {}", missing.join(", "))),
      Level::Fail,
    )
  }
}

fn dkms(release: &str, status: Option<&str>) -> Item {
  match status {
    Some(out) if dkms_installed(out) => {
      item("dkms", Ok(format!("installed for {release}")), Level::Fail)
    }
    Some(_) => warn(
      "dkms",
      format!("recoil16 not installed for {release} (modules loaded by hand?)"),
    ),
    None => warn("dkms", "dkms not found"),
  }
}

/// `dkms status` has an `installed` line for the package.
fn dkms_installed(status: &str) -> bool {
  status
    .lines()
    .any(|l| l.starts_with("recoil16/") && l.contains(": installed"))
}

fn battery_check(sys: &Sys) -> Result<String> {
  let st = battery::status(sys)?;
  match (st.limit, st.rule) {
    (None, _) => Err(anyhow!("charge limit not available")),
    (Some(limit), Some(rule)) if rule != limit => Err(anyhow!(
      "limit {limit}% but the boot rule sets {rule}% (run: sudo recoil16ctl battery limit {limit})"
    )),
    _ => Ok(st.to_string()),
  }
}

fn power_daemon(output: Option<&str>) -> Item {
  match output {
    Some(out) if ppd_uses_platform_profile(out) => item(
      "power daemon",
      Ok("power-profiles-daemon uses the platform profile".to_owned()),
      Level::Fail,
    ),
    Some(_) => warn(
      "power daemon",
      "power-profiles-daemon does not use platform_profile (restart it?)",
    ),
    None => warn("power daemon", "powerprofilesctl not found"),
  }
}

/// `powerprofilesctl` lists `PlatformDriver: platform_profile`.
fn ppd_uses_platform_profile(output: &str) -> bool {
  output.lines().any(|l| {
    l.trim()
      .strip_prefix("PlatformDriver:")
      .is_some_and(|d| d.trim() == "platform_profile")
  })
}

fn keyboard(sys: &Sys) -> Result<String> {
  let backlight = sensors::keyboard_backlight(sys)?;
  // the locks come from uniwill-laptop; report them if present
  match (
    locks::get(sys, Lock::FnLock),
    locks::get(sys, Lock::SuperKey),
  ) {
    (Ok(fn_lock), Ok(super_key)) => Ok(format!(
      "backlight {backlight}, fn lock {fn_lock}, super key {super_key}"
    )),
    _ => Ok(format!("backlight {backlight}")),
  }
}

fn copilot(sys: &Sys) -> Item {
  match sys.read_opt("/sys/module/copilot_rctrl/parameters/enabled") {
    Ok(Some(v)) if v == "Y" => item(
      "copilot",
      Ok("Copilot -> Right Ctrl".to_owned()),
      Level::Fail,
    ),
    Ok(Some(_)) => warn("copilot", "loaded but disabled (parameter enabled=N)"),
    Ok(None) => item(
      "copilot",
      Err(anyhow!("copilot-rctrl not loaded")),
      Level::Fail,
    ),
    Err(e) => item("copilot", Err(e), Level::Fail),
  }
}

fn kde_shortcut(sys: &Sys) -> Item {
  match KDE_SHORTCUT.into_iter().find(|p| sys.path(p).exists()) {
    Some(p) => item(
      "kde shortcut",
      Ok(format!("Sc -> screen rotate ({p})")),
      Level::Fail,
    ),
    None => warn(
      "kde shortcut",
      "recoil16.desktop not installed; Sc does nothing",
    ),
  }
}

/// `release` (e.g. "7.2.6-arch2-1") is at least `min` (major, minor).
fn kernel_at_least(release: &str, min: (u32, u32)) -> bool {
  let mut parts = release
    .split(|c: char| !c.is_ascii_digit())
    .map(|n| n.parse::<u32>().unwrap_or(0));
  let major = parts.next().unwrap_or(0);
  let minor = parts.next().unwrap_or(0);
  (major, minor) >= min
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::sys::testutil::{put, tempdir};

  #[test]
  fn kernel_versions() {
    assert!(kernel_at_least("7.2.6-arch2-1", (7, 2)));
    assert!(kernel_at_least("7.10.0", (7, 2)));
    assert!(kernel_at_least("8.0.1-zen1", (7, 2)));
    assert!(!kernel_at_least("6.18.53-1-lts", (7, 2)));
    assert!(!kernel_at_least("7.1.9", (7, 2)));
    assert!(!kernel_at_least("", (7, 2)));
  }

  /// Sample `dkms status` lines: the parser must not depend on the package or
  /// kernel version in them.
  #[test]
  fn parses_dkms_status_for_any_version() {
    for (package, kernel) in [
      ("1.0.0", "7.2.6-arch2-1"),
      ("2.3.4.r5.gabcdef0", "8.1.0-zen1"),
      ("1.0.0", "6.19.21-1-lts"),
    ] {
      let installed =
        format!("recoil16/{package}, {kernel}, x86_64: installed (Original modules exist)\n");
      assert!(dkms_installed(&installed), "{installed}");
      let built = format!("recoil16/{package}, {kernel}, x86_64: built\n");
      assert!(!dkms_installed(&built), "{built}");
    }
    assert!(!dkms_installed(""));
    assert!(!dkms_installed("other/1.0.0, 7.2.6, x86_64: installed\n"));
  }

  #[test]
  fn parses_powerprofilesctl() {
    let ok = "* balanced:\n    CpuDriver:\tamd_pstate\n    PlatformDriver:\tplatform_profile\n";
    assert!(ppd_uses_platform_profile(ok));
    let placeholder = "* balanced:\n    CpuDriver:\tamd_pstate\n    PlatformDriver:\tplaceholder\n";
    assert!(!ppd_uses_platform_profile(placeholder));
  }

  #[test]
  fn boot_rule_mismatch_is_reported() {
    let root = tempdir();
    let bat = "/sys/class/power_supply/BAT0";
    put(
      &root,
      &format!("{bat}/charge_control_end_threshold"),
      "80\n",
    );
    put(&root, &format!("{bat}/status"), "Charging\n");
    put(&root, &format!("{bat}/capacity"), "70\n");
    put(&root, &format!("{bat}/voltage_now"), "16000000\n");
    put(
      &root,
      "/etc/udev/rules.d/90-recoil16-charge-limit.rules",
      "RUN+=\"/bin/sh -c 'echo 90 > x'\"\n",
    );
    let err = battery_check(&Sys::with_root(&root)).unwrap_err();
    assert!(err.to_string().contains("boot rule sets 90%"));
  }

  #[test]
  fn older_kernel_turns_uniwill_failures_into_warnings() {
    let root = tempdir();
    for m in ["ite8291_mono", "ite8233_lightbar", "copilot_rctrl"] {
      std::fs::create_dir_all(root.join(format!("sys/module/{m}"))).unwrap();
    }
    let sys = Sys::with_root(&root);
    // sample output for an older kernel; the exact versions do not matter
    let release = "6.19.21-1-lts";
    let probes = Probes {
      dkms_status: Some(format!("recoil16/1.0.0, {release}, x86_64: installed\n")),
      powerprofilesctl: None,
    };
    let items = evaluate(&sys, release, &probes);
    let level = |name: &str| items.iter().find(|i| i.name == name).unwrap().level;

    assert_eq!(level("kernel"), Level::Warn);
    assert_eq!(level("modules"), Level::Pass, "uniwill_laptop not expected");
    assert_eq!(level("battery"), Level::Warn);
    assert_eq!(level("profile"), Level::Warn);
    assert_eq!(
      level("keyboard"),
      Level::Fail,
      "backlight missing is a real failure"
    );
    assert_eq!(level("dkms"), Level::Pass);
    assert_eq!(level("power daemon"), Level::Warn, "tool missing");
  }
}
