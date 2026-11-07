# Resilience

The Rust edition has to work after every update and every restart, across crashes, upgrades from the Python (0.3.x) and Go (0.4.0) editions, suspend and resume, damaged or missing files, and it must never leave the laptop in a dangerous state. "Dangerous" has one concrete meaning here: **a fan stopped or held low while the machine is hot, or a CPU clock cap or power limit that nobody will lift.** Everything below is ordered around that.

This page is the ledger: every failure mode that was considered, the evidence for what happens (test names, runnable with `cargo test` and `tools/shelltest/run.sh`), and what can only be shown on the real laptop (last section, with exact steps).

## The safe state and who guarantees it

| Layer | What it does | Independent of |
|---|---|---|
| Firmware | with `fan auto` it runs the fans by itself; it is the state every failure path ends in | all of our software |
| `ExecStopPost=-truesense fan auto` | after the daemon ends in ANY way (stop, crash, SIGKILL, watchdog, exit 78) writes `fan auto` to the firmware | the daemon (a new process) |
| `ExecStopPost=sh -c 'cat cpuinfo_max_freq > scaling_max_freq'` | lifts a CPU clock cap | the truesense program (plain shell) |
| the daemon's `shutdown()` | fans to Auto first and without the state lock, cap lifted, `STOPPING=1`, on stop, error and panic outside a tick | ExecStopPost |
| `guarded_step` | a panic in one tick is caught; hot or unknown temperature means fans at 100 % | the rest of the loop |
| `WatchdogSec=20` | a daemon that stops ticking is killed by systemd (then ExecStopPost) | the daemon |
| `TimeoutStopSec=10` | a daemon that ignores SIGTERM is killed after 10 s, not 90 | the daemon |
| kernel module | hands the fans back when it unloads | everything in user space |

## Bugs this work found and fixed

| # | Bug | Effect | Fix | Evidence |
|---|---|---|---|---|
| B1 | `truesense fan auto` asked the daemon whenever its socket FILE existed. A crashed daemon leaves that file behind, so the request failed with "connection refused". | The unit's `ExecStopPost=truesense fan auto` did nothing after exactly the crashes it exists for: a stopped fan stayed stopped until the restart. (Shown with the real binary: exit 1 before, 0 after.) | `Api::set_fans`: Auto, Max and a plain duty fall back to the firmware when the daemon is not answering; Smart and a stop still need the service. | `ts-api` `fan_auto_after_a_crash_reaches_the_firmware_although_the_socket_file_is_still_there`, `max_and_a_plain_duty_also_fall_back_but_smart_and_a_stop_need_a_live_service` |
| B2 | No single-instance protection; the socket server replaces any existing socket file. | A second daemon (a terminal `sudo truesense daemon`, the old Go daemon during an upgrade) deleted the first one's socket and both wrote the fans every second. | `flock` beside the socket plus a connect probe (catches other editions' daemons); the second one exits 78 before touching anything. | `ts-daemon` `a_second_daemon_refuses_and_changes_nothing`, `the_lock_names_the_holder_and_goes_with_the_process`, `a_daemon_of_another_edition_that_answers_on_the_socket_is_respected` |
| B3 | A configuration with any problem fell back to `Config::default()`, whose `user` is empty. | The desktop user could no longer talk to the daemon (the socket allows root and the configured user only) after a typo in a config file. | `Config::load_or_defaults` keeps `user`, read leniently from the files. | `ts-foundation` `config_resilience.rs` (6 tests) |
| B4 | A failed settings write (full or read-only disk) was never retried; a corrupt `state.json` was silently overwritten by the next change. | Settings made while the disk was full were lost at the next reboot; the evidence of a corruption was destroyed. | `Store` keeps a dirty flag, the loop calls `flush()` every 30 ticks; a damaged file is moved to `state.json.corrupt`; stale `.tmp` files of a killed writer are swept at start. | `ts-foundation` `state_resilience.rs`, `ts-daemon` `a_full_disk_does_not_stop_the_fans_and_the_settings_are_written_once_it_has_room`, `a_corrupt_settings_file_starts_on_the_defaults_says_so_and_is_kept`, `a_crashed_write_leaves_a_temp_file_and_the_next_start_sweeps_it` |
| B5 | A configuration or settings problem was only a journal line. | Nobody saw that defaults were in force. | Appended `warning=` key in the live state and `/run/truesense/state`. | `problems_found_in_the_configuration_are_visible_in_the_live_state_and_the_log` |
| B6 | Wall-clock decisions had no sanity bounds. | A clock set back (dead RTC battery, NTP) pinned a held power mode, a "charge to 100 % once" or a stale GPU reading for as long as the jump; a client could hold a mode "for 10^9 minutes". | Hold capped at 7 days and dropped when the clock went back; a charge-to-full stamped in the future ends (80 % limit returns); a GPU reading from the future is not fresh. | `a_clock_set_back_does_not_pin_a_held_power_mode_for_weeks`, `a_client_cannot_hold_a_power_mode_for_years`, `a_charge_to_full_stamped_in_the_future_ends_instead_of_lasting`, `a_gpu_reading_stamped_in_the_future_is_not_fresh` |
| B7 | `Updater::apply` with no tag installed the newest release even when it was not newer. | A programmatic caller could reinstall or downgrade without being asked (the CLI and window were guarded by `plan_update`). | `UpdateError::AlreadyNewest`; an explicit tag is still honoured. | `ts-system` `a_release_that_is_not_newer_is_not_installed_unless_a_tag_is_asked_for` |
| B8 | `health.sh` judged the state file stale by its age. After a resume the file is as old as the sleep, so a check landing in the first seconds after waking restarted a healthy daemon. | A needless restart (fans to Auto, smart curve restarts) after some resumes. | It looks a second time after 3 s (the next tick refreshes the file). | `tools/shelltest/test_health.sh` |
| B9 | `truesense-keys.service` and `-audio.service` had the default start limit (5 in 10 s). | A crash loop left them failed until the next login. | `StartLimitIntervalSec=0`. | `tools/shelltest/test_units.sh` |
| B10 | A daemon that found another one running would be restarted every second by `Restart=always`, running the fan hand-back each time. | A fight with the other daemon. | Exit status 78 and `RestartPreventExitStatus=78`. | `test_units.sh` (unit and `EXIT_ALREADY_RUNNING` agree) |

