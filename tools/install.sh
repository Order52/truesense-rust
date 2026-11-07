#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# TrueSense installer (Rust edition): one script for Arch/CachyOS/Manjaro, Debian/Ubuntu/Mint, Fedora and openSUSE (systemd required).
#   sudo tools/install.sh              install or upgrade in place (safe to re-run); missing prerequisites are installed for you
#   tools/install.sh --check           read-only report of what is missing (no root needed)
# Options:
#   --no-deps        do not install missing prerequisites (just report them)
#   --no-gui         accepted for compatibility (the window is part of the program now and needs no extra package)
#   --with-audio     also install the RNNoise plugin for microphone noise reduction where the distribution packages it
#   --no-services    install files but do not enable or start services
#   --module-only    only migrate old drivers and install the kernel module
#   --force          install on a model other than the Acer Predator PH16-71 (unsupported, can misbehave)
#   --prefix=DIR     install prefix, an absolute path (default /usr/local)
#   --user=NAME      the desktop user to set up (default: the user who ran sudo)
#   --uninstall      remove TrueSense (same as tools/uninstall.sh; with --purge also your settings)
# The program: bin/truesense of a release bundle, else target/x86_64-unknown-linux-musl/release/truesense (make build), else built here with cargo as the user who ran sudo;
# TRUESENSE_BIN=/path names one explicitly. A program whose version differs from this tree's Cargo.toml is refused (a stale build).
# Safety: the module is test-built before anything on your system changes; the program files are swapped atomically and rolled back if the service does not start.
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
PREFIX=/usr/local; MODULE_ONLY=0; AUDIO=0; SERVICES=1; FORCE=0; DEPS=1; CHECK=0; TARGET_USER=""; UNINSTALL=0; PURGE=0
say() { printf '  %s\n' "$*"; }; step() { printf '\n== %s\n' "$*"; }; die() { printf '!! %s\n' "$*" >&2; exit 1; }

# ---- which distribution is this? -----------------------------------------------------------------------------------------------------
detect_os() {
  local f=${TRUESENSE_OS_RELEASE:-$R/etc/os-release} id="" like=""
  # shellcheck disable=SC1090  # os-release is a shell-syntax file by specification; its path is fixed or set by the tests
  if [ -r "$f" ]; then id=$(. "$f"; printf '%s' "${ID:-}"); like=$(. "$f"; printf '%s' "${ID_LIKE:-}"); fi
  OS_ID=${id:-unknown}
  case " $id $like " in
    *" arch "*|*" archlinux "*|*" manjaro "*|*" cachyos "*|*" endeavouros "*|*" garuda "*) FAMILY=arch;;
    *" debian "*|*" ubuntu "*|*" linuxmint "*|*" pop "*) FAMILY=debian;;
    *" fedora "*|*" rhel "*|*" centos "*|*" nobara "*) FAMILY=fedora;;
    *" suse "*|*" opensuse "*|*" opensuse-tumbleweed "*|*" sles "*) FAMILY=suse;;
    *) FAMILY=unknown;;
  esac
}

# supported = verified on hardware; a similar Acer laptop is UNVERIFIED (read-only probe first); anything else does not apply
model_tier() {
  case "$1" in
    "Predator PH16-71") echo "(supported)";;
    Predator*|Nitro*) echo "(similar Acer laptop: NOT supported yet. The installer needs --force and nothing is verified on it. Safer first step: the read-only probe, see docs/INSTALL.md, 'Other models')";;
    *) echo "(NOT supported: this is not an Acer Predator; --force exists but TrueSense does not apply)";;
  esac
}

