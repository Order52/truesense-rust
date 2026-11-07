#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Remove TrueSense (services, files, kernel module). Your config in /etc/truesense and ~/.config/truesense is kept unless you pass --purge.
set -u
[ "$(id -u)" -eq 0 ] || { echo "run with sudo"; exit 1; }
PREFIX=${PREFIX:-/usr/local}; PURGE=0; [ "${1:-}" = "--purge" ] && PURGE=1
U=${SUDO_USER:-}; UID_=$([ -n "$U" ] && id -u "$U"); ur() { [ -n "$U" ] && runuser -u "$U" -- env XDG_RUNTIME_DIR=/run/user/$UID_ DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$UID_/bus "$@"; }
echo "== removing TrueSense"
[ -e /dev/truesense ] && echo set_fan_auto > /dev/truesense 2>/dev/null
systemctl disable --now truesense.service truesense-health.timer truesense-health.service >/dev/null 2>&1
rm -r -f /usr/local/share/licenses/truesense; rm -f /usr/share/applications/truesense.desktop /usr/share/applications/truesense-tui.desktop /usr/local/share/icons/hicolor/scalable/apps/truesense.svg /usr/local/share/icons/hicolor/256x256/apps/truesense.png /usr/local/bin/truesense-fast
systemctl --global disable truesense-update.timer truesense-keys.service truesense-audio.service truesense-startup.service >/dev/null 2>&1
ur systemctl --user disable --now truesense-keys.service truesense-audio.service truesense-startup.service truesense-update.timer >/dev/null 2>&1
rm -f /etc/systemd/system/truesense*.service /etc/systemd/system/truesense-health.timer /etc/systemd/user/truesense-*.service /etc/systemd/user/truesense-update.timer
rm -f "$PREFIX/bin/truesense" "$PREFIX/bin/truesense-fast" "$PREFIX/bin/truesense.prev" "$PREFIX/bin/truesense.new"; rm -r -f "$PREFIX/lib/truesense" "$PREFIX/share/truesense"
rm -f /usr/share/bash-completion/completions/truesense /usr/share/zsh/site-functions/_truesense /usr/share/fish/vendor_completions.d/truesense.fish
modprobe -r truesense_wmi 2>/dev/null
for ver in $(dkms status truesense-wmi 2>/dev/null | sed -E 's|^[^/]*/([^,: ]+).*|\1|' | sort -u); do dkms remove "truesense-wmi/$ver" --all >/dev/null 2>&1; done
rm -r -f /usr/src/truesense-wmi-*
rm -f /etc/modprobe.d/truesense.conf /etc/modprobe.d/truesense-force.conf /etc/modules-load.d/truesense.conf /etc/udev/rules.d/70-truesense.rules
[ "$PURGE" = 1 ] && rm -r -f /etc/truesense /var/lib/truesense "$(getent passwd "${U:-root}" | cut -d: -f6)/.config/truesense"
systemctl daemon-reload; udevadm control --reload; depmod -a
echo "done. (The acer_wmi blacklist stays removed; the original Acer drivers are NOT reinstalled.)"
