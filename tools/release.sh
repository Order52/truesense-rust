#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Release tooling: the one place that knows where the version number lives. Plain bash, sed and awk (the Go edition's cmd/release, without a toolchain).
#   tools/release.sh check [--tag vX.Y.Z]     every file agrees on one version (and it matches the tag, when given); CI runs this on every push and on release
#   tools/release.sh version                  the current version (the workspace Cargo.toml)
#   tools/release.sh notes X.Y.Z              the release notes of that version (from RELEASE_NOTES.md) as GitHub release text
#   tools/release.sh prepare X.Y.Z --title T  bump everything, turn "## Unreleased" into this version, refresh Cargo.lock, run the gate, commit and tag
#                                             (--push also pushes: CI then publishes; --dry-run says what it would do; --no-gate skips the gate: CI runs it again anyway)
# prepare refuses a dirty tree, a version that is not newer, an existing tag, empty "Unreleased" notes or a missing title. Nothing is pushed unless you say --push.
# The source of truth is the workspace version in Cargo.toml (compiled into the program). The program's updater reads that same file: it needs the FIRST standalone
# `version = "x.y.z"` in the top Cargo.toml of a release to equal the tag, so `check` also proves no earlier `version = "..."` line exists above it.
set -Eeuo pipefail
ROOT=${TRUESENSE_ROOT:-$(realpath "$(dirname "$0")/..")}
SEMVER='[0-9]+\.[0-9]+\.[0-9]+'
NOTES=RELEASE_NOTES.md

# Where a version number is written: "file|text before it|text after it|how many times" (extended regular expressions). Listing the places (instead of replacing every occurrence
# of the old digits) keeps a stray mention of the same digits elsewhere untouched when the version is bumped. The first entry is the source of truth.
TABLE=(
  'Cargo.toml|^version = "|"$|1'
  'docs/INSTALL.md|TRUESENSE_VERSION=||1'
  'kernel/truesense-wmi/dkms.conf|PACKAGE_VERSION="|"|1'
  'kernel/truesense-wmi/truesense_wmi.c|#define TS_VERSION[[:space:]]+"|"|1'
  'tools/aur-publish.sh|tools/aur-publish\.sh ||1'
  'tools/get.sh|TRUESENSE_VERSION=| \(pin|1'
  'packaging/arch/PKGBUILD|^pkgver=|$|1'
  'packaging/aur/PKGBUILD.template|^pkgver=|$|1'
)

die() { printf '%s\n' "$*" >&2; exit 1; }
usage() { sed -n '3,11p' "$0" | sed 's/^# \{0,1\}//'; }

# the version the source of truth declares
current() {
  local v; v=$(sed -n -E "s/^version = \"($SEMVER)\"$/\\1/p" "$ROOT/Cargo.toml" 2>/dev/null | head -n1) || true
  [ -n "$v" ] || die "cannot find the workspace version in Cargo.toml"
  printf '%s\n' "$v"
}

# the first standalone `version = "..."` in a Cargo.toml, found the way the updater finds it (`rust-version` is not `version`)
first_standalone_version() {
  awk '{ if (match($0, /(^|[^A-Za-z0-9_-])version[ \t]*=[ \t]*"[^"]+"/)) { s = substr($0, RSTART, RLENGTH); sub(/^[^"]*"/, "", s); sub(/"$/, "", s); print s; exit } }' "$1"
}

# a is older than, equal to or newer than b: prints -1, 0 or 1
compare() { if [ "$1" = "$2" ]; then echo 0; elif [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | head -n1)" = "$1" ]; then echo -1; else echo 1; fi; }

# the notes under "## VERSION (date): title", without the heading, to the next "## " heading; empty when there is no such section
section() {
  awk -v p="## $1 (" 'index($0, p) == 1 && !f { rest = substr($0, length(p) + 1); if (index(rest, ")")) { f = 1; next } } f && /^## / { exit } f { print }' "$ROOT/$NOTES" | sed '/./,$!d' | sed -e :a -e '/^[[:space:]]*$/{$d;N;ba' -e '}'
}

# the short title after the date in a version's heading, or nothing
title_of() {
  awk -v p="## $1 (" 'index($0, p) == 1 { rest = substr($0, length(p) + 1); i = index(rest, ")"); if (i) { after = substr(rest, i + 1); if (substr(after, 1, 1) == ":") { sub(/^:[ \t]*/, "", after); sub(/[ \t]+$/, "", after); print after } exit } }' "$ROOT/$NOTES"
}

unreleased() {
  awk '$0 == "## Unreleased" { f = 1; next } f && /^## / { exit } f { print }' "$ROOT/$NOTES" | sed '/./,$!d' | sed -e :a -e '/^[[:space:]]*$/{$d;N;ba' -e '}'
}

# the crates of this workspace in Cargo.lock (the packages without a `source`) whose version is not $1: "name version" lines
lock_mismatches() {
  awk -v want="$1" '
    function flush() { if (name != "" && !src && ver != want) print name " " ver; name = ""; ver = ""; src = 0 }
    /^\[\[package\]\]/ { flush(); next }
    /^name = "/ { name = $3; gsub(/"/, "", name) }
    /^version = "/ { ver = $3; gsub(/"/, "", ver) }
    /^source = / { src = 1 }
    END { flush() }' "$ROOT/Cargo.lock"
}

