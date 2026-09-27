//! `recoil16ctl power`: battery draw, time left, and the state of what draws
//! it (NVIDIA GPU, panel).
//!
//! The Recoil 16 battery reports charge (µAh, µA), not energy, so power is
//! current × voltage. Single readings jump around; they are averaged over
//! [`SAMPLES`] readings [`INTERVAL`] apart (5 s).

use std::{collections::VecDeque, fmt, time::Duration};

use anyhow::{Result, anyhow, bail};

use crate::{
  battery,
  gpu::{self, row},
  profile,
  sys::Sys,
};

const SUPPLY: &str = "/sys/class/power_supply";
const BACKLIGHT: &str = "/sys/class/backlight";
/// Readings per average, and the gap between them.
pub const SAMPLES: usize = 10;
pub const INTERVAL: Duration = Duration::from_millis(500);

/// One reading of a battery (or several, combined).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reading {
  pub current_ua: u64,
  pub voltage_uv: u64,
  pub charge_uah: u64,
}

impl Reading {
  /// Power in watts. Battery µA and µV fit in `u32` (up to 4294 A / 4294 V), which
  /// converts to `f64` exactly.
  pub fn watts(self) -> f64 {
    let exact = |v: u64| f64::from(u32::try_from(v).unwrap_or(u32::MAX));
    exact(self.current_ua) * exact(self.voltage_uv) / 1e12
  }
}

pub struct Battery {
  pub name: String,
  pub status: String,
  pub capacity: u8,
  pub reading: Reading,
}

/// One reading of the selected batteries.
pub struct Sample {
  pub batteries: Vec<Battery>,
  /// Currents and charges summed, voltage averaged.
  pub combined: Reading,
  /// Discharging if any is, else Charging if any is, else Full if all are.
  pub status: String,
}

/// What the battery is doing, from its status and an averaged reading.
#[derive(Debug, PartialEq)]
pub enum Flow {
  Discharging {
    watts: f64,
    left: Duration,
  },
  Charging {
    watts: f64,
  },
  Full,
  NotCharging,
  /// Discharging but no current yet (right after unplugging).
  Settling,
}

impl fmt::Display for Flow {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::Discharging { watts, .. } => write!(f, "discharging {watts:.1} W (5 s average)"),
      Self::Charging { watts } => write!(f, "charging at {watts:.1} W (5 s average)"),
      Self::Full => f.write_str("full"),
      Self::NotCharging => f.write_str("not charging"),
      Self::Settling => f.write_str("discharging, reading settling (try again in a few seconds)"),
    }
  }
}

/// System batteries: `type` Battery, except peripherals (`scope` Device), sorted.
pub fn batteries(sys: &Sys) -> Vec<String> {
  sys
    .list(SUPPLY)
    .into_iter()
    .filter(|n| {
      let dir = format!("{SUPPLY}/{n}");
      sys.get(&format!("{dir}/type")).as_deref() == Some("Battery")
        && sys.get(&format!("{dir}/scope")).as_deref() != Some("Device")
    })
    .collect()
}

/// Whether mains power is connected (any `type` Mains supply online); `None` if there is none.
pub fn on_ac(sys: &Sys) -> Option<bool> {
  let mains: Vec<String> = sys
    .list(SUPPLY)
    .into_iter()
    .filter(|n| sys.get(&format!("{SUPPLY}/{n}/type")).as_deref() == Some("Mains"))
    .collect();
  if mains.is_empty() {
    return None;
  }
  Some(
    mains
      .iter()
      .any(|n| sys.get(&format!("{SUPPLY}/{n}/online")).as_deref() == Some("1")),
  )
}

/// The batteries to report: `only` if given (must exist), else all of them.
pub fn select(sys: &Sys, only: Option<&str>) -> Result<Vec<String>> {
  let all = batteries(sys);
  match only {
    Some(n) if all.iter().any(|b| b == n) => Ok(vec![n.to_owned()]),
    Some(n) => {
      let found = if all.is_empty() {
        "none".to_owned()
      } else {
        all.join(", ")
      };
      bail!("no battery {n:?} (found: {found})")
    }
    None if all.is_empty() => bail!("no battery found in {SUPPLY}"),
    None => Ok(all),
  }
}