The installer, uninstaller and kernel-module script findings are in the installer section below.

## Failure modes

Legend: **T** = automated test (name or file), **L** = needs the real laptop (see "Live checks").

### Daemon: process life

| ID | Failure mode | What happens | Evidence |
|---|---|---|---|
| D1 | A second daemon starts | refuses, exit 78, changes nothing, the first keeps its socket | T `a_second_daemon_refuses_and_changes_nothing`; L2 |
| D2 | Stale socket and lock file after a crash | the restart starts at once (the kernel released the lock; the socket is replaced) | T `a_stale_socket_and_a_stale_lock_file_from_a_crash_do_not_block_the_restart`, `after_a_real_sigkill_the_next_daemon_starts_at_once_over_the_stale_socket_and_lock` (real SIGKILL of a child daemon) |
| D3 | SIGTERM (stop, update, reboot) | the signal thread asks the loop to stop; fans to Auto, cap lifted, socket removed, exit 0 | T `a_real_sigterm_makes_the_daemon_hand_everything_back_and_exit_cleanly` (child process, real `kill -TERM`) |
| D4 | SIGTERM during a fan-off episode | fans are back on Auto before the daemon is gone | T `a_stop_during_a_fan_off_episode_returns_the_fans_to_auto_before_the_daemon_ends` |
| D5 | SIGKILL, abort, power cut of the process | nothing of ours runs; `ExecStopPost` hands the fans back (B1 fixed) | T B1 tests; L1 |
| D6 | A panic in any step of a tick | caught; one tick lost; hot or unknown temperature puts the fans on 100 %; the lock is never left held | T `a_panic_at_every_firmware_command_of_the_loop_costs_one_tick_and_ends_with_the_fans_on_auto` (panic injected at every command of a 6-tick run), `a_panic_at_every_sensor_read_costs_one_tick_and_a_hot_machine_gets_full_fans`, `a_hot_machine_whose_tick_panics_gets_the_fans_at_full_at_that_moment`, `a_panic_outside_the_guarded_tick_still_hands_the_machine_back` |
| D7 | A panic inside the hand-back itself | the process dies; `ExecStopPost` repeats the hand-back (the test calls `shutdown` again) | T same D6 test (the `outcome.is_err()` branch) |
| D8 | The loop hangs (blocked ioctl, deadlock) | no watchdog ping; systemd kills it after 20 s, then ExecStopPost; stop hangs end after 10 s | T the_watchdog_is_pinged_only_as_often_as_systemd_wants; L3 |
| D9 | Restart storm: a daemon that crashes at once | see "Restart policy"; fans are on Auto between attempts | T `test_units.sh`; L1 |
| D10 | The device vanishes or answers garbage | the loop keeps waiting and retrying; a stopped fan is returned to Auto | T existing `a_stopped_fan_stays_watched_until_the_firmware_really_has_it_back`, `device_unavailable` tests |

