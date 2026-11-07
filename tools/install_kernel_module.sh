#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Build and install the truesense_wmi kernel module with DKMS for EVERY installed kernel (so it also works after you boot another kernel), then load it.
set -Eeuo pipefail
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
ROOT=$(realpath "$(dirname "$0")/.."); SRC="$ROOT/kernel/truesense-wmi"; N=truesense-wmi
V=$(sed -n 's/^PACKAGE_VERSION="\(.*\)"/\1/p' "$SRC/dkms.conf")
say() { printf '  %s\n' "$*"; }
echo "== kernel module $N $V"
# without a version nothing below makes sense (the old module would be removed and /usr/src/truesense-wmi- created): stop before anything changes
[ -n "$V" ] || { echo "no PACKAGE_VERSION in $SRC/dkms.conf: this source tree is incomplete"; exit 1; }
have dkms || { echo "dkms is not installed"; exit 1; }
# idempotent upgrade: drop every registered version of this package first
for ver in $(sys dkms status "$N" 2>/dev/null | sed -E 's|^[^/]*/([^,: ]+).*|\1|' | sort -u); do sys dkms remove "$N/$ver" --all >/dev/null 2>&1 || true; done
rm -r -f "$R"/usr/src/"$N"-*; install -d "$R/usr/src/$N-$V"; install -m644 "$SRC"/truesense_wmi.c "$SRC"/Makefile "$SRC"/dkms.conf "$R/usr/src/$N-$V/"
sys dkms add "$N/$V" >/dev/null
running=$(running_kernel); ok_running=0
# the build logs go to /var/log (a fixed name in the world-writable /tmp would let any user redirect root's write with a symlink)
mkdir -p "$R/var/log"
for kdir in "$R"/lib/modules/*/build; do
  [ -e "$kdir" ] || continue; kver=$(basename "$(dirname "$kdir")")
  if sys dkms install "$N/$V" -k "$kver" --force >"$R/var/log/truesense-dkms-$kver.log" 2>&1; then say "built + installed for $kver"; [ "$kver" = "$running" ] && ok_running=1
  else say "FAILED for $kver (see /var/log/truesense-dkms-$kver.log)"; fi
done
[ "$ok_running" = 1 ] || { echo "the module could not be built for the running kernel $running (are the kernel headers installed?). No truesense_wmi is installed for it now: the fans stay under the firmware's control until this succeeds."; exit 1; }
sys depmod -a
# A loaded module is only replaced when nothing holds /dev/truesense open, and the daemon keeps it open: stop the service first (its ExecStopPost hands the fans back to the firmware;
# install.sh starts it again at the end). Without this `modprobe -r` fails quietly and the OLD code stays loaded although the new build is installed.
if sys lsmod 2>/dev/null | grep -q '^truesense_wmi '; then sys systemctl stop truesense.service 2>/dev/null || true; fi
sys modprobe -r truesense_wmi 2>/dev/null || true
sys modprobe truesense_wmi || { echo "modprobe truesense_wmi failed (dmesg | grep truesense)"; exit 1; }
for _ in $(seq 1 50); do [ -e "$R/dev/truesense" ] && break; nap 0.1; done
[ -e "$R/dev/truesense" ] || { echo "module loaded but /dev/truesense did not appear: this is not a PH16-71, or the firmware lacks the Acer WMI interface (dmesg | grep truesense)"; exit 1; }
say "loaded: $(sys modinfo -F version truesense_wmi) -> /dev/truesense"
# the same version string can hide different code: compare what is loaded with what was just built
loaded_src=$(cat "$R/sys/module/truesense_wmi/srcversion" 2>/dev/null || true); built_src=$(sys modinfo -F srcversion truesense_wmi 2>/dev/null || true)
if [ -n "$loaded_src" ] && [ -n "$built_src" ] && [ "$loaded_src" != "$built_src" ]; then
  say "NOTE: the loaded module is not the one just built (it could not be unloaded: something still holds /dev/truesense). The new build is used after the next reboot; until then the older code runs."
fi
