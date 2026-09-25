//! Firmware power mode via the `platform_profile` class device of uniwill-laptop.
//!
//! Only the handler registered by uniwill-laptop (name "uniwill") is used, so
//! other laptops' profile drivers are never touched. power-profiles-daemon
//! (KDE/GNOME slider) follows changes made here.

use std::fmt;

use anyhow::{Result, anyhow};
use clap::ValueEnum;

use crate::sys::Sys;

const CLASS: &str = "/sys/class/platform-profile";
const HANDLER: &str = "uniwill";

/// Firmware mode: office (green), balance (blue), turbo (purple).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Profile {
  LowPower,
  Balanced,
  Performance,
}

impl Profile {
  const ALL: [Self; 3] = [Self::LowPower, Self::Balanced, Self::Performance];

  fn as_sysfs(self) -> &'static str {
    match self {
      Self::LowPower => "low-power",
      Self::Balanced => "balanced",
      Self::Performance => "performance",
    }
  }

  fn from_sysfs(s: &str) -> Option<Self> {
    Self::ALL.into_iter().find(|p| p.as_sysfs() == s)
  }

  fn mode(self) -> &'static str {
    match self {
      Self::LowPower => "office, green",
      Self::Balanced => "balance, blue",
      Self::Performance => "turbo, purple",
    }
  }

  /// The next profile in the button's order: low-power → balanced → performance → low-power.
  pub fn next(self) -> Self {
    let i = Self::ALL.iter().position(|p| *p == self).unwrap_or(0);
    Self::ALL[(i + 1) % Self::ALL.len()]
  }
}

impl fmt::Display for Profile {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "{} ({})", self.as_sysfs(), self.mode())
  }
}

/// The uniwill-laptop platform profile device directory.
fn handler(sys: &Sys) -> Result<String> {
  sys
    .find_by_name(CLASS, HANDLER)?
    .ok_or_else(|| anyhow!("uniwill platform profile not found (is the recoil16 driver loaded?)"))
}

pub fn current(sys: &Sys) -> Result<Profile> {
  let dir = handler(sys)?;
  let s = sys.read(&format!("{dir}/profile"))?;
  Profile::from_sysfs(&s).ok_or_else(|| anyhow!("unexpected platform profile {s:?}"))
}

pub fn set(sys: &Sys, profile: Profile) -> Result<()> {
  let dir = handler(sys)?;
  sys.write_attr(&format!("{dir}/profile"), profile.as_sysfs())
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::sys::testutil::{put, tempdir};

  #[test]
  fn cycles_in_button_order() {
    assert_eq!(Profile::LowPower.next(), Profile::Balanced);
    assert_eq!(Profile::Balanced.next(), Profile::Performance);
    assert_eq!(Profile::Performance.next(), Profile::LowPower);
  }

  fn handler_dir(root: &std::path::Path, index: u8, name: &str, profile: &str) -> String {
    let dir = format!("{CLASS}/platform-profile-{index}");
    put(root, &format!("{dir}/name"), &format!("{name}\n"));
    put(root, &format!("{dir}/profile"), &format!("{profile}\n"));
    dir
  }

  #[test]
  fn reads_and_writes_the_uniwill_handler() {
    let root = tempdir();
    handler_dir(&root, 0, "dell-pc", "quiet");
    let dir = handler_dir(&root, 1, "uniwill", "balanced");
    let sys = Sys::with_root(&root);

    assert_eq!(current(&sys).unwrap(), Profile::Balanced);
    set(&sys, Profile::Performance).unwrap();
    assert_eq!(sys.read(&format!("{dir}/profile")).unwrap(), "performance");
    assert_eq!(
      sys
        .read(&format!("{CLASS}/platform-profile-0/profile"))
        .unwrap(),
      "quiet",
      "other handlers untouched"
    );
    assert_eq!(
      Profile::Performance.to_string(),
      "performance (turbo, purple)"
    );
  }

  #[test]
  fn other_laptops_profile_drivers_are_ignored() {
    let root = tempdir();
    handler_dir(&root, 0, "dell-pc", "balanced");
    let sys = Sys::with_root(&root);
    assert!(current(&sys).is_err());
    assert!(set(&sys, Profile::Balanced).is_err());
  }
}
