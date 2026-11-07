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
#   --prefix=DIR     install prefix (default /usr/local)
#   --user=NAME      the desktop user to set up (default: the user who ran sudo)
#   --uninstall      remove TrueSense (same as tools/uninstall.sh)
# The program: bin/truesense of a release bundle, else target/x86_64-unknown-linux-musl/release/truesense (make build), else built here with cargo as the user who ran sudo;
# TRUESENSE_BIN=/path names one explicitly. A program whose version differs from this tree's Cargo.toml is refused (a stale build).
# Safety: the module is test-built before anything on your system changes; the program files are swapped atomically and rolled back if the service does not start.
set -Eeuo pipefail

PREFIX=/usr/local; MODULE_ONLY=0; AUDIO=0; SERVICES=1; FORCE=0; DEPS=1; CHECK=0; TARGET_USER=""
say() { printf '  %s\n' "$*"; }; step() { printf '\n== %s\n' "$*"; }; die() { printf '!! %s\n' "$*" >&2; exit 1; }

# ---- which distribution is this? -----------------------------------------------------------------------------------------------------
detect_os() {
  local f=${TRUESENSE_OS_RELEASE:-/etc/os-release} id="" like=""
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
  local model hint; model=$(cat "${TRUESENSE_DMI:-/sys/class/dmi/id}/product_name" 2>/dev/null || echo unknown)
  if [ "$model" = "Predator PH16-71" ]; then
    say "model: $model, BIOS $(cat "${TRUESENSE_DMI:-/sys/class/dmi/id}/bios_version" 2>/dev/null)"; [ "${TRUESENSE_LIB_ONLY:-}" = 1 ] || rm -f /etc/modprobe.d/truesense-force.conf; return 0
  fi
  case "$model" in
    Predator*|Nitro*) hint="This is a similar Acer laptop that TrueSense has not been verified on. Read-only first: load the module with 'modprobe truesense_wmi force=1', run 'truesense probe', and share the report (docs/INSTALL.md, 'Other models').";;
    *) hint="This is not an Acer Predator.";;
  esac
  if [ "$FORCE" != 1 ]; then printf '!! this machine is %s, not an Acer Predator PH16-71. TrueSense talks to firmware and is only verified on the PH16-71. %s Re-run with --force only if you accept that it can misbehave.\n' "'$model'" "$hint" >&2; return 1; fi
  say "WARNING: '$model' is unsupported; continuing because of --force"
  [ "${TRUESENSE_LIB_ONLY:-}" = 1 ] || printf 'options truesense_wmi force=1\n' > /etc/modprobe.d/truesense-force.conf
}

# the kernel-headers package that matches the RUNNING kernel (Arch kernels name it after their package base, e.g. linux-cachyos-headers)
headers_pkg() {
  local k=${TRUESENSE_KERNEL:-$(uname -r)} base=""
  case $FAMILY in
    arch) [ -r "/usr/lib/modules/$k/pkgbase" ] && base=$(cat "/usr/lib/modules/$k/pkgbase"); echo "${base:-linux}-headers";;
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
    arch) pacman -S --needed --noconfirm "$@";;
    debian) DEBIAN_FRONTEND=noninteractive apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y "$@";;
    fedora) dnf install -y "$@";;
    suse) zypper --non-interactive install "$@";;
    *) return 1;;
  esac
}

have() { command -v "$1" >/dev/null 2>&1; }

