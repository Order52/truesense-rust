#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Syntax-check (bash -n) and shellcheck (warnings and up) every shell script we ship. CI and `tools/release.sh prepare` run this.
#   tools/ci/check-shell.sh                shellcheck is optional locally (skipped with a note), required when CI=true or REQUIRE_SHELLCHECK=1
set -Eeuo pipefail
cd "$(dirname "$0")/../.."
shopt -s nullglob
scripts=(tools/*.sh tools/ci/*.sh tools/shelltest/*.sh packaging/health.sh)
for f in "${scripts[@]}"; do bash -n "$f" && echo "ok  bash -n $f"; done
if command -v shellcheck >/dev/null 2>&1; then
  shellcheck -S warning -x "${scripts[@]}" && echo "ok  shellcheck (${#scripts[@]} scripts)"
elif [ "${CI:-}" = true ] || [ "${REQUIRE_SHELLCHECK:-}" = 1 ]; then
  echo "!! shellcheck is required here but not installed" >&2; exit 1
else
  echo "skip shellcheck (not installed)"
fi