fn read_battery(sys: &Sys, name: &str) -> Result<Battery> {
  let dir = format!("{SUPPLY}/{name}");
  let num = |f: &str| -> Result<i64> {
    let path = format!("{dir}/{f}");
    let s = sys
      .get(&path)
      .ok_or_else(|| anyhow!("{path} not readable"))?;
    s.parse()
      .map_err(|_| anyhow!("unexpected value {s:?} in {path}"))
  };
  Ok(Battery {
    name: name.to_owned(),
    status: sys.get(&format!("{dir}/status")).unwrap_or_default(),
    capacity: u8::try_from(num("capacity")?).unwrap_or(0),
    reading: Reading {
      // some drivers report discharge current as negative
      current_ua: num("current_now")?.unsigned_abs(),
      voltage_uv: num("voltage_now")?.unsigned_abs(),
      charge_uah: num("charge_now")?.unsigned_abs(),
    },
  })
}

/// Read the named batteries once.
pub fn sample(sys: &Sys, names: &[String]) -> Result<Sample> {
  let batteries = names
    .iter()
    .map(|n| read_battery(sys, n))
    .collect::<Result<Vec<_>>>()?;
  let n = u64::try_from(batteries.len().max(1)).unwrap_or(1);
  let combined = Reading {
    current_ua: batteries.iter().map(|b| b.reading.current_ua).sum(),
    voltage_uv: batteries.iter().map(|b| b.reading.voltage_uv).sum::<u64>() / n,
    charge_uah: batteries.iter().map(|b| b.reading.charge_uah).sum(),
  };
  let any = |s: &str| batteries.iter().any(|b| b.status == s);
  let status = if any("Discharging") {
    "Discharging".to_owned()
  } else if any("Charging") {
    "Charging".to_owned()
  } else if batteries.iter().all(|b| b.status == "Full") {
    "Full".to_owned()
  } else {
    batteries
      .first()
      .map(|b| b.status.clone())
      .unwrap_or_default()
  };
  Ok(Sample {
    batteries,
    combined,
    status,
  })
}

/// The last [`SAMPLES`] readings of the batteries in one status.
#[derive(Default)]
pub struct Window {
  readings: VecDeque<Reading>,
  status: String,
  /// Readings since the status last changed.
  taken: usize,
}

impl Window {
  /// Add a reading taken while the batteries were `status`. Returns whether a
  /// report is due: once the window is full (5 s), then every 4 readings (2 s).
  /// A status change (plugging in, unplugging) starts over, so an average never
  /// mixes charging and discharging current.
  pub fn push(&mut self, status: &str, reading: Reading) -> bool {
    if status != self.status {
      self.readings.clear();
      self.taken = 0;
      status.clone_into(&mut self.status);
    }
    if self.readings.len() == SAMPLES {
      self.readings.pop_front();
    }
    self.readings.push_back(reading);
    self.taken += 1;
    self.taken >= SAMPLES && (self.taken - SAMPLES).is_multiple_of(4)
  }

  /// Mean of the readings in the window; `None` if empty.
  pub fn average(&mut self) -> Option<Reading> {
    average(self.readings.make_contiguous())
  }
}

/// Field-wise mean of `readings`; `None` if empty.
fn average(readings: &[Reading]) -> Option<Reading> {
  let n = u64::try_from(readings.len()).ok().filter(|&n| n > 0)?;
  let mean = |f: fn(&Reading) -> u64| readings.iter().map(f).sum::<u64>() / n;
  Some(Reading {
    current_ua: mean(|r| r.current_ua),
    voltage_uv: mean(|r| r.voltage_uv),
    charge_uah: mean(|r| r.charge_uah),
  })
}

pub fn flow(status: &str, avg: Reading) -> Flow {
  match status {
    "Discharging" if avg.current_ua == 0 => Flow::Settling,
    "Discharging" => Flow::Discharging {
      watts: avg.watts(),
      left: Duration::from_secs(avg.charge_uah * 3600 / avg.current_ua),
    },
    "Charging" => Flow::Charging { watts: avg.watts() },
    "Full" => Flow::Full,
    _ => Flow::NotCharging,
  }
}

fn format_left(left: Duration) -> String {
  let minutes = left.as_secs() / 60;
  let (h, m) = (minutes / 60, minutes % 60);
  if h == 0 {
    format!("~{m} min left")
  } else {
    format!("~{h} h {m} min left")
  }
}

/// Built-in panel backlight in percent: the AMD integrated GPU's device
/// (`amdgpu_bl*`), else any other except the NVIDIA one (`nvidia_*`).
pub fn backlight(sys: &Sys) -> Option<u32> {
  let names = sys.list(BACKLIGHT);
  let name = names
    .iter()
    .find(|n| n.starts_with("amdgpu_bl"))
    .or_else(|| names.iter().find(|n| !n.starts_with("nvidia")))?;
  let dir = format!("{BACKLIGHT}/{name}");
  let cur: u64 = sys.get(&format!("{dir}/brightness"))?.parse().ok()?;
  let max: u64 = sys.get(&format!("{dir}/max_brightness"))?.parse().ok()?;
  u32::try_from(cur * 100 / max.max(1)).ok()
}