# fills MISSING (package names) and PROBLEMS (things a package cannot fix)
check_prereqs() {
  MISSING=(); PROBLEMS=(); local k; k=${TRUESENSE_KERNEL:-$(uname -r)}
  have systemctl || PROBLEMS+=("systemd is required (TrueSense installs systemd services)")
  have dkms || MISSING+=("$(pkg dkms)")
  have make || MISSING+=("$(pkg make)")
  { have gcc || have cc || have clang; } || MISSING+=("$(pkg cc)")
  if grep -qi clang /proc/version 2>/dev/null && ! { have clang && have ld.lld; }; then MISSING+=("$(pkg clang)"); fi      # a clang-built kernel needs clang to build modules
  [ -e "/lib/modules/$k/build" ] || MISSING+=("$(headers_pkg)")
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
  if (cd "$tmp" && make KVER="$(uname -r)" >"$tmp/build.log" 2>&1); then say "the kernel module builds for $(uname -r)"; rm -r -f "$tmp"
  else cp "$tmp/build.log" /var/log/truesense-module-build.log 2>/dev/null || true; echo "  last lines of the build:"; tail -n 20 "$tmp/build.log" | sed 's/^/    /'; rm -r -f "$tmp"
    die "the kernel module does not build for $(uname -r) (full log: /var/log/truesense-module-build.log). Nothing was changed."; fi
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
  if [ -n "${SUDO_USER:-}" ] && [ "$SUDO_USER" != root ]; then
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
  [ -z "${SUDO_USER:-}" ] || [ "$SUDO_USER" = root ] || { mkdir -p "$ROOT/target"; chown "$SUDO_USER" "$ROOT/target"; }
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

wait_for_service() { local _; for _ in $(seq 1 30); do systemctl is-active --quiet truesense.service && [ -S /run/truesense/truesense.sock ] && return 0; sleep 0.5; done; return 1; }

rollback() {
  echo "!! the service did not start with the new version: restoring the previous one"
  if [ -e "$PREFIX/bin/truesense.prev" ]; then mv -f "$PREFIX/bin/truesense.prev" "$PREFIX/bin/truesense"; systemctl restart truesense.service || true
  else systemctl stop truesense.service || true; echo "   (no previous version: the service is stopped and the fans are back on the firmware's control)"; fi
  journalctl -u truesense.service -n 15 --no-pager 2>/dev/null | sed 's/^/    /' || true
}

main() {
  local a
  for a in "$@"; do case "$a" in
    --check|--dry-run) CHECK=1;; --no-deps) DEPS=0;; --no-gui) ;; --module-only) MODULE_ONLY=1;; --with-audio) AUDIO=1;; --no-services) SERVICES=0;; --force) FORCE=1;;
    --prefix=*) PREFIX=${a#*=};; --user=*) TARGET_USER=${a#*=};; --uninstall) exec "$(dirname "$0")/uninstall.sh";;
    -h|--help) sed -n '2,21p' "$0"; exit 0;; *) echo "unknown option: $a (see --help)" >&2; exit 2;; esac; done
  ROOT=$(realpath "$(dirname "$0")/.."); detect_os

  if [ "$CHECK" = 1 ]; then
    echo "TrueSense installer check (nothing is changed)"; step "this system"
    say "distribution: $OS_ID (family: $FAMILY), kernel $(uname -r), package manager: $(case $FAMILY in arch) echo pacman;; debian) echo apt;; fedora) echo dnf;; suse) echo zypper;; *) echo unknown;; esac)"
    model=$(cat "${TRUESENSE_DMI:-/sys/class/dmi/id}/product_name" 2>/dev/null || echo unknown); say "model: $model $(model_tier "$model")"
    step "prerequisites"; check_prereqs
    for p in "${PROBLEMS[@]}"; do say "problem: $p"; done
    if [ "${#MISSING[@]}" -eq 0 ] && [ "${#PROBLEMS[@]}" -eq 0 ]; then say "everything needed is present: sudo tools/install.sh will go straight to installing"
    else [ "${#MISSING[@]}" -eq 0 ] || say "would install: ${MISSING[*]}"; fi
    [ "${#MISSING[@]}" -eq 0 ] && [ "${#PROBLEMS[@]}" -eq 0 ]; return
  fi

  [ "$(id -u)" -eq 0 ] || die "run with sudo:  sudo $0 $*      (or check first without root:  $0 --check)"
  local U=$TARGET_USER
  [ -n "$U" ] || U=${SUDO_USER:-}
  [ -n "$U" ] || { [ -n "${PKEXEC_UID:-}" ] && U=$(getent passwd "$PKEXEC_UID" | cut -d: -f1); }          # started through pkexec (the window's updater)
  if [ -z "$U" ] || [ "$U" = root ]; then                                                                      # root login: use the only ordinary user, if there is exactly one
    mapfile -t cand < <(awk -F: '$3>=1000 && $3<65000 && $7 !~ /(nologin|false)$/ {print $1}' /etc/passwd)
    [ "${#cand[@]}" -eq 1 ] && U=${cand[0]} || die "which user should get the keyboard service? re-run with --user=NAME (or use sudo from your own account)"
  fi
  id "$U" >/dev/null 2>&1 || die "no such user: $U"
  UID_=$(id -u "$U"); USER_NAME=$U
  mkdir -p /var/log; exec > >(tee -a /var/log/truesense-install.log) 2>&1
  trap 'echo "!! install failed at line $LINENO (log: /var/log/truesense-install.log). Fix the problem and re-run; the installer is safe to repeat." >&2' ERR
  ur() { runuser -u "$USER_NAME" -- env XDG_RUNTIME_DIR=/run/user/$UID_ DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$UID_/bus "$@"; }
  echo "TrueSense installer $(date '+%F %T')  prefix=$PREFIX  user=$USER_NAME  distribution=$OS_ID ($FAMILY)"

  step "pre-flight checks"
  model_gate || exit 1
  ensure_deps
  if have mokutil && mokutil --sb-state 2>/dev/null | grep -qi "enabled"; then
    if [ -e /var/lib/dkms/mok.pub ] && mokutil --test-key /var/lib/dkms/mok.pub 2>&1 | grep -qi "already enrolled"; then say "Secure Boot: on, DKMS signing key enrolled: ok"
    else say "WARNING: Secure Boot is on and the DKMS signing key is not enrolled: enrol it (sudo mokutil --import /var/lib/dkms/mok.pub), reboot, then re-run"; fi
  else say "Secure Boot: off (or unknown): no key enrolment needed"; fi
  precompile_module
  # the program is found (or built) now, while nothing on the system has changed: a missing or stale program must not be discovered after the old helpers are gone
  [ "$MODULE_ONLY" = 1 ] || find_binary

