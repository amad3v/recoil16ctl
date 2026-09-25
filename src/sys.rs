//! Filesystem access with a configurable root, so tests can use a fake sysfs.

use std::{
  fs::{self, OpenOptions},
  io::{ErrorKind, Write},
  path::{Path, PathBuf},
  process::Command,
};

use anyhow::{Context, Result, anyhow, bail};

pub struct Sys {
  root: PathBuf,
}

impl Sys {
  /// The real system.
  pub fn new() -> Self {
    Self {
      root: PathBuf::from("/"),
    }
  }

  #[cfg(test)]
  pub fn with_root(root: impl Into<PathBuf>) -> Self {
    Self { root: root.into() }
  }

  pub fn path(&self, path: &str) -> PathBuf {
    self.root.join(path.trim_start_matches('/'))
  }

  /// Read a text file (sysfs attribute or config), without the trailing newline.
  pub fn read(&self, path: &str) -> Result<String> {
    let full = self.path(path);
    match fs::read_to_string(&full) {
      Ok(s) => Ok(s.trim_end().to_owned()),
      Err(e) if e.kind() == ErrorKind::NotFound => Err(not_loaded(&full)),
      Err(e) => Err(e).with_context(|| format!("reading {}", full.display())),
    }
  }

  /// Like [`Sys::read`], but a missing file is `None`.
  pub fn read_opt(&self, path: &str) -> Result<Option<String>> {
    let full = self.path(path);
    match fs::read_to_string(&full) {
      Ok(s) => Ok(Some(s.trim_end().to_owned())),
      Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
      Err(e) => Err(e).with_context(|| format!("reading {}", full.display())),
    }
  }

  pub fn read_num<T: std::str::FromStr>(&self, path: &str) -> Result<T> {
    let s = self.read(path)?;
    s.parse()
      .map_err(|_| anyhow!("unexpected value {s:?} in {}", self.path(path).display()))
  }

  /// The device directory under `class_dir` (e.g. `/sys/class/hwmon`) whose
  /// `name` attribute is `name`, as a path relative to the sysfs root.
  pub fn find_by_name(&self, class_dir: &str, name: &str) -> Result<Option<String>> {
    let Ok(entries) = fs::read_dir(self.path(class_dir)) else {
      return Ok(None);
    };
    for entry in entries.flatten() {
      let dir = format!("{class_dir}/{}", entry.file_name().to_string_lossy());
      if self.read_opt(&format!("{dir}/name"))?.as_deref() == Some(name) {
        return Ok(Some(dir));
      }
    }
    Ok(None)
  }

  /// Replace the value of an existing attribute (sysfs); never creates the file.
  pub fn write_attr(&self, path: &str, value: &str) -> Result<()> {
    let full = self.path(path);
    let res = OpenOptions::new()
      .write(true)
      .truncate(true)
      .open(&full)
      .and_then(|mut f| f.write_all(value.as_bytes()));
    map_write_err(res, &full)
  }

  /// Create or replace a regular file (udev rule, modprobe.d config).
  pub fn write_file(&self, path: &str, contents: &str) -> Result<()> {
    let full = self.path(path);
    map_write_err(fs::write(&full, contents), &full)
  }

  pub fn remove_file(&self, path: &str) -> Result<bool> {
    let full = self.path(path);
    match fs::remove_file(&full) {
      Ok(()) => Ok(true),
      Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
      Err(e) => map_write_err(Err(e), &full).map(|()| false),
    }
  }

  /// Ask udev to re-read its rules; only meaningful on the real system.
  pub fn reload_udev(&self) {
    if self.root != Path::new("/") {
      return;
    }
    match Command::new("udevadm")
      .args(["control", "--reload"])
      .status()
    {
      Ok(s) if s.success() => {}
      _ => eprintln!("warning: 'udevadm control --reload' failed; the rule applies after a reboot"),
    }
  }
}

fn map_write_err(res: std::io::Result<()>, path: &Path) -> Result<()> {
  match res {
    Ok(()) => Ok(()),
    Err(e) if e.kind() == ErrorKind::PermissionDenied => {
      bail!(
        "permission denied writing {}: run with sudo",
        path.display()
      )
    }
    Err(e) if e.kind() == ErrorKind::NotFound => Err(not_loaded(path)),
    Err(e) => Err(e).with_context(|| format!("writing {}", path.display())),
  }
}

/// A sysfs file that only exists while the recoil16 driver is loaded is missing.
fn not_loaded(path: &Path) -> anyhow::Error {
  anyhow!(
    "{} not found (is the recoil16 driver loaded?)",
    path.display()
  )
}

/// Effective uid of this process, from /proc (no libc dependency).
pub fn effective_uid() -> Option<u32> {
  let status = fs::read_to_string("/proc/self/status").ok()?;
  status
    .lines()
    .find_map(|l| l.strip_prefix("Uid:"))
    .and_then(|ids| ids.split_whitespace().nth(1))
    .and_then(|euid| euid.parse().ok())
}

#[cfg(test)]
pub mod testutil {
  use std::path::PathBuf;
  use std::sync::atomic::{AtomicUsize, Ordering};

  /// A fresh, empty directory under the system temp dir.
  pub fn tempdir() -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
      "recoil16ctl-test-{}-{}",
      std::process::id(),
      N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
  }

  /// Create `root/path` with `contents`, creating parent directories.
  pub fn put(root: &std::path::Path, path: &str, contents: &str) {
    let full = root.join(path.trim_start_matches('/'));
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(full, contents).unwrap();
  }
}
