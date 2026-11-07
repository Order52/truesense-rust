#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Tests of what is shipped around the program: the PKGBUILDs, the systemd units and desktop entries, the files the installer refers to, and tools/ci/check-bundle.sh (which guards
# every release: it must refuse a bundle that is incomplete, dynamic, stale or not reproducible).
# shellcheck source=tools/shelltest/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
P=$ROOT/packaging

t_packages_build_the_rust_program_and_need_no_python_or_socat() {
  local f text w line
  for f in packaging/arch/PKGBUILD packaging/aur/PKGBUILD.template; do
    text=$(cat "$ROOT/$f")
    for w in "makedepends=('rust' 'gcc')" "cargo build --release --locked" "-p truesense" "crt-static" "--target \"\$_target\"" "arch=('x86_64')" truesense-fast \
             tools/nvml/truesense-nvml.c usr/lib/truesense/truesense-nvml "cargo fetch --locked" readelf INTERP "options=('!debug')" 'depends=('"'dkms' 'systemd'"')'; do
      assert_contains "$text" "$w" "$f"
    done
    ! grep -qE '(^|[^a-z])go build' <<<"$text" || fail "$f still builds with go"        # (`cargo build` contains the letters, so the match needs a word start)
    assert_not_contains "$text" "GOFLAGS" "$f"
    # DKMS wants /usr/src/<name>-<the version in dkms.conf>: the package reads it from there instead of repeating a number that could drift
    assert_contains "$text" 'dkms.conf")' "$f"; assert_contains "$text" 'usr/src/truesense-wmi-$_dkms' "$f"
    while IFS= read -r line; do
      case $line in depends=*python*|depends=*socat*|depends=*qt*) fail "$f: $line";; esac
    done <<<"$text"
  done
}

t_both_pkgbuilds_build_the_same_way() {
  # the AUR template differs from the checkout PKGBUILD only in where the source comes from
  local diff; diff=$(diff <(sed -e '/^# /d' "$P/arch/PKGBUILD") <(sed -e '/^# /d' "$P/aur/PKGBUILD.template") | grep '^[<>]' | sed 's/^\(.\) */\1 /' | sort || true)
  assert_not_contains "$diff" "cargo build" "the build commands must be identical"
  assert_not_contains "$diff" "install -Dm" "the package() file list must be identical"
  assert_contains "$(cat "$P/aur/PKGBUILD.template")" "@SHA@" "the template waits for the tarball's hash"
  assert_contains "$(cat "$P/aur/PKGBUILD.template")" 'archive/refs/tags/v$pkgver.tar.gz'
}

