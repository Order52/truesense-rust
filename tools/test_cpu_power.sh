#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Checks CPU power limits and (read-only) the undervolt interface through the TrueSense service. Safe: it sets 70/80 W, then goes back to automatic.
# Run as your normal user (the service does the privileged part):  tools/test_cpu_power.sh
set -u
TS="${TS:-truesense}"; command -v "$TS" >/dev/null || TS="$(dirname "$0")/../truesense"
echo "== $(date +%T) power limits now";            "$TS" cpu limits
echo "== set PL1 70 W / PL2 80 W";                 "$TS" cpu limits 70 80
echo "== the service keeps them (read again)";     sleep 5; "$TS" cpu limits
echo "== a value beyond the danger line is refused without --force"; "$TS" cpu limits 110 118 && echo "UNEXPECTED: accepted" || echo "refused, as it should"
echo "== back to automatic";                       "$TS" cpu limits --auto
echo "== undervolt (read only): offsets right now"; "$TS" cpu undervolt get
echo "== undervolt profiles";                      "$TS" cpu undervolt list
echo "(nothing was undervolted; applying needs: truesense cpu undervolt use NAME --confirm)"
