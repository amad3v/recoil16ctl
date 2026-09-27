//! Battery state, true health (from the EC via uniwill-laptop) and charge modes.

use std::fmt;

use anyhow::Result;
use clap::ValueEnum;

use crate::sys::Sys;

const BAT: &str = "/sys/class/power_supply/BAT0";
const CHARGE_TYPES: &str = "/sys/class/power_supply/BAT0/charge_types";
/// uniwill-laptop platform device (EC battery data the firmware hides from ACPI).
const PLATFORM: &str = "/sys/bus/platform/devices/INOU0000:00";
const UNAVAILABLE_HEALTH: &str =
  "unavailable (needs uniwill-laptop; the firmware reports design capacity and 0 cycles)";
const RULE: &str = "/etc/udev/rules.d/90-recoil16-charge-mode.rules";
/// Percentage-limit rule written by 1.0.0; the limit is gone since 1.1.0.
const LIMIT_RULE: &str = "/etc/udev/rules.d/90-recoil16-charge-limit.rules";
/// Platform driver name of uniwill-laptop (`DRIVER_NAME` in uniwill-acpi.c).
const DRIVER: &str = "uniwill";
/// `charge_types` rules from versions before 1.0.0.
const LEGACY_RULES: [&str; 2] = [
  "/etc/udev/rules.d/90-recoil16-charge-profile.rules",
  "/etc/udev/rules.d/90-uniwill-charge-profile.rules",
];

/// EC charging profile (register 0x07A6 bits 4-5). The EC lowers the charge
/// voltage instead of stopping at a percentage, and still reports 100% / Full.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Mode {
  /// Full charge (4.35 V per cell, 17.4 V)
  Standard,
  /// About 93% (4.30 V per cell, 17.2 V)
  LongLife,
  /// About 90% (4.20 V per cell, 16.8 V)
  Trickle,
}

impl Mode {
  /// The value in sysfs `charge_types`.
  pub fn sysfs(self) -> &'static str {
    match self {
      Self::Standard => "Standard",
      Self::LongLife => "Long_Life",
      Self::Trickle => "Trickle",
    }
  }

  fn from_sysfs(s: &str) -> Option<Self> {
    [Self::Standard, Self::LongLife, Self::Trickle]
      .into_iter()
      .find(|m| m.sysfs() == s)
  }

  /// The name accepted by `recoil16ctl battery mode`.
  pub fn cli(self) -> String {
    self
      .to_possible_value()
      .map_or_else(String::new, |v| v.get_name().to_owned())
  }
}

pub struct BatteryStatus {
  pub mode: Option<String>,
  pub status: String,
  pub capacity: u8,
  pub voltage_uv: u64,
  /// Mode applied by the boot rule.
  pub rule: Option<Mode>,
  /// Percentage from a leftover 1.0.0 limit rule.
  pub limit_rule: Option<u8>,
}

impl fmt::Display for BatteryStatus {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(
      f,
      "{}% {}, {} V",
      self.capacity,
      self.status,
      centi(self.voltage_uv)
    )?;
    match &self.mode {
      Some(m) => write!(f, ", charge mode {m}")?,
      None => write!(f, ", charge mode n/a (uniwill-laptop not loaded)")?,
    }
    match self.rule {
      Some(r) => write!(f, " (boot rule: {})", r.sysfs())?,
      None => write!(f, " (no boot rule)")?,
    }
    if let Some(p) = self.limit_rule {
      write!(f, ", leftover 1.0.0 limit rule {p}%")?;
    }
    Ok(())
  }
}

pub struct BatteryHealth {
  pub status: String,
  pub capacity: u8,
  pub voltage_uv: u64,
  pub current_ua: u64,
  pub design_uah: u64,
  pub state_of_health: Option<u8>,
  pub full_uah: Option<u64>,
  pub cycles: Option<u32>,
  pub mode: Option<String>,
}

