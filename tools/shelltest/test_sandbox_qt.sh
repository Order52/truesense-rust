#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# The optional Qt window (truesense-qt) through the real install.sh and uninstall.sh in the sandbox (see sandbox.sh): where the installer finds it (named, bundled, built), that a stale
# or unrunnable one is refused (an error only when the user named it), that its Qt 6 run-time libraries are installed or reported, that it is swapped in and rolled back like the
# program, that every way of not having it leaves a complete working install with the web-window fallback, and that the launcher entry and the About-box texts follow.
# shellcheck source=tools/shelltest/sandbox.sh
source "$(dirname "${BASH_SOURCE[0]}")/sandbox.sh"

# qt_new: a fresh sandbox plus a Qt window of this tree's version in QTBIN
qt_new() { sb_new; QTBIN=$(tmpdir)/truesense-qt; fake_qt_bin "$QTBIN" "$NEWVER"; }
# sb_install_qt [ARGS...]: an install that names $QTBIN
sb_install_qt() { sb_run install.sh "TRUESENSE_QT_BIN=$QTBIN" -- "--user=$SB_USER" "$@"; }
sb_uninstall() { sb_run uninstall.sh -- "--user=$SB_USER"; }
desktop() { cat "$SB/usr/share/applications/truesense.desktop"; }
no_strays() { assert_eq "$(find "$SB" \( -name '*.new' -o -name '*.prev' -o -name '*.prev.new' -o -name 'units.prev' \) | wc -l)" 0 "stray .new/.prev files"; }

# ---- a window that is named, bundled or built ---------------------------------------------------------------------------------------------

t_a_named_window_is_installed_beside_the_program_and_the_launcher_gets_its_page_shortcuts() {
  qt_new
  sb_install_qt
  assert_code 0
  assert_same "$SB/usr/local/bin/truesense-qt" "$QTBIN"; assert_mode "$SB/usr/local/bin/truesense-qt" 755
  assert_contains "$R_OUT" "installed /usr/local/bin/truesense-qt (version $NEWVER)"
  # `truesense gui` stays the entry (it hands over to the window, or opens the web one); the shortcuts start the window itself
  assert_contains "$(desktop)" "Exec=/usr/local/bin/truesense gui"; assert_contains "$(desktop)" "StartupWMClass=truesense"
  assert_contains "$(desktop)" "Actions=Monitor;Cooling;Lighting;"
  assert_contains "$(desktop)" "Exec=/usr/local/bin/truesense-qt --page lighting"
  assert_contains "$(desktop)" "Exec=/usr/local/bin/truesense-qt --page cooling"
  assert_contains "$(desktop)" "Exec=/usr/local/bin/truesense-qt --page monitor"
  assert_not_contains "$(desktop)" "@PREFIX@" "the desktop file"
  assert_not_called "TRIPWIRE"; assert_not_called "cargo"
  no_strays
}

t_a_window_in_the_bundle_is_found_and_the_named_one_wins() {
  qt_new
  fake_qt_bin "$SRC/bin/truesense-qt" "$NEWVER"; cp "$SRC/bin/truesense-qt" "$TMP/bundled"
  sb_install
  assert_code 0; assert_same "$SB/usr/local/bin/truesense-qt" "$TMP/bundled" "the bundle's bin/truesense-qt"
  # target/release/truesense-qt (make qt-release) is found too, after the bundle's
  qt_new
  fake_qt_bin "$SRC/target/release/truesense-qt" "$NEWVER"; fake_qt_bin "$SRC/bin/truesense-qt" "$NEWVER"; printf '# bundled\n' >> "$SRC/bin/truesense-qt"; cp "$SRC/bin/truesense-qt" "$TMP/bundled"
  sb_install
  assert_code 0; assert_same "$SB/usr/local/bin/truesense-qt" "$TMP/bundled" "bin/ wins over target/release"
  rm -f "$SRC/bin/truesense-qt"; sb_install; assert_code 0
  assert_same "$SB/usr/local/bin/truesense-qt" "$SRC/target/release/truesense-qt" "the make qt-release build"
  qt_new; fake_qt_bin "$SRC/bin/truesense-qt" "$NEWVER"; printf '# bundled\n' >> "$SRC/bin/truesense-qt"
  sb_install_qt
  assert_code 0; assert_same "$SB/usr/local/bin/truesense-qt" "$QTBIN" "TRUESENSE_QT_BIN wins"
}

