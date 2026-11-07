#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Check every systemd unit the way systemd itself reads it: placeholders filled in, fake programs in place, then `systemd-analyze verify`.
# Also prints the hardening score. Used by CI; safe to run anywhere (changes nothing, needs no root).
set -Eeuo pipefail
ROOT=$(realpath "$(dirname "$0")/../..")
command -v systemd-analyze >/dev/null || { echo "systemd-analyze not found: skipping unit verification" >&2; exit 0; }
TMP=$(mktemp -d); trap 'rm -r -f "$TMP"' EXIT
mkdir -p "$TMP/prefix/bin" "$TMP/prefix/lib/truesense" "$TMP/units"
for f in "$TMP/prefix/bin/truesense" "$TMP/prefix/lib/truesense/health.sh" "$TMP/modprobe"; do printf '#!/bin/sh\nexit 0\n' > "$f"; chmod +x "$f"; done
for u in "$ROOT"/packaging/*.service "$ROOT"/packaging/*.timer; do
  sed -e "s|@PREFIX@|$TMP/prefix|g" -e "s|@MODPROBE@|$TMP/modprobe|g" "$u" > "$TMP/units/$(basename "$u")"
done
bad=0
for u in "$TMP"/units/*.service "$TMP"/units/*.timer; do
  # ExecStopPost uses /bin/sh and cat: present everywhere; --user units are verified the same way
  out=$(systemd-analyze verify "$u" 2>&1) || true
  # systemd-analyze also complains that a unit's [Install] target or a dependency is unknown to THIS machine: only real configuration errors matter here
  errs=$(printf '%s\n' "$out" | grep -E "$(basename "$u")|$TMP/units/" | grep -Ev "^$|Failed to (load|prepare)|not found|No such file|Cannot add dependency|is not a valid|multi-user.target|default.target|timers.target|graphical-session|pipewire|wireplumber|systemd-modules-load" || true)
  if [ -n "$errs" ]; then echo "FAIL $(basename "$u")"; printf '%s\n' "$errs" | sed 's/^/    /'; bad=1; else echo "ok   $(basename "$u")"; fi
done
echo; echo "hardening exposure of the daemon (lower is better; it must stay root with sysfs access):"
line=$(systemd-analyze security --offline=true "$TMP/units/truesense.service" 2>/dev/null | tail -1 || true); echo "$line"
score=$(printf '%s' "$line" | sed -n 's/.*: \([0-9][0-9]*\.[0-9]\).*/\1/p')
# a ratchet, not a goal: the score may only improve (sandboxing the daemon further needs testing on real hardware first)
if [ -n "$score" ] && awk -v s="$score" -v m="${MAX_EXPOSURE:-8.5}" 'BEGIN{exit !(s>m)}'; then echo "FAIL the daemon's exposure $score is worse than the allowed ${MAX_EXPOSURE:-8.5}"; bad=1; fi
exit "$bad"
