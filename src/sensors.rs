//! Read-only readings: keyboard backlight level, fans and temperatures.

use anyhow::{Result, anyhow};

use crate::sys::Sys;

const KBD: &str = "/sys/class/leds/ite8291:white:kbd_backlight";
const HWMON: &str = "/sys/class/hwmon";

pub fn keyboard_backlight(sys: &Sys) -> Result<String> {
  let level: u32 = sys.read_num(&format!("{KBD}/brightness"))?;
  let max: u32 = sys.read_num(&format!("{KBD}/max_brightness"))?;
  Ok(format!("{level}/{max} ({}%)", level * 100 / max.max(1)))
}

/// Fans and CPU/GPU temperatures from the uniwill hwmon device.
pub fn fans_and_temps(sys: &Sys) -> Result<String> {
  let dir = uniwill_hwmon(sys)?;
  let rpm = |n: u8| sys.read_num::<u32>(&format!("{dir}/fan{n}_input"));
  let celsius = |n: u8| {
    sys
      .read_num::<i32>(&format!("{dir}/temp{n}_input"))
      .map(|m| m / 1000)
  };
  Ok(format!(
    "fans {} / {} RPM, CPU {} °C, GPU {} °C",
    rpm(1)?,
    rpm(2)?,
    celsius(1)?,
    celsius(2)?
  ))
}

/// Path (relative to the sysfs root) of the hwmon device named "uniwill".
fn uniwill_hwmon(sys: &Sys) -> Result<String> {
  sys
    .find_by_name(HWMON, "uniwill")?
    .ok_or_else(|| anyhow!("uniwill sensors not found (is the recoil16 driver loaded?)"))
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::sys::testutil::{put, tempdir};

  #[test]
  fn reads_uniwill_hwmon_among_others() {
    let root = tempdir();
    put(&root, "/sys/class/hwmon/hwmon2/name", "nvme\n");
    put(&root, "/sys/class/hwmon/hwmon13/name", "uniwill\n");
    for (f, v) in [
      ("fan1_input", "1440"),
      ("fan2_input", "1450"),
      ("temp1_input", "49000"),
      ("temp2_input", "37000"),
    ] {
      put(&root, &format!("/sys/class/hwmon/hwmon13/{f}"), v);
    }
    put(&root, &format!("{KBD}/brightness"), "25\n");
    put(&root, &format!("{KBD}/max_brightness"), "50\n");
    let sys = Sys::with_root(&root);

    assert_eq!(
      fans_and_temps(&sys).unwrap(),
      "fans 1440 / 1450 RPM, CPU 49 °C, GPU 37 °C"
    );
    assert_eq!(keyboard_backlight(&sys).unwrap(), "25/50 (50%)");
  }

  #[test]
  fn missing_sensors_are_an_error() {
    assert!(fans_and_temps(&Sys::with_root(tempdir())).is_err());
  }
}