step "removing legacy helpers (backed up first)"
"$ROOT/tools/migrate-legacy.sh"

step "kernel module"
install -d /etc/modprobe.d /etc/modules-load.d /etc/udev/rules.d
install -m644 "$ROOT/packaging/truesense.modprobe.conf" /etc/modprobe.d/truesense.conf
install -m644 "$ROOT/packaging/truesense.modules-load.conf" /etc/modules-load.d/truesense.conf
install -m644 "$ROOT/packaging/70-truesense.rules" /etc/udev/rules.d/70-truesense.rules
udevadm control --reload
"$ROOT/tools/install_kernel_module.sh"
udevadm trigger --subsystem-match=misc --subsystem-match=hidraw 2>/dev/null || true; sleep 1
[ "$MODULE_ONLY" = 1 ] && { echo; echo "module-only install done."; return 0; }

step "TrueSense files"
if [ "$AUDIO" = 1 ]; then p=$(pkg audio); if [ -n "$p" ]; then pkg_install $p || say "(could not install $p)"; else say "(your distribution does not package the RNNoise LADSPA plugin: install noise-suppression-for-voice from its project page)"; fi; fi
install -d "$PREFIX/lib/truesense" "$PREFIX/bin" "$PREFIX/share/truesense/profiles" /etc/truesense
install -m755 "$ROOT/packaging/health.sh" "$PREFIX/lib/truesense/health.sh"
# The GPU undervolt helper: one small C program that talks to NVIDIA's library (the daemon is static and cannot load it, and must not hang on it). Not fatal if it cannot be built.
HELPER_CC=$(command -v cc || command -v gcc || command -v clang || true)
if [ -n "$HELPER_CC" ] && "$HELPER_CC" -O2 -Wall -o "$PREFIX/lib/truesense/truesense-nvml.new" "$ROOT/tools/nvml/truesense-nvml.c" -ldl 2>/dev/null; then
  chmod 755 "$PREFIX/lib/truesense/truesense-nvml.new"; mv -f "$PREFIX/lib/truesense/truesense-nvml.new" "$PREFIX/lib/truesense/truesense-nvml"