t_the_window_is_built_from_source_as_the_user_when_rust_and_the_qt_development_files_are_there() {
  sb_new
  fake_qt_bin "$TMP/built" "$NEWVER"; cp "$TMP/built" "$SB/sim/qt_built"; : > "$SB/sim/qt_dev"
  sb_install
  assert_code 0
  assert_contains "$R_OUT" "building the Qt window from source"
  assert_called "cargo build --release --locked -p ts-qt"
  assert_same "$SRC/target/release/truesense-qt" "$TMP/built"; assert_same "$SB/usr/local/bin/truesense-qt" "$TMP/built"
  # the build comes after the program was found and before the first change to the system
  assert_order "cargo build --release --locked -p ts-qt" "dkms add truesense-wmi/$NEWVER"
}

t_a_window_that_cannot_be_built_leaves_a_complete_install_with_the_web_window() {
  local why
  for why in "no_dev|Qt 6 development files, which are missing (install: qt6-base qt6-declarative qt6-svg qt6-tools)" "no_cargo|no Rust toolchain" "no_cxx|needs a C++ compiler" "build_fails|did not build"; do
    sb_new
    case ${why%%|*} in
      no_dev) ;;
      no_cargo) : > "$SB/sim/qt_dev"; printf 'cargo\n' > "$SB/sim/missing";;
      no_cxx) : > "$SB/sim/qt_dev"; printf 'c++\ng++\nclang++\n' > "$SB/sim/missing";;
      build_fails) : > "$SB/sim/qt_dev"; printf 'cargo build*\n' > "$SB/sim/fail";;
    esac
    sb_install
    assert_code 0; assert_contains "$R_OUT" "the Qt window was NOT installed: "; assert_contains "$R_OUT" "${why#*|}" "${why%%|*}"
    assert_contains "$R_OUT" "built-in web window" "the fallback is named"
    assert_no_file "$SB/usr/local/bin/truesense-qt"
    assert_same "$SB/usr/local/bin/truesense" "$NEWBIN"; [ -e "$SB/run/sandbox-active" ] || fail "${why%%|*}: the service is not running"
    # the launcher entry then has no shortcuts that would start a missing program
    assert_contains "$(desktop)" "Exec=/usr/local/bin/truesense gui"; assert_not_contains "$(desktop)" "Desktop Action" "no Qt window"; assert_not_contains "$(desktop)" "Actions=" "no Qt window"
    assert_not_contains "$(desktop)" "bin/truesense-qt" "no Qt window"
  done
}

t_no_qt_skips_the_window_even_when_one_is_named() {
  qt_new
  sb_install_qt --no-qt
  assert_code 0; assert_contains "$R_OUT" "--no-qt was given"; assert_no_file "$SB/usr/local/bin/truesense-qt"; assert_not_contains "$(desktop)" "Desktop Action"
  sb_run install.sh -- "--user=$SB_USER" --no-gui
  assert_code 0; assert_not_contains "$R_OUT" "--no-qt was given" "--no-gui does nothing"
}

# ---- stale and broken windows ---------------------------------------------------------------------------------------------------------------

t_a_named_stale_or_unrunnable_window_stops_the_install_before_anything_changes() {
  qt_new; sb_baseline
  fake_qt_bin "$QTBIN" 0.0.1
  sb_install_qt
  assert_code 1; assert_contains "$R_ERR$R_OUT" "0.0.1"; assert_contains "$R_ERR$R_OUT" "$NEWVER"; assert_contains "$R_ERR$R_OUT" "stale"; assert_contains "$R_ERR$R_OUT" "make qt-release"
  assert_untouched "a stale named window"
  fake_qt_bin "$QTBIN" "$NEWVER" noversion
  sb_install_qt
  assert_code 1; assert_contains "$R_ERR$R_OUT" "does not run on this machine"
  assert_untouched "an unrunnable named window"
  sb_run install.sh "TRUESENSE_QT_BIN=$TMP/nonexistent" -- "--user=$SB_USER"
  assert_code 1; assert_contains "$R_ERR$R_OUT" "is not an executable file"
  assert_untouched "a window that does not exist"
  for c in "systemctl restart" "dkms add" "dkms remove" "modprobe" "pacman"; do assert_not_called "$c"; done
}

t_a_stale_window_that_was_found_on_its_own_is_left_out_and_the_rest_installs() {
  sb_new
  fake_qt_bin "$SRC/target/release/truesense-qt" 0.0.1
  sb_install
  assert_code 0
  assert_contains "$R_OUT" "the Qt window was NOT installed: "; assert_contains "$R_OUT" "stale"; assert_contains "$R_OUT" "0.0.1"; assert_contains "$R_OUT" "make qt-release"
  assert_no_file "$SB/usr/local/bin/truesense-qt"
  assert_same "$SB/usr/local/bin/truesense" "$NEWBIN"; [ -e "$SB/run/sandbox-active" ] || fail "the service is not running"
}

