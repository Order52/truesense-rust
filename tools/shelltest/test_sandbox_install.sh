#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# The real install.sh (with its real helper scripts) run in a sandbox: a fake system tree in a temporary directory, recorders instead of systemctl, dkms and modprobe, and tripwires
# for every program that could touch a real machine (see sandbox.sh). Fresh install, upgrade from the Go edition and from the Python edition, reinstall, the rollback paths, the
# refusals, --no-services, concurrency, and the guards that make the sandbox itself safe.
# shellcheck source=tools/shelltest/sandbox.sh
source "$(dirname "${BASH_SOURCE[0]}")/sandbox.sh"

# every file a successful install leaves, relative to the sandbox, with its mode (the units and desktop files differ from the packaging only by their placeholders)
EXPECTED_FILES=(
  "usr/local/bin/truesense 755" "usr/local/lib/truesense/health.sh 755" "usr/local/lib/truesense/truesense-nvml 755"
  "etc/systemd/system/truesense.service 644" "etc/systemd/system/truesense-health.service 644" "etc/systemd/system/truesense-health.timer 644"
  "etc/systemd/user/truesense-keys.service 644" "etc/systemd/user/truesense-audio.service 644" "etc/systemd/user/truesense-startup.service 644"
  "etc/systemd/user/truesense-update.service 644" "etc/systemd/user/truesense-update.timer 644"
  "usr/share/applications/truesense.desktop 644" "usr/share/applications/truesense-tui.desktop 644"
  "usr/local/share/icons/hicolor/scalable/apps/truesense.svg 644" "usr/local/share/icons/hicolor/256x256/apps/truesense.png 644" "usr/local/share/licenses/truesense/LICENSE 644"
  "etc/modprobe.d/truesense.conf 644" "etc/modules-load.d/truesense.conf 644" "etc/udev/rules.d/70-truesense.rules 644" "etc/truesense/config.toml 644"
  "usr/share/bash-completion/completions/truesense 644" "usr/share/zsh/site-functions/_truesense 644" "usr/share/fish/vendor_completions.d/truesense.fish 644"
)