# refuses a machine that is not the verified model unless --force; says what to do instead. Writes the force option only when forced.
model_gate() {
  local model hint dmi=${TRUESENSE_DMI:-$R/sys/class/dmi/id}; model=$(cat "$dmi/product_name" 2>/dev/null || echo unknown)
  if [ "$model" = "Predator PH16-71" ]; then
    say "model: $model, BIOS $(cat "$dmi/bios_version" 2>/dev/null)"; [ "${TRUESENSE_LIB_ONLY:-}" = 1 ] || rm -f "$R/etc/modprobe.d/truesense-force.conf"; return 0
  fi
  case "$model" in
    Predator*|Nitro*) hint="This is a similar Acer laptop that TrueSense has not been verified on. Read-only first: load the module with 'modprobe truesense_wmi force=1', run 'truesense probe', and share the report (docs/INSTALL.md, 'Other models').";;
    *) hint="This is not an Acer Predator.";;
  esac
  if [ "$FORCE" != 1 ]; then printf '!! this machine is %s, not an Acer Predator PH16-71. TrueSense talks to firmware and is only verified on the PH16-71. %s Re-run with --force only if you accept that it can misbehave.\n' "'$model'" "$hint" >&2; return 1; fi
  say "WARNING: '$model' is unsupported; continuing because of --force"
  [ "${TRUESENSE_LIB_ONLY:-}" = 1 ] || printf 'options truesense_wmi force=1\n' > "$R/etc/modprobe.d/truesense-force.conf"
}

# the kernel-headers package that matches the RUNNING kernel (Arch kernels name it after their package base, e.g. linux-cachyos-headers)
headers_pkg() {
  local k=${TRUESENSE_KERNEL:-$(running_kernel)} base=""
  case $FAMILY in
    arch) [ -r "$R/usr/lib/modules/$k/pkgbase" ] && base=$(cat "$R/usr/lib/modules/$k/pkgbase"); echo "${base:-linux}-headers";;
    debian) echo "linux-headers-$k";;
    fedora) echo "kernel-devel-$k";;
    suse) case $k in *-default) echo kernel-default-devel;; *) echo "kernel-${k##*-}-devel";; esac;;
    *) echo "the kernel headers for $k";;
  esac
}

# package names per family: dkms, a C toolchain (for the kernel module), clang+lld (only for kernels built with clang), the RNNoise plugin
pkg() {
  case "$FAMILY:$1" in
    arch:dkms) echo dkms;; debian:dkms|fedora:dkms|suse:dkms) echo dkms;;
    arch:make) echo make;; debian:make|fedora:make|suse:make) echo make;;
    arch:cc) echo gcc;; debian:cc|fedora:cc|suse:cc) echo gcc;;
    arch:clang) echo "clang llvm lld";; debian:clang|fedora:clang|suse:clang) echo "clang lld";;
    arch:audio) echo noise-suppression-for-voice;;
    *) echo "";;
  esac
}

pkg_install() {
  case $FAMILY in
    arch) sys pacman -S --needed --noconfirm "$@";;
    debian) sys env DEBIAN_FRONTEND=noninteractive apt-get update -qq && sys env DEBIAN_FRONTEND=noninteractive apt-get install -y "$@";;
    fedora) sys dnf install -y "$@";;
    suse) sys zypper --non-interactive install "$@";;
    *) return 1;;
  esac
}

# fills MISSING (package names) and PROBLEMS (things a package cannot fix)
check_prereqs() {
  MISSING=(); PROBLEMS=(); local k; k=${TRUESENSE_KERNEL:-$(running_kernel)}
  have systemctl || PROBLEMS+=("systemd is required (TrueSense installs systemd services)")
  have dkms || MISSING+=("$(pkg dkms)")
  have make || MISSING+=("$(pkg make)")
  { have gcc || have cc || have clang; } || MISSING+=("$(pkg cc)")
  if grep -qi clang "$R/proc/version" 2>/dev/null && ! { have clang && have ld.lld; }; then MISSING+=("$(pkg clang)"); fi      # a clang-built kernel needs clang to build modules
  [ -e "$R/lib/modules/$k/build" ] || MISSING+=("$(headers_pkg)")
  local m parts; local flat=(); for m in "${MISSING[@]}"; do [ -n "$m" ] && read -ra parts <<<"$m" && flat+=("${parts[@]}"); done; MISSING=("${flat[@]}")                # "clang llvm lld" -> three entries
}

