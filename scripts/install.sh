#!/bin/sh
# Build recoil16ctl and install it, its shell completions, man page and KDE
# shortcut. Manual alternative to the Arch package. Needs cargo (Rust).
# Set a battery charge mode after the recoil16 drivers are installed:
#   sudo recoil16ctl battery mode long-life
# Usage: sudo scripts/install.sh
set -e
cd "$(dirname "$0")/.."
. scripts/common.sh
require_root
command -v cargo >/dev/null || { echo "cargo not found: install Rust (e.g. pacman -S rust)" >&2; exit 1; }

# build as the invoking user so target/ is not owned by root;
# CARGO_TARGET_DIR overrides any target-dir from a cargo config
build="CARGO_TARGET_DIR=target cargo build --release --locked"
if [ -n "${SUDO_USER:-}" ]; then su "$SUDO_USER" -c "$build"; else sh -c "$build"; fi

install -m 755 target/release/recoil16ctl /usr/local/bin/recoil16ctl
install -Dm644 data/kglobalaccel/recoil16.desktop -t /usr/local/share/kglobalaccel/
install_ctl_extras /usr/local/bin/recoil16ctl /usr/local/share

echo
echo "recoil16ctl installed. Sc shortcut active after the next login."
echo "Set a charge mode (after the drivers are installed): sudo recoil16ctl battery mode long-life"
echo "Verify: recoil16ctl check"
