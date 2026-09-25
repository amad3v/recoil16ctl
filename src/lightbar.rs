//! RGB lightbar (ite8233-lightbar), with the boot setting kept in modprobe.d.

use std::{fmt, str::FromStr};

use anyhow::{Result, bail};

use crate::sys::Sys;

const LED: &str = "/sys/class/leds/rgb:lightbar";
const CONF: &str = "/etc/modprobe.d/ite8233-lightbar.conf";
const MAX_BRIGHTNESS: u8 = 100;
/// Brightness used when switching on and nothing better is known.
const DEFAULT_ON: u8 = 60;

pub const COLOUR_NAMES: [(&str, Rgb); 10] = [
  ("white", Rgb([255, 255, 255])),
  ("red", Rgb([255, 0, 0])),
  ("green", Rgb([0, 255, 0])),
  ("blue", Rgb([0, 0, 255])),
  ("cyan", Rgb([0, 255, 255])),
  ("magenta", Rgb([255, 0, 255])),
  ("purple", Rgb([160, 0, 255])),
  ("yellow", Rgb([255, 200, 0])),
  ("orange", Rgb([255, 80, 0])),
  ("pink", Rgb([255, 60, 150])),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub [u8; 3]);

impl FromStr for Rgb {
  type Err = anyhow::Error;

  /// A colour name, `RRGGBB` / `#RRGGBB`, `0xRRGGBB`, or `"R G B"`.
  fn from_str(s: &str) -> Result<Self> {
    let s = s.trim();
    if let Some((_, rgb)) = COLOUR_NAMES
      .iter()
      .find(|(name, _)| name.eq_ignore_ascii_case(s))
    {
      return Ok(*rgb);
    }
    let hex = s
      .strip_prefix('#')
      .or_else(|| s.strip_prefix("0x"))
      .unwrap_or(s);
    if hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
      let [_, r, g, b] = u32::from_str_radix(hex, 16)?.to_be_bytes();
      return Ok(Self([r, g, b]));
    }
    let parts: Vec<&str> = s.split_whitespace().collect();
    if let [r, g, b] = parts[..]
      && let (Ok(r), Ok(g), Ok(b)) = (r.parse(), g.parse(), b.parse())
    {
      return Ok(Self([r, g, b]));
    }
    bail!("unknown colour {s:?} (a name, RRGGBB or \"R G B\"; see 'recoil16ctl lightbar colours')")
  }
}

impl fmt::Display for Rgb {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let [r, g, b] = self.0;
    write!(f, "#{r:02x}{g:02x}{b:02x}")
  }
}

/// What the user asked for.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
  Show,
  On,
  Off,
  Brightness(u8),
  Colour(Rgb, Option<u8>),
}

impl Action {
  pub fn parse(args: &[String]) -> Result<Self> {
    let brightness = |s: &str| -> Option<u8> { s.parse().ok().filter(|b| *b <= MAX_BRIGHTNESS) };
    Ok(match args {
      [] => Self::Show,
      [a] if a == "on" => Self::On,
      [a] if a == "off" => Self::Off,
      // up to three digits is a brightness; "112233" is an all-digit hex colour
      [a] if a.len() <= 3 && a.bytes().all(|b| b.is_ascii_digit()) => match brightness(a) {
        Some(b) => Self::Brightness(b),
        None => bail!("brightness must be 0-{MAX_BRIGHTNESS}"),
      },
      [colour] => Self::Colour(colour.parse()?, None),
      [colour, b] => match brightness(b) {
        Some(b) => Self::Colour(colour.parse()?, Some(b)),
        None => bail!("brightness must be 0-{MAX_BRIGHTNESS}"),
      },
      _ => bail!("too many arguments (quote an \"R G B\" colour)"),
    })
  }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct State {
  pub colour: Rgb,
  pub brightness: u8,
  /// Brightness "on" returns to; remembered across "off".
  pub on_brightness: u8,
}

impl State {
  /// Pure state transition for `action`.
  pub fn apply(self, action: &Action) -> Self {
    let mut next = self;
    match *action {
      Action::Show => {}
      Action::On => next.brightness = self.on_brightness,
      Action::Off => next.brightness = 0,
      Action::Brightness(b) => next.brightness = b,
      Action::Colour(colour, b) => {
        next.colour = colour;
        // a colour on a dark lightbar switches it on
        next.brightness = b.unwrap_or(if self.brightness == 0 {
          self.on_brightness
        } else {
          self.brightness
        });
      }
    }
    if next.brightness > 0 {
      next.on_brightness = next.brightness;
    }
    next
  }
}

impl fmt::Display for State {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    if self.brightness == 0 {
      write!(f, "off, colour {}", self.colour)
    } else {
      write!(
        f,
        "on {}/{MAX_BRIGHTNESS}, colour {}",
        self.brightness, self.colour
      )
    }
  }
}