t_a_window_that_cannot_start_is_left_out_when_it_was_found_on_its_own() {
  sb_new
  fake_qt_bin "$SRC/bin/truesense-qt" "$NEWVER" noversion
  sb_install
  assert_code 0; assert_contains "$R_OUT" "does not run on this machine"; assert_no_file "$SB/usr/local/bin/truesense-qt"
}

t_an_older_installed_window_is_kept_and_named_when_the_new_one_is_left_out() {
  sb_new; fake_qt_bin "$SB/usr/local/bin/truesense-qt" 0.4.0; cp -p "$SB/usr/local/bin/truesense-qt" "$TMP/old-qt"
  sb_install
  assert_code 0
  assert_contains "$R_OUT" "the Qt window was NOT updated: "; assert_contains "$R_OUT" "The one installed (0.4.0) stays"
  assert_same "$SB/usr/local/bin/truesense-qt" "$TMP/old-qt"
  assert_contains "$(desktop)" "Desktop Action Lighting" "the installed window keeps its shortcuts"
}

# ---- the Qt 6 run-time libraries ------------------------------------------------------------------------------------------------------------

t_missing_qt_libraries_are_installed_before_anything_changes_and_with_the_right_names_per_family() {
  local row fam pkgs
  for row in "arch|pacman -S --needed --noconfirm qt6-base qt6-declarative qt6-svg" \
             "debian|apt-get install -y libqt6svg6 qml6-module-qtquick qml6-module-qtquick-controls qml6-module-qtquick-layouts qml6-module-qtquick-window qml6-module-qt-labs-platform" \
             "fedora|dnf install -y qt6-qtbase-gui qt6-qtdeclarative qt6-qtsvg" \
             "suse|zypper --non-interactive install libQt6Svg6 qt6-declarative-imports"; do
    fam=${row%%|*}; pkgs=${row#*|}
    qt_new; : > "$SB/sim/qt_runtime_missing"
    case $fam in arch) ;; debian) printf 'ID=debian\n' > "$SB/etc/os-release";; fedora) printf 'ID=fedora\n' > "$SB/etc/os-release";; suse) printf 'ID=opensuse-tumbleweed\nID_LIKE="opensuse suse"\n' > "$SB/etc/os-release";; esac
    # the module toolchain is not what this test is about: every program the prerequisite check asks for is there, and so are the headers
    mkdir -p "$SB/usr/lib/modules/$SB_KERNEL"
    sb_install_qt
    assert_code 0; assert_contains "$R_OUT" "the Qt 6 run-time libraries are missing"
    assert_called "$pkgs"
    assert_order "$pkgs" "dkms add truesense-wmi/$NEWVER" "systemctl restart truesense.service"
    assert_same "$SB/usr/local/bin/truesense-qt" "$QTBIN" "$fam"
  done
}

t_missing_qt_libraries_with_no_deps_or_a_failing_package_manager_leave_the_window_out() {
  qt_new; : > "$SB/sim/qt_runtime_missing"
  sb_install_qt --no-deps
  assert_code 0; assert_contains "$R_OUT" "--no-deps was given (install: qt6-base qt6-declarative qt6-svg)"; assert_no_file "$SB/usr/local/bin/truesense-qt"; assert_not_called "pacman -S"
  qt_new; : > "$SB/sim/qt_runtime_missing"; printf 'pacman -S --needed --noconfirm qt6*\n' > "$SB/sim/fail"
  sb_install_qt
  assert_code 0; assert_contains "$R_OUT" "installing the Qt 6 run-time libraries failed (install them yourself: qt6-base qt6-declarative qt6-svg)"; assert_no_file "$SB/usr/local/bin/truesense-qt"
  assert_same "$SB/usr/local/bin/truesense" "$NEWBIN"
  qt_new; : > "$SB/sim/qt_runtime_missing"; : > "$SB/sim/qt_runtime_stays"
  sb_install_qt
  assert_code 0; assert_contains "$R_OUT" "still missing after installing"; assert_no_file "$SB/usr/local/bin/truesense-qt"
  printf 'ID=gentoo\n' > "$SB/etc/os-release"; : > "$SB/sim/qt_runtime_missing"
  sb_install_qt
  assert_code 0; assert_contains "$R_OUT" "this distribution is not recognised (install: Qt 6 base, declarative (QML) and svg)"
}

# ---- swap and rollback ----------------------------------------------------------------------------------------------------------------------

