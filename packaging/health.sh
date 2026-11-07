#!/bin/bash
# SPDX-License-Identifier: MIT
# TrueSense health check (run by truesense-health.timer): restart the daemon if it died or stalled, and rebuild the DKMS module once per 10 min
# if a kernel update left it missing.
# The paths can be overridden (TRUESENSE_DEVICE, TRUESENSE_RUN, TRUESENSE_HEALTH_STAMP, TRUESENSE_HEALTH_SETTLE) for the tests; the unit sets none of them.
STAMP=${TRUESENSE_HEALTH_STAMP:-/run/truesense-dkms-tried}
DEVICE=${TRUESENSE_DEVICE:-/dev/truesense}
STATE=${TRUESENSE_RUN:-/run/truesense}/state
SETTLE=${TRUESENSE_HEALTH_SETTLE:-3}
if [ ! -e "$DEVICE" ]; then
  modprobe truesense_wmi 2>/dev/null
  if [ ! -e "$DEVICE" ] && { [ ! -e "$STAMP" ] || [ $(( $(date +%s) - $(stat -c %Y "$STAMP") )) -gt 600 ]; }; then
    touch "$STAMP"; logger -t truesense-health "truesense_wmi missing for $(uname -r): dkms autoinstall"
    dkms autoinstall -k "$(uname -r)" >/dev/null 2>&1; modprobe truesense_wmi 2>/dev/null
    [ -e "$DEVICE" ] && systemctl restart truesense.service
  fi
fi
systemctl is-active --quiet truesense.service || { systemctl restart truesense.service; exit 0; }
state_age() { echo $(( $(date +%s) - $(stat -c %Y "$STATE" 2>/dev/null || echo 0) )); }
age=$(state_age)
if [ "$age" -gt 20 ]; then
  # Right after a resume from suspend the file is as old as the sleep was (the clock ran, the daemon did not) and the daemon's next tick,
  # one or two seconds away, makes it fresh: look again before calling a healthy daemon stalled. A really stalled one is also killed by its watchdog.
  sleep "$SETTLE"; age=$(state_age)
  [ "$age" -gt 20 ] && { logger -t truesense-health "state file stale (${age}s), restarting"; systemctl restart truesense.service; }
fi
exit 0