/// Current state; an error if the lightbar driver is not loaded.
pub fn current(sys: &Sys) -> Result<State> {
  let brightness: u8 = sys.read_num(&format!("{LED}/brightness"))?;
  let colour: Rgb = sys.read(&format!("{LED}/multi_intensity"))?.parse()?;
  let saved = boot(sys)?;
  let on_brightness = if brightness > 0 {
    brightness
  } else {
    saved.map_or(DEFAULT_ON, |s| s.on_brightness)
  };
  Ok(State {
    colour,
    brightness,
    on_brightness,
  })
}

/// The state applied at boot, if one is saved.
pub fn boot(sys: &Sys) -> Result<Option<State>> {
  Ok(sys.read_opt(CONF)?.and_then(|text| parse_conf(&text)))
}

/// Apply `action` now; with `persist`, save the result for the next boot.
pub fn set(sys: &Sys, action: &Action, persist: bool) -> Result<State> {
  let next = current(sys)?.apply(action);
  let [r, g, b] = next.colour.0;
  sys.write_attr(&format!("{LED}/multi_intensity"), &format!("{r} {g} {b}"))?;
  sys.write_attr(&format!("{LED}/brightness"), &next.brightness.to_string())?;
  if persist {
    sys.write_file(CONF, &format_conf(next))?;
  }
  Ok(next)
}

fn format_conf(state: State) -> String {
  let [r, g, b] = state.colour.0;
  format!(
    "# written by recoil16ctl; on_brightness={}\n\
         options ite8233-lightbar default_brightness={} default_color=0x{r:02x}{g:02x}{b:02x}\n",
    state.on_brightness, state.brightness
  )
}