ensure_deps() {
  check_prereqs
  [ "${#PROBLEMS[@]}" -eq 0 ] || { for p in "${PROBLEMS[@]}"; do echo "  problem: $p"; done; die "fix the problem above and re-run"; }
  if [ "${#MISSING[@]}" -eq 0 ]; then say "prerequisites: all present"; return 0; fi
  say "missing: ${MISSING[*]}"
  [ "$DEPS" = 1 ] || die "install them (or re-run without --no-deps):  ${MISSING[*]}"
  [ "$FAMILY" != unknown ] || die "this distribution is not recognised, so I cannot install packages for you. Install:  ${MISSING[*]}   (dkms, make, gcc, the headers of your running kernel)"
  say "installing with the system package manager ($FAMILY): ${MISSING[*]}"
  pkg_install "${MISSING[@]}" || die "installing the prerequisites failed; install them yourself:  ${MISSING[*]}"
  check_prereqs
  [ "${#MISSING[@]}" -eq 0 ] || die "still missing after installing: ${MISSING[*]}"
}

# build the module in a scratch folder first: if it does not compile for this kernel, nothing on the system has been touched
precompile_module() {
  local tmp; tmp=$(mktemp -d)
  cp "$ROOT/kernel/truesense-wmi/truesense_wmi.c" "$ROOT/kernel/truesense-wmi/Makefile" "$tmp/"
  if (cd "$tmp" && sys make KVER="$(running_kernel)" >"$tmp/build.log" 2>&1); then say "the kernel module builds for $(running_kernel)"; rm -r -f "$tmp"
  else mkdir -p "$R/var/log"; cp "$tmp/build.log" "$R/var/log/truesense-module-build.log" 2>/dev/null || true; echo "  last lines of the build:"; tail -n 20 "$tmp/build.log" | sed 's/^/    /'; rm -r -f "$tmp"
    die "the kernel module does not build for $(running_kernel) (full log: /var/log/truesense-module-build.log). Nothing was changed."; fi
}

# The version a source tree declares: the first standalone `version = "x.y.z"` in its Cargo.toml (the workspace version), found the way the program's own updater finds it
# (`rust-version` is not `version`). A file that is missing or has none prints nothing.
tree_version() {
  awk '{ if (match($0, /(^|[^A-Za-z0-9_-])version[ \t]*=[ \t]*"[^"]+"/)) { s = substr($0, RSTART, RLENGTH); sub(/^[^"]*"/, "", s); sub(/"$/, "", s); print s; exit } }' "$1" 2>/dev/null || true
}

# Builds the program from this source tree with the Rust toolchain of the user who ran sudo (never as root: that would leave root-owned files in their tree; rustup keeps
# cargo in ~/.cargo/bin, which sudo's PATH does not have). The static musl build when its standard library is installed, else a statically linked glibc program: the code
# uses no name-service or dlopen calls, so both are one self-contained file. Sets BIN; leaves it unset when there is no cargo (the caller explains).
build_from_source() {
  local path=$PATH home target=x86_64-unknown-linux-musl flags="" libdir
  local -a builder=()
  if [ "$SANDBOX" != 1 ] && [ -n "${SUDO_USER:-}" ] && [ "$SUDO_USER" != root ]; then
    home=$(getent passwd "$SUDO_USER" | cut -d: -f6); path="$home/.cargo/bin:$PATH"; builder=(runuser -u "$SUDO_USER" --)
  fi
  local -a run=(${builder[@]+"${builder[@]}"} env "PATH=$path")
  "${run[@]}" sh -c 'command -v cargo' >/dev/null 2>&1 || return 0
  libdir=$("${run[@]}" rustc --print target-libdir --target "$target" 2>/dev/null) || libdir=""
  if [ ! -d "$libdir" ]; then
    target=x86_64-unknown-linux-gnu; flags="-C target-feature=+crt-static"
    say "(the musl standard library is not installed: rustup target add x86_64-unknown-linux-musl. Building a statically linked glibc program instead)"
  fi
  say "building the program from source (cargo, $target)"
  [ "$SANDBOX" = 1 ] || [ -z "${SUDO_USER:-}" ] || [ "$SUDO_USER" = root ] || { mkdir -p "$ROOT/target"; chown "$SUDO_USER" "$ROOT/target"; }
  "${run[@]}" sh -c 'cd "$1" && export CARGO_TARGET_DIR="$1/target" && { [ -z "$3" ] || export RUSTFLAGS="$3"; } && cargo build --release --locked -p truesense --target "$2"' sh "$ROOT" "$target" "$flags" \
    || die "cargo build failed"
  BIN=$ROOT/target/$target/release/truesense
}