t_the_units_run_the_rust_program_with_the_right_subcommands() {
  local row unit want
  for row in "truesense.service|ExecStart=@PREFIX@/bin/truesense daemon" "truesense-keys.service|ExecStart=@PREFIX@/bin/truesense keys run" \
             "truesense-audio.service|ExecStart=@PREFIX@/bin/truesense audio voice watch" "truesense-startup.service|ExecStart=@PREFIX@/bin/truesense startup apply" \
             "truesense-update.service|ExecStart=@PREFIX@/bin/truesense update auto-run" "truesense-health.service|ExecStart=@PREFIX@/lib/truesense/health.sh" \
             "truesense.service|ExecStopPost=-@PREFIX@/bin/truesense fan auto" "truesense.service|Type=notify" "truesense.service|WatchdogSec=20" \
             "truesense.service|ExecStartPre=-@MODPROBE@ truesense_wmi"; do
    unit=${row%%|*}; want=${row#*|}
    assert_line "$(cat "$P/$unit")" "$want" "$unit"
  done
  for unit in "$P"/*.service "$P"/*.timer; do
    assert_not_contains "$(cat "$unit")" GOMAXPROCS "$(basename "$unit")"; assert_not_contains "$(cat "$unit")" GOGC "$(basename "$unit")"
    assert_not_contains "$(cat "$unit")" "Go workers" "$(basename "$unit")"
  done
}

t_the_desktop_entries_start_the_window_and_the_terminal_ui() {
  assert_line "$(cat "$P/truesense.desktop")" "Exec=@PREFIX@/bin/truesense gui"
  assert_line "$(cat "$P/truesense-tui.desktop")" "Exec=@PREFIX@/bin/truesense tui"
  assert_line "$(cat "$P/truesense-tui.desktop")" "Terminal=true"
}

# everything the installer and the packages copy must exist in the tree (a renamed file would otherwise be found only on a real install)
t_every_file_the_installer_and_the_packages_refer_to_exists() {
  local f missing=""
  for f in $(grep -ohE '(packaging|tools|kernel)/[A-Za-z0-9._/-]+\.(sh|service|timer|desktop|toml|svg|png|c|conf|rules|template|install)' "$ROOT/tools/install.sh" "$ROOT/tools/uninstall.sh" "$P/arch/PKGBUILD" "$P/aur/PKGBUILD.template" | sort -u); do
    [ -e "$ROOT/$f" ] || missing="$missing $f"
  done
  assert_eq "$missing" "" "files named by the scripts that do not exist"
  for f in LICENSE README.md docs/SAFETY.md packaging/profiles/gaming.toml packaging/icons/truesense.svg packaging/icons/truesense-256.png packaging/truesense.modules-load.conf; do assert_file "$ROOT/$f"; done
  assert_eq "$(ls "$P"/profiles/*.toml | wc -l)" 4 "profiles"
}

t_the_installer_and_the_uninstaller_agree_on_the_installed_files() {
  # what install.sh puts in place that uninstall.sh must take out again
  local text; text=$(cat "$ROOT/tools/uninstall.sh")
  local w; for w in '$PREFIX/bin/truesense' '$PREFIX/bin/truesense-fast' '$PREFIX/bin/truesense.prev' '$PREFIX/lib/truesense' '/etc/modprobe.d/truesense.conf' '/etc/udev/rules.d/70-truesense.rules' \
             '/usr/share/applications/truesense.desktop' '/etc/systemd/user/truesense-'; do assert_contains "$text" "$w" "uninstall.sh"; done
}

# ---- tools/ci/check-bundle.sh: the release gate must refuse a bad bundle -------------------------------------------------------------------

REQUIRED=(tools/install.sh tools/get.sh tools/uninstall.sh Cargo.toml Cargo.lock crates/truesense/src/main.rs kernel/truesense-wmi/truesense_wmi.c kernel/truesense-wmi/dkms.conf
          packaging/truesense.service packaging/health.sh tools/nvml/truesense-nvml.c LICENSE)

# static_program DIR OUT VERSION [dynamic]: a tiny program that answers --version; static-pie like the real one, or dynamic. Skips the test when gcc cannot make it.
static_program() {
  local flags=(-static-pie) src; src=$(tmpdir)/v.c
  [ "${4:-}" != dynamic ] || flags=()
  printf '#include <stdio.h>\nint main(void){puts("truesense %s");return 0;}\n' "$3" > "$src"
  gcc -O1 ${flags[@]+"${flags[@]}"} -o "$2" "$src" 2>/dev/null || skip "gcc cannot build a ${4:-static} test program here"
}

# make_bundle_dir VERSION [OPTION...]: a directory holding truesense-VERSION.tar.gz and truesense like build-bundle.sh writes them (prints its path).
# OPTIONS: drop=FILE (leave a file out), add=FILE (add a file), dynamic, saysversion=X, cargo=X (the Cargo.toml version), notroot, differentbin
make_bundle_dir() {
  local v=$1 out stage top o drop="" add="" dyn="" says=$1 cargo=$1 owner="--owner=0 --group=0" diffbin=0; shift
  for o in "$@"; do case $o in drop=*) drop=${o#drop=};; add=*) add=${o#add=};; dynamic) dyn=dynamic;; saysversion=*) says=${o#saysversion=};; cargo=*) cargo=${o#cargo=};; notroot) owner="";; differentbin) diffbin=1;; esac; done
  out=$(tmpdir); stage=$(tmpdir); top=$stage/truesense-$v
  local f; for f in "${REQUIRED[@]}"; do [ "$f" = "$drop" ] || write_file "$top/$f" "x\n"; done
  [ -z "$add" ] || write_file "$top/$add" "x\n"
  [ "$drop" = Cargo.toml ] || write_file "$top/Cargo.toml" "[workspace.package]\nversion = \"$cargo\"\nrust-version = \"1.85\"\n"
  static_program "$top" "$out/truesense" "$says" $dyn || return 1
  mkdir -p "$top/bin"; cp "$out/truesense" "$top/bin/truesense"
  [ "$diffbin" = 0 ] || printf 'extra' >> "$top/bin/truesense"
  # shellcheck disable=SC2086  # $owner is two options or nothing
  tar --sort=name --mtime=@0 $owner --numeric-owner --format=gnu -cf - -C "$stage" "truesense-$v" | gzip -n -9 > "$out/truesense-$v.tar.gz"
  printf '%s\n' "$out"
}
check_bundle() { run -- "$BASH_BIN" "$ROOT/tools/ci/check-bundle.sh" "$@"; }

t_check_bundle_accepts_a_complete_static_reproducible_bundle() {
  local a b; a=$(make_bundle_dir 0.5.0) || return 0; b=$(tmpdir); cp "$a"/* "$b/"
  check_bundle 0.5.0 "$a" "$b"
  assert_code 0; assert_contains "$R_OUT" "reproducible: truesense-0.5.0.tar.gz"; assert_contains "$R_OUT" "bundle ok"; assert_contains "$R_OUT" "truesense 0.5.0, static"
  check_bundle 0.5.0 "$a"; assert_code 0          # one directory: no comparison
}

t_check_bundle_refuses_a_bundle_that_is_not_reproducible() {
  local a b; a=$(make_bundle_dir 0.5.0) || return 0; b=$(tmpdir); cp "$a"/* "$b/"; printf 'x' >> "$b/truesense-0.5.0.tar.gz"
  check_bundle 0.5.0 "$a" "$b"
  assert_code 1; assert_contains "$R_ERR" "is not reproducible"
}

t_check_bundle_names_a_missing_file() {
  local f a
  for f in "${REQUIRED[@]}" bin/truesense; do
    [ "$f" != bin/truesense ] || continue        # bin/truesense is always added from the built program; its absence is covered by the executable check
    a=$(make_bundle_dir 0.5.0 "drop=$f") || return 0
    check_bundle 0.5.0 "$a"
    [ "$R_CODE" = 1 ] || fail "without $f: exit $R_CODE"
    assert_contains "$R_ERR" "the bundle lacks $f" "without $f"
  done
}

t_check_bundle_refuses_kernel_build_leftovers() {
  local f a
  for f in kernel/truesense-wmi/truesense_wmi.ko kernel/truesense-wmi/truesense_wmi.o kernel/truesense-wmi/Module.symvers kernel/truesense-wmi/.truesense_wmi.ko.cmd kernel/truesense-wmi/modules.order; do
    a=$(make_bundle_dir 0.5.0 "add=$f") || return 0
    check_bundle 0.5.0 "$a"; [ "$R_CODE" = 1 ] || fail "with $f: exit $R_CODE"
    assert_contains "$R_ERR" "kernel build leftovers" "with $f"
  done
}

t_check_bundle_refuses_a_program_that_is_not_root_owned_a_stale_one_a_dynamic_one_and_a_wrong_cargo_version() {
  local a
  a=$(make_bundle_dir 0.5.0 notroot) || return 0; check_bundle 0.5.0 "$a"; assert_code 1; assert_contains "$R_ERR" "owned by root:root"
  a=$(make_bundle_dir 0.5.0 saysversion=0.4.0) || return 0; check_bundle 0.5.0 "$a"; assert_code 1; assert_contains "$R_ERR" "says 'truesense 0.4.0', expected 'truesense 0.5.0'"
  a=$(make_bundle_dir 0.5.0 dynamic) || return 0; check_bundle 0.5.0 "$a"; assert_code 1; assert_contains "$R_ERR" "dynamically linked"
  a=$(make_bundle_dir 0.5.0 cargo=0.4.0) || return 0; check_bundle 0.5.0 "$a"; assert_code 1; assert_contains "$R_ERR" "the updater would refuse the release"
  a=$(make_bundle_dir 0.5.0 differentbin) || return 0; check_bundle 0.5.0 "$a"; assert_code 1; assert_contains "$R_ERR" "is not the program that was built"
}

t_build_bundle_refuses_what_is_not_a_version() {
  run -- "$BASH_BIN" "$ROOT/tools/ci/build-bundle.sh" "1.0; true" "$(tmpdir)"
  assert_code 1; assert_contains "$R_ERR" "not a version"
  run -- "$BASH_BIN" "$ROOT/tools/ci/build-binary.sh" "$(tmpdir)"
  assert_code 1; assert_contains "$R_ERR" "no Cargo.toml"
}

run_tests "$@"
