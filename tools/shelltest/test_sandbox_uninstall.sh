#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# The real uninstall.sh, install_kernel_module.sh and migrate-legacy.sh run in the sandbox (see sandbox.sh): the fans go back to the firmware before anything is deleted, the
# uninstall removes every file the install made and nothing else (and is repeatable), the kernel-module script replaces every registered DKMS version and refuses when the
# running kernel cannot be built for, the legacy cleanup backs everything up before it removes anything.
# shellcheck source=tools/shelltest/sandbox.sh
source "$(dirname "${BASH_SOURCE[0]}")/sandbox.sh"

sb_uninstall() { sb_run uninstall.sh -- "--user=$SB_USER" "$@"; }
dkms_versions() { sed -n 's|^truesense-wmi/\([^,:]*\).*|\1|p' "$SB/sim/dkms_status" | sort -u | tr '\n' ' '; }

# ---- uninstall ------------------------------------------------------------------------------------------------------------------------------

t_uninstall_hands_the_fans_back_and_stops_the_service_before_it_deletes_anything() {
  sb_new; sb_install; assert_code 0
  : > "$SB/calls.log"
  sb_uninstall; assert_code 0
  assert_order "@write /dev/truesense set_fan_auto" "systemctl disable --now truesense.service" "modprobe -r truesense_wmi" "dkms remove truesense-wmi/$NEWVER --all" "systemctl daemon-reload"
  assert_not_called "@ALERT"
  [ ! -e "$SB/run/sandbox-active" ] || fail "the service is still running after the uninstall"
}

t_uninstall_removes_every_file_the_install_made_and_nothing_else_and_keeps_the_config() {
  sb_new
  # something of the distribution's and of the user's that must survive: the icon cache neighbours, other units, other users' files
  write_file "$SB/etc/systemd/system/other.service" '[Unit]\n'; write_file "$SB/usr/share/applications/other.desktop" '[Desktop Entry]\n'
  write_file "$SB/etc/modprobe.d/other.conf" '# mine\n'; write_file "$SB/usr/src/other-1.0/Makefile" 'all:\n'
  sb_baseline
  sb_install; assert_code 0
  sb_uninstall; assert_code 0
  # only the system config the installer created is left
  assert_eq "$(sb_tree | grep -v ' etc/truesense/config.toml ' | grep -v '^\./etc/truesense/config.toml')" "$SB_BASE" "the tree after install and uninstall"
  assert_file "$SB/etc/truesense/config.toml" "the config is kept without --purge"
  # a second uninstall changes nothing and does not fail
  local after; after=$(sb_tree)
  sb_uninstall; assert_code 0
  assert_eq "$(sb_tree)" "$after" "the tree after the second uninstall"
}

t_uninstall_purge_also_removes_config_state_and_the_users_settings() {
  sb_new; sb_baseline
  sb_install; assert_code 0
  write_file "$SB/var/lib/truesense/state.json" '{}\n'; write_file "$SB/home/$SB_USER/.config/truesense/config.toml" '# user\n'
  sb_uninstall --purge; assert_code 0
  assert_eq "$(sb_tree)" "$SB_BASE" "the tree after install and uninstall --purge"
  sb_uninstall --purge; assert_code 0
}

t_uninstall_from_a_go_layout_removes_it_all_including_the_old_module() {
  sb_new; sb_baseline; sb_layout_go
  local before; before=$(sb_tree)
  sb_uninstall --purge; assert_code 0
  assert_eq "$(dkms_versions)" "" "registered DKMS versions"
  assert_no_file "$SB/usr/src/truesense-wmi-0.4.0"; assert_no_file "$SB/usr/local/bin/truesense"; assert_no_file "$SB/etc/systemd/system/truesense.service"
  [ "$(sb_tree | wc -l)" -lt "$(printf '%s\n' "$before" | wc -l)" ] || fail "nothing was removed"
}

t_a_service_that_cannot_be_stopped_stops_the_uninstall_before_any_file_is_removed() {
  sb_new; sb_install; assert_code 0
  printf '%s\n' '*disable --now truesense.service*' '*systemctl stop truesense.service*' > "$SB/sim/fail"; : > "$SB/calls.log"
  sb_uninstall; assert_code 1
  assert_contains "$R_ERR$R_OUT" "could not be stopped: nothing was removed"
  # the fans were handed back and the health timer disabled (both harmless); no file, unit, helper or module was removed
  assert_called "@write /dev/truesense set_fan_auto"
  local f; for f in usr/local/bin/truesense usr/local/lib/truesense/health.sh etc/systemd/system/truesense.service etc/systemd/system/truesense-health.timer \
    etc/modprobe.d/truesense.conf usr/src/truesense-wmi-$NEWVER/dkms.conf usr/share/applications/truesense.desktop etc/systemd/user/truesense-keys.service; do assert_file "$SB/$f"; done
  assert_not_called "dkms remove"; assert_not_called "modprobe -r"
}

# ---- the kernel module script ----------------------------------------------------------------------------------------------------------------

