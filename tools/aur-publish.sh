#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Publish/update the AUR package "truesense" for a released tag (the PKGBUILD builds the Rust program from the tag archive). Run by the maintainer or by the release workflow (needs the AUR SSH key).
#   tools/aur-publish.sh 0.5.0
set -Eeuo pipefail
VER=${1:?usage: aur-publish.sh X.Y.Z}; ROOT=$(realpath "$(dirname "$0")/.."); W=$(mktemp -d)
REPO=${TRUESENSE_REPO:-Order52/truesense-rust}
[[ "$VER" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "!! not a version: $VER" >&2; exit 1; }
curl -fsSL -o "$W/src.tgz" "https://github.com/$REPO/archive/refs/tags/v$VER.tar.gz"; SHA=$(sha256sum "$W/src.tgz" | cut -d' ' -f1)
# the package builds the program from this archive with `cargo build --locked`: it must be the source of this version (a workspace Cargo.toml whose first standalone version is $VER,
# the rule the program's own updater applies) and have a Cargo.lock, or the AUR would ship another release
first=$(tar -xOzf "$W/src.tgz" "truesense-$VER/Cargo.toml" | awk '{ if (match($0, /(^|[^A-Za-z0-9_-])version[ \t]*=[ \t]*"[^"]+"/)) { s = substr($0, RSTART, RLENGTH); sub(/^[^"]*"/, "", s); sub(/"$/, "", s); print s; exit } }') || true
[ "$first" = "$VER" ] || { echo "!! the v$VER archive declares version '${first:-?}' in Cargo.toml, not $VER: refusing" >&2; exit 1; }
tar -tzf "$W/src.tgz" "truesense-$VER/Cargo.lock" >/dev/null 2>&1 || { echo "!! the v$VER archive has no Cargo.lock: the package could not be built reproducibly: refusing" >&2; exit 1; }
git clone ssh://aur@aur.archlinux.org/truesense.git "$W/aur"
sed -e "s/^pkgver=.*/pkgver=$VER/" -e "s/^pkgrel=.*/pkgrel=1/" -e "s/@SHA@/$SHA/" "$ROOT/packaging/aur/PKGBUILD.template" > "$W/aur/PKGBUILD"
cp "$ROOT/packaging/aur/truesense.install" "$W/aur/"
cd "$W/aur" && makepkg --printsrcinfo > .SRCINFO && git add PKGBUILD truesense.install .SRCINFO
git -c user.name="${GIT_AUTHOR_NAME:-$(git config user.name)}" commit -m "truesense $VER" && git push origin HEAD:master
