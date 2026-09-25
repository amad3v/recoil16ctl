//! Battery charge limit (EC register CGLM, exposed by uniwill-laptop).

use std::fmt;

use anyhow::Result;

use crate::sys::Sys;

const BAT: &str = "/sys/class/power_supply/BAT0";
const LIMIT: &str = "/sys/class/power_supply/BAT0/charge_control_end_threshold";
const RULE: &str = "/etc/udev/rules.d/90-recoil16-charge-limit.rules";
/// Platform driver name of uniwill-laptop (`DRIVER_NAME` in uniwill-acpi.c).
const DRIVER: &str = "uniwill";
/// `charge_types` rules from earlier versions (the EC ignores `charge_types`).
const LEGACY_RULES: [&str; 2] = [
  "/etc/udev/rules.d/90-recoil16-charge-profile.rules",
  "/etc/udev/rules.d/90-uniwill-charge-profile.rules",
];

pub struct BatteryStatus {
  pub limit: Option<u8>,
  pub status: String,
  pub capacity: u8,
  pub voltage_uv: u64,
  pub rule: Option<u8>,
}

impl fmt::Display for BatteryStatus {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    // uV to volts with two decimals, in integer arithmetic
    let centivolts = self.voltage_uv / 10_000;
    write!(
      f,
      "{}% {}, {}.{:02} V",
      self.capacity,
      self.status,
      centivolts / 100,
      centivolts % 100
    )?;
    match self.limit {
      Some(l) => write!(f, ", limit {l}%")?,
      None => write!(f, ", limit n/a (uniwill-laptop not loaded)")?,
    }
    match self.rule {
      Some(r) => write!(f, " (boot rule: {r}%)"),
      None => write!(f, " (no boot rule)"),
    }
  }
}

pub fn status(sys: &Sys) -> Result<BatteryStatus> {
  Ok(BatteryStatus {
    limit: sys.read_opt(LIMIT)?.and_then(|s| s.parse().ok()),
    status: sys.read(&format!("{BAT}/status"))?,
    capacity: sys.read_num(&format!("{BAT}/capacity"))?,
    voltage_uv: sys.read_num(&format!("{BAT}/voltage_now"))?,
    rule: rule_limit(sys)?,
  })
}

/// Set the charge limit now; with `persist`, also re-apply it whenever the driver binds.
pub fn set_limit(sys: &Sys, percent: u8, persist: bool) -> Result<()> {
  sys.write_attr(LIMIT, &percent.to_string())?;
  if persist {
    for legacy in LEGACY_RULES {
      sys.remove_file(legacy)?;
    }
    sys.write_file(RULE, &rule_text(percent))?;
    sys.reload_udev();
  }
  Ok(())
}

/// Remove the boot rule; the EC keeps its current limit.
pub fn clear_rule(sys: &Sys) -> Result<bool> {
  let removed = sys.remove_file(RULE)?;
  for legacy in LEGACY_RULES {
    sys.remove_file(legacy)?;
  }
  sys.reload_udev();
  Ok(removed)
}

fn rule_text(percent: u8) -> String {
  format!(
    "# Battery charge limit in percent (EC register CGLM, 0x07B9), written by recoil16ctl\n\
         ACTION==\"bind\", SUBSYSTEM==\"platform\", DRIVER==\"{DRIVER}\", \
         RUN+=\"/bin/sh -c 'echo {percent} > {LIMIT}'\"\n"
  )
}

/// The limit a boot rule applies, if a rule exists.
fn rule_limit(sys: &Sys) -> Result<Option<u8>> {
  Ok(sys.read_opt(RULE)?.and_then(|text| {
    let after = text.split("echo ").nth(1)?;
    after.split_whitespace().next()?.parse().ok()
  }))
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::sys::testutil::{put, tempdir};

  fn fake_battery() -> (std::path::PathBuf, Sys) {
    let root = tempdir();
    put(&root, LIMIT, "100\n");
    put(&root, &format!("{BAT}/status"), "Not charging\n");
    put(&root, &format!("{BAT}/capacity"), "90\n");
    put(&root, &format!("{BAT}/voltage_now"), "16701000\n");
    std::fs::create_dir_all(root.join("etc/udev/rules.d")).unwrap();
    let sys = Sys::with_root(&root);
    (root, sys)
  }

  #[test]
  fn set_limit_persists_and_status_reads_it_back() {
    let (root, sys) = fake_battery();
    put(&root, LEGACY_RULES[0], "old");

    set_limit(&sys, 80, true).unwrap();

    let st = status(&sys).unwrap();
    assert_eq!(st.limit, Some(80));
    assert_eq!(st.rule, Some(80));
    assert!(!sys.path(LEGACY_RULES[0]).exists(), "legacy rule removed");
    assert_eq!(
      st.to_string(),
      "90% Not charging, 16.70 V, limit 80% (boot rule: 80%)"
    );
  }

  #[test]
  fn temporary_limit_leaves_rule_alone() {
    let (_root, sys) = fake_battery();
    set_limit(&sys, 90, true).unwrap();
    set_limit(&sys, 60, false).unwrap();

    let st = status(&sys).unwrap();
    assert_eq!((st.limit, st.rule), (Some(60), Some(90)));
  }

  #[test]
  fn clear_rule_reports_whether_a_rule_existed() {
    let (_root, sys) = fake_battery();
    assert!(!clear_rule(&sys).unwrap());
    set_limit(&sys, 90, true).unwrap();
    assert!(clear_rule(&sys).unwrap());
    assert_eq!(status(&sys).unwrap().rule, None);
  }

  #[test]
  fn rule_matches_the_driver_and_attribute() {
    let rule = rule_text(90);
    // the platform driver registers as "uniwill", not "uniwill-laptop"
    assert!(rule.contains("DRIVER==\"uniwill\","));
    assert!(rule.contains(
      "RUN+=\"/bin/sh -c 'echo 90 > /sys/class/power_supply/BAT0/charge_control_end_threshold'\""
    ));
  }
}