t_the_module_script_replaces_every_registered_version_and_builds_for_every_kernel() {
  sb_new; sb_layout_go
  mkdir -p "$SB/lib/modules/6.98.0-other/build"; sb_old_kernel 0.3.17
  printf 'truesense-wmi/0.3.17, %s, x86_64: installed\ntruesense-wmi/0.4.0, %s, x86_64: installed\n' "$SB_KERNEL" "$SB_KERNEL" > "$SB/sim/dkms_status"
  mkdir -p "$SB/usr/src/truesense-wmi-0.4.0"
  sb_run install_kernel_module.sh -- ; assert_code 0
  assert_eq "$(dkms_versions)" "$NEWVER " "registered DKMS versions"
  assert_order "dkms remove truesense-wmi/0.3.17 --all" "dkms remove truesense-wmi/0.4.0 --all" "dkms add truesense-wmi/$NEWVER" "dkms install truesense-wmi/$NEWVER -k 6.98.0-other --force" "dkms install truesense-wmi/$NEWVER -k $SB_KERNEL --force" "depmod -a" "modprobe -r truesense_wmi" "modprobe truesense_wmi"
  assert_no_file "$SB/usr/src/truesense-wmi-0.4.0"; assert_no_file "$SB/usr/src/truesense-wmi-0.3.17"; assert_file "$SB/usr/src/truesense-wmi-$NEWVER/truesense_wmi.c"
  assert_file "$SB/dev/truesense" "the device appeared"
}

t_the_module_script_refuses_when_the_running_kernel_cannot_be_built_for() {
  sb_new; sb_old_kernel 0.4.0
  printf '%s\n' "*dkms install*-k $SB_KERNEL*" > "$SB/sim/fail"
  sb_run install_kernel_module.sh -- ; assert_code 1
  assert_contains "$R_ERR$R_OUT" "could not be built for the running kernel"; assert_contains "$R_ERR$R_OUT" "firmware's control"
  assert_not_called "modprobe truesense_wmi"
  # a build that fails for ANOTHER installed kernel only is not fatal
  sb_new; mkdir -p "$SB/lib/modules/6.98.0-other/build"
  printf '%s\n' "*dkms install*-k 6.98.0-other*" > "$SB/sim/fail"
  sb_run install_kernel_module.sh -- ; assert_code 0
  assert_contains "$R_OUT" "FAILED for 6.98.0-other"
}

t_the_module_script_stops_before_changing_anything_without_a_version_or_without_dkms() {
  sb_new; sb_old_kernel 0.4.0; sed -i '/PACKAGE_VERSION/d' "$SRC/kernel/truesense-wmi/dkms.conf"
  local before; before=$(sb_tree)
  sb_run install_kernel_module.sh -- ; assert_code 1; assert_contains "$R_ERR$R_OUT" "no PACKAGE_VERSION"
  assert_eq "$(sb_tree)" "$before" "the tree"; assert_not_called "dkms remove"
  sb_new; printf 'dkms\n' > "$SB/sim/missing"
  sb_run install_kernel_module.sh -- ; assert_code 1; assert_contains "$R_ERR$R_OUT" "dkms is not installed"
}

# ---- the legacy cleanup ----------------------------------------------------------------------------------------------------------------------

legacy_layout() {
  write_file "$SB/etc/systemd/system/predator.service" '[Service]\nExecStart=/usr/local/lib/predator/daemon\n'
  write_file "$SB/usr/local/lib/predator/daemon" '#!/bin/sh\n' 755
  write_file "$SB/etc/rpredatorsense/config.toml" 'user = "tester"\n'
  write_file "$SB/etc/modules-load.d/linuwu_sense.conf" 'linuwu_sense\n'
  mkdir -p "$SB/usr/src/linuwu_sense-1.0"; write_file "$SB/usr/src/linuwu_sense-1.0/Makefile" 'all:\n'
  printf 'linuwu_sense/1.0, %s, x86_64: installed\n' "$SB_KERNEL" > "$SB/sim/dkms_status"
  write_file "$SB/home/$SB_USER/.cache/predator-fan-mode" 'manual\n'
}

t_the_legacy_cleanup_backs_everything_up_before_it_removes_anything() {
  sb_new; legacy_layout
  sb_run migrate-legacy.sh -- "--user=$SB_USER"; assert_code 0
  local backup; backup=$(ls "$SB"/var/backups/truesense/legacy-2*.tar.gz 2>/dev/null | head -n1); [ -n "$backup" ] || { fail "no backup was written: $R_OUT"; return; }
  local listing; listing=$(tar -tzf "$backup")
  assert_contains "$listing" "etc/systemd/system/predator.service"; assert_contains "$listing" "usr/local/lib/predator/daemon"; assert_contains "$listing" "usr/src/linuwu_sense-1.0/Makefile"
  assert_no_file "$SB/etc/systemd/system/predator.service"; assert_no_file "$SB/usr/local/lib/predator"; assert_no_file "$SB/usr/src/linuwu_sense-1.0"
  assert_no_file "$SB/home/$SB_USER/.cache/predator-fan-mode"
  assert_contains "$(cat "$SB/var/lib/truesense/migrated_user")" "tester" "the old login is carried over"
  assert_order "systemctl disable --now predator.service" "dkms remove linuwu_sense/1.0 --all" "systemctl daemon-reload"
  assert_contains "$R_OUT" "restore with"
  # idempotent: a second run finds nothing
  sb_run migrate-legacy.sh -- "--user=$SB_USER"; assert_code 0; assert_contains "$R_OUT" "nothing to back up"
}

t_a_backup_that_cannot_be_written_stops_the_cleanup_before_anything_is_removed() {
  sb_new; legacy_layout
  printf '%s\n' "*tar -czf*" > "$SB/sim/fail"
  local before; before=$(sb_tree)
  sb_run migrate-legacy.sh -- "--user=$SB_USER"; assert_code 1
  assert_contains "$R_ERR$R_OUT" "nothing was removed"
  assert_eq "$(sb_tree)" "$before" "the tree"
  assert_not_called "dkms remove"; assert_not_called "systemctl disable"
}

run_tests