t_the_window_is_swapped_in_with_a_rollback_copy_that_is_gone_once_the_service_runs() {
  qt_new; fake_qt_bin "$SB/usr/local/bin/truesense-qt" 0.4.0
  sb_install_qt
  assert_code 0
  assert_contains "$(grep '^@start' "$SB/calls.log" | head -n1)" "truesense-qt.prev" "the old window was in place as the rollback copy when the service started"
  no_strays; assert_same "$SB/usr/local/bin/truesense-qt" "$QTBIN"
}

t_a_service_that_does_not_start_puts_the_previous_window_back_with_the_previous_program() {
  qt_new; sb_layout_go
  fake_qt_bin "$SB/usr/local/bin/truesense-qt" 0.4.0; cp -p "$SB/usr/local/bin/truesense-qt" "$TMP/old-qt"; cp -p "$SB/usr/local/bin/truesense" "$TMP/old-bin"
  fake_bin "$NEWBIN" "$NEWVER" crash
  sb_install_qt
  assert_code 1; assert_contains "$R_ERR$R_OUT" "restoring the previous one"
  assert_same "$SB/usr/local/bin/truesense" "$TMP/old-bin"; assert_same "$SB/usr/local/bin/truesense-qt" "$TMP/old-qt" "the window goes back with its daemon"
  no_strays
}

t_an_interrupted_earlier_run_does_not_cost_the_older_window() {
  # killed after the swap: the new window is in place, the old one only in truesense-qt.prev; the next run must keep that old copy
  qt_new; fake_qt_bin "$TMP/old-qt" 0.4.0
  cp -p "$QTBIN" "$SB/usr/local/bin/truesense-qt"; cp -p "$TMP/old-qt" "$SB/usr/local/bin/truesense-qt.prev"
  sb_install_qt --no-services
  assert_code 0
  assert_same "$SB/usr/local/bin/truesense-qt.prev" "$TMP/old-qt" "the older window"
  assert_same "$SB/usr/local/bin/truesense-qt" "$QTBIN"
}

t_reinstalling_with_a_window_gives_a_byte_identical_tree() {
  qt_new
  sb_install_qt; assert_code 0
  local first; first=$(sb_tree)
  sb_install_qt; assert_code 0
  assert_eq "$(sb_tree)" "$first" "the tree after the second install"; no_strays
}

# ---- prefix, texts, uninstall ----------------------------------------------------------------------------------------------------------------

t_a_custom_prefix_moves_the_window_and_its_shortcuts() {
  qt_new
  sb_install_qt --prefix=/opt/ts
  assert_code 0
  assert_same "$SB/opt/ts/bin/truesense-qt" "$QTBIN"; assert_no_file "$SB/usr/local/bin/truesense-qt"
  assert_contains "$(desktop)" "Exec=/opt/ts/bin/truesense-qt --page lighting"
}

t_the_attribution_texts_the_about_box_reads_are_installed_with_or_without_the_window() {
  local d
  sb_new; sb_install; assert_code 0
  for d in doc licenses; do
    assert_same "$SB/usr/local/share/$d/truesense/THIRD_PARTY.md" "$ROOT/THIRD_PARTY.md"; assert_mode "$SB/usr/local/share/$d/truesense/THIRD_PARTY.md" 644
    assert_same "$SB/usr/local/share/$d/truesense/NOTICE-ts-fx.md" "$ROOT/crates/ts-fx/NOTICE.md"
  done
  # the places the Settings page tries (it must try the prefix the installer defaults to)
  local qml; qml=$(cat "$ROOT/crates/ts-qt/qml/pages/SettingsPage.qml")
  for d in "/usr/share/doc/truesense/THIRD_PARTY.md" "/usr/share/licenses/truesense/THIRD_PARTY.md" "/usr/local/share/doc/truesense/THIRD_PARTY.md"; do assert_contains "$qml" "file://$d" "the About box"; done
}

t_uninstall_removes_the_window_its_copies_and_the_texts_and_nothing_else() {
  qt_new
  write_file "$SB/usr/share/applications/other.desktop" '[Desktop Entry]\n'; write_file "$SB/usr/local/bin/other" '#!/bin/sh\n' 755
  sb_baseline
  sb_install_qt; assert_code 0
  fake_qt_bin "$SB/usr/local/bin/truesense-qt.prev" 0.4.0; : > "$SB/usr/local/bin/truesense-qt.new"
  sb_uninstall; assert_code 0
  assert_eq "$(sb_tree | grep -v '^\./etc/truesense/config.toml')" "$SB_BASE" "the tree after install and uninstall"
  assert_no_file "$SB/usr/local/share/doc/truesense"
}

run_tests "$@"