impl fmt::Display for BatteryHealth {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    writeln!(
      f,
      "{:<10}{}, {}%, {} V, {} A",
      "State",
      self.status,
      self.capacity,
      centi(self.voltage_uv),
      centi(self.current_ua)
    )?;
    match (self.state_of_health, self.full_uah) {
      (Some(soh), Some(full)) => writeln!(
        f,
        "{:<10}{soh}% (full {} of {} mAh design)",
        "Health",
        full / 1000,
        self.design_uah / 1000
      )?,
      (Some(soh), None) => writeln!(f, "{:<10}{soh}%", "Health")?,
      (None, _) => writeln!(f, "{:<10}{UNAVAILABLE_HEALTH}", "Health")?,
    }
    match self.cycles {
      Some(c) => writeln!(f, "{:<10}{c}", "Cycles")?,
      None => writeln!(f, "{:<10}unavailable (needs uniwill-laptop)", "Cycles")?,
    }
    match &self.mode {
      Some(m) => write!(f, "{:<10}{m}", "Charging"),
      None => write!(f, "{:<10}n/a (uniwill-laptop not loaded)", "Charging"),
    }
  }
}

pub fn status(sys: &Sys) -> Result<BatteryStatus> {
  Ok(BatteryStatus {
    mode: current_mode(sys)?,
    status: sys.read(&format!("{BAT}/status"))?,
    capacity: sys.read_num(&format!("{BAT}/capacity"))?,
    voltage_uv: sys.read_num(&format!("{BAT}/voltage_now"))?,
    rule: rule_mode(sys)?,
    limit_rule: rule_limit(sys)?,
  })
}

/// True battery state: learned health and cycles come from the EC through
/// uniwill-laptop, never from the ACPI placeholders `charge_full`/`cycle_count`.
pub fn health(sys: &Sys) -> Result<BatteryHealth> {
  Ok(BatteryHealth {
    status: sys.read(&format!("{BAT}/status"))?,
    capacity: sys.read_num(&format!("{BAT}/capacity"))?,
    voltage_uv: sys.read_num(&format!("{BAT}/voltage_now"))?,
    current_ua: sys.read_num(&format!("{BAT}/current_now"))?,
    design_uah: sys.read_num(&format!("{BAT}/charge_full_design"))?,
    state_of_health: opt_num(sys, &format!("{BAT}/state_of_health"))?,
    full_uah: opt_num(sys, &format!("{PLATFORM}/battery_full_capacity"))?,
    cycles: opt_num(sys, &format!("{PLATFORM}/battery_cycle_count"))?,
    mode: current_mode(sys)?,
  })
}

/// Set the charge mode now; with `persist`, also re-apply it whenever the driver
/// binds (the EC forgets it when the driver releases manual control at shutdown).
pub fn set_mode(sys: &Sys, mode: Mode, persist: bool) -> Result<()> {
  sys.write_attr(CHARGE_TYPES, mode.sysfs())?;
  if persist {
    remove_old_rules(sys)?;
    sys.write_file(RULE, &rule_text(mode))?;
    sys.reload_udev();
  }
  Ok(())
}

/// Remove the boot rule (and any rule from older versions).
pub fn clear_rule(sys: &Sys) -> Result<bool> {
  let removed = sys.remove_file(RULE)?;
  let old = remove_old_rules(sys)?;
  sys.reload_udev();
  Ok(removed || old)
}

fn remove_old_rules(sys: &Sys) -> Result<bool> {
  let mut removed = sys.remove_file(LIMIT_RULE)?;
  for legacy in LEGACY_RULES {
    removed |= sys.remove_file(legacy)?;
  }
  Ok(removed)
}

fn current_mode(sys: &Sys) -> Result<Option<String>> {
  Ok(
    sys
      .read_opt(CHARGE_TYPES)?
      .map(|s| active_choice(&s).to_owned()),
  )
}

fn rule_text(mode: Mode) -> String {
  format!(
    "# Battery charge mode (EC charging profile, 0x07A6), written by recoil16ctl\n\
         ACTION==\"bind\", SUBSYSTEM==\"platform\", DRIVER==\"{DRIVER}\", \
         RUN+=\"/bin/sh -c 'echo {} > {CHARGE_TYPES}'\"\n",
    mode.sysfs()
  )
}

/// The word after `echo ` in a rule file, if the file exists.
fn rule_value(sys: &Sys, path: &str) -> Result<Option<String>> {
  Ok(sys.read_opt(path)?.and_then(|text| {
    let after = text.split("echo ").nth(1)?;
    Some(after.split_whitespace().next()?.to_owned())
  }))
}

/// The mode the boot rule applies, if a rule exists.
fn rule_mode(sys: &Sys) -> Result<Option<Mode>> {
  Ok(rule_value(sys, RULE)?.and_then(|v| Mode::from_sysfs(&v)))
}

/// The percentage of a leftover 1.0.0 limit rule.
fn rule_limit(sys: &Sys) -> Result<Option<u8>> {
  Ok(rule_value(sys, LIMIT_RULE)?.and_then(|v| v.parse().ok()))
}