### Daemon: suspend, resume, clocks

| ID | Failure mode | What happens | Evidence |
|---|---|---|---|
| R1 | Suspend/resume between two ticks | the wall clock ran and the monotonic one did not: fans and mode written again, lighting restored, no catch-up burst | T `after_a_suspend_that_changed_the_firmware_state_the_fans_are_written_again`, `a_loop_that_wakes_late_resyncs_instead_of_racing_to_catch_up`, existing `after_a_resume_the_fans_are_written_again_at_once` |
| R2 | Suspend in the middle of a tick, at any firmware command | the fans still follow the heat afterwards | T `a_suspend_in_the_middle_of_any_command_of_a_tick_still_ends_with_the_fans_following_the_heat` (every command of a tick) |
| R3 | `health.sh` runs right after the resume | does not restart the daemon (second look) | T `test_health.sh` |
| K1 | Clock jumps forward (NTP, long suspend) | looks like a resume: one rewrite; a hold and a charge-to-full end early (the safe direction) | T `a_clock_that_jumps_forward_ends_a_hold_and_a_charge_to_full_and_nothing_else` |
| K2 | Clock jumps back | hold dropped, charge-to-full ends, a future GPU reading is ignored (B6) | T the B6 tests |
| K3 | Wall clock wrong at boot (1970) | a stored hold in the future is bounded by 7 days; the crash guard uses the monotonic clock | T `a_clock_set_back_does_not_pin_a_held_power_mode_for_weeks` |

### Settings and configuration

| ID | Failure mode | What happens | Evidence |
|---|---|---|---|
| S1 | `state.json` truncated at ANY byte | opens as defaults (never a half-read file), kept as `.corrupt`, reported | T `a_file_cut_at_every_byte_offset_opens_as_defaults_or_whole` |
| S2 | A bit flipped anywhere | never panics | T `a_flipped_byte_at_every_offset_never_panics` |
| S3 | Writer killed at any point of its protocol | the target is the old file, complete (atomic write: temp file, fsync, rename) | T `a_writer_that_fails_at_every_point_leaves_the_old_file_whole`, and with the real thing `killing_a_writer_with_sigkill_never_leaves_a_damaged_file` (40 SIGKILLs of a process rewriting the file in a loop; the generation number never goes back) |
| S4 | Full or read-only `/var/lib` | logged once a minute, fan control unaffected, retried every 30 ticks, written when it works | T `a_disk_that_fails_for_a_while_catches_up_by_itself`, `a_read_only_directory_is_an_error_and_not_a_panic`, `a_full_disk_does_not_stop_the_fans_...` |
| S5 | Keys an older or newer edition wrote | kept on every rewrite | T `keys_the_store_does_not_know_survive_rewrites`, editions F2 |
| S6 | Hostile settings file (huge, deep, binary, a directory) | "no settings"; a directory is never moved | T `hostile_settings_files_are_no_settings` |
| C1 | Invalid configuration at boot | built-in defaults, `user` kept, fans safe, warning in log and live state | T `config_resilience.rs`, `problems_found_in_the_configuration_...` |
| C2 | Config does not parse at all | same; `user` found by a lenient line scan (top-level, before the first section, safe characters only) | T `a_file_that_does_not_parse_keeps_the_user_read_leniently` |
| C3 | Wrong type in the user's file | blamed on that file in the warning | T `a_wrong_type_in_the_users_file_is_blamed_on_that_file` |
| C4 | No files at all (fresh install) | defaults, no warning | T `a_missing_file_is_not_a_problem`, `nothing_there_is_a_normal_start_...` |

