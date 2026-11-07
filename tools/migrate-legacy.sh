#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# TrueSense: remove every older helper that could claim the Acer WMI interfaces, so exactly one driver ever owns them.
# Removes: the old linuwu_sense / acer_rear_wmi kernel modules (loaded + DKMS + boot auto-load), the rpredatorsense and predator* services/daemons.
# Everything removed is first copied into /var/backups/truesense/legacy-<date>.tar.gz. Idempotent: safe to run twice. Run by install.sh; usable alone (sudo).
set -u
[ "$(id -u)" -eq 0 ] || { echo "run with sudo"; exit 1; }
U=${SUDO_USER:-}; UID_=""; [ -n "$U" ] && UID_=$(id -u "$U" 2>/dev/null)
say() { printf '  %s\n' "$*"; }
ur() { [ -n "$UID_" ] && runuser -u "$U" -- env XDG_RUNTIME_DIR=/run/user/$UID_ DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$UID_/bus "$@"; }

SYS_UNITS="rpredatorsense.service rpredatorsense-health.timer rpredatorsense-health.service predator.service predator-health.timer predator-health.service linuwu_sense.service"
USER_UNITS="rpredatorsense-keys.service rpredatorsense-audio.service rps-keys-dev.service predator.service"
FILES="/etc/systemd/system/rpredatorsense.service /etc/systemd/system/rpredatorsense-health.service /etc/systemd/system/rpredatorsense-health.timer
/etc/systemd/system/predator.service /etc/systemd/system/predator-health.service /etc/systemd/system/predator-health.timer /etc/systemd/system/linuwu_sense.service
/etc/systemd/system/predator.service.d /etc/systemd/user/rpredatorsense-keys.service /etc/systemd/user/rpredatorsense-audio.service
/usr/local/lib/rpredatorsense /usr/local/lib/predator /usr/local/bin/rpredatorsense /etc/rpredatorsense
/etc/udev/rules.d/99-acer-rear.rules /etc/udev/rules.d/70-rpredatorsense.rules /etc/modules-load.d/linuwu_sense.conf /etc/modules-load.d/acer-rear-wmi.conf"
[ -n "$U" ] && FILES="$FILES $(getent passwd "$U" | cut -d: -f6)/.config/systemd/user/predator.service"

echo "== TrueSense: removing legacy helpers"
# 1. back up everything that exists (and the DKMS sources) before touching anything
mkdir -p /var/backups/truesense; B=/var/backups/truesense/legacy-$(date +%Y%m%d-%H%M%S).tar.gz
EXIST=""; for p in $FILES /usr/src/linuwu_sense-* /usr/src/acer-rear-wmi-*; do [ -e "$p" ] && EXIST="$EXIST $p"; done
if [ -n "$EXIST" ]; then tar -czf "$B" $EXIST 2>/dev/null && say "backup: $B ($(echo $EXIST | wc -w) items)"; else say "nothing to back up (no legacy files found)"; B=""; fi

# 2. fans back to firmware control, then stop and disable the old services (system, then the invoking user's)
[ -w /dev/acer-rear ] && echo set_fan_auto > /dev/acer-rear 2>/dev/null
for f in /sys/devices/platform/acer-wmi/predator_sense/fan_speed /sys/module/linuwu_sense/drivers/platform:acer-wmi/acer-wmi/predator_sense/fan_speed; do [ -w "$f" ] && echo "0,0" > "$f" 2>/dev/null; done
for u in $SYS_UNITS; do systemctl disable --now "$u" >/dev/null 2>&1; done
systemctl --global disable rpredatorsense-keys.service rpredatorsense-audio.service >/dev/null 2>&1
for u in $USER_UNITS; do ur systemctl --user disable --now "$u" >/dev/null 2>&1; done
say "old services stopped and disabled"