/// The active entry of a sysfs choice list such as `Standard [Long_Life] Trickle`.
fn active_choice(list: &str) -> &str {
  list
    .split_whitespace()
    .find_map(|w| w.strip_prefix('[')?.strip_suffix(']'))
    .unwrap_or(list.trim())
}

/// A missing or unparsable optional attribute is `None`.
fn opt_num<T: std::str::FromStr>(sys: &Sys, path: &str) -> Result<Option<T>> {
  Ok(sys.read_opt(path)?.and_then(|s| s.parse().ok()))
}

/// Micro-units to units rounded to two decimals (uV -> "16.23"), in integer arithmetic.
fn centi(micro: u64) -> String {
  let c = (micro + 5_000) / 10_000;
  format!("{}.{:02}", c / 100, c % 100)
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::sys::testutil::{put, tempdir};

  const PLATFORM_T: &str = "/sys/bus/platform/devices/INOU0000:00";

  fn fake_battery() -> (std::path::PathBuf, Sys) {
    let root = tempdir();
    put(&root, CHARGE_TYPES, "Trickle [Standard] Long_Life\n");
    put(&root, &format!("{BAT}/status"), "Not charging\n");
    put(&root, &format!("{BAT}/capacity"), "90\n");
    put(&root, &format!("{BAT}/voltage_now"), "16701000\n");
    std::fs::create_dir_all(root.join("etc/udev/rules.d")).unwrap();
    let sys = Sys::with_root(&root);
    (root, sys)
  }

  /// A fake sysfs attribute does not turn a write into the `[selected]` list the
  /// kernel shows, so mimic the driver after a write.
  fn select(root: &std::path::Path, mode: Mode) {
    let list = [Mode::Trickle, Mode::Standard, Mode::LongLife]
      .map(|m| {
        if m == mode {
          format!("[{}]", m.sysfs())
        } else {
          m.sysfs().to_owned()
        }
      })
      .join(" ");
    put(root, CHARGE_TYPES, &format!("{list}\n"));
  }

  #[test]
  fn set_mode_writes_sysfs_and_persists() {
    let (root, sys) = fake_battery();
    put(&root, LIMIT_RULE, "RUN+=\"/bin/sh -c 'echo 90 > x'\"\n");
    put(&root, LEGACY_RULES[0], "old");

    set_mode(&sys, Mode::LongLife, true).unwrap();

    assert_eq!(sys.read(CHARGE_TYPES).unwrap(), "Long_Life");
    select(&root, Mode::LongLife);
    let st = status(&sys).unwrap();
    assert_eq!(st.mode.as_deref(), Some("Long_Life"));
    assert_eq!(st.rule, Some(Mode::LongLife));
    assert_eq!(st.limit_rule, None, "old limit rule removed");
    assert!(!sys.path(LEGACY_RULES[0]).exists(), "legacy rule removed");
    assert_eq!(
      st.to_string(),
      "90% Not charging, 16.70 V, charge mode Long_Life (boot rule: Long_Life)"
    );
  }

  #[test]
  fn temporary_mode_leaves_rule_alone() {
    let (root, sys) = fake_battery();
    set_mode(&sys, Mode::Trickle, true).unwrap();
    set_mode(&sys, Mode::Standard, false).unwrap();
    select(&root, Mode::Standard);

    let st = status(&sys).unwrap();
    assert_eq!(st.mode.as_deref(), Some("Standard"));
    assert_eq!(st.rule, Some(Mode::Trickle));
  }

  #[test]
  fn clear_rule_reports_whether_a_rule_existed() {
    let (root, sys) = fake_battery();
    assert!(!clear_rule(&sys).unwrap());
    set_mode(&sys, Mode::LongLife, true).unwrap();
    assert!(clear_rule(&sys).unwrap());
    assert_eq!(status(&sys).unwrap().rule, None);
    put(&root, LIMIT_RULE, "RUN+=\"/bin/sh -c 'echo 80 > x'\"\n");
    assert!(clear_rule(&sys).unwrap(), "a leftover limit rule counts");
    assert_eq!(status(&sys).unwrap().limit_rule, None);
  }

  #[test]
  fn status_reports_a_leftover_limit_rule() {
    let (root, sys) = fake_battery();
    put(&root, LIMIT_RULE, "RUN+=\"/bin/sh -c 'echo 80 > x'\"\n");
    let st = status(&sys).unwrap();
    assert_eq!(st.limit_rule, Some(80));
    assert_eq!(
      st.to_string(),
      "90% Not charging, 16.70 V, charge mode Standard (no boot rule), leftover 1.0.0 limit rule 80%"
    );
  }

  #[test]
  fn status_without_driver() {
    let (root, sys) = fake_battery();
    std::fs::remove_file(root.join(CHARGE_TYPES.trim_start_matches('/'))).unwrap();
    assert_eq!(
      status(&sys).unwrap().to_string(),
      "90% Not charging, 16.70 V, charge mode n/a (uniwill-laptop not loaded) (no boot rule)"
    );
  }

  #[test]
  fn rule_matches_the_driver_and_attribute() {
    let rule = rule_text(Mode::LongLife);
    // the platform driver registers as "uniwill", not "uniwill-laptop"
    assert!(rule.contains("DRIVER==\"uniwill\","));
    assert!(rule.contains(
      "RUN+=\"/bin/sh -c 'echo Long_Life > /sys/class/power_supply/BAT0/charge_types'\""
    ));
  }

  #[test]
  fn mode_names() {
    assert_eq!(Mode::LongLife.cli(), "long-life");
    assert_eq!(Mode::from_sysfs("Long_Life"), Some(Mode::LongLife));
    assert_eq!(Mode::from_sysfs("Bogus"), None);
  }

  fn fake_health(driver: bool) -> (std::path::PathBuf, Sys) {
    let root = tempdir();
    put(&root, &format!("{BAT}/status"), "Charging\n");
    put(&root, &format!("{BAT}/capacity"), "69\n");
    put(&root, &format!("{BAT}/voltage_now"), "16232000\n");
    put(&root, &format!("{BAT}/current_now"), "1938000\n");
    put(&root, &format!("{BAT}/charge_full_design"), "6400000\n");
    // firmware placeholders, never to be shown as health
    put(&root, &format!("{BAT}/charge_full"), "6400000\n");
    put(&root, &format!("{BAT}/cycle_count"), "0\n");
    if driver {
      put(&root, &format!("{BAT}/state_of_health"), "97\n");
      put(
        &root,
        &format!("{BAT}/charge_types"),
        "Standard [Long_Life] Trickle\n",
      );
      put(&root, &format!("{PLATFORM_T}/battery_cycle_count"), "9\n");
      put(
        &root,
        &format!("{PLATFORM_T}/battery_full_capacity"),
        "6208000\n",
      );
    }
    let sys = Sys::with_root(&root);
    (root, sys)
  }

  #[test]
  fn health_with_driver_reads_the_ec_values() {
    let (_root, sys) = fake_health(true);
    let h = health(&sys).unwrap();
    assert_eq!(h.state_of_health, Some(97));
    assert_eq!(h.cycles, Some(9));
    assert_eq!(h.full_uah, Some(6_208_000));
    assert_eq!(
      h.to_string(),
      "State     Charging, 69%, 16.23 V, 1.94 A\n\
       Health    97% (full 6208 of 6400 mAh design)\n\
       Cycles    9\n\
       Charging  Long_Life"
    );
  }

  #[test]
  fn health_without_driver_never_shows_firmware_placeholders() {
    let (_root, sys) = fake_health(false);
    let text = health(&sys).unwrap().to_string();
    assert!(text.contains(
      "Health    unavailable (needs uniwill-laptop; the firmware reports design capacity and 0 cycles)"
    ));
    assert!(text.contains("Cycles    unavailable (needs uniwill-laptop)"));
    assert!(text.contains("Charging  n/a (uniwill-laptop not loaded)"));
    assert!(
      !text.contains("100%"),
      "placeholder capacity leaked: {text}"
    );
  }

  #[test]
  fn health_formats_zero_current() {
    let (root, sys) = fake_health(true);
    put(&root, &format!("{BAT}/current_now"), "0\n");
    assert!(
      health(&sys)
        .unwrap()
        .to_string()
        .starts_with("State     Charging, 69%, 16.23 V, 0.00 A\n")
    );
  }

  #[test]
  fn active_choice_takes_the_bracketed_entry() {
    assert_eq!(active_choice("[Standard] Trickle Long_Life"), "Standard");
    assert_eq!(active_choice("Standard Trickle [Long_Life]"), "Long_Life");
    assert_eq!(active_choice("Standard"), "Standard");
  }
}
