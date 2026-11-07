#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# TrueSense: remove every older helper that could claim the Acer WMI interfaces, so exactly one driver ever owns them.
# Removes: the old linuwu_sense / acer_rear_wmi kernel modules (loaded + DKMS + boot auto-load), the rpredatorsense and predator* services/daemons.
# Everything removed is first copied into /var/backups/truesense/legacy-<date>.tar.gz; if that copy cannot be written and read back, NOTHING is removed and this exits 1.
# Idempotent: safe to run twice. Run by install.sh; usable alone (sudo). --user=NAME names the desktop user whose files are carried over (default: the user who ran sudo).
set -u
# >>> sandbox (the same text in install.sh, uninstall.sh, install_kernel_module.sh and migrate-legacy.sh: tools/shelltest keeps the four copies equal)
# TRUESENSE_ROOT=DIR runs a script against a FAKE system tree, for the shell tests (tools/shelltest/test_sandbox_*.sh). Every system path (/etc /usr /var /lib/modules /run /dev
# /sys and the install prefix) lives under DIR ($R below), no system program runs (systemctl, dkms, modprobe, ... are only recorded in DIR/calls.log and answered from the files
# in DIR/sim/), and nothing needs root. It is refused unless DIR is an absolute path of an existing directory that holds a file named .truesense-sandbox, so it cannot be set off
# by accident. Without it (the normal case) R is empty: every path is the real one, sys runs the real program, and nothing in this block changes anything.
R=""; SANDBOX=0
if [ -n "${TRUESENSE_ROOT:-}" ]; then
  if [ "${TRUESENSE_ROOT#/}" = "$TRUESENSE_ROOT" ] || [ ! -d "$TRUESENSE_ROOT" ] || [ ! -e "$TRUESENSE_ROOT/.truesense-sandbox" ]; then
    printf '!! TRUESENSE_ROOT=%s refused: it must be an absolute path of an existing directory that holds a file named .truesense-sandbox. Nothing was done.\n' "$TRUESENSE_ROOT" >&2; exit 2
  fi
  R=$(cd "$TRUESENSE_ROOT" && pwd -P); SANDBOX=1
  [ "$R" != / ] || { echo "!! TRUESENSE_ROOT must not be /. Nothing was done." >&2; exit 2; }
  printf '################ SANDBOX MODE: TRUESENSE_ROOT=%s ################\n#  a fake system tree: nothing real is installed, started, loaded or removed\n' "$R" >&2
