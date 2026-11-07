#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# shellcheck disable=SC2034  # SB, SRC, SB_PATH ... are for the test files that source this one
# The sandbox for the install, upgrade, rollback and uninstall tests. The real scripts (install.sh, uninstall.sh, install_kernel_module.sh, migrate-legacy.sh) are run with
# TRUESENSE_ROOT pointing at a fake system tree in a temporary directory ($SB): every system path they use lives under it, the programs that would change a real system
# (systemctl, dkms, modprobe, ...) are replaced inside the scripts by recorders that append to $SB/calls.log and answer from files in $SB/sim/ (see the "sandbox" block at
# the top of each script). On top of that, the PATH given to the scripts holds only the plain tools they need and a TRIPWIRE for every program that could touch a real system:
# a tripwire logs its call, fails, and makes the test fail, so a call that bypassed the recorders could never act on the machine that runs the tests.
# shellcheck source=tools/shelltest/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

SB_USER=tester
SB_KERNEL=6.99.0-sandbox
# programs that must never run for real while testing: if one is reached, the test fails
SB_DANGEROUS="systemctl modprobe rmmod insmod lsmod modinfo dkms udevadm loginctl runuser su sudo pkexec depmod pacman apt-get apt dnf zypper mokutil gtk-update-icon-cache restorecon
journalctl chown chgrp mount umount reboot shutdown poweroff service make cc gcc clang cargo rustc"
# the plain tools the scripts use
SB_TOOLS="gzip bash awk basename cat chmod cmp cp cut date dirname env find flock grep head id install ln ls mkdir mktemp mv readlink realpath rm sed seq sleep sort tail tar tee touch tr uname wc sh"

