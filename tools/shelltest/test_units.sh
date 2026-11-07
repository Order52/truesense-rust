#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# The restart and shutdown policy of the units (docs/RESILIENCE.md, "Restart policy"): what must hold for the laptop to be safe whatever the daemon does.
# shellcheck source=tools/shelltest/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
P=$ROOT/packaging

t_the_daemon_unit_hands_the_machine_back_however_it_ends() {
  local u; u=$(cat "$P/truesense.service")
  assert_line "$u" "ExecStopPost=-@PREFIX@/bin/truesense fan auto"
  assert_contains "$u" "scaling_max_freq" "the clock cap is lifted by a shell that does not depend on the program"
  assert_contains "$u" "cpuinfo_max_freq"
  assert_line "$u" "Restart=always"; assert_line "$u" "RestartSec=1"
  assert_line "$u" "Type=notify"; assert_line "$u" "WatchdogSec=20"
  assert_line "$u" "TimeoutStopSec=10"
  assert_line "$u" "StartLimitIntervalSec=0"
}

# the daemon exits 78 when another one runs; the unit must not restart on it, and the two must agree on the number
t_the_unit_and_the_program_agree_on_the_already_running_exit_status() {
  local u code; u=$(cat "$P/truesense.service")
  assert_line "$u" "RestartPreventExitStatus=78"
  code=$(sed -nE 's/^pub(\(crate\))? const EXIT_ALREADY_RUNNING: i32 = ([0-9]+);/\2/p' "$ROOT/crates/ts-daemon/src/assembly.rs")
  assert_eq "$code" 78 "EXIT_ALREADY_RUNNING"
}

t_the_user_services_that_must_survive_are_never_given_up_on() {
  local f
  for f in truesense-keys.service truesense-audio.service; do
    assert_line "$(cat "$P/$f")" "StartLimitIntervalSec=0" "$f"; assert_line "$(cat "$P/$f")" "Restart=always" "$f"
  done
}

# no unit may stop a fan or write a setting by itself except through the program's own safe commands
t_no_unit_writes_the_fans_except_through_fan_auto() {
  local f
  for f in "$P"/*.service "$P"/*.timer; do
    assert_not_contains "$(cat "$f")" "fan max" "$(basename "$f")"
    assert_not_contains "$(cat "$f")" "fan off" "$(basename "$f")"
    assert_not_contains "$(cat "$f")" "fan set" "$(basename "$f")"
    assert_not_contains "$(cat "$f")" "set_fan_manual" "$(basename "$f")"
  done
}

t_the_timers_survive_a_suspend_and_a_missed_run() {
  assert_line "$(cat "$P/truesense-update.timer")" "Persistent=true"
  assert_contains "$(cat "$P/truesense-health.timer")" "OnUnitActiveSec=30"
}

run_tests