# the program to install: a prebuilt static binary (a release bundle ships bin/truesense; `make build` makes target/x86_64-unknown-linux-musl/release/truesense), or one built
# from this source tree when Rust is installed. TRUESENSE_BIN names one explicitly. A program of another version than the source tree is refused: it would be a stale build.
find_binary() {
  local c want got
  for c in "${TRUESENSE_BIN:-}" "$ROOT/bin/truesense" "$ROOT/target/x86_64-unknown-linux-musl/release/truesense"; do [ -n "$c" ] && [ -x "$c" ] && { BIN=$c; break; }; done
  if [ -z "${BIN:-}" ] && [ -f "$ROOT/Cargo.toml" ]; then build_from_source; fi
  [ -n "${BIN:-}" ] || die "no program to install: there is no bin/truesense or target/x86_64-unknown-linux-musl/release/truesense next to this script. Build it (make build; needs Rust: https://rustup.rs, then rustup target add x86_64-unknown-linux-musl) or download a release from https://github.com/Order52/truesense/releases"
  # `|| true`: under pipefail a missing version file or a program that fails must reach the messages below, not abort the script silently
  want=$(tree_version "$ROOT/Cargo.toml") || true
  got=$("$BIN" --version 2>/dev/null | awk '{print $2}') || true
  [ -n "$got" ] || die "$BIN does not run on this machine (is it an x86-64 Linux build?)"
  [ -z "$want" ] || [ "$got" = "$want" ] || die "$BIN is version $got but this source tree is $want: rebuild it (make build) so you do not install a stale program"
}

# the service is up when systemd says active AND the daemon's control socket exists (the socket is what the program's own commands need)
sock_ready() { if [ "$SANDBOX" = 1 ]; then [ -e "$R/run/truesense/truesense.sock" ]; else [ -S /run/truesense/truesense.sock ]; fi; }
wait_for_service() { local _; for _ in $(seq 1 "${1:-30}"); do sys systemctl is-active --quiet truesense.service && sock_ready && return 0; nap 0.5; done; return 1; }

# render_to SRC DEST SED-ARGS...: DEST becomes SRC with the placeholders replaced, written to a temporary file and renamed into place (a crash can never leave a half-written
# unit that systemd would refuse at the next boot). When it replaces a DIFFERENT file and SNAP is set, the old file is first copied into SNAP: a rollback puts the old
# program back together with the unit it was written for (a unit can change between editions). An unchanged file keeps whatever snapshot an interrupted run left.
render_to() {
  local src=$1 dest=$2; shift 2
  sed "$@" "$src" > "$dest.new"
  if [ -n "${SNAP:-}" ] && [ -e "$dest" ] && ! cmp -s "$dest.new" "$dest"; then mkdir -p "$SNAP"; cp -p "$dest" "$SNAP/$(basename "$dest")"; fi
  mv -f "$dest.new" "$dest"
}

