#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Runs the tests of the shipped shell scripts (the installer, get.sh, release.sh, the packaging): every tools/shelltest/test_*.sh, each in its own bash.
#   tools/shelltest/run.sh               all of them
#   tools/shelltest/run.sh installer     only test_installer.sh   (a second argument selects tests by name: run.sh get checksum)
# Needs bash, coreutils, tar, curl, git and a Rust compiler (rustc builds the small fake GitHub that get.sh talks to; it is std-only and lives in fakegithub.rs).
# Nothing is installed and nothing outside a temporary directory is written. CI treats a missing compiler as an error; locally the get.sh tests are skipped with a note.
set -Eeuo pipefail
here=$(realpath "$(dirname "$0")")
work=$(mktemp -d); trap 'rm -r -f "$work"' EXIT
if command -v rustc >/dev/null 2>&1; then
  rustc --edition 2021 -O -o "$work/fakegithub" "$here/fakegithub.rs"
  export TS_FAKEGITHUB="$work/fakegithub"
elif [ "${CI:-}" = true ]; then echo "!! rustc is required here (it builds the fake GitHub)" >&2; exit 1
else echo "note: rustc not found: the tests that need the fake GitHub will fail (install Rust, or run them in CI)"; fi

failed=0; files=()
if [ -n "${1:-}" ]; then files=("$here/test_$1.sh"); else for f in "$here"/test_*.sh; do files+=("$f"); done; fi
for f in "${files[@]}"; do
  [ -f "$f" ] || { echo "no such test file: $f" >&2; exit 2; }
  bash "$f" ${2:+"$2"} || failed=1
done
[ "$failed" -eq 0 ] && echo "shell tests: all passed" || { echo "shell tests: FAILED" >&2; exit 1; }
