#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Verify the release bundle (tools/ci/build-bundle.sh): it must hold what an installer and the program's updater need, and a program that runs, is static and says the right version.
# With a second directory (another run of build-bundle.sh, even on another machine) the two must also be byte-identical: reproducible.
#   tools/ci/check-bundle.sh X.Y.Z DIR_A [DIR_B]
set -Eeuo pipefail
VER=${1:?usage: check-bundle.sh X.Y.Z DIR_A [DIR_B]}; A=${2:?usage: check-bundle.sh X.Y.Z DIR_A [DIR_B]}; B=${3:-}
fail() { echo "::error::$*" >&2; exit 1; }

for f in "truesense-$VER.tar.gz" truesense; do
  if [ -n "$B" ]; then
    cmp -s "$A/$f" "$B/$f" || fail "$f is not reproducible: two builds of the same commit differ ($(sha256sum "$A/$f" | cut -c1-12) vs $(sha256sum "$B/$f" | cut -c1-12))"
    echo "reproducible: $f $(sha256sum "$A/$f" | cut -d' ' -f1)"
  else echo "$f $(sha256sum "$A/$f" | cut -d' ' -f1)"; fi
done

tmp=$(mktemp -d); trap 'rm -r -f "$tmp"' EXIT
tar -tzf "$A/truesense-$VER.tar.gz" > "$tmp/list"
# tools/install.sh, kernel/truesense-wmi/truesense_wmi.c and a Cargo.toml are what the program's own updater requires of a release (ts-system::update Layout::rust_edition)
for f in tools/install.sh tools/get.sh tools/uninstall.sh bin/truesense Cargo.toml Cargo.lock crates/truesense/src/main.rs \
         kernel/truesense-wmi/truesense_wmi.c kernel/truesense-wmi/dkms.conf packaging/truesense.service packaging/health.sh tools/nvml/truesense-nvml.c LICENSE; do
  grep -qx "truesense-$VER/$f" "$tmp/list" || fail "the bundle lacks $f"
done
if grep -E '\.(ko|o|mod|mod\.c)$|/Module\.symvers$|/modules\.order$|/\.[^/]*\.cmd$' "$tmp/list" | grep -q .; then fail "the bundle holds kernel build leftovers"; fi
tar -tvzf "$A/truesense-$VER.tar.gz" "truesense-$VER/bin/truesense" | grep -q '^-rwxr-xr-x 0/0 ' || fail "bin/truesense must be executable and owned by root:root in the bundle"
tar -xzf "$A/truesense-$VER.tar.gz" -C "$tmp"
bin="$tmp/truesense-$VER/bin/truesense"
got=$("$bin" --version) || fail "the bundled program does not run"
[ "$got" = "truesense $VER" ] || fail "the bundled program says '$got', expected 'truesense $VER'"
# `ldd` is no test: it exits 0 for a static-pie program. Static means no interpreter and no needed libraries.
if readelf -lW "$bin" | grep -q INTERP || readelf -dW "$bin" 2>/dev/null | grep -q NEEDED; then fail "the program is dynamically linked: it must be static"; fi
cmp -s "$bin" "$A/truesense" || fail "bin/truesense in the bundle is not the program that was built"
# what the updater reads: the first standalone `version = "..."` of the top Cargo.toml must be the version (and so the tag)
first=$(awk '{ if (match($0, /(^|[^A-Za-z0-9_-])version[ \t]*=[ \t]*"[^"]+"/)) { s = substr($0, RSTART, RLENGTH); sub(/^[^"]*"/, "", s); sub(/"$/, "", s); print s; exit } }' "$tmp/truesense-$VER/Cargo.toml")
[ "$first" = "$VER" ] || fail "the first standalone version in the bundle's Cargo.toml is '$first', not $VER: the updater would refuse the release"
echo "bundle ok: $(wc -l < "$tmp/list") entries, $got, static"