# the repository files a script needs, copied (the tests may edit the copy: a version, a unit): SRC is the source tree an installer would be run from
sb_source_tree() {
  SRC=$(tmpdir)
  mkdir -p "$SRC/kernel/truesense-wmi" "$SRC/tools"
  cp -a "$ROOT/packaging" "$SRC/packaging"; cp -a "$ROOT/LICENSE" "$ROOT/Cargo.toml" "$SRC/"
  cp -a "$ROOT"/kernel/truesense-wmi/{dkms.conf,Makefile,truesense_wmi.c} "$SRC/kernel/truesense-wmi/"
  cp -a "$ROOT"/tools/*.sh "$ROOT/tools/nvml" "$SRC/tools/"
  NEWVER=$(awk '{ if (match($0, /(^|[^A-Za-z0-9_-])version[ \t]*=[ \t]*"[^"]+"/)) { s = substr($0, RSTART, RLENGTH); sub(/^[^"]*"/, "", s); sub(/"$/, "", s); print s; exit } }' "$SRC/Cargo.toml")
  [ -n "$NEWVER" ] || fail "no version in the repository's Cargo.toml"
}

# fake_bin PATH VERSION [crash|noversion|python]: a stand-in for the program. It answers --version like the real one; "crash" makes the sandbox service refuse to start with it,
# "noversion" makes --version fail (a broken download), "python" is the 0.3.x shell wrapper (it needs the python tree to start).
fake_bin() {
  local f=$1 ver=$2 kind=${3:-} body
  mkdir -p "$(dirname "$f")"
  case $kind in
    python) body="#!/bin/sh\nexec env PYTHONPATH=/usr/local/lib/truesense python3 -m truesense \"\$@\"  # $ver\n";;
    noversion) body="#!/bin/sh\n# fake truesense $ver without a working --version\nexit 1\n";;
    *) body="#!/bin/sh\n# fake truesense $ver ${kind:+TRUESENSE_SANDBOX_${kind^^}}\ncase \"\$1\" in --version) echo \"truesense $ver\";; completion) echo \"# completion for \$2 ($ver)\";; doctor) echo \"doctor ok\";; *) echo \"fake \$*\";; esac\n";;
  esac
  printf '%b' "$body" > "$f"; chmod 755 "$f"
}

# sb_new: a fresh fake system (a distribution's usual directories, a PH16-71, the running kernel's headers), a source tree (SRC), the PATH for the scripts and a new program NEWBIN
sb_new() {
  SB=$(tmpdir); : > "$SB/.truesense-sandbox"
  local d; for d in etc/modprobe.d etc/modules-load.d etc/udev/rules.d etc/systemd/system etc/systemd/user usr/share/applications usr/share/bash-completion/completions \
    usr/share/zsh/site-functions usr/share/fish/vendor_completions.d usr/share/icons/hicolor usr/src usr/local/bin usr/local/lib usr/local/share var/lib var/log var/backups run dev \
    "lib/modules/$SB_KERNEL/build" sys/class/dmi/id "home/$SB_USER" sim; do mkdir -p "$SB/$d"; done
  printf 'Predator PH16-71\n' > "$SB/sys/class/dmi/id/product_name"; printf 'V1.0\n' > "$SB/sys/class/dmi/id/bios_version"
  printf 'ID=arch\n' > "$SB/etc/os-release"; printf '%s:x:1000:1000::/home/%s:/bin/bash\n' "$SB_USER" "$SB_USER" > "$SB/etc/passwd"; printf '%s\n' "$SB_KERNEL" > "$SB/sim/kernel"
  : > "$SB/calls.log"
  sb_source_tree
  NEWBIN=$(tmpdir)/truesense; fake_bin "$NEWBIN" "$NEWVER"
  # the PATH of the scripts: tripwires first, then only the plain tools
  SB_TRIP=$TMP/tripwire.log; : > "$SB_TRIP"
  SB_PATH=$(tmpdir); local n p
  for n in $SB_DANGEROUS; do fake_program "$SB_PATH" "$n" "echo \"TRIPWIRE: \$0 \$*\" >> '$SB_TRIP'; exit 97"; done
  for n in $SB_TOOLS; do p=$(command -v "$n") || { fail "the tests need $n"; continue; }; ln -sf "$p" "$SB_PATH/$n"; done
}

# sb_run SCRIPT [VAR=value ...] -- ARGS...   runs tools/SCRIPT of the source tree in the sandbox (--user is NOT added: pass it); sets R_OUT, R_ERR, R_CODE (stdout and stderr
# are both kept). The program to install is $NEWBIN unless TRUESENSE_BIN is given. Fails the test when a tripwire was hit.
sb_run() {
  local script=$1; shift; local -a vars=()
  while [ "$1" != -- ]; do vars+=("$1"); shift; done; shift
  run "PATH=$SB_PATH" "TRUESENSE_ROOT=$SB" "TRUESENSE_BIN=$NEWBIN" "TMPDIR=$TMP" ${vars[@]+"${vars[@]}"} -- "$BASH_BIN" "$SRC/tools/$script" "$@"
  [ ! -s "$SB_TRIP" ] || fail "a program that must never run for real was called: $(cat "$SB_TRIP")"
}
sb_install() { sb_run install.sh -- "--user=$SB_USER" "$@"; }

sb_calls() { cat "$SB/calls.log"; }
# sb_baseline: remember the tree of a fresh sandbox (what a distribution has before an install); assert_untouched checks nothing was added or changed since
sb_baseline() { SB_BASE=$(sb_tree); }
assert_untouched() { assert_eq "$(sb_tree)" "$SB_BASE" "${1:-the tree}"; }
# the 1-based position of the first call line that contains the glob PATTERN in calls.log, or empty
# shellcheck disable=SC2254  # the pattern is meant to be a glob
sb_call_pos() { local i=0 l; while IFS= read -r l; do i=$((i + 1)); case $l in *$1*) echo "$i"; return 0;; esac; done < "$SB/calls.log"; return 0; }
# shellcheck disable=SC2254  # the patterns are meant to be globs
# assert_order PATTERN...: lines containing the patterns (globs) occur in calls.log, in this order (each after the previous)
assert_order() {
  local last=0 p pos i l
  for p in "$@"; do
    pos=""; i=0
    while IFS= read -r l; do i=$((i + 1)); [ "$i" -gt "$last" ] || continue; case $l in *$p*) pos=$i; break;; esac; done < "$SB/calls.log"
    if [ -z "$pos" ]; then fail "calls.log has no '$p' after line $last:
$(cat "$SB/calls.log")"; return 1; fi
    last=$pos
  done
}
assert_called() { grep -q -- "$1" "$SB/calls.log" || fail "calls.log lacks '$1'"; }
assert_not_called() { ! grep -q -- "$1" "$SB/calls.log" || fail "calls.log must not contain '$1': $(grep -- "$1" "$SB/calls.log" | head -n3)"; }

# sb_tree [DIR]: one line per file or symlink under DIR (default the sandbox), without the bookkeeping the scripts and the fake service keep (calls.log, sim, run, var/log):
# path, mode and a hash of the content (a symlink: its target). Two equal listings are two byte-identical trees.
sb_tree() {
  (cd "${1:-$SB}" && find . \( -path ./calls.log -o -path ./sim -o -path ./run -o -path ./var/log \) -prune -o \( -type f -o -type l \) -print | LC_ALL=C sort | while IFS= read -r f; do
    if [ -L "$f" ]; then echo "$f -> $(readlink "$f")"; else echo "$f $(stat -c %a "$f") $(sha256sum < "$f" | cut -c1-16)"; fi
  done)
}
# sb_dirs: the directories of the sandbox (so a test can see which ones an install made)
sb_dirs() { (cd "$SB" && find . -type d -not -path ./sim | LC_ALL=C sort); }

# assert_mode FILE MODE; assert_same FILE_A FILE_B (byte-identical)
assert_mode() { [ -e "$1" ] || { fail "$1 does not exist (wanted mode $2)"; return; }; assert_eq "$(stat -c %a "$1")" "$2" "mode of $1"; }
assert_same() { cmp -s "$1" "$2" || fail "$1 and $2 differ"; }

# ---- the layouts of the older editions, laid down in the sandbox ------------------------------------------------------------------------------

# the system units as an older edition shipped them (the current packaging with the prefix filled in; Go also set its runtime variables), the user units, the enable links
sb_old_units() {
  local kind=$1 u
  for u in truesense.service truesense-health.service truesense-health.timer; do
    sed -e 's|@PREFIX@|/usr/local|g' -e 's|@MODPROBE@|/usr/sbin/modprobe|g' ${kind:+-e "s|^Type=notify|Type=notify\nEnvironment=OLD_EDITION=$kind|"} "$SRC/packaging/$u" > "$SB/etc/systemd/system/$u"
  done
  for u in truesense-keys.service truesense-audio.service truesense-startup.service truesense-update.service truesense-update.timer; do sed 's|@PREFIX@|/usr/local|g' "$SRC/packaging/$u" > "$SB/etc/systemd/user/$u"; done
  mkdir -p "$SB/etc/systemd/system/multi-user.target.wants" "$SB/etc/systemd/system/timers.target.wants"
  ln -sf ../truesense.service "$SB/etc/systemd/system/multi-user.target.wants/truesense.service"; ln -sf ../truesense-health.timer "$SB/etc/systemd/system/timers.target.wants/truesense-health.timer"
}
# the service running (as it would be after the old edition's own install)
sb_service_running() { mkdir -p "$SB/run/truesense"; : > "$SB/run/truesense/truesense.sock"; : > "$SB/run/sandbox-active"; }
# the things an upgrade must never touch: the system config, the state file, the user's own settings (all with content the installer could not guess)
sb_user_data() {
  write_file "$SB/etc/truesense/config.toml" '# my own config\nuser = "tester"\n[keys]\ntap = "none"\n'
  write_file "$SB/var/lib/truesense/state.json" '{"mode":"balanced","fan":{"smart":true}}\n' 600
  write_file "$SB/home/$SB_USER/.config/truesense/settings.json" '{"theme":"dark"}\n'
  write_file "$SB/home/$SB_USER/.config/truesense/lighting/mine.json" '{"zones":[1,2,3,4]}\n'
}
# the kernel-side files an install of any edition leaves
sb_old_kernel() {
  local ver=$1
  mkdir -p "$SB/usr/src/truesense-wmi-$ver"; cp "$SRC/kernel/truesense-wmi/"* "$SB/usr/src/truesense-wmi-$ver/"; sed -i "s/^PACKAGE_VERSION=.*/PACKAGE_VERSION=\"$ver\"/" "$SB/usr/src/truesense-wmi-$ver/dkms.conf"
  printf 'truesense-wmi/%s, %s, x86_64: installed\n' "$ver" "$SB_KERNEL" > "$SB/sim/dkms_status"
  cp "$SRC/packaging/truesense.modprobe.conf" "$SB/etc/modprobe.d/truesense.conf"; cp "$SRC/packaging/truesense.modules-load.conf" "$SB/etc/modules-load.d/truesense.conf"
  cp "$SRC/packaging/70-truesense.rules" "$SB/etc/udev/rules.d/70-truesense.rules"; : > "$SB/dev/truesense"
}
# the Go edition 0.4.0: one program, the helper, units with its runtime variables, DKMS 0.4.0, config, state, user settings, the service running
sb_layout_go() {
  local ver=${1:-0.4.0}
  fake_bin "$SB/usr/local/bin/truesense" "$ver" go; ln -sf truesense "$SB/usr/local/bin/truesense-fast"
  write_file "$SB/usr/local/lib/truesense/health.sh" '#!/bin/bash\n# old health script\n' 755; write_file "$SB/usr/local/lib/truesense/truesense-nvml" '#!/bin/sh\n# old helper\n' 755
  mkdir -p "$SB/usr/local/share/truesense/profiles"; write_file "$SB/usr/local/share/truesense/profiles/balanced.toml" '# old\n'
  sb_old_units go; sb_old_kernel "$ver"; sb_user_data; sb_service_running
}
# the Python edition 0.3.x: python tree in lib/truesense/truesense, a shell wrapper as bin/truesense, the previous tree in lib/truesense.prev, DKMS of that version
sb_layout_python() {
  local ver=${1:-0.3.17} d
  fake_bin "$SB/usr/local/bin/truesense" "$ver" python; write_file "$SB/usr/local/bin/truesense-fast" '#!/bin/sh\n# python fast client\n' 755
  for d in lib/truesense lib/truesense.prev; do
    write_file "$SB/usr/local/$d/truesense/__init__.py" "VERSION = '$ver'\n"; write_file "$SB/usr/local/$d/truesense/daemon.py" '# daemon\n'
    write_file "$SB/usr/local/$d/health.sh" '#!/bin/bash\n# old health script\n' 755
  done
  sb_old_units ""; sb_old_kernel "$ver"; sb_user_data; sb_service_running
}
