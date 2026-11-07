#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Tests of packaging/health.sh (the timer's check): a stalled daemon is restarted, a healthy one and one that has just woken from suspend are not, a missing module is
# rebuilt at most once per ten minutes. Every program it calls is a fake that logs its arguments; nothing real is touched.
# shellcheck source=tools/shelltest/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
HEALTH=$ROOT/packaging/health.sh

# health_world: a fake machine in $TMP: fake systemctl (active unless $TMP/inactive exists), modprobe, dkms, logger, date unchanged; prints the PATH to use
health_world() {
  local bin="$TMP/bin"; mkdir -p "$bin" "$TMP/run"; : > "$TMP/calls"
  fake_program "$bin" systemctl 'echo "systemctl $*" >> "$TMP/calls"; if [ "$1" = is-active ]; then [ ! -e "$TMP/inactive" ]; fi'
  fake_program "$bin" modprobe 'echo "modprobe $*" >> "$TMP/calls"; [ ! -e "$TMP/loads" ] || : > "$TMP/dev"'
  fake_program "$bin" dkms 'echo "dkms $*" >> "$TMP/calls"; [ ! -e "$TMP/dkms_fixes" ] || : > "$TMP/dev"'
  fake_program "$bin" logger 'echo "logger $*" >> "$TMP/calls"'
  fake_program "$bin" sleep 'echo "sleep $*" >> "$TMP/calls"; [ -z "$TS_REFRESH_STATE" ] || : > "$TMP/run/state"'
  [ -e "$TMP/nodev" ] || : > "$TMP/dev"
  printf '%s:/usr/bin:/bin\n' "$bin"
}

# health [VAR=value ...]: runs health.sh in the fake world
health() {
  local path; path=$(health_world)
  run "PATH=$path" "TRUESENSE_DEVICE=$TMP/dev" "TRUESENSE_RUN=$TMP/run" "TRUESENSE_HEALTH_STAMP=$TMP/stamp" TMP="$TMP" "$@" -- "$BASH_BIN" "$HEALTH"
}
calls() { cat "$TMP/calls"; }
age_state() { : > "$TMP/run/state"; touch -d "$1 seconds ago" "$TMP/run/state"; }

t_a_healthy_daemon_is_left_alone() {
  health_world >/dev/null; : > "$TMP/run/state"
  health; assert_code 0
  assert_not_contains "$(calls)" "systemctl restart" "a fresh state file"
}

t_a_dead_daemon_is_restarted() {
  health_world >/dev/null; : > "$TMP/inactive"; : > "$TMP/run/state"
  health; assert_code 0
  assert_contains "$(calls)" "systemctl restart truesense.service"
}

t_a_stalled_daemon_is_restarted_after_one_more_look() {
  health_world >/dev/null; age_state 300
  health; assert_code 0
  assert_contains "$(calls)" "sleep 3" "it looks again"
  assert_contains "$(calls)" "systemctl restart truesense.service"
  assert_contains "$(calls)" "state file stale"
}

# after a resume the file is as old as the sleep, and the daemon's next tick makes it fresh during the pause
t_a_daemon_that_has_just_woken_from_suspend_is_not_restarted() {
  health_world >/dev/null; age_state 7200
  health TS_REFRESH_STATE=1; assert_code 0
  assert_contains "$(calls)" "sleep 3"
  assert_not_contains "$(calls)" "systemctl restart" "the daemon ticked while we waited"
}

t_a_missing_state_file_is_stale_too_but_gets_the_same_second_look() {
  health_world >/dev/null; rm -f "$TMP/run/state"
  health TS_REFRESH_STATE=1; assert_not_contains "$(calls)" "systemctl restart"
  health_world >/dev/null; rm -f "$TMP/run/state"
  health; assert_contains "$(calls)" "systemctl restart truesense.service"
}

t_a_missing_module_is_rebuilt_once_per_ten_minutes_and_the_daemon_restarted() {
  health_world >/dev/null; rm -f "$TMP/dev"; : > "$TMP/nodev"; : > "$TMP/dkms_fixes"; : > "$TMP/run/state"
  health; assert_code 0
  assert_contains "$(calls)" "dkms autoinstall -k"
  assert_contains "$(calls)" "systemctl restart truesense.service" "the daemon is restarted once the device exists"
  # a second run within ten minutes does not run dkms again
  health_world >/dev/null; rm -f "$TMP/dev" "$TMP/dkms_fixes"; : > "$TMP/run/state"
  health; assert_not_contains "$(calls)" "dkms autoinstall" "stamp is fresh"
  # after eleven minutes it may try again
  touch -d "11 minutes ago" "$TMP/stamp"
  health; assert_contains "$(calls)" "dkms autoinstall"
}

t_a_plain_modprobe_that_finds_the_module_is_enough() {
  health_world >/dev/null; rm -f "$TMP/dev"; : > "$TMP/nodev"; : > "$TMP/loads"; : > "$TMP/run/state"
  health; assert_not_contains "$(calls)" "dkms" "no rebuild when modprobe works"
}

run_tests
