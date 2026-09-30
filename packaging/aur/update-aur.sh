#!/bin/sh
# Copy an AUR package's files into a clone of its AUR repository and
# regenerate .SRCINFO. Commit and push in the clone afterwards.
#
# Usage: packaging/aur/update-aur.sh <recoil16ctl|recoil16ctl-git> <aur clone>
#   first time: git clone ssh://aur@aur.archlinux.org/<package>.git <aur clone>
set -e
pkg="${1:?usage: $0 <recoil16ctl|recoil16ctl-git> <aur clone>}"
dest="${2:?usage: $0 <recoil16ctl|recoil16ctl-git> <aur clone>}"
here=$(cd "$(dirname "$0")" && pwd)

[ -f "$here/$pkg/PKGBUILD" ] || { echo "unknown package: $pkg" >&2; exit 1; }
[ -d "$dest/.git" ] || {
	echo "$dest is not a git clone; run: git clone ssh://aur@aur.archlinux.org/$pkg.git $dest" >&2
	exit 1
}

# makepkg needs recoil16ctl.install as a regular file next to each PKGBUILD, so
# both packages carry a copy; they must stay identical
cmp -s "$here/recoil16ctl/recoil16ctl.install" "$here/recoil16ctl-git/recoil16ctl.install" || {
	echo "recoil16ctl.install differs between recoil16ctl and recoil16ctl-git; sync them first" >&2
	exit 1
}

cp "$here/$pkg/PKGBUILD" "$here/$pkg/recoil16ctl.install" "$dest/"
cd "$dest"

if [ "$pkg" = recoil16ctl-git ]; then
	# fetch the source and let pkgver() write the current version into PKGBUILD
	makepkg --nobuild --nodeps --noprepare --cleanbuild >/dev/null
else
	# the release archive must match the pinned checksum
	makepkg --verifysource --nodeps >/dev/null
fi
makepkg --printsrcinfo >.SRCINFO

# the AUR repository holds only these files
printf '*\n!PKGBUILD\n!recoil16ctl.install\n!.SRCINFO\n!.gitignore\n' >.gitignore

echo "== $pkg $(sed -n 's/^\tpkgver = //p' .SRCINFO)-$(sed -n 's/^\tpkgrel = //p' .SRCINFO)"
git status --short
echo "Review, then: git add PKGBUILD recoil16ctl.install .SRCINFO .gitignore && git commit -m '...' && git push origin HEAD:master"
