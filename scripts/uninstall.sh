#!/bin/sh
# Remove what scripts/install.sh installed.
set -e
cd "$(dirname "$0")/.."
. scripts/common.sh
require_root

rm -f /usr/local/bin/recoil16ctl /usr/local/share/kglobalaccel/recoil16.desktop
remove_ctl_extras /usr/local/share
rm -f /etc/udev/rules.d/90-recoil16-charge-mode.rules \
      /etc/udev/rules.d/90-recoil16-charge-limit.rules \
      /etc/udev/rules.d/90-recoil16-charge-profile.rules \
      /etc/udev/rules.d/90-uniwill-charge-profile.rules
udevadm control --reload
echo "recoil16ctl uninstalled. The EC returns to Standard at the next boot."