fi
sim() { [ -e "$R/sim/$1" ]; }                                      # a simulated condition is on when the file DIR/sim/NAME exists
have() { if [ "$SANDBOX" = 1 ]; then ! grep -qxF -- "$1" "$R/sim/missing" 2>/dev/null; else command -v "$1" >/dev/null 2>&1; fi; }
nap() { [ "$SANDBOX" = 1 ] || sleep "$1"; }
running_kernel() { if [ "$SANDBOX" = 1 ]; then cat "$R/sim/kernel" 2>/dev/null || echo 6.99.0-sandbox; else uname -r; fi; }
user_home() { if [ "$SANDBOX" = 1 ]; then echo "$R/home/$1"; else getent passwd "$1" | cut -d: -f6; fi; }
own() { if [ "$SANDBOX" = 1 ]; then printf '@chown %s\n' "$*" >> "$R/calls.log"; else chown -R "$1:" "${@:2}" 2>/dev/null || true; fi; }           # own USER PATH...
# dev_write DEVICE TEXT: one command line to a kernel device (the fans-back-to-firmware failsafe); the sandbox records it and writes the fake device file
dev_write() { [ "$SANDBOX" != 1 ] || printf '@write %s %s\n' "${1#"$R"}" "$2" >> "$R/calls.log"; { printf '%s\n' "$2" > "$1"; } 2>/dev/null; }
# sys PROGRAM ARGS...: every call to a program that changes or asks the system. Real: runs it. Sandbox: appends the call to calls.log, fails it when a line of sim/fail (a shell
# pattern matched against the whole call text) matches, else answers from the fake state below.
sys() { if [ "$SANDBOX" = 1 ]; then sandbox_sys "$@"; else "$@"; fi; }
sandbox_sys() {
  local line="${*//"$R"/}" pat      # the log and the fail patterns see the path a real system would have (the sandbox directory is cut off)
  printf '%s\n' "$line" >> "$R/calls.log"
  if [ -r "$R/sim/fail" ]; then
    while IFS= read -r pat || [ -n "$pat" ]; do
      # shellcheck disable=SC2254  # the pattern is meant to be a glob
      [ -z "$pat" ] || case $line in $pat) return 1;; esac
    done < "$R/sim/fail"
  fi
  case $1 in
    systemctl) sandbox_systemctl "${@:2}";;
    dkms) sandbox_dkms "${@:2}";;
    modprobe) case " $* " in *" -r "*) rm -f "$R/dev/truesense";; *) if ! sim no_device; then mkdir -p "$R/dev"; : > "$R/dev/truesense"; fi;; esac;;
    lsmod) cat "$R/sim/lsmod" 2>/dev/null || true;;
    rmmod) if [ -r "$R/sim/lsmod" ]; then grep -v "^$2 " "$R/sim/lsmod" > "$R/sim/lsmod.new" || true; mv -f "$R/sim/lsmod.new" "$R/sim/lsmod"; fi;;
    make) if sim module_build_fails; then echo "make: *** simulated build failure"; return 2; fi;;
    mokutil) case ${2:-} in --sb-state) cat "$R/sim/sb_state" 2>/dev/null || echo "SecureBoot disabled";; --test-key) cat "$R/sim/mok_test" 2>/dev/null || echo "is not enrolled";; esac;;
    modinfo) echo sandbox;;
    tar) command tar "${@:2}";;
    journalctl) cat "$R/sim/journal" 2>/dev/null || true;;
    cc|gcc|clang|*/cc|*/gcc|*/clang) sandbox_cc "$@";;
    *) ;;
  esac
}
sandbox_cc() {           # a C compiler call: "builds" the file named by -o (unless sim/helper_build_fails)
  local out="" prev="" a; shift
  if sim helper_build_fails; then return 1; fi
  for a in "$@"; do if [ "$prev" = -o ]; then out=$a; fi; prev=$a; done
  [ -z "$out" ] || printf '#!/bin/sh\n# simulated helper\n' > "$out"
}
# a stateful fake of the one service this project has (truesense.service); the others only answer yes. It starts when the program of the unit's ExecStart exists, is
# executable and does not contain TRUESENSE_SANDBOX_CRASH (a python wrapper additionally needs the python tree it points at), unless sim/service_start_fails. A stop while
# the program of ExecStopPost is missing is logged as "@ALERT" (systemd would run a missing program there).
sandbox_systemctl() {
  local a verb="" now=0 global=0 u d t unit="$R/etc/systemd/system/truesense.service"
  local -a units=()
  for a in "$@"; do case $a in --now) now=1;; --global) global=1;; -*) ;; *) if [ -z "$verb" ]; then verb=$a; else units+=("$a"); fi;; esac; done
  d=$R/etc/systemd/system; [ "$global" = 0 ] || d=$R/etc/systemd/user
  case $verb in
    enable|disable)
      for u in "${units[@]}"; do [ -e "$d/$u" ] || { echo "Failed to $verb unit: Unit file $u does not exist." >&2; return 1; }; done
      for u in "${units[@]}"; do
        t=$(sed -n 's/^WantedBy=//p' "$d/$u" | head -n1)
        if [ "$verb" = enable ]; then [ -z "$t" ] || { mkdir -p "$d/$t.wants"; ln -sf "../$u" "$d/$t.wants/$u"; }; else rm -f "$d"/*.wants/"$u"; fi
        if [ "$now" = 1 ] && [ "$u" = truesense.service ]; then if [ "$verb" = enable ]; then sandbox_start "$unit"; else sandbox_stop "$unit"; fi; fi
      done;;
    start|restart|stop|is-active)
      for u in "${units[@]}"; do
        if [ "$u" != truesense.service ]; then [ "$verb" != is-active ] || return 3; continue; fi
        case $verb in
          start) sandbox_start "$unit" || return 1;;
          restart) sandbox_stop "$unit"; sandbox_start "$unit" || return 1;;
          stop) sandbox_stop "$unit";;
          is-active) [ -e "$R/run/sandbox-active" ] || return 3;;
        esac
      done;;
  esac
  return 0
}
sandbox_start() {
  local bin
  bin=$(sed -n 's|^ExecStart=\(/[^ ]*\) .*|\1|p' "$1" 2>/dev/null | head -n1)
  # what the install directories hold at the moment the service is started (the tests check that old files are still there then)
  printf '@start bin=[ %s] lib=[ %s]\n' "$(ls -A "$(dirname "$R$bin")" 2>/dev/null | tr '\n' ' ')" "$(ls -A "$R${bin%/bin/truesense}/lib/truesense" 2>/dev/null | tr '\n' ' ')" >> "$R/calls.log"
  if sim service_start_fails || [ -z "$bin" ] || [ ! -x "$R$bin" ] || grep -qa TRUESENSE_SANDBOX_CRASH "$R$bin" \
     || { grep -qa 'PYTHONPATH=' "$R$bin" && [ ! -d "$R${bin%/bin/truesense}/lib/truesense/truesense" ]; }; then
    rm -f "$R/run/sandbox-active" "$R/run/truesense/truesense.sock"; echo "Job for truesense.service failed (simulated)." >&2; return 1
  fi
  mkdir -p "$R/run/truesense"; : > "$R/run/truesense/truesense.sock"; : > "$R/run/sandbox-active"
}
sandbox_stop() {
  local post
  post=$(sed -n 's|^ExecStopPost=-\{0,1\}\(/[^ ]*/truesense\) .*|\1|p' "$1" 2>/dev/null | head -n1)
  if [ -e "$R/run/sandbox-active" ] && [ -n "$post" ] && [ ! -x "$R$post" ]; then printf '@ALERT ExecStopPost program missing: %s\n' "$post" >> "$R/calls.log"; fi
  rm -f "$R/run/sandbox-active" "$R/run/truesense/truesense.sock"
}
# a stateful fake of dkms: DIR/sim/dkms_status holds `dkms status` lines; add / install / remove edit it, and install needs the version to be added first
sandbox_dkms() {
  local st="$R/sim/dkms_status" mod="${2:-}"
  mkdir -p "$R/sim"; touch "$st"
  case ${1:-} in
    status) if [ -n "$mod" ]; then grep -E "^${mod}[/,:]" "$st" || true; else cat "$st"; fi;;
    add) printf '%s: added\n' "$mod" >> "$st";;
    install) grep -qE "^${mod}[,:]" "$st" || { echo "Error! The module/version combo: $mod is not located in the DKMS tree." >&2; return 1; }
             grep -v "^${mod}: added" "$st" > "$st.new" || true; printf '%s, %s, x86_64: installed\n' "$mod" "${4:-?}" >> "$st.new"; mv -f "$st.new" "$st";;
    remove) grep -vE "^${mod}[,:]" "$st" > "$st.new" || true; mv -f "$st.new" "$st";;
  esac
}
# <<< sandbox
[ "$SANDBOX" = 1 ] || [ "$(id -u)" -eq 0 ] || { echo "run with sudo"; exit 1; }
umask 022
U=""
for a in "$@"; do case $a in --user=*) U=${a#*=};; *) echo "unknown option: $a" >&2; exit 2;; esac; done
[ -n "$U" ] || U=${SUDO_USER:-}
[ -n "$U" ] || { [ -n "${PKEXEC_UID:-}" ] && U=$(getent passwd "$PKEXEC_UID" | cut -d: -f1); }           # started through pkexec (the window's updater)
[ "$U" != root ] || U=""
UID_=""; H=""
if [ -n "$U" ]; then if [ "$SANDBOX" = 1 ]; then UID_=1000; else UID_=$(id -u "$U" 2>/dev/null) || UID_=""; fi; H=$(user_home "$U"); fi
say() { printf '  %s\n' "$*"; }
ur() { [ -n "$UID_" ] || return 0; if [ "$SANDBOX" = 1 ]; then sandbox_sys "as-user:$U" "$@"; else runuser -u "$U" -- env XDG_RUNTIME_DIR=/run/user/$UID_ DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$UID_/bus "$@"; fi; }
# backup ARCHIVE PATH...: the paths (stored relative to the root, so `tar -xzf ARCHIVE -C /` restores them) in a gzip archive that is then read back; fails when it cannot be both
backup() { local out=$1 p; shift; local -a rel=(); for p in "$@"; do rel+=("${p#"$R"/}"); done; sys tar -czf "$out" -C "${R:-/}" "${rel[@]}" 2>/dev/null && tar -tzf "$out" >/dev/null 2>&1; }

SYS_UNITS="rpredatorsense.service rpredatorsense-health.timer rpredatorsense-health.service predator.service predator-health.timer predator-health.service linuwu_sense.service"
USER_UNITS="rpredatorsense-keys.service rpredatorsense-audio.service rps-keys-dev.service predator.service"
FILES="$R/etc/systemd/system/rpredatorsense.service $R/etc/systemd/system/rpredatorsense-health.service $R/etc/systemd/system/rpredatorsense-health.timer
$R/etc/systemd/system/predator.service $R/etc/systemd/system/predator-health.service $R/etc/systemd/system/predator-health.timer $R/etc/systemd/system/linuwu_sense.service
$R/etc/systemd/system/predator.service.d $R/etc/systemd/user/rpredatorsense-keys.service $R/etc/systemd/user/rpredatorsense-audio.service
$R/usr/local/lib/rpredatorsense $R/usr/local/lib/predator $R/usr/local/bin/rpredatorsense $R/etc/rpredatorsense
$R/etc/udev/rules.d/99-acer-rear.rules $R/etc/udev/rules.d/70-rpredatorsense.rules $R/etc/modules-load.d/linuwu_sense.conf $R/etc/modules-load.d/acer-rear-wmi.conf"
[ -z "$H" ] || FILES="$FILES $H/.config/systemd/user/predator.service"

echo "== TrueSense: removing legacy helpers"
# 1. back up everything that exists (and the DKMS sources) before touching anything; no backup, no removal
mkdir -p "$R/var/backups/truesense"; chmod 700 "$R/var/backups/truesense" 2>/dev/null || true
B=$R/var/backups/truesense/legacy-$(date +%Y%m%d-%H%M%S).tar.gz
EXIST=""; for p in $FILES "$R"/usr/src/linuwu_sense-* "$R"/usr/src/acer-rear-wmi-*; do if [ -e "$p" ]; then EXIST="$EXIST $p"; fi; done
if [ -n "$EXIST" ]; then
  # shellcheck disable=SC2086  # the list is paths without spaces, split on purpose
  if backup "$B" $EXIST; then say "backup: ${B#"$R"} ($(echo $EXIST | wc -w) items)"
  else rm -f "$B"; echo "!! the backup of the old files could not be written or read back (is /var/backups full?): nothing was removed. Free some space and run the installer again." >&2; exit 1; fi
else say "nothing to back up (no legacy files found)"; B=""; fi

# 2. fans back to firmware control, then stop and disable the old services (system, then the invoking user's)
if [ -w "$R/dev/acer-rear" ]; then dev_write "$R/dev/acer-rear" set_fan_auto; fi
for f in "$R/sys/devices/platform/acer-wmi/predator_sense/fan_speed" "$R/sys/module/linuwu_sense/drivers/platform:acer-wmi/acer-wmi/predator_sense/fan_speed"; do if [ -w "$f" ]; then dev_write "$f" "0,0"; fi; done
for u in $SYS_UNITS; do sys systemctl disable --now "$u" >/dev/null 2>&1; done
for u in rpredatorsense-keys.service rpredatorsense-audio.service; do sys systemctl --global disable "$u" >/dev/null 2>&1; done
for u in $USER_UNITS; do ur systemctl --user disable --now "$u" >/dev/null 2>&1; done
say "old services stopped and disabled"

# 3. unload the old kernel modules (retry once: a service may still hold them for a moment)
for m in acer_rear_wmi linuwu_sense; do
  if sys lsmod | grep -q "^$m "; then sys rmmod "$m" 2>/dev/null || { nap 1; sys rmmod "$m" 2>/dev/null; }; if sys lsmod | grep -q "^$m "; then say "WARNING: could not unload $m (in use); it will be gone after a reboot"; else say "unloaded $m"; fi; fi
done

# 4. remove the old DKMS packages for every kernel, and their sources
for pkg in linuwu_sense acer-rear-wmi; do
  for ver in $(sys dkms status "$pkg" 2>/dev/null | sed -E 's|^[^/]*/([^,: ]+).*|\1|' | sort -u); do if sys dkms remove "$pkg/$ver" --all >/dev/null 2>&1; then say "dkms removed $pkg/$ver"; fi; done
done
rm -r -f "$R"/usr/src/linuwu_sense-* "$R"/usr/src/acer-rear-wmi-*

# 4b. the invoking user's own files: carry settings over, then remove the old-named PipeWire filter configs (TrueSense recreates them under its own names)
if [ -n "$H" ]; then
  PWD_="$H/.config/pipewire/pipewire.conf.d"
  mode_of() { local thr; thr=$(sed -n 's/.*"VAD Threshold (%)" = \([0-9]*\).*/\1/p' "$1" 2>/dev/null | head -1); case "$thr" in 85) echo personal;; 50) echo conference;; *) echo off;; esac; }
  if [ -e "$PWD_/rps-pv-headset.conf" ] || [ -e "$PWD_/rps-pv-array.conf" ]; then
    mkdir -p "$R/var/lib/truesense"; printf 'headset=%s\narray=%s\n' "$(mode_of "$PWD_/rps-pv-headset.conf")" "$(mode_of "$PWD_/rps-pv-array.conf")" > "$R/var/lib/truesense/migrated_voice"
    # shellcheck disable=SC2046  # the file names are plain, split on purpose
    if backup "$R/var/backups/truesense/legacy-userfiles-$(date +%Y%m%d-%H%M%S).tar.gz" $(ls -d "$PWD_"/rps-* 2>/dev/null); then
      rm -f "$PWD_"/rps-pv-*.conf "$PWD_"/rps-purified-voice-*.conf
      say "old Purified Voice filters removed (modes kept: $(tr '\n' ' ' < "$R/var/lib/truesense/migrated_voice"))"
    else say "WARNING: could not back up the old Purified Voice filters, so they were left where they are"; fi
  fi
  OLDSTATE=""; for f in "$H"/.cache/predator-fan-mode "$H"/.cache/predator-auto-off "$H"/.cache/predator-profile-hold "$H"/.cache/predator-log "$H"/.cache/rpredatorsense-kb; do if [ -e "$f" ]; then OLDSTATE="$OLDSTATE $f"; fi; done
  if [ -n "$OLDSTATE" ]; then
    # shellcheck disable=SC2086  # plain file names, split on purpose
    if backup "$R/var/backups/truesense/legacy-state-$(date +%Y%m%d-%H%M%S).tar.gz" $OLDSTATE; then rm -f $OLDSTATE; say "old state files cleared from ~/.cache"
    else say "WARNING: could not back up the old state files in ~/.cache, so they were left where they are"; fi
  fi
  if [ -d "$H/.config/rpredatorsense/profiles" ]; then
    install -d "$H/.config/truesense/lighting"
    cp -n "$H"/.config/rpredatorsense/profiles/*.json "$H/.config/truesense/lighting/" 2>/dev/null; own "$U" "$H/.config/truesense"
    say "keyboard lighting profiles carried over to ~/.config/truesense/lighting"
  fi
fi

# 5. remove the files themselves (they are in the backup); carry the old config's login over to TrueSense
if [ -r "$R/etc/rpredatorsense/config.toml" ]; then mkdir -p "$R/var/lib/truesense"; sed -n 's/^user *= *"\(.*\)"/\1/p' "$R/etc/rpredatorsense/config.toml" | head -1 > "$R/var/lib/truesense/migrated_user"; fi
for p in $FILES; do if [ -e "$p" ]; then rm -r -f "$p"; fi; done
sys systemctl daemon-reload; sys udevadm control --reload 2>/dev/null
ur systemctl --user daemon-reload >/dev/null 2>&1
sys depmod -a 2>/dev/null
say "legacy files removed${B:+ (restore with: sudo tar -xzf ${B#"$R"} -C /)}"
echo "== legacy cleanup done"
