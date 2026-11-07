#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Remove TrueSense (services, files, kernel module). Your config in /etc/truesense and ~/.config/truesense is kept unless you pass --purge.
#   sudo tools/uninstall.sh [--purge] [--prefix=DIR] [--user=NAME]      (PREFIX=DIR in the environment does the same as --prefix)
# The fans are handed back to the firmware and the service is stopped BEFORE anything is deleted; if the service cannot be stopped nothing is removed.
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
PREFIX=${PREFIX:-/usr/local}; PURGE=0; U=""
for a in "$@"; do case $a in
  --purge) PURGE=1;; --prefix=*) PREFIX=${a#*=};; --user=*) U=${a#*=};;
  -h|--help) sed -n '2,6p' "$0"; exit 0;; *) echo "unknown option: $a (see --help)" >&2; exit 2;; esac; done
case $PREFIX in /*) ;; *) echo "--prefix must be an absolute path (got '$PREFIX')" >&2; exit 2;; esac
[ "$SANDBOX" = 1 ] || [ "$(id -u)" -eq 0 ] || { echo "run with sudo"; exit 1; }
[ -n "$U" ] || U=${SUDO_USER:-}
[ -n "$U" ] || { [ -n "${PKEXEC_UID:-}" ] && U=$(getent passwd "$PKEXEC_UID" | cut -d: -f1); }           # started through pkexec (the window's updater)
[ "$U" != root ] || U=""
UID_=""; if [ -n "$U" ]; then if [ "$SANDBOX" = 1 ]; then UID_=1000; else UID_=$(id -u "$U" 2>/dev/null); fi; fi
ur() { [ -n "$U" ] && [ -n "$UID_" ] || return 0; if [ "$SANDBOX" = 1 ]; then sandbox_sys "as-user:$U" "$@"; else runuser -u "$U" -- env XDG_RUNTIME_DIR=/run/user/$UID_ DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$UID_/bus "$@"; fi; }
mkdir -p "$R/run"; exec 9> "$R/run/truesense-install.lock"
flock -n 9 || { echo "another TrueSense install or uninstall is running: wait for it to finish" >&2; exit 1; }
trap '' HUP
SVC_SYS="truesense-health.timer truesense-health.service truesense.service"   # the timer first: it restarts a dead daemon
SVC_GLOBAL="truesense-update.timer truesense-keys.service truesense-audio.service truesense-startup.service"
echo "== removing TrueSense"
# 1. fans back to the firmware, then stop the daemon while its program (the unit's ExecStopPost) still exists. One unit at a time: systemctl refuses a whole
#    `disable` when ONE of the names has no unit file (a partly installed system), and the daemon would then keep running while its files are deleted.
[ ! -e "$R/dev/truesense" ] || dev_write "$R/dev/truesense" set_fan_auto
for u in $SVC_SYS; do sys systemctl disable --now "$u" >/dev/null 2>&1; done
if sys systemctl is-active --quiet truesense.service; then sys systemctl stop truesense.service >/dev/null 2>&1; fi
if sys systemctl is-active --quiet truesense.service; then
  echo "!! truesense.service is still running and could not be stopped: nothing was removed (its program must stay until it has stopped). Stop it (systemctl stop truesense.service) and run this again." >&2; exit 1
fi
for u in $SVC_GLOBAL; do sys systemctl --global disable "$u" >/dev/null 2>&1; done
ur systemctl --user disable --now truesense-keys.service truesense-audio.service truesense-startup.service truesense-update.timer >/dev/null 2>&1
# 2. the files
rm -r -f "$R$PREFIX/share/licenses/truesense"; rm -f "$R/usr/share/applications/truesense.desktop" "$R/usr/share/applications/truesense-tui.desktop" "$R$PREFIX/share/icons/hicolor/scalable/apps/truesense.svg" "$R$PREFIX/share/icons/hicolor/256x256/apps/truesense.png"
rm -f "$R"/etc/systemd/system/truesense*.service "$R/etc/systemd/system/truesense-health.timer" "$R"/etc/systemd/user/truesense-*.service "$R/etc/systemd/user/truesense-update.timer"
# a link left in a .wants directory (a disable that failed above) would point at a unit that no longer exists
for f in "$R"/etc/systemd/system/*.wants/truesense* "$R"/etc/systemd/user/*.wants/truesense*; do if [ -L "$f" ] && [ ! -e "$f" ]; then rm -f "$f"; fi; done
rm -f "$R$PREFIX/bin/truesense" "$R$PREFIX/bin/truesense-fast" "$R$PREFIX/bin/.truesense-fast.new" "$R$PREFIX/bin/truesense.prev" "$R$PREFIX/bin/truesense.prev.new" "$R$PREFIX/bin/truesense.new"
rm -r -f "$R$PREFIX/lib/truesense" "$R$PREFIX/lib/truesense.prev" "$R$PREFIX/lib/truesense.new" "$R$PREFIX/share/truesense"
rm -f "$R/usr/share/bash-completion/completions/truesense" "$R/usr/share/zsh/site-functions/_truesense" "$R/usr/share/fish/vendor_completions.d/truesense.fish"
sys modprobe -r truesense_wmi 2>/dev/null
for ver in $(sys dkms status truesense-wmi 2>/dev/null | sed -E 's|^[^/]*/([^,: ]+).*|\1|' | sort -u); do sys dkms remove "truesense-wmi/$ver" --all >/dev/null 2>&1; done
rm -r -f "$R"/usr/src/truesense-wmi-*
rm -f "$R/etc/modprobe.d/truesense.conf" "$R/etc/modprobe.d/truesense-force.conf" "$R/etc/modules-load.d/truesense.conf" "$R/etc/udev/rules.d/70-truesense.rules"
if [ "$PURGE" = 1 ]; then
  rm -r -f "$R/etc/truesense" "$R/var/lib/truesense"
  if [ -n "$U" ]; then H=$(user_home "$U"); if [ -n "$H" ]; then rm -r -f "$H/.config/truesense"; fi; else echo "  (no user known: ~/.config/truesense was not removed; pass --user=NAME)"; fi
fi
sys systemctl daemon-reload; sys udevadm control --reload; sys depmod -a
echo "done. (The acer_wmi blacklist stays removed; the original Acer drivers are NOT reinstalled.)"