t_fresh_install_puts_every_file_in_place_with_the_right_mode() {
  sb_new
  sb_install
  assert_code 0
  local row f mode
  for row in "${EXPECTED_FILES[@]}"; do f=${row% *}; mode=${row#* }; assert_mode "$SB/$f" "$mode"; done
  assert_same "$SB/usr/local/bin/truesense" "$NEWBIN"
  [ -L "$SB/usr/local/bin/truesense-fast" ] && assert_eq "$(readlink "$SB/usr/local/bin/truesense-fast")" truesense "the fast client is a link to the one program" || fail "truesense-fast is not a symlink"
  assert_same "$SB/usr/local/lib/truesense/health.sh" "$ROOT/packaging/health.sh"
  for f in "$ROOT"/packaging/profiles/*.toml; do assert_same "$SB/usr/local/share/truesense/profiles/$(basename "$f")" "$f"; done
  assert_same "$SB/etc/modprobe.d/truesense.conf" "$ROOT/packaging/truesense.modprobe.conf"
  assert_same "$SB/etc/modules-load.d/truesense.conf" "$ROOT/packaging/truesense.modules-load.conf"
  assert_same "$SB/etc/udev/rules.d/70-truesense.rules" "$ROOT/packaging/70-truesense.rules"
  assert_same "$SB/usr/local/share/licenses/truesense/LICENSE" "$ROOT/LICENSE"
  assert_same "$SB/usr/local/share/icons/hicolor/scalable/apps/truesense.svg" "$ROOT/packaging/icons/truesense.svg"
  # the placeholders are filled in everywhere, and in the right way
  local text; text=$(cat "$SB"/etc/systemd/system/truesense*.service "$SB"/etc/systemd/system/*.timer "$SB"/etc/systemd/user/* "$SB"/usr/share/applications/*.desktop)
  assert_not_contains "$text" "@PREFIX@" "units and desktop files"; assert_not_contains "$text" "@MODPROBE@" "units"
  assert_contains "$(cat "$SB/etc/systemd/system/truesense.service")" "ExecStart=/usr/local/bin/truesense daemon"
  assert_contains "$(cat "$SB/etc/systemd/system/truesense.service")" "ExecStopPost=-/usr/local/bin/truesense fan auto"
  assert_contains "$(cat "$SB/etc/systemd/system/truesense.service")" "ExecStartPre=-/usr/sbin/modprobe truesense_wmi"
  assert_contains "$(cat "$SB/etc/systemd/system/truesense-health.service")" "ExecStart=/usr/local/lib/truesense/health.sh"
  assert_contains "$(cat "$SB/usr/share/applications/truesense.desktop")" "Exec=/usr/local/bin/truesense gui"
  assert_contains "$(cat "$SB/etc/systemd/user/truesense-keys.service")" "ExecStart=/usr/local/bin/truesense keys run"
  # the default config names the desktop user; completions come from the new program
  assert_contains "$(cat "$SB/etc/truesense/config.toml")" 'user = "tester"'
  assert_contains "$(cat "$SB/usr/share/bash-completion/completions/truesense")" "completion for bash ($NEWVER)"
  # DKMS source and registration, the enable links
  for f in truesense_wmi.c Makefile dkms.conf; do assert_mode "$SB/usr/src/truesense-wmi-$NEWVER/$f" 644; done
  assert_eq "$(cat "$SB/sim/dkms_status")" "truesense-wmi/$NEWVER, $SB_KERNEL, x86_64: installed" "dkms status"
  [ -L "$SB/etc/systemd/system/multi-user.target.wants/truesense.service" ] || fail "truesense.service is not enabled"
  [ -L "$SB/etc/systemd/system/timers.target.wants/truesense-health.timer" ] || fail "the health timer is not enabled"
  # nothing half-installed is left behind
  assert_eq "$(find "$SB" \( -name '*.new' -o -name '*.prev' -o -name '*.prev.new' -o -name 'units.prev' \) | wc -l)" 0 "stray .new/.prev files"
  assert_contains "$R_OUT$R_ERR" "SANDBOX MODE" "the sandbox banner"
}

# ---- helpers ----------------------------------------------------------------------------------------------------------------------------------

# the files an upgrade must never touch, as a listing (path, mode, content hash)
user_data_tree() { sb_tree | grep -E '^\./(etc/truesense/|var/lib/truesense/|home/tester/\.config/truesense/)' || true; }
# the versions DKMS knows for our package, e.g. "0.5.0 "
dkms_versions() { sed -n 's|^truesense-wmi/\([^,:]*\).*|\1|p' "$SB/sim/dkms_status" | sort -u | tr '\n' ' '; }
# the @start line of calls.log: what the install directories held when the service was (re)started; $1 = which one (1 = first start)
start_line() { grep '^@start' "$SB/calls.log" | sed -n "${1:-1}p"; }
no_strays() { assert_eq "$(find "$SB" \( -name '*.new' -o -name '*.prev' -o -name '*.prev.new' -o -name 'units.prev' \) | wc -l)" 0 "stray .new/.prev files"; }

# ---- upgrade from the Go edition ----------------------------------------------------------------------------------------------------------

t_upgrade_from_the_go_edition_replaces_the_program_and_the_module_and_keeps_every_setting() {
  sb_new; sb_layout_go
  local before; before=$(user_data_tree); [ -n "$before" ] || fail "the test setup made no user data"
  sb_install
  assert_code 0
  assert_same "$SB/usr/local/bin/truesense" "$NEWBIN"
  assert_contains "$("$SB/usr/local/bin/truesense" --version)" "truesense $NEWVER"
  # the old program was in place (as the rollback copy) when the new service was started, and is gone once it runs
  assert_contains "$(start_line 1)" "truesense.prev" "the rollback copy at the first start"
  no_strays
  # config, state and the user's own settings: byte-identical, same modes
  assert_eq "$(user_data_tree)" "$before" "config, state and user settings"
  assert_contains "$(cat "$SB/etc/truesense/config.toml")" "# my own config" "an existing config.toml is never overwritten"
  # DKMS: the old version is removed (everywhere) and the new one added and built, in that order; the old source tree is gone
  assert_eq "$(dkms_versions)" "$NEWVER " "registered DKMS versions"
  assert_order "dkms remove truesense-wmi/0.4.0 --all" "dkms add truesense-wmi/$NEWVER" "dkms install truesense-wmi/$NEWVER -k $SB_KERNEL --force"
  assert_no_file "$SB/usr/src/truesense-wmi-0.4.0"; assert_file "$SB/usr/src/truesense-wmi-$NEWVER/dkms.conf"
  # the new unit replaced the Go one, and the service was restarted (not just enabled) and waited for
  assert_not_contains "$(cat "$SB/etc/systemd/system/truesense.service")" "OLD_EDITION" "the unit is the new edition's"
  assert_order "systemctl daemon-reload" "systemctl enable truesense.service truesense-health.timer" "systemctl restart truesense.service" "systemctl is-active --quiet truesense.service" "systemctl start truesense-health.timer"
  assert_not_called "@ALERT"
  [ -e "$SB/run/sandbox-active" ] || fail "the service is not running at the end"
}

t_upgrade_from_the_python_edition_removes_the_python_tree_only_after_the_service_started() {
  sb_new; sb_layout_python
  local before; before=$(user_data_tree)
  sb_install
  assert_code 0
  assert_same "$SB/usr/local/bin/truesense" "$NEWBIN"           # the shell wrapper is gone, the program is in its place
  [ -L "$SB/usr/local/bin/truesense-fast" ] || fail "the python fast client was not replaced by the link"
  # when the new service was started the python tree and its previous copy were still there (a rollback to them was possible) ...
  assert_contains "$(start_line 1)" " truesense " "the python tree at the start"
  assert_contains "$(start_line 1)" "truesense.prev" "the rollback copy at the start"
  # ... and afterwards they are gone, the helper files of the new edition are in lib/truesense
  assert_no_file "$SB/usr/local/lib/truesense/truesense"; assert_no_file "$SB/usr/local/lib/truesense.prev"
  assert_same "$SB/usr/local/lib/truesense/health.sh" "$ROOT/packaging/health.sh"; assert_file "$SB/usr/local/lib/truesense/truesense-nvml"
  no_strays
  assert_eq "$(user_data_tree)" "$before" "config, state and user settings"
  assert_eq "$(dkms_versions)" "$NEWVER " "registered DKMS versions"
  assert_order "dkms remove truesense-wmi/0.3.17 --all" "dkms add truesense-wmi/$NEWVER"
  assert_not_called "@ALERT"
}

# ---- reinstall -----------------------------------------------------------------------------------------------------------------------------

t_reinstalling_twice_gives_a_byte_identical_tree_and_no_stray_files() {
  sb_new
  sb_install; assert_code 0
  local first dkms1; first=$(sb_tree); dkms1=$(cat "$SB/sim/dkms_status")
  : > "$SB/calls.log"
  sb_install; assert_code 0
  assert_eq "$(sb_tree)" "$first" "the tree after the second install"
  assert_eq "$(cat "$SB/sim/dkms_status")" "$dkms1" "dkms status after the second install"
  no_strays
  assert_called "systemctl restart truesense.service"       # a reinstall restarts too, so it always runs the code that is on disk
  assert_not_called "@ALERT"
  sb_install; assert_code 0
  assert_eq "$(sb_tree)" "$first" "the tree after the third install"
}

t_a_config_that_exists_is_never_overwritten_and_a_missing_one_is_created_once() {
  sb_new
  sb_install; assert_code 0
  local cfg; cfg=$SB/etc/truesense/config.toml
  printf 'user = "someone"\n# mine\n' > "$cfg"; chmod 600 "$cfg"
  sb_install; assert_code 0
  assert_eq "$(cat "$cfg")" $'user = "someone"\n# mine' "the edited config"; assert_mode "$cfg" 600
  rm "$cfg"; sb_install; assert_code 0
  assert_contains "$(cat "$cfg")" 'user = "tester"' "a deleted config is created again"
}

# ---- rollback -------------------------------------------------------------------------------------------------------------------------------

t_a_program_that_cannot_report_its_version_stops_the_install_before_anything_changes() {
  sb_new; sb_layout_go
  fake_bin "$NEWBIN" "$NEWVER" noversion
  local before; before=$(sb_tree); local dk; dk=$(cat "$SB/sim/dkms_status")
  sb_install
  assert_code 1; assert_contains "$R_ERR$R_OUT" "does not run on this machine"
  assert_eq "$(sb_tree)" "$before" "the tree"; assert_eq "$(cat "$SB/sim/dkms_status")" "$dk" "dkms status"
  local c; for c in "systemctl disable" "systemctl restart" "systemctl stop" "dkms remove" "dkms add" "dkms install" "modprobe" "systemctl daemon-reload"; do assert_not_called "$c"; done
  [ -e "$SB/run/sandbox-active" ] || fail "the running service was disturbed"
}

t_a_stale_program_is_refused_before_anything_changes() {
  sb_new; sb_layout_go
  fake_bin "$NEWBIN" "0.4.9"
  local before; before=$(sb_tree)
  sb_install
  assert_code 1; assert_contains "$R_ERR$R_OUT" "0.4.9"; assert_contains "$R_ERR$R_OUT" "$NEWVER"; assert_contains "$R_ERR$R_OUT" "stale"
  assert_eq "$(sb_tree)" "$before" "the tree"
  assert_not_called "systemctl restart"; assert_not_called "dkms remove"
  # a bundle's own bin/truesense is checked the same way
  fake_bin "$SRC/bin/truesense" "0.1.0"
  sb_run install.sh TRUESENSE_BIN= -- "--user=$SB_USER"
  assert_code 1; assert_contains "$R_ERR$R_OUT" "stale"
  assert_eq "$(sb_tree)" "$before" "the tree"
}

t_the_service_failing_after_the_swap_restores_the_go_program_and_its_unit() {
  sb_new; sb_layout_go
  cp -p "$SB/usr/local/bin/truesense" "$TMP/old-bin"; cp -p "$SB/etc/systemd/system/truesense.service" "$TMP/old-unit"
  local before; before=$(user_data_tree)
  fake_bin "$NEWBIN" "$NEWVER" crash
  sb_install
  assert_code 1
  assert_contains "$R_ERR$R_OUT" "restoring the previous one"; assert_contains "$R_ERR$R_OUT" "the previous version is running again"
  assert_contains "$R_ERR$R_OUT" "the new version did not start"
  assert_same "$SB/usr/local/bin/truesense" "$TMP/old-bin"                                  # byte-identical
  assert_same "$SB/etc/systemd/system/truesense.service" "$TMP/old-unit"                    # the unit that program was written for
  [ "$(grep -c 'systemctl restart truesense.service' "$SB/calls.log")" -ge 2 ] || fail "no restart after the rollback: $(cat "$SB/calls.log")"
  assert_order "systemctl restart truesense.service" "systemctl daemon-reload" "systemctl restart truesense.service" "systemctl is-active"
  [ -e "$SB/run/sandbox-active" ] || fail "the previous version is not running again (the sandbox service is down)"
  assert_no_file "$SB/usr/local/bin/truesense.prev"; assert_no_file "$SB/usr/local/lib/truesense/units.prev"
  assert_eq "$(user_data_tree)" "$before" "config, state and user settings"
  assert_not_called "@ALERT"
}

t_the_service_failing_when_upgrading_from_python_rolls_back_to_the_python_wrapper_and_tree() {
  sb_new; sb_layout_python
  cp -p "$SB/usr/local/bin/truesense" "$TMP/old-wrapper"
  local tree_before; tree_before=$(cd "$SB/usr/local/lib" && find truesense truesense.prev -type f | sort | while read -r f; do echo "$f $(sha256sum < "$f")"; done)
  fake_bin "$NEWBIN" "$NEWVER" crash
  sb_install
  assert_code 1
  assert_same "$SB/usr/local/bin/truesense" "$TMP/old-wrapper"                              # the wrapper, byte-identical
  assert_file "$SB/usr/local/lib/truesense/truesense/__init__.py" "the python tree"; assert_file "$SB/usr/local/lib/truesense.prev/truesense/__init__.py" "the python tree's own backup"
  # the sandbox service only starts a python wrapper when the tree it points at exists: it is running again, on python
  [ -e "$SB/run/sandbox-active" ] || fail "the python edition is not running again"
  assert_contains "$R_ERR$R_OUT" "the previous version is running again"
  assert_eq "$(cd "$SB/usr/local/lib" && find truesense/truesense truesense.prev -type f | sort | while read -r f; do echo "$f $(sha256sum < "$f")"; done)" \
            "$(printf '%s\n' "$tree_before" | grep -E '^(truesense/truesense|truesense.prev)')" "the python files"
}

t_a_fresh_install_whose_service_does_not_start_leaves_the_service_stopped_and_the_fans_to_the_firmware() {
  sb_new
  fake_bin "$NEWBIN" "$NEWVER" crash
  sb_install
  assert_code 1
  assert_contains "$R_ERR$R_OUT" "no previous version"; assert_contains "$R_ERR$R_OUT" "back on the firmware's control"
  assert_not_contains "$R_ERR$R_OUT" "is running again"
  assert_order "systemctl restart truesense.service" "systemctl stop truesense.service" "@write /dev/truesense set_fan_auto"
  [ ! -e "$SB/run/sandbox-active" ] || fail "the service is still marked running"
  assert_eq "$(cat "$SB/dev/truesense")" "set_fan_auto" "the device was told to hand the fans back"
}

t_a_rollback_whose_previous_version_does_not_start_either_hands_the_fans_back() {
  sb_new; sb_layout_go
  : > "$SB/sim/service_start_fails"
  sb_install
  assert_code 1; assert_contains "$R_ERR$R_OUT" "did not start either"
  assert_order "systemctl restart truesense.service" "systemctl restart truesense.service" "@write /dev/truesense set_fan_auto"
}

t_an_interrupted_earlier_run_does_not_cost_the_older_program() {
  # a previous run was killed after the swap: the new program is already in place, the old one is only in truesense.prev. Running the installer again must keep that old copy.
  sb_new; sb_layout_go
  cp -p "$SB/usr/local/bin/truesense" "$TMP/old-bin"
  cp -p "$NEWBIN" "$SB/usr/local/bin/truesense"; cp -p "$TMP/old-bin" "$SB/usr/local/bin/truesense.prev"
  fake_bin "$NEWBIN" "$NEWVER"; cp -p "$NEWBIN" "$SB/usr/local/bin/truesense"                  # (same bytes as what the installer will install)
  printf '# TRUESENSE_SANDBOX_CRASH\n' >> "$NEWBIN"; cp -p "$NEWBIN" "$SB/usr/local/bin/truesense"
  sb_install
  assert_code 1
  assert_same "$SB/usr/local/bin/truesense" "$TMP/old-bin"
}

# ---- the unit never points at a missing program ---------------------------------------------------------------------------------------------

t_no_moment_exists_where_the_units_stop_program_is_missing() {
  # the fake service logs "@ALERT" when it is stopped while the program of ExecStopPost is not there; every path of an upgrade must leave that out of calls.log
  local layout
  for layout in go python; do
    sb_new; "sb_layout_$layout"
    sb_install; assert_code 0; assert_not_called "@ALERT"
    : > "$SB/calls.log"; rm -f "$SB/run/sandbox-active"; sb_service_running
    fake_bin "$NEWBIN" "$NEWVER" crash; sb_install; assert_code 1; assert_not_called "@ALERT"
  done
}

t_the_stop_program_detector_itself_works() {
  # the control for the test above: if the program is missing when the service stops, the fake service says so
  sb_new; sb_install; assert_code 0
  rm -f "$SB/usr/local/bin/truesense"
  run "PATH=$SB_PATH" "TRUESENSE_ROOT=$SB" TRUESENSE_LIB_ONLY=1 -- "$BASH_BIN" -c 'source "$0"; sys systemctl stop truesense.service' "$SRC/tools/install.sh"
  assert_contains "$(cat "$SB/calls.log")" "@ALERT ExecStopPost program missing: /usr/local/bin/truesense"
}

# ---- options -----------------------------------------------------------------------------------------------------------------------------

t_no_services_installs_the_files_but_enables_and_starts_nothing() {
  sb_new; sb_layout_go
  cp -p "$SB/usr/local/bin/truesense" "$TMP/old-bin"
  : > "$SB/calls.log"
  sb_install --no-services
  assert_code 0; assert_contains "$R_OUT" "not enabled (--no-services)"
  assert_same "$SB/usr/local/bin/truesense" "$NEWBIN"
  assert_same "$SB/usr/local/bin/truesense.prev" "$TMP/old-bin" "the rollback copy stays while nothing proved the new program"
  local c; for c in "systemctl enable" "systemctl restart" "systemctl start" "systemctl --global enable" "--user restart" "--user enable"; do assert_not_called "$c"; done
  assert_called "systemctl daemon-reload"
  assert_not_contains "$(cat "$SB/etc/systemd/system/truesense.service")" "OLD_EDITION" "the unit was still updated"
  # the next full install finishes the job
  sb_install; assert_code 0; no_strays
}

t_a_custom_prefix_moves_every_file_and_every_path_in_the_units() {
  sb_new
  sb_install --prefix=/opt/ts
  assert_code 0
  assert_same "$SB/opt/ts/bin/truesense" "$NEWBIN"; assert_file "$SB/opt/ts/lib/truesense/health.sh"; assert_file "$SB/opt/ts/share/licenses/truesense/LICENSE"
  assert_no_file "$SB/usr/local/bin/truesense"
  assert_contains "$(cat "$SB/etc/systemd/system/truesense.service")" "ExecStart=/opt/ts/bin/truesense daemon"
  assert_contains "$(cat "$SB/usr/share/applications/truesense.desktop")" "Exec=/opt/ts/bin/truesense gui"
  assert_contains "$(cat "$SB/etc/systemd/user/truesense-keys.service")" "ExecStart=/opt/ts/bin/truesense keys run"
}

t_the_prefix_must_be_an_absolute_plain_path() {
  local bad
  for bad in "relative/dir" "/with space" "/pipe|x" "/amp&x"; do
    sb_new; sb_baseline; sb_install "--prefix=$bad"
    assert_code 2; assert_contains "$R_ERR" "--prefix"
    assert_untouched "nothing is installed for the prefix '$bad'"
  done
}

t_the_gpu_helper_failing_to_build_is_not_fatal() {
  sb_new; : > "$SB/sim/helper_build_fails"
  sb_install
  assert_code 0; assert_contains "$R_OUT" "could not build the GPU helper"
  assert_no_file "$SB/usr/local/lib/truesense/truesense-nvml"; assert_no_file "$SB/usr/local/lib/truesense/truesense-nvml.new"
  assert_same "$SB/usr/local/bin/truesense" "$NEWBIN"
}

t_a_wrong_model_is_refused_before_anything_changes_and_force_is_remembered_only_when_given() {
  sb_new; printf 'Predator PH18-71\n' > "$SB/sys/class/dmi/id/product_name"; sb_baseline
  sb_install
  assert_code 1; assert_contains "$R_ERR$R_OUT" "not an Acer Predator PH16-71"
  assert_untouched "nothing is installed on another model"
  sb_install --force
  assert_code 0; assert_contains "$(cat "$SB/etc/modprobe.d/truesense-force.conf")" "options truesense_wmi force=1"
  printf 'Predator PH16-71\n' > "$SB/sys/class/dmi/id/product_name"
  sb_install; assert_code 0; assert_no_file "$SB/etc/modprobe.d/truesense-force.conf" "a normal install on the real model drops the force option"
}

t_missing_prerequisites_are_reported_with_no_deps_and_installed_otherwise() {
  sb_new; printf 'dkms\n' > "$SB/sim/missing"; sb_baseline
  sb_install --no-deps
  assert_code 1; assert_contains "$R_ERR$R_OUT" "install them"; assert_contains "$R_ERR$R_OUT" "dkms"
  assert_untouched "nothing is installed"
  sb_install
  assert_called "pacman -S --needed --noconfirm dkms"
}

t_the_voice_filters_of_the_old_edition_are_recreated_after_the_service_started() {
  sb_new
  write_file "$SB/var/lib/truesense/migrated_voice" 'headset=personal\narray=conference\n'
  sb_install; assert_code 0
  assert_order "systemctl restart truesense.service" "as-user:tester /usr/local/bin/truesense audio voice set headset personal" "as-user:tester /usr/local/bin/truesense audio voice set array conference"
  assert_no_file "$SB/var/lib/truesense/migrated_voice"
}

# ---- hardening ------------------------------------------------------------------------------------------------------------------------------

t_a_hardened_root_umask_does_not_make_units_or_config_unreadable() {
  sb_new
  umask 077; sb_install; umask 022
  assert_code 0
  local row f mode
  for row in "${EXPECTED_FILES[@]}"; do f=${row% *}; mode=${row#* }; assert_mode "$SB/$f" "$mode"; done
  assert_mode "$SB/etc/systemd/user" 755; assert_mode "$SB/usr/local/lib/truesense" 755; assert_mode "$SB/etc/truesense" 755
}

t_a_second_install_at_the_same_time_is_refused_and_changes_nothing() {
  sb_new; sb_layout_go
  local before; before=$(sb_tree)
  ( exec 8> "$SB/run/truesense-install.lock"; flock 8; exec sleep 60 ) >/dev/null 2>&1 & PIDS+=("$!")
  for _ in $(seq 1 100); do flock -n "$SB/run/truesense-install.lock" true 2>/dev/null || break; sleep 0.05; done
  sb_install
  assert_code 1; assert_contains "$R_ERR$R_OUT" "another TrueSense install or uninstall is running"
  assert_eq "$(sb_tree)" "$before" "the tree"
  assert_not_called "systemctl restart"
  # the lock is released with the process that held it: the next install goes through
  kill "${PIDS[0]}" 2>/dev/null; wait "${PIDS[0]}" 2>/dev/null
  sb_install; assert_code 0
}

t_module_only_installs_the_module_and_stops_there() {
  sb_new; sb_layout_go
  cp -p "$SB/usr/local/bin/truesense" "$TMP/old-bin"
  sb_install --module-only
  assert_code 0; assert_contains "$R_OUT" "module-only install done"
  assert_same "$SB/usr/local/bin/truesense" "$TMP/old-bin" "the program is not touched"
  assert_eq "$(dkms_versions)" "$NEWVER " "the module was replaced"
  assert_not_called "systemctl restart"; assert_not_called "systemctl enable"
}

t_the_installer_log_is_written_under_var_log() {
  sb_new; sb_install; assert_code 0
  assert_contains "$(cat "$SB/var/log/truesense-install.log")" "TrueSense installer" "the log"
}

# ---- the sandbox protects itself ------------------------------------------------------------------------------------------------------------

t_the_sandbox_refuses_anything_but_an_absolute_existing_marked_directory() {
  sb_new
  local script bad nomarker; nomarker=$(tmpdir)
  for script in install.sh uninstall.sh install_kernel_module.sh migrate-legacy.sh; do
    for bad in "relative" "/nonexistent/dir" "$nomarker" "/"; do
      run "PATH=$SB_PATH" "TRUESENSE_ROOT=$bad" -- "$BASH_BIN" "$SRC/tools/$script"
      assert_code 2; assert_contains "$R_ERR" "Nothing was done" "$script with TRUESENSE_ROOT=$bad"
      assert_not_contains "$R_ERR" "SANDBOX MODE" "$script must not enter sandbox mode for '$bad'"
    done
  done
  assert_eq "$(ls -A "$nomarker" | wc -l)" 0 "the unmarked directory was not written to"
  [ ! -s "$SB_TRIP" ] || fail "tripwire: $(cat "$SB_TRIP")"
}

t_every_script_announces_the_sandbox_loudly() {
  sb_new
  local script
  for script in install.sh uninstall.sh install_kernel_module.sh migrate-legacy.sh; do
    case $script in install.sh) sb_run "$script" -- "--user=$SB_USER";; *) sb_run "$script" -- ;; esac
    assert_contains "$R_ERR$R_OUT" "################ SANDBOX MODE: TRUESENSE_ROOT=$SB" "$script"
  done
}

t_the_four_copies_of_the_sandbox_block_are_identical() {
  local f first="" text
  for f in install.sh uninstall.sh install_kernel_module.sh migrate-legacy.sh; do
    text=$(sed -n '/^# >>> sandbox/,/^# <<< sandbox/p' "$ROOT/tools/$f")
    [ -n "$text" ] || { fail "$f has no sandbox block"; continue; }
    if [ -z "$first" ]; then first=$text; else [ "$text" = "$first" ] || fail "the sandbox block of $f differs from install.sh's"; fi
  done
}

t_root_is_not_required_in_the_sandbox_but_is_in_real_mode() {
  [ "$(id -u)" -ne 0 ] || skip "running as root: the refusal cannot be observed (and the scripts must never run for real)"
  local script
  for script in install.sh uninstall.sh install_kernel_module.sh migrate-legacy.sh; do
    run -- "$BASH_BIN" "$ROOT/tools/$script"
    assert_code 1; assert_contains "$R_OUT$R_ERR" "run with sudo" "$script (real mode)"
  done
}

# Every call that could change a real system goes through `sys` (so the sandbox can record it instead): a bare call in a script would bypass the sandbox
t_no_script_calls_a_system_program_outside_sys() {
  local f hits
  for f in install.sh uninstall.sh install_kernel_module.sh migrate-legacy.sh; do
    # the code outside the sandbox block, comments and quoted messages left aside: a command word at the start of a statement
    hits=$(sed '/^# >>> sandbox/,/^# <<< sandbox/d' "$ROOT/tools/$f" | grep -v '^[[:space:]]*#' \
      | grep -nE '(^|[;&|]|\<then|\<do|\<else|\{)[[:space:]]*(systemctl|dkms|modprobe|rmmod|insmod|depmod|udevadm|loginctl|runuser|pacman|apt-get|dnf|zypper|mokutil|gtk-update-icon-cache|restorecon|journalctl|lsmod|chown|cargo)[[:space:]]' || true)
    # the real-mode build of the program as the desktop user, and the user-session wrappers, are the only allowed bare uses (they are not sandboxed by design: sandbox mode skips them)
    hits=$(printf '%s\n' "$hits" | grep -vE 'builder=\(runuser|runuser -u "\$USER_NAME"|runuser -u "\$U"|chown "\$SUDO_USER"|cargo build|command -v cargo|are the only|^$' || true)
    [ -z "$hits" ] || fail "$f calls a system program without sys:
$hits"
  done
}

run_tests "$@"