# prints the problems that make this tree unreleasable, one per line (nothing means consistent)
problems() {
  local tag=${1:-} want entry file before after count text got v n name
  want=$(current)
  for entry in "${TABLE[@]}"; do
    IFS='|' read -r file before after count <<<"$entry"
    if [ ! -r "$ROOT/$file" ]; then echo "$file: cannot read"; continue; fi
    got=$(grep -o -E "$before$SEMVER$after" "$ROOT/$file" | grep -o -E "$SEMVER" || true)
    n=$(printf '%s' "$got" | grep -c . || true)
    [ "$n" -eq "$count" ] || echo "$file: expected $count version mention(s) like '$before<x.y.z>$after', found $n"
    for v in $got; do [ "$v" = "$want" ] || echo "$file: says $v, Cargo.toml says $want"; done
  done
  if [ ! -r "$ROOT/Cargo.lock" ]; then echo "Cargo.lock: cannot read (it is committed: the build uses --locked)"
  else
    grep -q '^name = "truesense"$' "$ROOT/Cargo.lock" || echo "Cargo.lock: does not list the truesense crate"
    lock_mismatches "$want" | while read -r name v; do echo "Cargo.lock: $name is $v, Cargo.toml says $want (run: cargo update --workspace --offline)"; done
  fi
  got=$(first_standalone_version "$ROOT/Cargo.toml")
  [ "$got" = "$want" ] || echo "Cargo.toml: the first standalone version = \"...\" is '${got:-none}', not $want: the updater would read the wrong version from a release"
  if [ ! -r "$ROOT/$NOTES" ]; then echo "$NOTES: cannot read"
  else
    text=$(cat "$ROOT/$NOTES")
    if ! grep -q -E "^## $(printf '%s' "$want" | sed 's/\./\\./g') \(" <<<"$text" && ! grep -q '^## Unreleased$' <<<"$text"; then echo "$NOTES: no section for $want"; fi
  fi
  if [ -n "$tag" ]; then
    [[ "$tag" =~ ^v?$SEMVER$ ]] || die "not a version: $tag (expected X.Y.Z)"
    [ "${tag#v}" = "$want" ] || echo "tag $tag does not match version $want"
  fi
}