### Formats: upgrading from the Python and Go editions

Fixtures: `crates/ts-daemon/testdata/editions/go-0.4.0` (shapes taken from a laptop that ran the Go edition; names and device identifiers stripped, no serial number) plus Python-shaped files written inline from the Python source. Tests: `crates/ts-daemon/tests/editions.rs`.

| ID | File | Evidence |
|---|---|---|
| F1 | `/etc/truesense/config.toml`, `~/.config/truesense/config.toml` (unknown sections and keys listed by `unknown_keys`, kept by `write_user`) | `the_go_editions_config_files_load_validate_and_name_the_keys_...`, `rewriting_the_users_config_keeps_every_key_an_older_edition_wrote` |
| F2 | `/var/lib/truesense/state.json` (every key survives a rewrite, including one only Go knew; Python files get this edition's defaults for what they lack) | `the_go_editions_state_json_starts_...`, `the_python_editions_state_json_loads_...` |
| F3 | `~/.local/state/truesense/user.json` (and a corrupt one) | `the_go_editions_user_json_is_read_...`, `a_user_json_nobody_can_parse_...` |
| F4 | `smart/*.json` | `the_smart_profiles_the_go_edition_saved_load_and_list` |
| F5 | `curves/*.json` (Python and Go shapes; a curve that no longer validates falls back by name) | `the_fan_curves_of_both_older_editions_load_and_names_list` |
| F6 | `keys/`, `undervolt/` (including the old name `stock`), `gpu-undervolt/` | `key_cpu_and_gpu_undervolt_profiles_of_the_older_editions_load` |
| F7 | `looks/*.json`, lighting profiles (`{"cells": ...}`), `boot-look.json` | `looks_lighting_profiles_and_the_boot_look_of_the_older_editions_parse` |
| F8 | `uv-pending` flag | `an_undervolt_flag_the_go_edition_left_trips_the_guard_once_and_is_gone` |
| F9 | live `/run/truesense/state` | `the_live_state_the_go_daemon_published_parses_as_it_always_did` |
| F10 | update cache `~/.cache/truesense/update.json` (Go shape; no update offered, no request when fresh) | `ts-system` `the_update_cache_the_go_edition_left_is_understood_and_gives_no_update` |
| F11 | rollback: what Rust writes is read by the older editions (same key sets and order in profile files, plain TOML/JSON objects elsewhere) | `what_this_edition_writes_keeps_the_key_sets_and_shapes_the_older_editions_read` |
| F12 | garbage in every profile directory | `nothing_there_is_a_normal_start_and_garbage_everywhere_is_a_start_on_defaults` |

Not added: a `schema` or `version` key. Every file above is read by the older editions too, and a new key in `state.json` or `user.json` would be preserved by them but a new key in a profile file is dropped by their validators (Python and Go rebuild a profile from the keys they know), so a marker would be lost on rollback and gain nothing. Compatibility is carried by tolerant readers instead.

Known, deliberate: a profile saved by Rust loses keys it does not know (Python's `validate` does the same). Pre-0.3 state files that have only `auto_profile` (no `auto_mode`) get `auto_mode` from the config default, as in Python 0.3.

### Updater

Tests: `crates/ts-system/src/update/tests.rs` (fake HTTP fetcher; bundles built in memory with `testkit::archive` as real tar.gz, `SHA256SUMS` computed from them).

| ID | Failure mode | Evidence |
|---|---|---|
| U1 | check, cache, offline, wrong GitHub answers | existing `a_check_finds_a_newer_release_...` and the group around it |
| U2 | checksum mismatch or missing list | `a_checksum_mismatch_refuses_the_download`, `a_release_without_a_checksum_list_is_always_refused...` |
| U3 | download truncated at any point | `a_download_cut_short_anywhere_never_reaches_extraction_and_leaves_nothing` |
| U4 | archive truncated but published with its own checksum: extraction interrupted | `an_archive_that_is_truncated_but_published_with_its_own_checksum_fails_extraction_and_leaves_nothing` |
| U5 | tar extractor strict (symlinks, path escapes, special files) | existing `a_symlink_a_path_escape_or_a_special_file_in_the_archive_is_refused` and the extractor tests |
| U6 | hand-off to the installer with the real `Layout::rust_edition` and a real script run from the verified tree, exit code reported, tree deleted | `the_real_installer_of_a_hand_built_bundle_is_run_from_the_verified_tree` |
| U7 | same version or older: nothing downloaded, nothing run (B7); an explicit tag is honoured | `a_release_that_is_not_newer_is_not_installed_unless_a_tag_is_asked_for`; CLI: `plan_update` (`needed`) |
| U8 | a release of another edition (Go) | `a_release_of_another_edition_is_refused_by_the_layout_check_not_installed` |
| U9 | the user update timer: `service repair` enables only what is not running, second run changes nothing | `ts-system` services `the_second_run_changes_nothing`, `stopped_services_are_enabled_and_started_and_running_ones_left_alone` |

The Go edition's own `update apply` (running on a laptop that has not yet moved to Rust) checks for ITS layout and will not understand a Rust release; the first move from Go to Rust is `tools/get.sh` or `tools/install.sh` by hand (see "Live checks", L8).

## Restart policy

`truesense.service` has `Restart=always`, `RestartSec=1`, `StartLimitIntervalSec=0`, `WatchdogSec=20`. A daemon that crashes immediately therefore starts once a second for ever. Alternatives considered:

* **`StartLimitBurst` with a give-up.** After the limit the unit stays failed. The fans are on the firmware's Auto (ExecStopPost), which is safe, but TrueSense's own features stop until someone restarts it; the health timer would restart it only after `reset-failed`, which it does not call. A transient condition (the module not yet loaded after a kernel update, a full `/run`) could turn into a permanent loss of the smart fan for the rest of the boot. Worse than a storm.
* **`RestartSteps=` / `RestartMaxDelaySec=` backoff (systemd 254).** Better in principle (1 s growing to 30 s), but how the step counter is reset after a long healthy run differs between systemd versions, and an old version ignores the keys: a daemon that crashed once a week could end up with a permanent 30 s delay. Not applied without a live test.
* **Keep as is.** Each failed start runs the failsafe (fans to Auto, cap lifted), so between attempts the machine is in the firmware's hands; the price is one journal line per second and a little CPU. Chosen.

What changed: a daemon that finds another one running exits 78 and is **not** restarted (`RestartPreventExitStatus=78`): retrying cannot help and each attempt would run the hand-back against the other daemon. `TimeoutStopSec=10` bounds a stop. The watchdog is untouched.

## Units review

| Unit | Policy | Notes |
|---|---|---|
| `truesense.service` | notify, watchdog 20 s, always restart 1 s, no start limit, stop timeout 10 s, exit 78 not restarted, ExecStopPost fan auto + cpu uncap | After `systemd-modules-load`; `ExecStartPre=-modprobe`. No ordering is needed for resume: the loop detects it |
| `truesense-health.timer/.service` | every 30 s awake; the check looks twice before calling the daemon stalled; rebuilds the module at most every 10 min | timer clocks are monotonic: no burst after a suspend |
| `truesense-keys.service` (user) | always restart 2 s, never give up | reconnects when the keyboard vanishes (tests `the_keyboard_going_away_and_coming_back_is_followed`, `a_keyboard_that_never_comes_back_does_not_end_the_service`) |
| `truesense-audio.service` (user) | always restart 3 s, never give up | After pipewire and wireplumber |
| `truesense-startup.service` (user) | oneshot, on-failure 5 s, 5 in 120 s | the retries wait for the keyboard to enumerate |
| `truesense-update.timer` (user) | daily, `Persistent=true`, randomised | a missed run happens at the next login |

Sandboxing: `ProtectSystem=full`, `ProtectHome=read-only`, no realtime, no namespaces; the daemon must write sysfs, so `tools/ci/verify-units.sh` keeps its ratchet (exposure 8.0, limit 8.5). Nothing was loosened or tightened here: more sandboxing needs a live test (see L11).

`truesense service check` is read-only; `service repair` runs `daemon-reload`, rebuilds the module with `dkms autoinstall -k <running kernel>` only when `/dev/truesense` is missing, enables what is not enabled, restarts the daemon only when it does not answer. After an upgrade that changes the module version (0.4.0 to 0.5.0) the registered old DKMS version is removed by `tools/install_kernel_module.sh` (it removes every registered version before adding the new one) and not by `repair`; `repair` and `health.sh` only ever run `dkms autoinstall`, which builds what is registered. The installer tests below prove the removal.

## Installer: install, upgrade, rollback, uninstall

Every script (`install.sh`, `uninstall.sh`, `install_kernel_module.sh`, `migrate-legacy.sh`) has a sandbox mode: `TRUESENSE_ROOT=DIR`, refused unless DIR is an absolute existing directory holding a file `.truesense-sandbox` (so it cannot be set by accident, and never `/`). All system paths live under DIR, no system program runs (`systemctl`, `dkms`, `modprobe`, ... are recorded in `DIR/calls.log` and answered from files in `DIR/sim/`), no root is needed, and a loud banner is printed. The real mode runs the same code; only the leaf calls differ. The tests (`tools/shelltest/sandbox.sh`) additionally put a tripwire on `PATH` for every dangerous program, so a call that bypassed the recorders fails the test instead of acting on the machine.

| What | Evidence (`tools/shelltest/test_sandbox_install.sh` unless noted) |
|---|---|
| Fresh install: every file, mode, placeholder, completion, DKMS source, enable links | `fresh_install_puts_every_file_in_place_with_the_right_mode` |
| Upgrade from the Go edition: new program, old DKMS version removed then new added and built, config/state/user settings byte-identical, rollback copy present at the first start and gone afterwards | `upgrade_from_the_go_edition_replaces_the_program_and_the_module_and_keeps_every_setting` |
| Upgrade from the Python tree: the python tree and `lib/truesense.prev` are removed only after the service started | `upgrade_from_the_python_edition_removes_the_python_tree_only_after_the_service_started`, and `test_installer.sh` `the_old_python_tree_is_removed_only_after_the_new_service_started` (the structural order test, updated for the new variable names, invariant unchanged) |
| Reinstall is idempotent (byte-identical tree, no `.new`/`.prev`) | `reinstalling_twice_gives_a_byte_identical_tree_and_no_stray_files`, `a_config_that_exists_is_never_overwritten_...` |
| The new program fails `--version` or is stale: nothing changes | `a_program_that_cannot_report_its_version_stops_the_install_before_anything_changes`, `a_stale_program_is_refused_before_anything_changes` |
| The service fails after the swap: previous program and unit restored, restarted, fans handed to the firmware if even that fails; also for Python (wrapper and tree) and for a fresh install ("no previous version": stopped, fans back) | `the_service_failing_after_the_swap_restores_the_go_program_and_its_unit`, `..._rolls_back_to_the_python_wrapper_and_tree`, `a_rollback_whose_previous_version_does_not_start_either_hands_the_fans_back`, `a_fresh_install_whose_service_does_not_start_...`, `rollback_without_a_previous_program_...` |
| No moment where the unit's `ExecStopPost` program is missing (the program is swapped by ONE rename, the previous copy is itself written beside and renamed) | `no_moment_exists_where_the_units_stop_program_is_missing` |
| An interrupted earlier run does not cost the older program | `an_interrupted_earlier_run_does_not_cost_the_older_program` |
| Concurrent installs are refused (lock) | `a_second_install_at_the_same_time_is_refused_and_changes_nothing` |
| Wrong model refused; `--force` remembered only when given; `--no-services`; `--module-only`; custom prefix; hardened umask | the tests of the same names |
| Uninstall: fans to the firmware and the service stopped before anything is deleted; removes every installed file and nothing else; keeps the config unless `--purge`; repeatable; refuses to delete anything while the daemon cannot be stopped | `test_sandbox_uninstall.sh` (`uninstall_*`, `a_service_that_cannot_be_stopped_...`) |
| Module script: removes every registered DKMS version, builds for every kernel, refuses when the running kernel cannot be built for (a failure on another kernel is not fatal), stops before changing anything without a version or without dkms | `test_sandbox_uninstall.sh` (`the_module_script_*`) |
| Legacy cleanup: backs everything up and reads the backup back before removing anything; no backup, no removal; idempotent | `test_sandbox_uninstall.sh` (`the_legacy_cleanup_*`, `a_backup_that_cannot_be_written_*`) |

## Live checks (what only the laptop can show)

Do these once on the real laptop, in this order. Have a second terminal open with `journalctl -u truesense -f`, and `watch -n1 truesense fan status` in a third. Nothing here is dangerous if the CPU is cool; stop and run `truesense fan auto` if it is not. The user's own `predator.service` must be stopped first (it confounds fan tests).

* **L1, SIGKILL during a fan-off episode.** `truesense fan off --confirm` (CPU under 60 C). Then `sudo systemctl kill -s KILL truesense.service`. Observe: within about 2 s the journal shows the unit restarting, and `truesense fan status` says `auto` (the firmware may keep a minimum rpm). Before the fix this stayed `manual 0`. Also `systemctl show truesense -p ExecStopPost` lists both commands.
* **L2, second daemon.** With the service running: `sudo truesense daemon`. Observe: `another TrueSense daemon is already running (it holds /run/truesense/truesense.lock)`, exit status 78 (`echo $?`), the service and its socket unaffected, fans unaffected. Then `sudo systemctl stop truesense`, run the same command again in a terminal and check it serves, stop it with Ctrl-C and check `fan status` is auto and the socket file is gone. Does the user's own `predator.service` coexist? It does not take our lock: the connect probe is what protects (start the service while it runs, expect exit 78).
* **L3, hung daemon.** `sudo kill -STOP $(pgrep -x truesense | head -1)` for the daemon pid (check `systemctl status truesense`). Observe: after about 20 s systemd reports a watchdog timeout, restarts it, fans are Auto in between, no manual action needed.
* **L4, suspend and resume.** Smart fan on, laptop warm: `systemctl suspend`, wake it. Observe: within 3 s fans follow the temperature, the keyboard light is restored, the journal has no `state file stale` line and no restart, `truesense status` shows fresh values.
* **L5, full disk.** `sudo systemctl stop truesense; sudo mount -t tmpfs -o size=4k tmpfs /var/lib/truesense; sudo systemctl start truesense`. Change a setting (for example `truesense fan smart` then `truesense fan auto`). Observe: `could not save the setting` in the journal at most once a minute, fans still controlled; `sudo umount /var/lib/truesense` after `systemctl stop truesense` (the settings of that boot are lost, as designed).
* **L6, bad configuration.** Put `[fan]` / `levels = [10]` in `/etc/truesense/config.toml`, restart. Observe: journal `config problem`, `grep warning /run/truesense/state`, and that `truesense status` as the desktop user still works (the user kept). Remove the lines, restart, the warning is gone.
* **L7, crash loop.** `sudo systemctl edit truesense --runtime` with `[Service]` and `ExecStart=` then `ExecStart=/bin/false`; start it. Observe: one start per second, fans Auto between, the journal readable; revert with `sudo systemctl revert truesense`. (This is the restart-storm cost, measured.)
* **L8, upgrade Go 0.4.0 to Rust.** First `sudo cp -a /var/lib/truesense /var/lib/truesense.bak` and `cp -a ~/.config/truesense ~/.config/truesense.bak`. Run `tools/install.sh --check`, then `pkexec` or `sudo tools/install.sh`. Observe: the service restarts once, `truesense --version` is 0.5.0, `dkms status` lists exactly one `truesense-wmi` version, `/usr/local/bin/truesense.prev` is gone, `diff -r` of the two backups against the live directories shows only files the daemon legitimately rewrote, the smart fan and profile come back as before, `truesense doctor` is clean. Roll back by the same procedure with the Go release if wanted.
* **L9, failed upgrade.** (Only with a spare binary.) Run the installer with `TRUESENSE_BIN` pointing at a build that exits 1 on `--version`: expect "does not run on this machine" and nothing changed. A binary that passes `--version` but whose daemon dies at start: expect "the new version did not start" and the previous binary restored (`truesense --version` back to the old one).
* **L10, module version change.** After L8 on a kernel update (`sudo dkms autoinstall -k <kernel>` for a second installed kernel): `dkms status` shows the new version built for both kernels and no old version.
* **L11, sandboxing.** To tighten the unit further, try `systemd-analyze security truesense.service` suggestions one at a time on the laptop (a run of fan, power, undervolt, GPU, battery commands after each) before changing `packaging/truesense.service`.
* **L12, the real updater.** Once a release with a `SHA256SUMS` is published: `truesense update check`, then `truesense update apply` from a terminal and from the window; confirm the installer runs as root and the service restarts. The Go edition's updater cannot install this release (layout), so the first move is by hand (L8).
