//! The built-in panel: show its orientation and flip it 180° (KDE Plasma,
//! kscreen-doctor).
//!
//! Must run as the desktop user: kscreen-doctor talks to the user's session.

use std::{fmt, process::Command};

use anyhow::{Context, Result, bail};

use crate::sys;

/// Panel output name and kscreen rotation (1 normal, 2 left, 4 inverted, 8 right).
#[derive(Debug, PartialEq, Eq)]
pub struct Panel {
  pub name: String,
  pub rotation: u32,
}

const NORMAL: u32 = 1;
const INVERTED: u32 = 4;

impl Panel {
  fn orientation(&self) -> &'static str {
    match self.rotation {
      NORMAL => "normal",
      2 => "left",
      INVERTED => "inverted",
      8 => "right",
      _ => "unknown",
    }
  }
}

impl fmt::Display for Panel {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "{} {}", self.name, self.orientation())
  }
}

/// The built-in panel and its current rotation.
pub fn current() -> Result<Panel> {
  if sys::effective_uid() == Some(0) {
    bail!("run 'recoil16ctl screen' as your desktop user, not with sudo");
  }
  let out = Command::new("kscreen-doctor")
    .arg("-o")
    .output()
    .context("running kscreen-doctor (KDE Plasma only; package libkscreen)")?;
  if !out.status.success() {
    bail!(
      "kscreen-doctor -o failed: {}",
      String::from_utf8_lossy(&out.stderr).trim()
    );
  }
  find_panel(&String::from_utf8_lossy(&out.stdout))
    .context("built-in panel not found in kscreen-doctor output")
}

/// Flip the panel between normal and inverted; returns its new state.
pub fn rotate() -> Result<Panel> {
  let mut panel = current()?;
  let (target, rotation) = if panel.rotation == INVERTED {
    ("none", NORMAL)
  } else {
    ("inverted", INVERTED)
  };
  let status = Command::new("kscreen-doctor")
    .arg(format!("output.{}.rotation.{target}", panel.name))
    .status()
    .context("running kscreen-doctor")?;
  if !status.success() {
    bail!("kscreen-doctor could not rotate {}", panel.name);
  }
  panel.rotation = rotation;
  Ok(panel)
}

/// Find the output marked "Panel" in `kscreen-doctor -o` output (ANSI colours allowed).
pub fn find_panel(output: &str) -> Option<Panel> {
  let text = strip_ansi(output);
  let mut name: Option<&str> = None;
  let mut is_panel = false;
  for line in text.lines() {
    let trimmed = line.trim();
    if let Some(rest) = line.strip_prefix("Output:") {
      name = rest.split_whitespace().nth(1);
      is_panel = false;
    } else if trimmed == "Panel" {
      is_panel = true;
    } else if let Some(rot) = trimmed.strip_prefix("Rotation:")
      && is_panel
    {
      return Some(Panel {
        name: name?.to_owned(),
        rotation: rot.trim().parse().ok()?,
      });
    }
  }
  None
}

fn strip_ansi(s: &str) -> String {
  let mut out = String::with_capacity(s.len());
  let mut chars = s.chars();
  while let Some(c) = chars.next() {
    if c == '\x1b' {
      // skip "ESC [ ... <letter>"
      for c in chars.by_ref() {
        if c.is_ascii_alphabetic() {
          break;
        }
      }
    } else {
      out.push(c);
    }
  }
  out
}

#[cfg(test)]
mod tests {
  use super::*;

  // Shape of `kscreen-doctor -o` on the Recoil 16 (Plasma 6), with colour codes.
  const SAMPLE: &str = "\x1b[01;32mOutput: \x1b[0;0m1 HDMI-A-1 aaaa\n\
        \tenabled\n\tconnected\n\tRotation: 1\n\
        \x1b[01;32mOutput: \x1b[0;0m2 eDP-2 5f5a116d-41be\n\
        \tenabled\n\tconnected\n\tpriority 1\n\tPanel\n\
        \tModes:  1:2560x1600@240.00*!\n\tScale: 1.25\n\tRotation: 4\n\tOverscan: 0\n";

  #[test]
  fn finds_the_panel_not_the_external_monitor() {
    assert_eq!(
      find_panel(SAMPLE),
      Some(Panel {
        name: "eDP-2".to_owned(),
        rotation: 4
      })
    );
  }

  #[test]
  fn shows_name_and_orientation() {
    let panel = find_panel(SAMPLE).unwrap();
    assert_eq!(panel.to_string(), "eDP-2 inverted");
  }

  #[test]
  fn no_panel_means_none() {
    assert_eq!(find_panel("Output: 1 HDMI-A-1 x\n\tRotation: 1\n"), None);
  }
}
