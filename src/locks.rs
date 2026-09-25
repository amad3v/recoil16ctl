//! Fn lock and Super-key lock (uniwill-laptop attributes).

use std::fmt;

use anyhow::{Result, bail};
use clap::ValueEnum;

use crate::sys::Sys;

const DEVICE: &str = "/sys/bus/platform/devices/INOU0000:00";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lock {
  /// Fn keys act as F1-F12 without holding Fn.
  FnLock,
  /// Super (Windows) key enabled; toggled by Fn+F2.
  SuperKey,
}

impl Lock {
  fn attr(self) -> String {
    let name = match self {
      Self::FnLock => "fn_lock",
      Self::SuperKey => "super_key_enable",
    };
    format!("{DEVICE}/{name}")
  }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum OnOff {
  On,
  Off,
}

impl From<bool> for OnOff {
  fn from(on: bool) -> Self {
    if on { Self::On } else { Self::Off }
  }
}

impl fmt::Display for OnOff {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(match self {
      Self::On => "on",
      Self::Off => "off",
    })
  }
}

pub fn get(sys: &Sys, lock: Lock) -> Result<OnOff> {
  match sys.read(&lock.attr())?.as_str() {
    "1" => Ok(OnOff::On),
    "0" => Ok(OnOff::Off),
    other => bail!("unexpected value {other:?} in {}", lock.attr()),
  }
}

pub fn set(sys: &Sys, lock: Lock, state: OnOff) -> Result<()> {
  let value = match state {
    OnOff::On => "1",
    OnOff::Off => "0",
  };
  sys.write_attr(&lock.attr(), value)
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::sys::testutil::{put, tempdir};

  #[test]
  fn get_and_set() {
    let root = tempdir();
    put(&root, &Lock::FnLock.attr(), "0\n");
    put(&root, &Lock::SuperKey.attr(), "1\n");
    let sys = Sys::with_root(&root);

    assert_eq!(get(&sys, Lock::FnLock).unwrap(), OnOff::Off);
    assert_eq!(get(&sys, Lock::SuperKey).unwrap(), OnOff::On);
    set(&sys, Lock::FnLock, OnOff::On).unwrap();
    assert_eq!(get(&sys, Lock::FnLock).unwrap(), OnOff::On);
  }

  #[test]
  fn missing_driver_is_an_error() {
    assert!(get(&Sys::with_root(tempdir()), Lock::FnLock).is_err());
  }
}