cmd_check() {
  local tag="" out want
  while [ $# -gt 0 ]; do case $1 in --tag) tag=${2:?--tag needs a value}; shift 2;; *) die "check: unknown argument: $1";; esac; done
  want=$(current)
  out=$(problems "$tag")
  if [ -n "$out" ]; then printf 'version problems:\n%s\n' "$(sed 's/^/  /' <<<"$out")" >&2; return 1; fi
  echo "version $want: every file agrees${tag:+ (tag $tag)}"
}

cmd_notes() {
  local v=${1:?usage: release.sh notes X.Y.Z}; v=${v#v}
  local body head t
  body=$(section "$v"); [ -n "$body" ] || die "$NOTES has no notes for $v"
  head="TrueSense $v for the Acer Predator Helios 16 (PH16-71)"; t=$(title_of "$v")
  if [ -n "$t" ]; then head="$head: $t."; else head="$head."; fi
  printf '%s\n\n%s\n\n' "$head" "$body"
  printf '**Install / update:** `curl -fsSL https://raw.githubusercontent.com/Order52/truesense/main/tools/get.sh | sudo bash`, or from the window: Settings, Update (on a desktop without a polkit agent a terminal window opens for the password), or `truesense update apply` in a terminal.\n\n'
  printf 'Every release asset is listed in SHA256SUMS and carries a build provenance attestation (`gh attestation verify <file> --repo Order52/truesense`); the SBOM (`.sbom.cdx.json`) lists the Rust crates compiled into the program. What is verified on hardware and what is not is stated in the notes above and in docs/SAFETY.md.\n'
}

# rewrites the version only where the table says it lives
bump() {
  local new=$1 entry file before after count
  for entry in "${TABLE[@]}"; do
    IFS='|' read -r file before after count <<<"$entry"
    # `|` is the sed delimiter because the patterns hold slashes (and no `|`: the table uses it as its field separator)
    sed -i -E "s|($before)$SEMVER($after)|\\1$new\\2|g" "$ROOT/$file"
  done
}

retitle() { # version title today
  local head="## $1 ($3)"; [ -z "$2" ] || head="$head: $2"
  HEAD=$head awk '$0 == "## Unreleased" && !d { print ENVIRON["HEAD"]; d = 1; next } { print }' "$ROOT/$NOTES" > "$ROOT/$NOTES.new" && mv "$ROOT/$NOTES.new" "$ROOT/$NOTES"
}

cmd_prepare() {
  local ver="" title="" gate=1 push=0 dry=0 old tag
  while [ $# -gt 0 ]; do case $1 in
    --title) title=${2:?--title needs a value}; shift 2;; --no-gate) gate=0; shift;; --push) push=1; shift;; --dry-run) dry=1; shift;;
    -*) die "prepare: unknown option: $1";; *) [ -z "$ver" ] || die "prepare takes one version"; ver=${1#v}; shift;; esac; done
  [ -n "$ver" ] || die "usage: release.sh prepare X.Y.Z --title T"
  [[ "$ver" =~ ^$SEMVER$ ]] || die "not a version: $ver (expected X.Y.Z)"
  old=$(current)
  [ "$(compare "$ver" "$old")" = 1 ] || die "$ver is not newer than $old"
  [ -n "$(unreleased)" ] || die "$NOTES has no notes under \"## Unreleased\": write what changed first"
  [ -n "$title" ] || die 'give the release a short title: --title "..."'
  git -C "$ROOT" rev-parse --git-dir >/dev/null 2>&1 || die "git status failed (is $ROOT a git repository?)"
  [ -z "$(git -C "$ROOT" status --porcelain)" ] || die "the working tree has uncommitted changes: commit or stash them first"
  tag=v$ver
  ! git -C "$ROOT" rev-parse -q --verify "refs/tags/$tag" >/dev/null || die "tag $tag already exists"
  echo "release $old -> $ver: $title"
  if [ "$dry" = 1 ]; then echo "(dry run: nothing changed)"; return 0; fi
  command -v cargo >/dev/null || die "cargo is required (it refreshes Cargo.lock)"
  bump "$ver"; retitle "$ver" "$title" "${TODAY:-$(date +%F)}"
  step cargo update --workspace --offline --manifest-path "$ROOT/Cargo.toml"          # the workspace crates' versions in Cargo.lock; no dependency moves
  local out; out=$(problems "")
  [ -z "$out" ] || die "after bumping:
$(sed 's/^/  /' <<<"$out")"
  if [ "$gate" = 1 ]; then
    (cd "$ROOT" && step make ci && step bash tools/ci/check-shell.sh && step bash tools/shelltest/run.sh)
  fi
  step git -C "$ROOT" add -A
  step git -C "$ROOT" commit -q -m "Release $ver: $title"
  step git -C "$ROOT" tag -a "$tag" -m "TrueSense $ver"
  if [ "$push" = 0 ]; then echo "committed and tagged $tag. Publish with:  git push origin HEAD $tag"; return 0; fi
  step git -C "$ROOT" push origin HEAD "$tag"
  echo "pushed: CI publishes https://github.com/Order52/truesense/releases/tag/$tag"
}

step() { echo "  \$ $*"; "$@"; }

main() {
  local c=${1:-}
  case $c in
    check) shift; cmd_check "$@";;
    version) current;;
    notes) shift; cmd_notes "$@";;
    prepare) shift; cmd_prepare "$@";;
    -h|--help|help) usage;;
    "") usage >&2; exit 2;;
    *) printf 'release.sh: unknown command %s\n\n' "$c" >&2; usage >&2; exit 2;;
  esac
}
main "$@"
