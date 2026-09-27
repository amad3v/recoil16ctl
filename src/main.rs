//! recoil16ctl: control the `PCSpecialist Recoil 16 AMD` extras provided by the
//! recoil16 drivers (charge modes, battery health, lightbar, power profile, Fn/Super lock,
//! screen rotation) and check NVIDIA GPU power, app offloading and battery draw.

mod battery;
mod check;
mod check_power;
mod gpu;
mod gpu_test;
mod lightbar;
mod locks;
mod power;
mod profile;
mod screen;
mod sensors;
mod sys;

use std::{
  io,
  os::unix::process::CommandExt,
  process::{Command, ExitCode},
  thread,
};

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
  /// NVIDIA GPU: power state, driver, which of your apps use it; run an app on it
  Gpu {
    #[command(subcommand)]
    cmd: Option<GpuCmd>,
  },
  /// Battery draw (5 s average), time left, NVIDIA GPU and panel state
  Power {
    /// Keep going: a new 5 s average every 2 s, until Ctrl+C
    #[arg(long)]
    watch: bool,
    #[arg(
      long,
      value_name = "NAME",
      help = "Only this battery (a name in /sys/class/power_supply, e.g. BAT0)"
    )]
    battery: Option<String>,
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
  #[command(hide = true, about = "Removed in 1.1.0: use 'battery mode'")]
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

#[derive(Subcommand)]
enum GpuCmd {
  #[command(
    about = "Run a program on the NVIDIA GPU (PRIME render offload), e.g. gpu run -- freecad"
  )]
  Run {
    /// The program and its arguments
    #[arg(
      required = true,
      trailing_var_arg = true,
      allow_hyphen_values = true,
      value_name = "COMMAND"
    )]
    command: Vec<String>,
  },
  /// Wake the NVIDIA GPU and check offloaded apps really use it (desktop session; mesa-utils, vulkan-tools)
  Test,
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
    Ok(code) => code,
    Err(e) => {
      eprintln!("recoil16ctl: {e:#}");
      ExitCode::FAILURE
    }
  }
}

/// Run the command. `gpu run` replaces this process with the program, so it
/// only returns (127 or 126) when the program can't be started.
fn run(cli: Cli) -> Result<ExitCode> {
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
    Cmd::Gpu { cmd: None } => gpu_cmd(&sys),
    Cmd::Gpu {
      cmd: Some(GpuCmd::Run { command }),
    } => return Ok(gpu_run(&sys, &command)),
    Cmd::Gpu {
      cmd: Some(GpuCmd::Test),
    } => gpu_test_cmd(),
    Cmd::Power { watch, battery } => power_cmd(&sys, watch, battery.as_deref()),
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
  }?;
  Ok(ExitCode::SUCCESS)
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
  recoil16ctl check                        verify the installation and power settings
  recoil16ctl power --watch                battery draw and time left, every 2 s
  recoil16ctl gpu                          is the NVIDIA GPU asleep? which apps use it?
  recoil16ctl gpu run -- freecad           run an app on the NVIDIA GPU
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
  println!("\nPower:");
  for item in check_power::run(sys) {
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

/// Replace this process with `command`, offloaded to the NVIDIA GPU. Returns
/// only if the program can't be started.
fn gpu_run(sys: &Sys, command: &[String]) -> ExitCode {
  if let Err(e) = gpu::find(sys) {
    eprintln!("recoil16ctl: warning: {e:#}; running on the default GPU");
  }
  let err = Command::new(&command[0])
    .args(&command[1..])
    .envs(gpu::OFFLOAD_ENV)
    .exec();
  let (reason, code) = exec_failure(&err);
  eprintln!("recoil16ctl: {}: {reason}", command[0]);
  ExitCode::from(code)
}

/// Why `exec` failed and the exit status, by shell convention: 127 command
/// not found, 126 found but not runnable.
fn exec_failure(err: &io::Error) -> (String, u8) {
  match err.kind() {
    io::ErrorKind::NotFound => ("not found".to_owned(), 127),
    _ => (err.to_string(), 126),
  }
}

fn gpu_cmd(sys: &Sys) -> Result<()> {
  let g = gpu::find(sys)?;
  println!("{g}");
  let users = gpu::users(sys, &g);
  let scope = if crate::sys::effective_uid() == Some(0) {
    "all processes"
  } else {
    "your processes only; sudo includes all"
  };
  let (apps, displays): (Vec<_>, Vec<_>) = users.iter().partition(|u| u.role == gpu::Role::App);
  let list = if apps.is_empty() {
    "nothing".to_owned()
  } else {
    apps
      .iter()
      .map(|u| format!("{} (pid {})", u.name, u.pid))
      .collect::<Vec<_>>()
      .join(", ")
  };
  println!("{}", gpu::row("using it", list));
  for d in &displays {
    println!(
      "{}",
      gpu::row(
        "display",
        format_args!(
          "{} (pid {}): has the NVIDIA display device open; this doesn't keep the GPU awake",
          d.name, d.pid
        )
      )
    );
  }
  println!("{}", gpu::row("", format_args!("({scope})")));
  Ok(())
}

fn gpu_test_cmd() -> Result<()> {
  let results = gpu_test::run()?;
  let mut failed = false;
  for (name, verdict) in &results {
    let (mark, text) = match verdict {
      gpu_test::Verdict::Pass(t) => ("ok  ", t),
      gpu_test::Verdict::Fail(t) => {
        failed = true;
        ("FAIL", t)
      }
      gpu_test::Verdict::Skipped(t) => ("skip", t),
    };
    println!("[{mark}] {name:<8} {text}");
  }
  gpu_test::ensure_ran(&results)?;
  println!("\nThe NVIDIA GPU was woken for the test; it suspends again about 20 s later.");
  if failed {
    bail!("offload test failed");
  }
  Ok(())
}

fn power_cmd(sys: &Sys, watch: bool, only: Option<&str>) -> Result<()> {
  let names = power::select(sys, only)?;
  let mut window = power::Window::default();
  loop {
    let sample = power::sample(sys, &names)?;
    if window.push(&sample.status, sample.combined) {
      let avg = window.average().unwrap_or(sample.combined);
      println!(
        "{}",
        power::report(sys, &sample, avg, screen::mode().ok().as_deref())
      );
      if !watch {
        return Ok(());
      }
      println!();
    }
    thread::sleep(power::INTERVAL);
  }
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
  line("gpu", gpu::find(sys).map(|g| g.summary()));
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

  #[test]
  fn exec_failures_map_to_shell_exit_codes() {
    use std::io;
    assert_eq!(
      super::exec_failure(&io::Error::from(io::ErrorKind::NotFound)),
      ("not found".to_owned(), 127)
    );
    let denied = io::Error::from(io::ErrorKind::PermissionDenied);
    assert_eq!(super::exec_failure(&denied), (denied.to_string(), 126));
  }

  #[test]
  fn offload_env_is_the_switcheroo_set() {
    let keys: Vec<&str> = crate::gpu::OFFLOAD_ENV.iter().map(|(k, _)| *k).collect();
    assert_eq!(
      keys,
      [
        "__NV_PRIME_RENDER_OFFLOAD",
        "__GLX_VENDOR_LIBRARY_NAME",
        "__VK_LAYER_NV_optimus",
        "VK_LOADER_DRIVERS_SELECT"
      ]
    );
  }
}
