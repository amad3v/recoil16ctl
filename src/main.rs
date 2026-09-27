//! recoil16ctl: control the `PCSpecialist Recoil 16 AMD` extras provided by the
//! recoil16 drivers (charge modes, battery health, lightbar, power profile, Fn/Super lock,
//! screen rotation).

mod battery;
mod check;
mod lightbar;
mod locks;
mod profile;
mod screen;
mod sensors;
mod sys;

use std::{io, process::ExitCode};

use anyhow::{Result, bail};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};

use crate::{
  lightbar::Action,
  locks::{Lock, OnOff},
  profile::Profile,
  sys::Sys,
};

#[derive(Parser)]
#[command(
  version,
  about,
  long_about = None,
  arg_required_else_help = false,
  after_long_help = EXAMPLES
)]
struct Cli {
  #[command(subcommand)]
  cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
  /// Show everything (default)
  Status,
  /// Check the installation and every feature; lists manual checks too
  #[command(visible_alias = "verify")]
  Check,
  /// Battery state, health and charge mode
  Battery {
    #[command(subcommand)]
    cmd: Option<BatteryCmd>,
  },
  /// Lightbar: [on|off|<0-100>|<colour> [0-100]|colours]
  #[command(
    after_help = "COLOUR: a name (see 'colours'), RRGGBB, #RRGGBB or \"R G B\".\n\
        Changes are saved for the next boot unless --temp is given."
  )]
  Lightbar {
    /// on | off | <brightness 0-100> | <colour> [brightness] | colours
    #[arg(num_args = 0..=2, value_name = "ARGS")]
    args: Vec<String>,
    /// Change it for this boot only
    #[arg(long)]
    temp: bool,
  },
  /// Firmware power mode (the KDE/GNOME power slider follows)
  Profile {
    #[arg(value_enum)]
    profile: Option<ProfileArg>,
  },
  /// Keyboard: backlight level, Fn lock, Super key
  Keyboard {
    #[command(subcommand)]
    cmd: Option<KeyboardCmd>,
  },
  /// Built-in screen orientation (KDE; run as your user, not sudo)
  Screen {
    #[command(subcommand)]
    cmd: Option<ScreenCmd>,
  },
  /// Print shell completions
  Completions {
    #[arg(value_enum)]
    shell: clap_complete::Shell,
  },
  /// Write the man pages (recoil16ctl.1 and one per subcommand) to DIR
  #[command(hide = true)]
  Man { dir: std::path::PathBuf },
}

#[derive(Subcommand)]
enum BatteryCmd {
  /// True health, cycle count and charge state (from the EC via uniwill-laptop)
  Status,
  /// Set the charge mode; re-applied at boot unless --temp
  Mode {
    /// standard (full), long-life (~93%) or trickle (~90%)
    #[arg(value_enum)]
    mode: battery::Mode,
    /// Change it for this boot only
    #[arg(long)]
    temp: bool,
  },
  /// Remove the boot rule (and any charge-limit rule from 1.0.0)
  ClearRule,
  /// Removed in 1.1.0: use `battery mode`
  #[command(hide = true)]
  Limit {
    #[arg(num_args = 0.., allow_hyphen_values = true)]
    args: Vec<String>,
  },
}

#[derive(Subcommand)]
enum KeyboardCmd {
  /// Fn lock: F1-F12 without holding Fn
  FnLock {
    #[arg(value_enum)]
    state: Option<OnOff>,
  },
  /// Super (Windows) key; Fn+F2 toggles it too
  SuperKey {
    #[arg(value_enum)]
    state: Option<OnOff>,
  },
}

#[derive(Subcommand)]
enum ScreenCmd {
  /// Flip the built-in screen 180° (normal <-> inverted)
  Rotate,
}

#[derive(Clone, Copy, ValueEnum)]
enum ProfileArg {
  LowPower,
  Balanced,
  Performance,
  /// Next profile, like the mode button
  Cycle,
}

fn main() -> ExitCode {
  match run(Cli::parse()) {
    Ok(()) => ExitCode::SUCCESS,
    Err(e) => {
      eprintln!("recoil16ctl: {e:#}");
      ExitCode::FAILURE
    }
  }
}

