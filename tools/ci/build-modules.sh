#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Build the kernel module against EVERY kernel header set installed on this machine (warnings on), and report each one.
#   tools/ci/build-modules.sh               all of /lib/modules/*/build
#   tools/ci/build-modules.sh 6.12.1-arch1  only that version
# CI runs it inside Arch, Ubuntu, Debian and Fedora containers (each with its own distribution kernel headers), so a kernel API change is caught before users hit it.
set -Eeuo pipefail
ROOT=$(realpath "$(dirname "$0")/../..")
shopt -s nullglob
declare -a want=("$@"); built=0; failed=0; declare -A seen=()
# header sets: /lib/modules/<v>/build (most distributions) and /usr/src/kernels/<v> (Fedora and relatives, where kernel-devel alone does not create the link)
declare -a sets=()
for b in /lib/modules/*/build /usr/lib/modules/*/build; do sets+=("$(basename "$(dirname "$b")")|$b"); done
for b in /usr/src/kernels/*; do sets+=("$(basename "$b")|$b"); done
for entry in "${sets[@]}"; do
  v=${entry%%|*}; b=${entry#*|}
  [ -e "$b/Makefile" ] || continue
  [ -z "${seen[$v]:-}" ] || continue; seen[$v]=1                         # /lib is a symlink to /usr/lib on most systems
  if [ "${#want[@]}" -gt 0 ] && [[ ! " ${want[*]} " == *" $v "* ]]; then continue; fi
  tmp=$(mktemp -d); cp "$ROOT/kernel/truesense-wmi/truesense_wmi.c" "$ROOT/kernel/truesense-wmi/Makefile" "$tmp/"
  if (cd "$tmp" && make KVER="$v" KDIR="$b" W=1 >"$tmp/build.log" 2>&1) && [ -f "$tmp/truesense_wmi.ko" ]; then
    warns=$(grep -c "warning:" "$tmp/build.log" || true)
    echo "ok   $v  ($(modinfo -F version "$tmp/truesense_wmi.ko" 2>/dev/null || echo '?'), $warns warning(s))"; built=$((built+1))
    [ "$warns" -eq 0 ] || grep "warning:" "$tmp/build.log" | head -5 | sed 's/^/       /'
  else echo "FAIL $v"; tail -n 25 "$tmp/build.log" | sed 's/^/    /'; failed=$((failed+1)); fi
  rm -r -f "$tmp"
done
echo "built $built, failed $failed"
[ "$built" -gt 0 ] || { echo "no kernel headers found: install them first" >&2; exit 1; }
[ "$failed" -eq 0 ]