else rm -f "$PREFIX/lib/truesense/truesense-nvml.new"; say "(could not build the GPU helper: GPU undervolt will be unavailable; everything else works)"; fi
rm -f "$PREFIX/bin/truesense.new" "$PREFIX/bin/truesense.prev"
install -m755 "$BIN" "$PREFIX/bin/truesense.new"                                                              # beside the old one, then swap atomically
[ -e "$PREFIX/bin/truesense" ] && cp -p "$PREFIX/bin/truesense" "$PREFIX/bin/truesense.prev"
mv -f "$PREFIX/bin/truesense.new" "$PREFIX/bin/truesense"
ln -sf truesense "$PREFIX/bin/truesense-fast"                                                                 # one binary: the name selects the instant status-bar client
[ -d "$ROOT/packaging/profiles" ] && install -m644 "$ROOT"/packaging/profiles/*.toml "$PREFIX/share/truesense/profiles/"
if [ ! -e /etc/truesense/config.toml ]; then
  printf '# TrueSense system configuration (see docs/USAGE.md). Built-in defaults apply to everything not set here.\nuser = "%s"\n\n# First install: the PREDATOR key behaves like PredatorSense on Windows (it opens the app). Change it in the app: PREDATOR key.\n[keys]\nprofile = "native"\ntap = "exec:truesense gui"\ndouble = "none"\nlong = "none"\n' "$U" > /etc/truesense/config.toml
fi
for sh in bash zsh fish; do
  case $sh in bash) d=/usr/share/bash-completion/completions/truesense;; zsh) d=/usr/share/zsh/site-functions/_truesense;; fish) d=/usr/share/fish/vendor_completions.d/truesense.fish;; esac
  install -d "$(dirname "$d")"; "$PREFIX/bin/truesense" completion $sh > "$d" 2>/dev/null || rm -f "$d"
done
say "installed $PREFIX/bin/truesense (version $("$PREFIX/bin/truesense" --version | awk '{print $2}'))"

step "services"
for u in truesense.service truesense-health.service truesense-health.timer; do sed -e "s|@PREFIX@|$PREFIX|g" -e "s|@MODPROBE@|$(command -v modprobe)|g" "$ROOT/packaging/$u" > /etc/systemd/system/$u; done
install -Dm644 "$ROOT/LICENSE" "$PREFIX/share/licenses/truesense/LICENSE"
install -Dm644 "$ROOT/packaging/icons/truesense.svg" "$PREFIX/share/icons/hicolor/scalable/apps/truesense.svg"; install -Dm644 "$ROOT/packaging/icons/truesense-256.png" "$PREFIX/share/icons/hicolor/256x256/apps/truesense.png"
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t -f "$PREFIX/share/icons/hicolor" 2>/dev/null || true
install -d /usr/share/applications; sed "s|@PREFIX@|$PREFIX|g" "$ROOT/packaging/truesense.desktop" > /usr/share/applications/truesense.desktop; sed "s|@PREFIX@|$PREFIX|g" "$ROOT/packaging/truesense-tui.desktop" > /usr/share/applications/truesense-tui.desktop
for u in truesense-keys.service truesense-audio.service truesense-startup.service truesense-update.service truesense-update.timer; do sed -e "s|@PREFIX@|$PREFIX|g" "$ROOT/packaging/$u" > /etc/systemd/user/$u; done
have restorecon && restorecon -R "$PREFIX/lib/truesense" "$PREFIX/bin" /etc/systemd/system /usr/share/applications 2>/dev/null || true     # SELinux (Fedora)
systemctl daemon-reload; ur systemctl --user daemon-reload || true
if [ "$SERVICES" = 1 ]; then
  systemctl enable truesense.service truesense-health.timer >/dev/null 2>&1
  systemctl restart truesense.service || true    # `enable --now` would NOT restart a running service, so upgrades would keep the old code
  if ! wait_for_service; then rollback; die "the new version did not start (the previous one was restored if there was one)"; fi
  rm -f "$PREFIX/bin/truesense.prev"
  rm -r -f "$PREFIX/lib/truesense/truesense" "$PREFIX/lib/truesense.prev"                                     # the Python program of 0.3.x (the Go edition left nothing here), replaced by the binary
  systemctl start truesense-health.timer
  systemctl --global enable truesense-keys.service truesense-audio.service truesense-startup.service truesense-update.timer >/dev/null 2>&1
  ur systemctl --user restart truesense-keys.service truesense-audio.service truesense-startup.service || say "(user services start at your next login)"
  ur systemctl --user enable --now truesense-update.timer >/dev/null 2>&1 || true
  sleep 2
  if [ -r /var/lib/truesense/migrated_voice ]; then      # the user already had Purified Voice filters: recreate them under the new names, same modes
    hs=$(sed -n 's/^headset=//p' /var/lib/truesense/migrated_voice); ar=$(sed -n 's/^array=//p' /var/lib/truesense/migrated_voice)
    ur "$PREFIX/bin/truesense" audio voice set headset "${hs:-off}" >/dev/null 2>&1 || true
    ur "$PREFIX/bin/truesense" audio voice set array "${ar:-off}" >/dev/null 2>&1 || true
    rm -f /var/lib/truesense/migrated_voice; say "Purified Voice filters recreated (headset: ${hs:-off}, array: ${ar:-off})"
  fi
else say "services installed but not enabled (--no-services)"; fi

step "health check"
"$PREFIX/bin/truesense" doctor || say "the checker found problems: see above (re-run it any time: truesense doctor)"
echo; echo "TrueSense is installed. Try:  truesense status   |   truesense --help"
}

if [ -z "${TRUESENSE_LIB_ONLY:-}" ]; then main "$@"; fi