/// The lines `recoil16ctl power` prints. `panel` is `screen::mode()`, `None`
/// without a KDE session (or under sudo).
pub fn report(sys: &Sys, sample: &Sample, avg: Reading, panel: Option<&str>) -> String {
  let flow = flow(&sample.status, avg);
  let n = u32::try_from(sample.batteries.len().max(1)).unwrap_or(1);
  let capacity = sample
    .batteries
    .iter()
    .map(|b| u32::from(b.capacity))
    .sum::<u32>()
    / n;
  let battery = match (&flow, on_ac(sys)) {
    (Flow::Discharging { left, .. }, _) => {
      format!("{flow}, {capacity} %, {}", format_left(*left))
    }
    (Flow::Full | Flow::NotCharging, Some(true)) => format!("on AC, {flow}, {capacity} %"),
    _ => format!("{flow}, {capacity} %"),
  };
  let mut lines = vec![row("battery", battery)];
  if sample.batteries.len() > 1 {
    for b in &sample.batteries {
      lines.push(row(
        "",
        format_args!("{} {} %, {:.1} W", b.name, b.capacity, b.reading.watts()),
      ));
    }
  }
  let mode = battery::status(sys)
    .ok()
    .and_then(|s| s.mode)
    .unwrap_or_else(|| "n/a".to_owned());
  let profile = profile::current(sys).map_or_else(|_| "n/a".to_owned(), |p| p.to_string());
  lines.push(row(
    "",
    format_args!("charge mode {mode}, platform profile {profile}"),
  ));
  let dgpu = gpu::find(sys).map_or_else(
    |e| format!("n/a ({e:#})"),
    |g| g.controller.shown_status().to_owned(),
  );
  lines.push(row("dgpu", dgpu));
  let light = backlight(sys).map_or_else(|| "n/a".to_owned(), |p| format!("{p} %"));
  // kscreen-doctor needs the user's session, which sudo doesn't have
  let unknown = if crate::sys::effective_uid() == Some(0) {
    "refresh unknown (run as your user)"
  } else {
    "refresh unknown (no KDE session)"
  };
  lines.push(row(
    "panel",
    format_args!("{}, backlight {light}", panel.unwrap_or(unknown)),
  ));
  lines.join("\n")
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::sys::testutil::{put, tempdir};

  fn reading(current_ua: u64, voltage_uv: u64, charge_uah: u64) -> Reading {
    Reading {
      current_ua,
      voltage_uv,
      charge_uah,
    }
  }

  fn fake_battery(root: &std::path::Path, name: &str, status: &str, current: &str) {
    let d = format!("/sys/class/power_supply/{name}");
    put(root, &format!("{d}/type"), "Battery\n");
    put(root, &format!("{d}/status"), &format!("{status}\n"));
    put(root, &format!("{d}/capacity"), "50\n");
    put(root, &format!("{d}/current_now"), &format!("{current}\n"));
    put(root, &format!("{d}/voltage_now"), "16000000\n");
    put(root, &format!("{d}/charge_now"), "4000000\n");
  }

  #[test]
  fn discovers_system_batteries_only() {
    let root = tempdir();
    fake_battery(&root, "BAT0", "Discharging", "500000");
    put(
      &root,
      "/sys/class/power_supply/hidpp_battery_0/type",
      "Battery\n",
    );
    put(
      &root,
      "/sys/class/power_supply/hidpp_battery_0/scope",
      "Device\n",
    );
    put(
      &root,
      "/sys/class/power_supply/ucsi-source-psy-USBC000:001/type",
      "USB\n",
    );
    put(&root, "/sys/class/power_supply/AC0/type", "Mains\n");
    put(&root, "/sys/class/power_supply/AC0/online", "1\n");
    let sys = Sys::with_root(&root);
    assert_eq!(batteries(&sys), ["BAT0"]);
    assert_eq!(on_ac(&sys), Some(true));
  }

  #[test]
  fn select_checks_the_name_and_needs_a_battery() {
    let root = tempdir();
    fake_battery(&root, "BAT0", "Discharging", "500000");
    fake_battery(&root, "BAT1", "Discharging", "500000");
    let sys = Sys::with_root(&root);
    assert_eq!(select(&sys, None).unwrap(), ["BAT0", "BAT1"]);
    assert_eq!(select(&sys, Some("BAT1")).unwrap(), ["BAT1"]);
    assert_eq!(
      select(&sys, Some("BAT7")).unwrap_err().to_string(),
      "no battery \"BAT7\" (found: BAT0, BAT1)"
    );
    assert!(select(&Sys::with_root(tempdir()), None).is_err());
  }

  #[test]
  fn samples_and_combines_batteries() {
    let root = tempdir();
    fake_battery(&root, "BAT0", "Discharging", "500000");
    fake_battery(&root, "BAT1", "Full", "-300000"); // signed current on some drivers
    let s = sample(&Sys::with_root(&root), &["BAT0".into(), "BAT1".into()]).unwrap();
    assert_eq!(s.status, "Discharging");
    assert_eq!(s.combined, reading(800_000, 16_000_000, 8_000_000));
    assert_eq!(s.batteries[1].reading.current_ua, 300_000);
  }

  #[test]
  fn averages_and_computes_power() {
    let avg = average(&[
      reading(400_000, 16_000_000, 4_000_000),
      reading(600_000, 16_200_000, 4_000_000),
    ])
    .unwrap();
    assert_eq!(avg, reading(500_000, 16_100_000, 4_000_000));
    assert!((reading(500_000, 16_000_000, 0).watts() - 8.0).abs() < 1e-9);
    assert_eq!(average(&[]), None);
  }

  #[test]
  fn window_reports_when_full_then_every_four_readings() {
    let mut w = Window::default();
    let due: Vec<bool> = (0..SAMPLES + 4)
      .map(|_| w.push("Discharging", reading(500_000, 16_000_000, 4_000_000)))
      .collect();
    let first = due.iter().position(|&d| d);
    assert_eq!(first, Some(SAMPLES - 1));
    assert_eq!(due.iter().filter(|&&d| d).count(), 2);
    assert!(due[SAMPLES + 3]);
  }

  #[test]
  fn window_starts_over_when_the_status_changes() {
    let mut w = Window::default();
    for _ in 0..SAMPLES {
      w.push("Charging", reading(3_000_000, 17_000_000, 4_000_000));
    }
    let unplugged = reading(500_000, 16_000_000, 4_000_000);
    assert!(!w.push("Discharging", unplugged));
    assert_eq!(w.average(), Some(unplugged));
    let due = (1..SAMPLES)
      .map(|_| w.push("Discharging", unplugged))
      .last();
    assert_eq!(due, Some(true));
  }

  #[test]
  fn flow_from_status_and_reading() {
    let r = reading(500_000, 16_000_000, 4_000_000);
    assert_eq!(
      flow("Discharging", r),
      Flow::Discharging {
        watts: 8.0,
        left: Duration::from_hours(8)
      }
    );
    assert_eq!(flow("Charging", r), Flow::Charging { watts: 8.0 });
    assert_eq!(flow("Full", r), Flow::Full);
    assert_eq!(flow("Not charging", r), Flow::NotCharging);
    assert_eq!(
      flow("Discharging", reading(0, 16_000_000, 4_000_000)),
      Flow::Settling
    );
  }

  #[test]
  fn backlight_prefers_the_amd_panel_device() {
    let root = tempdir();
    put(
      &root,
      "/sys/class/backlight/amdgpu_bl1/brightness",
      "19661\n",
    );
    put(
      &root,
      "/sys/class/backlight/amdgpu_bl1/max_brightness",
      "65535\n",
    );
    put(&root, "/sys/class/backlight/nvidia_0/brightness", "100\n");
    put(
      &root,
      "/sys/class/backlight/nvidia_0/max_brightness",
      "100\n",
    );
    assert_eq!(backlight(&Sys::with_root(&root)), Some(30));
  }

  #[test]
  fn report_lines() {
    let root = tempdir();
    fake_battery(&root, "BAT0", "Discharging", "500000");
    put(
      &root,
      "/sys/class/power_supply/BAT0/charge_types",
      "Standard [Trickle] Long_Life\n",
    );
    put(
      &root,
      "/sys/class/backlight/amdgpu_bl1/brightness",
      "19661\n",
    );
    put(
      &root,
      "/sys/class/backlight/amdgpu_bl1/max_brightness",
      "65535\n",
    );
    let sys = Sys::with_root(&root);
    let s = sample(&sys, &["BAT0".into()]).unwrap();
    assert_eq!(
      report(&sys, &s, s.combined, Some("2560x1600 @ 60 Hz")),
      "battery     discharging 8.0 W (5 s average), 50 %, ~8 h 0 min left\n\
       \x20           charge mode Trickle, platform profile n/a\n\
       dgpu        n/a (no NVIDIA GPU found)\n\
       panel       2560x1600 @ 60 Hz, backlight 30 %"
    );
  }
}
