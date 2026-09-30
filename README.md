# recoil16ctl

`recoil16ctl` is a small Rust command-line tool that controls the
[recoil16](https://github.com/amad3v/recoil16) drivers for the
**PCSpecialist Recoil 16 AMD** (TUXEDO Stellaris 16 Gen7): charge modes,
battery health, lightbar, power profiles, Fn/Super lock, screen rotation,
NVIDIA GPU power and offload, and battery draw.

> [!WARNING]
> **Use at your own risk.** Everything in this README was thoroughly tested,
> physically, on a PCSpecialist Recoil 16 AMD. Nothing was
> tested on other models, including the TUXEDO Stellaris 16 Gen7 it is based
> on. recoil16ctl changes charge modes, power and boot settings through the
> recoil16 drivers. The author takes no responsibility for any damage to your
> laptop, its battery or your data. The software comes with no warranty
> (GPL-2.0, sections 11 and 12).

## Install

```sh
# Arch Linux (AUR): latest release, or recoil16ctl-git for the latest commit
yay -S recoil16ctl

# or, from source on any distribution (needs cargo)
sudo scripts/install.sh
```

Install the [recoil16](https://github.com/amad3v/recoil16) drivers too (most
commands need them), then run `recoil16ctl check`.

Upgrading from recoil16-dkms 1.3.0 or older: `yay -Syu` first (recoil16-dkms
1.4.0 drops these files), then `yay -S recoil16ctl`.

## Commands

A small Rust command that controls everything the drivers expose. Reading
works as a normal user; changing settings needs `sudo`, except `screen`,
which must run as your desktop user.

```sh
recoil16ctl                                # status: battery, profile, keyboard, lightbar, gpu, fans
recoil16ctl check                          # verify the installation (alias: verify)
recoil16ctl power                          # battery draw (5 s average), time left, GPU, panel
recoil16ctl power --watch --battery BAT0   # every 2 s until Ctrl+C, one battery only
recoil16ctl gpu                            # NVIDIA GPU power state, driver, which of your apps use it
recoil16ctl gpu run -- freecad             # run a program on the NVIDIA GPU
recoil16ctl gpu test                       # prove offloading works (wakes the GPU briefly)
recoil16ctl battery                        # charge, voltage, charge mode, boot rule
recoil16ctl battery status                 # true health, cycles, charge mode (from the EC)
sudo recoil16ctl battery mode long-life [--temp] # standard | long-life | trickle; saved for boot unless --temp
sudo recoil16ctl battery clear-rule        # drop the boot rule (Standard from the next boot)
sudo recoil16ctl lightbar blue 60 [--temp] # https://github.com/amad3v/recoil16#lightbar
sudo recoil16ctl profile performance       # low-power | balanced | performance | cycle
recoil16ctl keyboard                       # backlight level, Fn lock, Super key
sudo recoil16ctl keyboard fn-lock on       # F1-F12 without Fn
sudo recoil16ctl keyboard super-key off    # same as Fn+F2
recoil16ctl screen                         # built-in panel orientation (KDE)
recoil16ctl screen rotate                  # flip it 180°; run as your user, not sudo
recoil16ctl completions zsh                # bash | zsh | fish | elvish | powershell
```

The AUR package installs the man page (`man recoil16ctl`) and the bash, zsh
and fish completions. `profile` changes only the handler registered by
`uniwill-laptop`, and power-profiles-daemon (the KDE/GNOME slider) follows it.

## Usage examples

**Is everything working?**

```sh
recoil16ctl check          # drivers, settings, and a "Power" group with fixes for anything off
recoil16ctl                # one line per feature
```

**How long will the battery last?**

```sh
recoil16ctl power          # waits 5 s, then: draw in W, % and time left, GPU and panel state
recoil16ctl power --watch  # a fresh 5 s average every 2 s; Ctrl+C to stop
```

**Keep the battery healthy**

```sh
sudo recoil16ctl battery mode long-life       # stop at ~93 %, also after reboots
sudo recoil16ctl battery mode trickle --temp  # ~90 %, this boot only
recoil16ctl battery status                    # learned health and cycle count
```

**Run an app on the NVIDIA GPU**

```sh
recoil16ctl gpu run -- freecad    # or blender, a game, anything drawn with OpenGL/Vulkan
recoil16ctl gpu                   # asleep? which of your apps hold it?
recoil16ctl gpu test              # checks that offloaded apps really land on it
```

Everything else runs on the AMD GPU and the NVIDIA GPU sleeps. Only apps that
draw with OpenGL or Vulkan get faster; offloading others (Inkscape, a browser)
only wakes the GPU and costs battery. In KDE, right-click an app in the menu →
**Edit Application… → Advanced → Run using dedicated graphics card** makes it
permanent. `gpu test` needs `mesa-utils` and `vulkan-tools`.

**Faster or quieter**

```sh
sudo recoil16ctl profile performance   # turbo; low-power / balanced; cycle = mode button
```

**Lights and keys**

```sh
sudo recoil16ctl lightbar purple 40    # colour and brightness, also after reboots
sudo recoil16ctl lightbar off
sudo recoil16ctl keyboard fn-lock on   # F1-F12 without holding Fn
recoil16ctl screen rotate              # flip the screen 180° (the Sc key does this too)
```

## Power on the hybrid GPU

The internal panel runs on the AMD GPU; the NVIDIA GPU should be fully off
until an app is offloaded to it. Out of the box several things keep it on.
`recoil16ctl check` lists them in its **Power** group; these are the fixes.

**Let the kernel suspend the NVIDIA GPU** (it otherwise idles at ~9 W).
`/etc/udev/rules.d/80-nvidia-pm.rules`:

```
ACTION=="bind", SUBSYSTEM=="pci", ATTR{vendor}=="0x10de", ATTR{class}=="0x030000", TEST=="power/control", ATTR{power/control}="auto"
ACTION=="bind", SUBSYSTEM=="pci", ATTR{vendor}=="0x10de", ATTR{class}=="0x040300", TEST=="power/control", ATTR{power/control}="auto"
ACTION=="unbind", SUBSYSTEM=="pci", ATTR{vendor}=="0x10de", ATTR{class}=="0x030000", TEST=="power/control", ATTR{power/control}="on"
```

and `/etc/modprobe.d/nvidia-pm.conf`:

```
options nvidia "NVreg_DynamicPowerManagement=0x02"
```

**Let the network, card reader and NVMe controllers suspend** (the PCI classes
`check` looks at: Ethernet, Wi-Fi, SD host, NVMe).
`/etc/udev/rules.d/81-pci-runtime-pm.rules`:

```
ACTION=="add", SUBSYSTEM=="pci", ATTR{class}=="0x0200*", TEST=="power/control", ATTR{power/control}="auto"
ACTION=="add", SUBSYSTEM=="pci", ATTR{class}=="0x0280*", TEST=="power/control", ATTR{power/control}="auto"
ACTION=="add", SUBSYSTEM=="pci", ATTR{class}=="0x0805*", TEST=="power/control", ATTR{power/control}="auto"
ACTION=="add", SUBSYSTEM=="pci", ATTR{class}=="0x0108*", TEST=="power/control", ATTR{power/control}="auto"
```

**Install the AMD Vulkan driver**: `sudo pacman -S vulkan-radeon`. Without it
every Vulkan app runs on the NVIDIA GPU.

**Don't early-load the NVIDIA modules**: keep `nvidia*` out of `MODULES=` in
`/etc/mkinitcpio.conf`. The AMD GPU drives the panel at boot, and the NVIDIA
firmware adds about 110 MB to every boot image.

Reboot, then `recoil16ctl gpu` should show `suspended` when no app uses it.
Don't check with `nvidia-smi`: it wakes the GPU to answer.

## Uninstall

`yay -R recoil16ctl` (or `sudo scripts/uninstall.sh`) removes the tool and its
charge-mode boot rule (the EC returns to Standard at the next boot); switching
between `recoil16ctl` and `recoil16ctl-git` also removes the rule, so run
`sudo recoil16ctl battery mode …` again afterwards.

## Versioning

recoil16ctl is versioned independently of the recoil16 drivers, starting at
1.4.0. Releases up to 1.3.0 were made from the
[recoil16](https://github.com/amad3v/recoil16) repository, where recoil16ctl
and the DKMS modules shared one version. Releases are tagged `vX.Y.Z`, and the
`recoil16ctl-git` Arch package version is derived from the tag
(`1.0.0.r<commits since the tag>.g<hash>`). Changes per release are in
[CHANGELOG.md](CHANGELOG.md).

## License

GPL-2.0-only. See [LICENSE](LICENSE).