/// Parse the modprobe.d file (also the format written by the old shell script).
fn parse_conf(text: &str) -> Option<State> {
  let value = |key: &str| {
    text
      .split_whitespace()
      .find_map(|w| w.split_once('=').filter(|(k, _)| *k == key).map(|(_, v)| v))
  };
  let brightness: u8 = value("default_brightness")?.parse().ok()?;
  let colour: Rgb = value("default_color")?.parse().ok()?;
  let on_brightness = value("on_brightness")
    .and_then(|v| v.parse().ok())
    .filter(|b| *b > 0)
    .unwrap_or(if brightness > 0 {
      brightness
    } else {
      DEFAULT_ON
    });
  Some(State {
    colour,
    brightness,
    on_brightness,
  })
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::sys::testutil::{put, tempdir};

  fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_owned()).collect()
  }

  #[test]
  fn colours_parse_in_all_forms() {
    assert_eq!("blue".parse::<Rgb>().unwrap(), Rgb([0, 0, 255]));
    assert_eq!("BLUE".parse::<Rgb>().unwrap(), Rgb([0, 0, 255]));
    assert_eq!("ff8800".parse::<Rgb>().unwrap(), Rgb([255, 136, 0]));
    assert_eq!("#00a0ff".parse::<Rgb>().unwrap(), Rgb([0, 160, 255]));
    assert_eq!("0x0a141e".parse::<Rgb>().unwrap(), Rgb([10, 20, 30]));
    assert_eq!("10 20 30".parse::<Rgb>().unwrap(), Rgb([10, 20, 30]));
    for bad in ["teal", "300 0 0", "12345", "1 2", "#gg0000"] {
      assert!(bad.parse::<Rgb>().is_err(), "{bad} should be rejected");
    }
    assert_eq!(Rgb([0, 160, 255]).to_string(), "#00a0ff");
  }

  #[test]
  fn actions_parse() {
    assert_eq!(Action::parse(&args(&[])).unwrap(), Action::Show);
    assert_eq!(Action::parse(&args(&["off"])).unwrap(), Action::Off);
    assert_eq!(
      Action::parse(&args(&["70"])).unwrap(),
      Action::Brightness(70)
    );
    assert_eq!(
      Action::parse(&args(&["ff8800", "40"])).unwrap(),
      Action::Colour(Rgb([255, 136, 0]), Some(40))
    );
    assert_eq!(
      Action::parse(&args(&["10 20 30"])).unwrap(),
      Action::Colour(Rgb([10, 20, 30]), None)
    );
    assert!(Action::parse(&args(&["150"])).is_err());
    // all-digit hex colours are colours, not brightness
    assert_eq!(
      Action::parse(&args(&["112233"])).unwrap(),
      Action::Colour(Rgb([0x11, 0x22, 0x33]), None)
    );
    assert_eq!(
      Action::parse(&args(&["000000", "0"])).unwrap(),
      Action::Colour(Rgb([0, 0, 0]), Some(0))
    );
    assert!(Action::parse(&args(&["blue", "150"])).is_err());
    assert!(Action::parse(&args(&["10", "20", "30"])).is_err());
  }

  #[test]
  fn on_returns_to_last_brightness_after_off() {
    let s = State {
      colour: Rgb([0, 0, 255]),
      brightness: 55,
      on_brightness: 55,
    };
    let off = s.apply(&Action::Off);
    assert_eq!((off.brightness, off.on_brightness), (0, 55));
    assert_eq!(off.apply(&Action::On).brightness, 55);
    // a colour on a dark lightbar switches it on at the remembered level
    let green = off.apply(&Action::Colour(Rgb([0, 255, 0]), None));
    assert_eq!(green.brightness, 55);
    // an explicit brightness wins and becomes the new "on" level
    let dim = green.apply(&Action::Colour(Rgb([0, 0, 255]), Some(20)));
    assert_eq!((dim.brightness, dim.on_brightness), (20, 20));
  }

  #[test]
  fn conf_round_trips_and_reads_the_shell_format() {
    let s = State {
      colour: Rgb([0, 255, 0]),
      brightness: 0,
      on_brightness: 55,
    };
    assert_eq!(parse_conf(&format_conf(s)), Some(s));

    let shell = "# written by recoil16-lightbar; on_brightness=55\n\
                     options ite8233-lightbar default_brightness=55 default_color=0x00ff00\n";
    assert_eq!(
      parse_conf(shell),
      Some(State {
        colour: Rgb([0, 255, 0]),
        brightness: 55,
        on_brightness: 55
      })
    );
    // hand-written file without the comment
    let plain = "options ite8233-lightbar default_brightness=0 default_color=0x0000ff\n";
    assert_eq!(parse_conf(plain).unwrap().on_brightness, DEFAULT_ON);
  }

  #[test]
  fn set_writes_sysfs_and_boot_config() {
    let root = tempdir();
    put(&root, &format!("{LED}/brightness"), "0\n");
    put(&root, &format!("{LED}/multi_intensity"), "255 255 255\n");
    std::fs::create_dir_all(root.join("etc/modprobe.d")).unwrap();
    let sys = Sys::with_root(&root);

    let st = set(&sys, &Action::Colour(Rgb([0, 0, 255]), None), true).unwrap();
    assert_eq!(st.brightness, DEFAULT_ON);
    assert_eq!(
      sys.read(&format!("{LED}/multi_intensity")).unwrap(),
      "0 0 255"
    );
    assert_eq!(boot(&sys).unwrap(), Some(st));

    // --temp: sysfs changes, boot config does not
    set(&sys, &Action::Off, false).unwrap();
    assert_eq!(sys.read(&format!("{LED}/brightness")).unwrap(), "0");
    assert_eq!(boot(&sys).unwrap().unwrap().brightness, DEFAULT_ON);
  }

  #[test]
  fn missing_driver_is_reported() {
    let sys = Sys::with_root(tempdir());
    assert!(current(&sys).is_err());
    assert!(set(&sys, &Action::On, false).is_err());
  }
}
