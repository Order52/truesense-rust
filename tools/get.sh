#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# One-line installer (Rust edition). Downloads the newest TrueSense release bundle from GitHub (the source tree plus the prebuilt static program bin/truesense), checks it against the
# release's SHA256SUMS, and runs the bundle's tools/install.sh.
#   curl -fsSL https://raw.githubusercontent.com/Order52/truesense/main/tools/get.sh | sudo bash
#   ... | sudo bash -s -- --no-gui          (extra options go to install.sh)
# Environment: TRUESENSE_VERSION=0.5.0 (pin a version), TRUESENSE_SHA256=<hash> (also require this exact hash),
#              TRUESENSE_VERIFY_ATTESTATION=1 (also check the build provenance with the GitHub CLI: gh attestation verify)
# Every release is published by CI with a SHA256SUMS file and the download MUST match it: a release without the list is refused, never fetched unchecked. Only bundles of this edition
# are accepted (the version is read from the workspace Cargo.toml inside the download), so an older Python or Go release is refused.
set -Eeuo pipefail
REPO=${TRUESENSE_REPO:-Order52/truesense}
WEB=${TRUESENSE_WEB:-https://github.com}; API=${TRUESENSE_API:-https://api.github.com}          # overridable for the tests (a local fake GitHub)
die() { echo "!! $*" >&2; exit 1; }
if [ -z "${TRUESENSE_GET_ONLY:-}" ]; then [ "$(id -u)" -eq 0 ] || die "run as root:  curl -fsSL https://raw.githubusercontent.com/$REPO/main/tools/get.sh | sudo bash"; fi
for c in curl tar sha256sum; do command -v "$c" >/dev/null || die "$c is required (install it with your package manager)"; done
VERSION="${TRUESENSE_VERSION:-}"
if [ -z "$VERSION" ]; then                                   # newest tag by version order (no jq needed)
  VERSION=$(curl -fsSL "$API/repos/$REPO/tags?per_page=100" | sed -n 's/.*"name": *"v\([0-9][0-9.]*\)".*/\1/p' | sort -V | tail -n1)
  [ -n "$VERSION" ] || die "could not find the latest version (GitHub unreachable or rate-limited): set TRUESENSE_VERSION=x.y.z"
fi
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "not a version: $VERSION"
TMP=$(mktemp -d); trap 'rm -r -f "$TMP"' EXIT
ASSET="truesense-$VERSION.tar.gz"; REL="$WEB/$REPO/releases/download/v$VERSION"
echo "TrueSense $VERSION: downloading"
CODE=$(curl --retry 3 --retry-delay 2 -sSL -o "$TMP/SHA256SUMS" -w '%{http_code}' "$REL/SHA256SUMS" || echo 000)
case $CODE in
  200)
    WANT=$(awk -v f="$ASSET" '{n=$2; sub(/^\*/,"",n); if (n==f) print $1}' "$TMP/SHA256SUMS" | head -n1)
    [ -n "$WANT" ] || die "SHA256SUMS of v$VERSION does not list $ASSET: refusing"
    curl --retry 3 --retry-delay 2 -fsSL -o "$TMP/ts.tar.gz" "$REL/$ASSET"
    GOT=$(sha256sum "$TMP/ts.tar.gz" | awk '{print $1}')
    [ "$GOT" = "$WANT" ] || die "checksum mismatch for $ASSET (expected ${WANT:0:12}..., got ${GOT:0:12}...): refusing to install"
    echo "  checksum verified (${GOT:0:12}...)"
    if [ "${TRUESENSE_VERIFY_ATTESTATION:-}" = 1 ]; then
      command -v gh >/dev/null || die "TRUESENSE_VERIFY_ATTESTATION=1 needs the GitHub CLI (gh)"
      gh attestation verify "$TMP/ts.tar.gz" --repo "$REPO" >/dev/null || die "the build provenance attestation does not verify: refusing"
      echo "  build provenance verified"
    fi ;;
  404) die "v$VERSION has no SHA256SUMS: refusing to install something that cannot be verified" ;;
  *) die "could not fetch the release checksums (HTTP $CODE): try again later" ;;
esac
[ -z "${TRUESENSE_SHA256:-}" ] || echo "$TRUESENSE_SHA256  $TMP/ts.tar.gz" | sha256sum -c -
tar -xzf "$TMP/ts.tar.gz" -C "$TMP"
for d in "$TMP"/truesense-*/; do [ -f "$d/tools/install.sh" ] || die "the download is not a TrueSense release"; done
# the version the download declares: the first standalone `version = "x.y.z"` of its workspace Cargo.toml (the way the program's own updater reads it; `rust-version` is not `version`).
# A missing file must not stop the script silently (pipefail): the check below says what is wrong.
INSIDE=$(awk '{ if (match($0, /(^|[^A-Za-z0-9_-])version[ \t]*=[ \t]*"[^"]+"/)) { s = substr($0, RSTART, RLENGTH); sub(/^[^"]*"/, "", s); sub(/"$/, "", s); print s; exit } }' "$TMP"/truesense-*/Cargo.toml 2>/dev/null | head -n1) || true
[ "$INSIDE" = "$VERSION" ] || die "the download says version ${INSIDE:-?}, not $VERSION: refusing"
if [ -n "${TRUESENSE_GET_ONLY:-}" ]; then echo "verified and unpacked (TRUESENSE_GET_ONLY set: not installing)"; exit 0; fi
cd "$TMP"/truesense-*/ && exec bash tools/install.sh "$@"