# 3. unload the old kernel modules (retry once: a service may still hold them for a moment)
for m in acer_rear_wmi linuwu_sense; do
  if lsmod | grep -q "^$m "; then rmmod "$m" 2>/dev/null || { sleep 1; rmmod "$m" 2>/dev/null; }; lsmod | grep -q "^$m " && say "WARNING: could not unload $m (in use); it will be gone after a reboot" || say "unloaded $m"; fi
done

# 4. remove the old DKMS packages for every kernel, and their sources
for pkg in linuwu_sense acer-rear-wmi; do
  for ver in $(dkms status "$pkg" 2>/dev/null | sed -E 's|^[^/]*/([^,: ]+).*|\1|' | sort -u); do dkms remove "$pkg/$ver" --all >/dev/null 2>&1 && say "dkms removed $pkg/$ver"; done
done
rm -r -f /usr/src/linuwu_sense-* /usr/src/acer-rear-wmi-*

# 4b. the invoking user's own files: carry settings over, then remove the old-named PipeWire filter configs (TrueSense recreates them under its own names)
if [ -n "$U" ]; then
  H=$(getent passwd "$U" | cut -d: -f6); PWD_="$H/.config/pipewire/pipewire.conf.d"
  mode_of() { thr=$(sed -n 's/.*"VAD Threshold (%)" = \([0-9]*\).*/\1/p' "$1" 2>/dev/null | head -1); case "$thr" in 85) echo personal;; 50) echo conference;; *) echo off;; esac; }
  if [ -e "$PWD_/rps-pv-headset.conf" ] || [ -e "$PWD_/rps-pv-array.conf" ]; then
    mkdir -p /var/lib/truesense; printf 'headset=%s\narray=%s\n' "$(mode_of "$PWD_/rps-pv-headset.conf")" "$(mode_of "$PWD_/rps-pv-array.conf")" > /var/lib/truesense/migrated_voice
    tar -czf "/var/backups/truesense/legacy-userfiles-$(date +%Y%m%d-%H%M%S).tar.gz" "$PWD_"/rps-* 2>/dev/null; rm -f "$PWD_"/rps-pv-*.conf "$PWD_"/rps-purified-voice-*.conf
    say "old Purified Voice filters removed (modes kept: $(tr '\n' ' ' < /var/lib/truesense/migrated_voice))"
  fi
  OLDSTATE=""; for f in "$H"/.cache/predator-fan-mode "$H"/.cache/predator-auto-off "$H"/.cache/predator-profile-hold "$H"/.cache/predator-log "$H"/.cache/rpredatorsense-kb; do [ -e "$f" ] && OLDSTATE="$OLDSTATE $f"; done
  [ -n "$OLDSTATE" ] && { tar -czf "/var/backups/truesense/legacy-state-$(date +%Y%m%d-%H%M%S).tar.gz" $OLDSTATE 2>/dev/null; rm -f $OLDSTATE; say "old state files cleared from ~/.cache"; }
  if [ -d "$H/.config/rpredatorsense/profiles" ]; then
    install -d -o "$U" -g "$(id -gn "$U")" "$H/.config/truesense/lighting"
    cp -n "$H"/.config/rpredatorsense/profiles/*.json "$H/.config/truesense/lighting/" 2>/dev/null; chown -R "$U": "$H/.config/truesense" 2>/dev/null
    say "keyboard lighting profiles carried over to ~/.config/truesense/lighting"
  fi
fi

# 5. remove the files themselves (they are in the backup); carry the old config's login over to TrueSense
if [ -r /etc/rpredatorsense/config.toml ]; then mkdir -p /var/lib/truesense; sed -n 's/^user *= *"\(.*\)"/\1/p' /etc/rpredatorsense/config.toml | head -1 > /var/lib/truesense/migrated_user; fi
for p in $FILES; do [ -e "$p" ] && rm -r -f "$p"; done
systemctl daemon-reload; udevadm control --reload 2>/dev/null
ur systemctl --user daemon-reload >/dev/null 2>&1
depmod -a 2>/dev/null
say "legacy files removed${B:+ (restore with: sudo tar -xzf $B -C /)}"
echo "== legacy cleanup done"
