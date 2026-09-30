# Changelog

All notable changes to recoil16ctl.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Releases up to 1.3.0 were made from the
[recoil16](https://github.com/amad3v/recoil16) repository, where recoil16ctl
and the drivers shared one version.

## [Unreleased]

## [1.4.0] - 2026-09-30

### Changed

- recoil16ctl has its own repository and AUR packages (`recoil16ctl`,
  `recoil16ctl-git`); versions are independent of the drivers from now on; the
  KDE Sc shortcut and the removal of the charge-mode boot rule moved to this
  package.

## [1.3.0] - 2026-09-28

### Added

- `recoil16ctl gpu`: the NVIDIA GPU's runtime power state (read without waking
  it), driver settings, which of your apps use it, and what else has its
  display device open.
- `recoil16ctl gpu run -- <command>`: run a program on the NVIDIA GPU (PRIME
  render offload for OpenGL and Vulkan).
- `recoil16ctl gpu test`: wakes the GPU and checks that offloaded apps land on
  it and others don't (Vulkan, and OpenGL on the Wayland session).
- `recoil16ctl power`: battery draw averaged over 5 s, time left or charging
  power, charge mode, platform profile, GPU state, panel mode and backlight;
  `--watch` keeps updating, `--battery` picks one battery.
- `recoil16ctl check`: a Power group — NVIDIA runtime PM (GPU and its audio
  function), dynamic power management, an idle GPU kept awake, NVIDIA modules
  early-loaded in the initramfs, the AMD Vulkan driver, and PCI devices that
  never suspend. Warnings only; each names its fix.
- `recoil16ctl` status shows the GPU state.
- README: usage examples by task, and "Power on the hybrid GPU".
- This changelog.

### Changed

- Arch packages: optional dependencies `mesa-utils` and `vulkan-tools` for
  `gpu test`.

## [1.2.1] - 2026-09-27

### Changed

- Fn+F6 / Fn+F7 can be handled by ite8291-mono itself (module parameter
  `key_step`, e.g. 10): the keyboard backlight keys then also work at the login
  screen and on a text console. Off by default (`key_step=0`): the desktop is
  then told about these changes only through UPower, and UPower 1.91.4 stops
  watching at startup (fixed upstream, unreleased). By default the keys stay
  with the desktop, as in 1.1.0.

## [1.2.0] - 2026-09-27 [YANKED]

Tagged, not released: driver handling of Fn+F6 / Fn+F7 on by default, which
the UPower issue above breaks.

### Added

- Fn+F6 / Fn+F7 handled by ite8291-mono itself (an input filter on the Uniwill
  hotkey device), stepping the brightness by `key_step` and reporting it as a
  hardware change.

### Changed

- `patches/` written with a zero hash in each From line (content unchanged).

## [1.1.0] - 2026-09-27

### Added

- `recoil16ctl battery status`: the battery's learned health and cycle count
  from the EC (via uniwill-laptop `state_of_health`, `battery_cycle_count`,
  `battery_full_capacity`).

### Changed

- Charge modes replace the percentage charge limit: Standard (4.35 V per cell),
  Long_Life (4.30 V, about 93%) and Trickle (4.20 V, about 90%).
  `recoil16ctl battery mode` sets the mode and a boot rule; upgrading removes
  the 1.0.0 limit rule, and `check` reports one left behind.
- The platform profile is marked experimental (not upstream).
- `patches/` follows the upstream v2 series.

### Removed

- The percentage charge limit (EC 0x07B9): a Uniwill preview feature that may
  damage the battery on this model. `battery limit` explains the change.

## [1.0.0] - 2026-09-25

### Added

- uniwill-laptop: the mainline driver (7.2) with the Recoil 16 DMI entry,
  platform profile support for the firmware power modes, and the Sc key.
- ite8291-mono: monochrome, colour-corrected keyboard backlight.
- ite8233-lightbar: RGB lightbar.
- copilot-rctrl: the Copilot key back to Right Ctrl.
- recoil16ctl: status, check, battery charge limit, lightbar, power profile,
  keyboard locks and screen rotation, with man pages and shell completions.
- DKMS packaging, Arch PKGBUILD, install scripts and a KDE shortcut for Sc.

[Unreleased]: https://github.com/amad3v/recoil16ctl/compare/v1.4.0...HEAD
[1.4.0]: https://github.com/amad3v/recoil16ctl/compare/v1.3.0...v1.4.0
[1.3.0]: https://github.com/amad3v/recoil16ctl/compare/v1.2.1...v1.3.0
[1.2.1]: https://github.com/amad3v/recoil16ctl/compare/v1.2.0...v1.2.1
[1.2.0]: https://github.com/amad3v/recoil16ctl/compare/v1.1.0...v1.2.0
[1.1.0]: https://github.com/amad3v/recoil16ctl/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/amad3v/recoil16ctl/releases/tag/v1.0.0
