#!/bin/bash
# SPDX-License-Identifier: MIT
# TrueSense health check (run by truesense-health.timer): restart the daemon if it died or stalled, and rebuild the DKMS module once per 10 min
# if a kernel update left it missing.
STAMP=/run/truesense-dkms-tried
if [ ! -e /dev/truesense ]; then
  modprobe truesense_wmi 2>/dev/null
  if [ ! -e /dev/truesense ] && { [ ! -e "$STAMP" ] || [ $(( $(date +%s) - $(stat -c %Y "$STAMP") )) -gt 600 ]; }; then
    touch "$STAMP"; logger -t truesense-health "truesense_wmi missing for $(uname -r): dkms autoinstall"
    dkms autoinstall -k "$(uname -r)" >/dev/null 2>&1; modprobe truesense_wmi 2>/dev/null
    [ -e /dev/truesense ] && systemctl restart truesense.service
  fi
fi
systemctl is-active --quiet truesense.service || { systemctl restart truesense.service; exit 0; }
age=$(( $(date +%s) - $(stat -c %Y /run/truesense/state 2>/dev/null || echo 0) ))
[ "$age" -gt 20 ] && { logger -t truesense-health "state file stale (${age}s), restarting"; systemctl restart truesense.service; }
exit 0