fn run(cli: Cli) -> Result<()> {
  let sys = Sys::new();
  match cli.cmd.unwrap_or(Cmd::Status) {
    Cmd::Status => {
      status(&sys);
      Ok(())
    }
    Cmd::Battery { cmd: None } => {
      println!("{}", battery::status(&sys)?);
      Ok(())
    }
    Cmd::Battery {
      cmd: Some(BatteryCmd::Status),
    } => {
      println!("{}", battery::health(&sys)?);
      Ok(())
    }
    Cmd::Battery {
      cmd: Some(BatteryCmd::Mode { mode, temp }),
    } => {
      battery::set_mode(&sys, mode, !temp)?;
      println!("{}", battery::status(&sys)?);
      Ok(())
    }
    Cmd::Battery {
      cmd: Some(BatteryCmd::ClearRule),
    } => {
      let msg = if battery::clear_rule(&sys)? {
        "boot rule removed; the mode resets to Standard at the next boot"
      } else {
        "no boot rule"
      };
      println!("{msg}");
      Ok(())
    }
    Cmd::Battery {
      cmd: Some(BatteryCmd::Limit { .. }),
    } => bail!(
      "the percentage charge limit was removed in 1.1.0: it is a preview feature \
       of this EC that may damage the battery.\n\
       Use a charge mode instead: sudo recoil16ctl battery mode long-life (~93%) or trickle (~90%)"
    ),
    Cmd::Lightbar { args, temp } => lightbar_cmd(&sys, &args, temp),
    Cmd::Profile { profile } => {
      if let Some(p) = profile {
        let target = match p {
          ProfileArg::LowPower => Profile::LowPower,
          ProfileArg::Balanced => Profile::Balanced,
          ProfileArg::Performance => Profile::Performance,
          ProfileArg::Cycle => profile::current(&sys)?.next(),
        };
        profile::set(&sys, target)?;
      }
      println!("{}", profile::current(&sys)?);
      Ok(())
    }
    Cmd::Keyboard { cmd: None } => {
      println!("backlight    {}", sensors::keyboard_backlight(&sys)?);
      println!("fn lock      {}", locks::get(&sys, Lock::FnLock)?);
      println!("super key    {}", locks::get(&sys, Lock::SuperKey)?);
      Ok(())
    }
    Cmd::Keyboard {
      cmd: Some(KeyboardCmd::FnLock { state }),
    } => lock_cmd(&sys, Lock::FnLock, state),
    Cmd::Keyboard {
      cmd: Some(KeyboardCmd::SuperKey { state }),
    } => lock_cmd(&sys, Lock::SuperKey, state),
    Cmd::Screen { cmd } => {
      let panel = match cmd {
        None => screen::current()?,
        Some(ScreenCmd::Rotate) => screen::rotate()?,
      };
      println!("screen {panel}");
      Ok(())
    }
    Cmd::Check => check_cmd(&sys),
    Cmd::Man { dir } => {
      std::fs::create_dir_all(&dir)?;
      clap_mangen::generate_to(Cli::command(), &dir)?;
      Ok(())
    }
    Cmd::Completions { shell } => {
      clap_complete::generate(shell, &mut Cli::command(), "recoil16ctl", &mut io::stdout());
      Ok(())
    }
  }
}

fn lightbar_cmd(sys: &Sys, args: &[String], temp: bool) -> Result<()> {
  if matches!(args, [a] if a == "colours" || a == "colors") {
    let names: Vec<&str> = lightbar::COLOUR_NAMES.iter().map(|(n, _)| *n).collect();
    println!("{}", names.join(" "));
    return Ok(());
  }
  let action = Action::parse(args)?;
  let state = if action == Action::Show {
    lightbar::current(sys)?
  } else {
    lightbar::set(sys, &action, !temp)?
  };
  println!("lightbar {state}");
  if let Some(boot) = lightbar::boot(sys)? {
    println!("at boot: {boot}");
  }
  Ok(())
}

const EXAMPLES: &str = "Examples:
  recoil16ctl                              status of everything
  recoil16ctl check                        verify the installation
  sudo recoil16ctl battery mode long-life  charge to ~93% (lower voltage), also after reboots
  sudo recoil16ctl lightbar blue 60        blue lightbar at 60%, also after reboots
  sudo recoil16ctl profile cycle           next power mode, like the mode button
  recoil16ctl screen rotate                flip the screen (as your user)

Reading works as a normal user; changing settings needs sudo, except screen,
which must run as the desktop user.";

fn check_cmd(sys: &Sys) -> Result<()> {
  let items = check::run(sys);
  for item in &items {
    println!("{item}");
  }
  println!("\nBy hand:");
  for manual in check::MANUAL {
    println!("  - {manual}");
  }
  let failed = items
    .iter()
    .filter(|i| i.level == check::Level::Fail)
    .count();
  if failed > 0 {
    bail!("{failed} check(s) failed");
  }
  Ok(())
}

fn lock_cmd(sys: &Sys, lock: Lock, state: Option<OnOff>) -> Result<()> {
  if let Some(s) = state {
    locks::set(sys, lock, s)?;
  }
  println!("{}", locks::get(sys, lock)?);
  Ok(())
}

/// One line per feature; a missing driver shows up as "n/a" instead of failing.
fn status(sys: &Sys) {
  let line = |label: &str, value: Result<String>| match value {
    Ok(v) => println!("{label:<12} {v}"),
    Err(e) => println!("{label:<12} n/a ({e})"),
  };
  line("battery", battery::status(sys).map(|s| s.to_string()));
  line("profile", profile::current(sys).map(|p| p.to_string()));
  line("keyboard", sensors::keyboard_backlight(sys));
  line("lightbar", lightbar::current(sys).map(|s| s.to_string()));
  line(
    "fn lock",
    locks::get(sys, Lock::FnLock).map(|s| s.to_string()),
  );
  line(
    "super key",
    locks::get(sys, Lock::SuperKey).map(|s| s.to_string()),
  );
  line("sensors", sensors::fans_and_temps(sys));
}

#[cfg(test)]
mod tests {
  /// recoil16ctl and the DKMS modules are released together with one version.
  #[test]
  fn version_matches_dkms_conf() {
    let expected = format!("PACKAGE_VERSION=\"{}\"", env!("CARGO_PKG_VERSION"));
    assert!(
      include_str!("../../dkms.conf")
        .lines()
        .any(|l| l == expected),
      "Cargo.toml version and dkms.conf PACKAGE_VERSION differ"
    );
  }
}