# the units the previous version came with, put back (used by the rollback)
restore_units() {
  local snap=$R$PREFIX/lib/truesense/units.prev f
  if [ ! -d "$snap" ]; then return 0; fi
  for f in "$snap"/system/*; do if [ -e "$f" ]; then mv -f "$f" "$R/etc/systemd/system/$(basename "$f")"; fi; done
  for f in "$snap"/user/*; do if [ -e "$f" ]; then mv -f "$f" "$R/etc/systemd/user/$(basename "$f")"; fi; done
  rm -r -f "$snap"
}

# fans back to the firmware's control by hand (the unit does it too when the service stops, but with the program that may be the broken one)
fans_to_firmware() { if [ -e "$R/dev/truesense" ]; then dev_write "$R/dev/truesense" set_fan_auto || true; fi; }

rollback() {
  echo "!! the service did not start with the new version: restoring the previous one"
  if [ -e "$R$PREFIX/bin/truesense.prev" ]; then
    restore_units
    mv -f "$R$PREFIX/bin/truesense.prev" "$R$PREFIX/bin/truesense"
    sys systemctl daemon-reload || true
    sys systemctl restart truesense.service || true
    if wait_for_service 10; then echo "   the previous version is running again"
    else fans_to_firmware; echo "   WARNING: the previous version did not start either: the service is down and the fans are on the firmware's control (see the log below)"; fi
  else sys systemctl stop truesense.service || true; fans_to_firmware; echo "   (no previous version: the service is stopped and the fans are back on the firmware's control)"; fi
  { sys journalctl -u truesense.service -n 15 --no-pager 2>/dev/null | sed 's/^/    /'; } || true
}

main() {
  local a
  for a in "$@"; do case "$a" in
    --check|--dry-run) CHECK=1;; --no-deps) DEPS=0;; --no-gui) ;; --module-only) MODULE_ONLY=1;; --with-audio) AUDIO=1;; --no-services) SERVICES=0;; --force) FORCE=1;;
    --prefix=*) PREFIX=${a#*=};; --user=*) TARGET_USER=${a#*=};; --uninstall) UNINSTALL=1;; --purge) PURGE=1;;
    -h|--help) sed -n '2,21p' "$0"; exit 0;; *) echo "unknown option: $a (see --help)" >&2; exit 2;; esac; done
  # the prefix ends up in unit files (ExecStart=) and in sed commands: it must be an absolute path made of plain characters
  case $PREFIX in /*) ;; *) echo "--prefix must be an absolute path (got '$PREFIX')" >&2; exit 2;; esac
  case $PREFIX in *[!A-Za-z0-9_./+@:-]*) echo "--prefix may only contain letters, digits and _ . / + @ : - (got '$PREFIX')" >&2; exit 2;; esac
  if [ "$UNINSTALL" = 1 ]; then
    local -a uargs=(); [ "$PURGE" = 0 ] || uargs+=(--purge); [ -z "$TARGET_USER" ] || uargs+=("--user=$TARGET_USER")
    PREFIX=$PREFIX exec "$(dirname "$0")/uninstall.sh" ${uargs[@]+"${uargs[@]}"}     # the same prefix and user: it must remove what THIS prefix installed
  fi
  [ "$PURGE" = 0 ] || { echo "--purge only goes with --uninstall" >&2; exit 2; }
  ROOT=$(realpath "$(dirname "$0")/..") ; detect_os

  if [ "$CHECK" = 1 ]; then
    echo "TrueSense installer check (nothing is changed)"; step "this system"
    say "distribution: $OS_ID (family: $FAMILY), kernel $(running_kernel), package manager: $(case $FAMILY in arch) echo pacman;; debian) echo apt;; fedora) echo dnf;; suse) echo zypper;; *) echo unknown;; esac)"
    model=$(cat "${TRUESENSE_DMI:-$R/sys/class/dmi/id}/product_name" 2>/dev/null || echo unknown); say "model: $model $(model_tier "$model")"
    step "prerequisites"; check_prereqs
    for p in "${PROBLEMS[@]}"; do say "problem: $p"; done
    if [ "${#MISSING[@]}" -eq 0 ] && [ "${#PROBLEMS[@]}" -eq 0 ]; then say "everything needed is present: sudo tools/install.sh will go straight to installing"
    else [ "${#MISSING[@]}" -eq 0 ] || say "would install: ${MISSING[*]}"; fi
    [ "${#MISSING[@]}" -eq 0 ] && [ "${#PROBLEMS[@]}" -eq 0 ]; return
  fi

  [ "$SANDBOX" = 1 ] || [ "$(id -u)" -eq 0 ] || die "run with sudo:  sudo $0 $*      (or check first without root:  $0 --check)"
  umask 022          # a hardened root umask (077) must not leave unit files, desktop files or the config unreadable to systemd's user manager and to the user
  local U=$TARGET_USER
  [ -n "$U" ] || U=${SUDO_USER:-}
  [ -n "$U" ] || { [ -n "${PKEXEC_UID:-}" ] && U=$(getent passwd "$PKEXEC_UID" | cut -d: -f1); }          # started through pkexec (the window's updater)
  if [ -z "$U" ] || [ "$U" = root ]; then                                                                      # root login: use the only ordinary user, if there is exactly one
    mapfile -t cand < <(awk -F: '$3>=1000 && $3<65000 && $7 !~ /(nologin|false)$/ {print $1}' "$R/etc/passwd" 2>/dev/null)
    [ "${#cand[@]}" -eq 1 ] && U=${cand[0]} || die "which user should get the keyboard service? re-run with --user=NAME (or use sudo from your own account)"
  fi
  if [ "$SANDBOX" = 1 ]; then [ -d "$R/home/$U" ] || die "no such user: $U"; else id "$U" >/dev/null 2>&1 || die "no such user: $U"; fi
  if [ "$SANDBOX" = 1 ]; then UID_=1000; else UID_=$(id -u "$U"); fi; USER_NAME=$U
  mkdir -p "$R/var/log" "$R/run"
  # two installers at once would race on the swap: the second one stops here (the lock is released when this script ends)
  exec 9> "$R/run/truesense-install.lock"
  flock -n 9 || die "another TrueSense install or uninstall is running (lock $R/run/truesense-install.lock): wait for it to finish"
  # a closing terminal or window (the updater runs this through pkexec) must not kill the installer between the swap and the service start (set before tee starts, so it ignores it too)
  trap '' HUP
  exec > >(tee -a "$R/var/log/truesense-install.log") 2>&1
  trap 'exec >&- 2>&-; wait' EXIT          # let tee print the last lines (the reason of a failure) before the script ends
  trap 'echo "!! install failed at line $LINENO (log: /var/log/truesense-install.log). Fix the problem and re-run; the installer is safe to repeat." >&2' ERR
  ur() { if [ "$SANDBOX" = 1 ]; then sandbox_sys "as-user:$USER_NAME" "$@"; else runuser -u "$USER_NAME" -- env XDG_RUNTIME_DIR=/run/user/$UID_ DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$UID_/bus "$@"; fi; }
  echo "TrueSense installer $(date '+%F %T')  prefix=$PREFIX  user=$USER_NAME  distribution=$OS_ID ($FAMILY)"

  step "pre-flight checks"
  model_gate || exit 1
  ensure_deps
  if have mokutil && sys mokutil --sb-state 2>/dev/null | grep -qi "enabled"; then
    if [ -e "$R/var/lib/dkms/mok.pub" ] && sys mokutil --test-key /var/lib/dkms/mok.pub 2>&1 | grep -qi "already enrolled"; then say "Secure Boot: on, DKMS signing key enrolled: ok"
    else say "WARNING: Secure Boot is on and the DKMS signing key is not enrolled: enrol it (sudo mokutil --import /var/lib/dkms/mok.pub), reboot, then re-run"; fi
  else say "Secure Boot: off (or unknown): no key enrolment needed"; fi
  precompile_module
  # the program is found (or built) now, while nothing on the system has changed: a missing or stale program must not be discovered after the old helpers are gone
  [ "$MODULE_ONLY" = 1 ] || find_binary

step "removing legacy helpers (backed up first)"
"$ROOT/tools/migrate-legacy.sh"

step "kernel module"
install -d "$R/etc/modprobe.d" "$R/etc/modules-load.d" "$R/etc/udev/rules.d"
install -m644 "$ROOT/packaging/truesense.modprobe.conf" "$R/etc/modprobe.d/truesense.conf"
install -m644 "$ROOT/packaging/truesense.modules-load.conf" "$R/etc/modules-load.d/truesense.conf"
install -m644 "$ROOT/packaging/70-truesense.rules" "$R/etc/udev/rules.d/70-truesense.rules"
sys udevadm control --reload
"$ROOT/tools/install_kernel_module.sh"
sys udevadm trigger --subsystem-match=misc --subsystem-match=hidraw 2>/dev/null || true; nap 1
[ "$MODULE_ONLY" = 1 ] && { echo; echo "module-only install done."; return 0; }

step "TrueSense files"
local B=$R$PREFIX/bin L=$R$PREFIX/lib/truesense
if [ "$AUDIO" = 1 ]; then p=$(pkg audio); if [ -n "$p" ]; then pkg_install $p || say "(could not install $p)"; else say "(your distribution does not package the RNNoise LADSPA plugin: install noise-suppression-for-voice from its project page)"; fi; fi
install -d "$L" "$B" "$R$PREFIX/share/truesense/profiles" "$R/etc/truesense"
install -m755 "$ROOT/packaging/health.sh" "$L/health.sh"
# The GPU undervolt helper: one small C program that talks to NVIDIA's library (the daemon is static and cannot load it, and must not hang on it). Not fatal if it cannot be built.
HELPER_CC=""; for c in cc gcc clang; do if have "$c"; then HELPER_CC=$c; [ "$SANDBOX" = 1 ] || HELPER_CC=$(command -v "$c"); break; fi; done
if [ -n "$HELPER_CC" ] && sys "$HELPER_CC" -O2 -Wall -o "$L/truesense-nvml.new" "$ROOT/tools/nvml/truesense-nvml.c" -ldl 2>/dev/null; then
  chmod 755 "$L/truesense-nvml.new"; mv -f "$L/truesense-nvml.new" "$L/truesense-nvml"
else rm -f "$L/truesense-nvml.new"; say "(could not build the GPU helper: GPU undervolt will be unavailable; everything else works)"; fi
# The program is swapped in with ONE rename, so the path in the unit (ExecStart and ExecStopPost) never points at a missing file, not even for a moment. The copy of the old one
# (the rollback) is itself written beside and renamed; an interrupted earlier run that already put THIS program in place keeps the older copy it made.
rm -f "$B/truesense.new" "$B/truesense.prev.new"
install -m755 "$BIN" "$B/truesense.new"                                                                       # beside the old one, then swap atomically
if [ -e "$B/truesense" ] && ! { [ -e "$B/truesense.prev" ] && cmp -s "$BIN" "$B/truesense"; }; then
  cp -p "$B/truesense" "$B/truesense.prev.new"; mv -f "$B/truesense.prev.new" "$B/truesense.prev"
fi
mv -f "$B/truesense.new" "$B/truesense"
ln -s truesense "$B/.truesense-fast.new"; mv -f -T "$B/.truesense-fast.new" "$B/truesense-fast"                # one binary: the name selects the instant status-bar client
[ -d "$ROOT/packaging/profiles" ] && install -m644 "$ROOT"/packaging/profiles/*.toml "$R$PREFIX/share/truesense/profiles/"
if [ ! -e "$R/etc/truesense/config.toml" ]; then
  printf '# TrueSense system configuration (see docs/USAGE.md). Built-in defaults apply to everything not set here.\nuser = "%s"\n\n# First install: the PREDATOR key behaves like PredatorSense on Windows (it opens the app). Change it in the app: PREDATOR key.\n[keys]\nprofile = "native"\ntap = "exec:truesense gui"\ndouble = "none"\nlong = "none"\n' "$U" > "$R/etc/truesense/config.toml.new"
  mv -n "$R/etc/truesense/config.toml.new" "$R/etc/truesense/config.toml"; rm -f "$R/etc/truesense/config.toml.new"      # never replaces a config that appeared meanwhile
fi
for sh in bash zsh fish; do
  case $sh in bash) d=$R/usr/share/bash-completion/completions/truesense;; zsh) d=$R/usr/share/zsh/site-functions/_truesense;; fish) d=$R/usr/share/fish/vendor_completions.d/truesense.fish;; esac
  install -d "$(dirname "$d")"; "$B/truesense" completion $sh > "$d" 2>/dev/null || rm -f "$d"
done
say "installed $PREFIX/bin/truesense (version $("$B/truesense" --version | awk '{print $2}'))"

step "services"
MODPROBE=/usr/sbin/modprobe; [ "$SANDBOX" = 1 ] || MODPROBE=$(command -v modprobe)
SNAP=$L/units.prev/system
for u in truesense.service truesense-health.service truesense-health.timer; do render_to "$ROOT/packaging/$u" "$R/etc/systemd/system/$u" -e "s|@PREFIX@|$PREFIX|g" -e "s|@MODPROBE@|$MODPROBE|g"; done
install -Dm644 "$ROOT/LICENSE" "$R$PREFIX/share/licenses/truesense/LICENSE"
install -Dm644 "$ROOT/packaging/icons/truesense.svg" "$R$PREFIX/share/icons/hicolor/scalable/apps/truesense.svg"; install -Dm644 "$ROOT/packaging/icons/truesense-256.png" "$R$PREFIX/share/icons/hicolor/256x256/apps/truesense.png"
if have gtk-update-icon-cache; then sys gtk-update-icon-cache -q -t -f "$R$PREFIX/share/icons/hicolor" 2>/dev/null || true; fi
install -d "$R/usr/share/applications"; unset SNAP
for u in truesense.desktop truesense-tui.desktop; do render_to "$ROOT/packaging/$u" "$R/usr/share/applications/$u" "s|@PREFIX@|$PREFIX|g"; done
install -d "$R/etc/systemd/user"; SNAP=$L/units.prev/user
for u in truesense-keys.service truesense-audio.service truesense-startup.service truesense-update.service truesense-update.timer; do render_to "$ROOT/packaging/$u" "$R/etc/systemd/user/$u" -e "s|@PREFIX@|$PREFIX|g"; done
unset SNAP
if have restorecon; then sys restorecon -R "$R$PREFIX/lib/truesense" "$B" "$R/etc/systemd/system" "$R/usr/share/applications" 2>/dev/null || true; fi     # SELinux (Fedora)
sys systemctl daemon-reload; ur systemctl --user daemon-reload || true
if [ "$SERVICES" = 1 ]; then
  sys systemctl enable truesense.service truesense-health.timer >/dev/null 2>&1
  sys systemctl restart truesense.service || true    # `enable --now` would NOT restart a running service, so upgrades would keep the old code
  if ! wait_for_service; then rollback; die "the new version did not start (the previous one was restored if there was one)"; fi
  rm -f "$B/truesense.prev"; rm -r -f "$L/units.prev"
  rm -r -f "$L/truesense" "$R$PREFIX/lib/truesense.prev"                                                      # the Python program of 0.3.x (the Go edition left nothing here), replaced by the binary
  sys systemctl start truesense-health.timer
  for u in truesense-keys.service truesense-audio.service truesense-startup.service truesense-update.timer; do sys systemctl --global enable "$u" >/dev/null 2>&1 || say "(could not enable $u for the user sessions)"; done
  ur systemctl --user restart truesense-keys.service truesense-audio.service truesense-startup.service || say "(user services start at your next login)"
  ur systemctl --user enable --now truesense-update.timer >/dev/null 2>&1 || true
  nap 2
  if [ -r "$R/var/lib/truesense/migrated_voice" ]; then      # the user already had Purified Voice filters: recreate them under the new names, same modes
    hs=$(sed -n 's/^headset=//p' "$R/var/lib/truesense/migrated_voice"); ar=$(sed -n 's/^array=//p' "$R/var/lib/truesense/migrated_voice")
    ur "$PREFIX/bin/truesense" audio voice set headset "${hs:-off}" >/dev/null 2>&1 || true
    ur "$PREFIX/bin/truesense" audio voice set array "${ar:-off}" >/dev/null 2>&1 || true
    rm -f "$R/var/lib/truesense/migrated_voice"; say "Purified Voice filters recreated (headset: ${hs:-off}, array: ${ar:-off})"
  fi
else say "services installed but not enabled (--no-services)"; fi

step "health check"
"$B/truesense" doctor || say "the checker found problems: see above (re-run it any time: truesense doctor)"
echo; echo "TrueSense is installed. Try:  truesense status   |   truesense --help"
}

if [ -z "${TRUESENSE_LIB_ONLY:-}" ]; then main "$@"; fi
