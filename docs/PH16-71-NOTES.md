# PH16-71: what another project learned on the same laptop, and what it means for TrueSense

The author of predator-sense (GTK4, GPL-3.0, used with his permission) tested on the same laptop as ours (Predator PH16-71,
BIOS INSYDE V1.18, an i7-13700HX with an RTX 4060, a recent CachyOS kernel) and wrote a long findings file. This page goes through
it section by section and says, for each finding that can touch us, whether TrueSense already handles it, was affected and is fixed
here, does not apply, or needs a test on the real laptop. Only facts are used; the file itself is GPL prose and is not copied.

"Source" is the line range in his `PH16-71-FINDINGS.md`. "Where" names our code. The verdicts:

* **handled**: the behaviour is already right; the evidence is in the code or a test.
* **fixed here**: we were affected; changed on this branch (commit named), with tests.
* **n/a**: concerns his GUI, his helper or his kernel driver; nothing like it exists here.
* **live test**: cannot be settled without the laptop; the exact check is in "Live checks" at the end.

## 1. Findings table

| # | Finding | Source | Verdict | Where, and what |
|---|---|---|---|---|
| 1 | A mode already in force counted as "not compliant", so the policy re-applied it every minute and the mode key blinked | 43-71 | handled, hardened | `ts-decision` `Engine::step`: a mode equal to the target is no write; `Hw::set_mode` settles on the read-back. **Fixed here**: a target the firmware keeps refusing was retried every 30 s forever (each write blinks the key); the wait now doubles to 8 min and resets on a confirmed write (`autoprofile.rs`, tests `a_mode_the_firmware_keeps_refusing...`, `a_confirmed_write_resets...`) |
| 2 | `nvidia-smi -pl` fails on this card and was retried and logged on every mode switch | 75-91 | n/a, but see 38 | No GPU limit is written on a mode change; `set_nvidia_limit` runs only on an explicit request. His real lesson (an `nvidia-smi` call has costs) is row 38 |
| 3 | Keyboard and light bar lost their colour at every reboot | 95-106 | handled | `ts-daemon` `boot.rs` / `step.rs`: the boot look (keyboard and rear) is applied at start and retried for 20 s because the keyboard enumerates late. Live check L9 |
| 4a | The light-bar page never appeared on a USB-keyboard chassis | 110-117 | n/a | His GUI only |
| 4b | WMI frame for the bar: byte 9 is a target (2 = light bar), byte 8 = 3, speed 1-5, brightness 0-100 | 119-155 | handled | `docs/PROTOCOL.md` and `truesense_wmi.c` `c_set_rear` use the same 16-byte layout (`... R G B 3 2 0...`), `hw/rear.rs` builds it. Difference in the mode table, see row 5 |
| 5 | Mode ids: 0 = Static (not Off), 1 Breathing, 2 Neon, 3 Rainbow, 4 Wave, 5 Ripple, 6 Scanner, 7 Strobe, 0xFF solid; everything from 0x08 to 0xFE is a silent no-op | 139-148, 377-387 | differs, no change | We name 3 "wave" and 7 "twinkling" (same codes), accept only 0-3 and 7, and use brightness 0 for off (as he found it must be). Modes 4, 5, 6 and 0xFF are rejected by the module's validation. Not widened without a test (L6). The module is unchanged |
| 6 | After brightness 0, or on a fresh boot, an effect mode is accepted but the bar stays dark until one Breathing frame is sent | 153-155 | live test | `hw/rear.rs` `rear_set` has no wake step and the boot look writes the effect directly. If L6 shows the bar dark, add one breathing frame before an effect when the last known brightness is 0 or unknown |
| 7 | Method 21 reads the bar back (argument 1 keyboard, 2 bar), 16 bytes; frame bytes 3, 8, 10-15 are ignored by the firmware | 263-282 | handled | Module `get_rear` (argument 2) and `get_slot` (1); `rear_get` decodes the same layout; `set_rear` validation is stricter than the firmware needs, which is safe |
| 8 | The bar has no firmware idle timeout, and no per-zone control: method 6/7 stores per-zone colours that light nothing | 283-300, 358-375, 403-424 | **differs, live test** | We do paint the bar's three parts with method 6 after a static frame (`rear_set`, `set_color`, zone bits 1, 2, 4, payload with the bar index 8 in byte 4). His payload had no index byte and lit nothing; ours is the owner-verified one for the left part, middle and right are marked "assumed" in `rear.rs`. L7 settles whether middle and right really light, and whether a plain static frame (mode 0xFF) would be simpler |
| 9 | The Chicony keyboard takes a 24-bit colour: `B1`, `14 00 00 R G B`, `08 02 <effect> ...` | 304-328 | handled | `docs/PROTOCOL.md` documents the same three packets; `ts-domain` `keyboard` sends them |
| 10 | Live apply debounced to 120 ms because each write is a USB control transfer | 339-342 | handled, open question stays | Our per-key effects run at 5 frames/s (`perkeyfx::runner` 200 ms) and a frame is 8 reports 16 ms apart plus a 50 ms settle (`keyboard::FRAME_SPACING`, `FRAME_SETTLE`). His file has no measurement of a higher rate, so the APP-DESIGN question "does it tolerate 30 fps" is still open (L8). Nothing changed |
| 11 | The idle watcher (his GUI) parked on a blocking read | 377-401 | n/a | He reads evdev in a GUI process; we have no idle blanking in the daemon |
| 12 | All 25 WMI sub-functions go through SMM (a software SMI, `0xD0`); a WMI read is an SMI | 403-424, 2278-2370 | live test | Each daemon tick reads the five sensors (five WMI calls in `c_sensors`), `get_mode` every third tick and `get_fan` every 15th. That is about 2.5 SMIs/s at the idle two-second pace and 5/s when busy. No cost is quoted by him; L4 measures it |
| 13 | One timer should own both lights, idle-off in the app | 426-448, 773-895 | n/a | App-level behaviour. Our side is consistent with his result: the keyboard's own sleep timer is the HID flag `30 01 <0|1>` (`Keyboard::hid_timeout_set`, `Hw::get_kb_timeout` reads the controller first); the WMI flag is only the fallback |
| 14 | The WMI keyboard-timeout function works but does not govern this keyboard | 820-838 | handled | See 13: `Hw::set_kb_timeout` verifies the controller's flag, not the firmware's |
| 15 | The mode key is a WMI event handled in the driver, never a hidraw or evdev event; upstream drivers drop it | 465-497, 899-963 | **live test** | `truesense_wmi.c` binds with no WMI event handler, and `acer_wmi` is blacklisted by `packaging/truesense.modprobe.conf`, so nothing in our stack handles that event. Whether the key still works (the firmware cycling the mode itself) is not known: L2 |
| 16 | The PredatorSense key reports HID usage `0xFF81` on interface 2 (`04 81 FF`), nothing in evdev | 499-527 | handled | `ts-keys` reads the same report; `docs/PROTOCOL.md` lists it; `fnkeys::node::find_node` picks the interface |
| 17 | udev: match the interface with `ID_USB_INTERFACE_NUM`, not `ATTRS{bInterfaceNumber}` | 520-524 | handled, differs | `packaging/70-truesense.rules` matches only `idVendor` and `idProduct` (same parent, so it works) and gives `uaccess` to every hidraw node of the keyboard, wider than his rule (interface 2 only). The seat user can already read their own keyboard, so nothing new is exposed. Could be narrowed with `ENV{ID_USB_INTERFACE_NUM}` if wanted |
| 18 | Behaviour that lived on GUI timers died when the window closed | 540-565 | handled | The daemon owns every continuous behaviour (APP-DESIGN 2.9); `truesense.service` runs without a GUI |
| 19 | Gate hardware paths on DMI model, not on a USB id shared with other chassis | 583-611, 660-695, 2249-2274 | handled, one residual | The module binds only on `Predator PH16-71` (`ts_dmi_table`, `force=1` overrides) and the installer refuses unsupported model tiers. `ts-keys` itself checks only the USB id `04F2:0117`; harmless on this package's target, noted for a future second model |
| 20 | "Lighting restore failed" at every boot on chassis without the ENE chip: retry sleeps, then an error | 615-637 | handled | We have no ENE path. The boot look retries for 20 s only when the keyboard is missing and then logs one warning (`boot_look_worker`) |
| 21 | `on_ac_power()` gave up on the first unreadable power-supply entry; the mode key then ran the AC cycle on battery | 673-677 | handled | `LinuxSensors::on_ac` reads fixed supply names (`ACAD`, `AC`, `AC0`, `ADP0`, `ADP1`), never the type of other supplies, so a wireless mouse cannot matter. A machine charged over USB-C only (a `ucsi` supply, no `ADP*`) would read "on AC" always; not this laptop |
| 22 | Fan-curve bounce through the thermal profile "to wake the fans" | 666-671, 2146-2154 | handled | No code bounces a profile. Confirmed by search: nothing in `ts-platform` or `ts-daemon` writes a neighbouring power mode and back |
| 23 | Lighting must be re-applied after resume (read the bar back to prove it) | 706-748 | handled, hardened | `step.rs` detects a resume as wall clock minus monotonic clock above 5 s (same idea as his CLOCK_BOOTTIME drift; the first tick is only a baseline) and runs `apply_boot_look("resume from suspend")`, test `the_boot_look_is_put_back_after_a_resume_from_suspend`. **Fixed here**: the fan writer, the auto engine and the governor also forget what they wrote at a resume, so the firmware's post-sleep fan and mode state is overwritten on the first tick instead of at the next 15-tick verify (`after_a_resume_the_fans_are_written_again_at_once`). Live check L5 |
| 24 | Running the tests wiped the user's log | 752-769 | handled | Every daemon, domain and system test uses a temp directory (`testkit::TempDir`) and fake ports; nothing reads `/run`, `/sys` or `$HOME` |
| 25 | A hot GPU was invisible to the software fan curve; max(CPU, GPU) drives both fans because the coupling is strong (GPU fan 0 to 255 gives the CPU +4.5 W); a card parked in D3cold reads 0 | 967-1020, 2322-2340 | handled, partly, **fixed here**, live test | Our smart fan has per-side curves with a `couple` of 15 %, not max-of-both. The fallback when the GPU gives no temperature is the CPU's, as in Python, and is kept for the fan curve. **Fixed here** (commits 7722211, 152e44d): that fallback no longer leaks into what is shown or into hot detection: live `tgpu` is a real GPU temperature or 0 and `tgpu_real` says which; the classifier's GPU hot line (82 C) is no longer compared with the CPU temperature (a busy CPU at 82 to 91 C read as "hot": fans at 100 % and a Turbo back-off, while the CPU's own line is 92 C); the temperature alert uses only a real reading. Whether the default `couple` should be higher on this chassis is a tuning question: L10 |
| 26 | Two controls wrote one fan setting and one deleted the other | 1024-1101 | n/a | His GUI. The equivalent here is atomic: `cmd_fan` switches the curve off and writes in one lock |
| 27 | Eco chosen with the mode key reverted a minute later (the mode moved half the machine; `min_perf` floor estimate wrong) | 1105-1229 | n/a | A TrueSense power mode is one firmware setting (`misc 0x0B`); no governor, EPP, `no_turbo` or `min_perf_pct` is written per mode, so there is no half-applied mode |
| 28 | The policy overrode the mode the user had just picked; moving the mode only at the plug event; PPD re-writes EPP behind the app and blinded the CPU read | 1242-1359, 1809-1881 | handled, hardened | The engine reads the firmware's mode (what he concluded is the right input) and never the CPU state. A hand-picked mode holds for `[profile] hold_minutes` and ends when the situation changes (plug, load, heat, low battery). At a plug event native auto writes once (`native_auto_writes_the_mode_once_per_plug_event_and_never_again`); Smart auto writes once through the dwell timers. **Fixed here**: a contact that flips the charger repeatedly wrote the mode on every flip, now the first change is followed at once and the next only after 3 s (`plug.rs`, test `a_charger_that_bounces...`); the low-battery rule (Eco below `battery_low_pct`) wobbled at the line with a one-point gauge change and now releases 3 points above it or on power (`a_low_battery_is_released_only_a_few_points_above_the_line`) |
| 29 | Turbo with the `performance` governor locks `energy_performance_preference` | 1363-1487 | n/a | We write neither governor nor EPP (except `powermgr` restoring what another manager changed) |
| 30 | `min_perf_pct` floor is the kernel's, not an estimate from `cpuinfo_*` | 1135-1189, 1493-1565 | n/a | We write no `min_perf_pct`. A related, smaller point is row 37 |
| 31 | The desktop indicator and notifications | 1569-1731 | n/a | His GUI and `power-profiles-daemon`. Ours stops other managers on request (`powermgr`); the on-screen message uses normal urgency (`osd.rs`) |
| 32 | Event-driven mode changes (`sysfs_notify`) instead of a poll | 1735-1805 | not applicable now | The module has no such attribute; the daemon reads the mode every third tick (2 to 6 s). A change that lasts less than that is not seen, which does not matter for us because the daemon never applies a per-mode CPU state |
| 33 | `notify-send` (libnotify) is not a declared dependency, so alerts failed silently | 1792-1797 | **fixed here** | Same for us: `osd.rs` and the alerts call `notify-send`. Added `libnotify` to `optdepends` of `packaging/arch/PKGBUILD` (commit 5ceaa79) |
| 34 | A "critical" notification is never dismissed by the desktop; a threshold must be adjustable | 1970-2089 | handled | The on-screen display uses normal urgency and says so (`daemon/alerts.rs` header); the CPU and GPU alert limits are configuration (`[alerts]`) with ranges |
| 35 | An unparseable config must say so and not be overwritten | 1883-1968 | handled | `assembly.rs` `load_config`: a configuration with problems (or that cannot be read) is replaced by the built-in defaults for the run and the problems are returned to be logged; the daemon never rewrites `config.toml` (its changing settings live in the state file) |
| 36 | Fixed 0 % is not off: the EC holds a floor of about 1670 rpm for a manual 0; real 0 rpm only by handing the fans to the firmware; 0 rpm at idle in Auto is the curve, not a stall | 209-234, 1000-1004, 2129-2134 | handled | `docs/SAFETY.md` already records the floor ("about 1700 rpm, this does not actually silence a fan"). The smart fan hands the fans to the firmware when calm (`fanctl::Smart::step`, `Duties::AUTO`) and clamps manual duties to 20 (`FanWriter::write`). No code treats 0 rpm as a fault (searched the daemon, the API and the TUI). The stop path keeps its confirm, the 60/55 C start limit and the 70/65 C watchdog. **Fixed here**: `fans: OFF` now carries the same hint as a manual 0 (`ts-api` `FanOutcome`); the watchdog's message said "the laptop got warm" also when the cause was a silent device, it now says which (`fan_watchdog`, test `the_watchdog_says_a_silent_device...`) |
| 37 | Switching the software curve off left both fans at the last percentage | 240-243 | **fixed here** | `set smart_fan false` only saved the setting (the Python daemon too); the curve's last manual duty stayed until somebody pressed Auto. The change from on to off now hands the fans to the firmware once; a manual duty set through `fan` stays (commit a518a89, tests `switching_the_smart_fan_off_hands...`, `a_manual_duty_survives...`) |
| 38 | Every `nvidia-smi` start wakes a suspended GPU and initialises NVML; a long stream is silent once initialised; a reading of a sleeping card is 0 | 75-91 (cost), 987-989 | **fixed here** | Not in his findings as a chapter, but his experience agrees with `ts-system::nvidia` (measured by us earlier: about 30 SMIs/s during NVML initialisation). Two places woke the card: the daemon's GPU stream, and `PowerLimits::info()` (every `cpu_info` poll ran `nvidia-smi`). See section 2 |
| 39 | `acer_wmi`/other drivers must not run next to the module; reloading a module drops watchers | 2351-2363 | handled | `truesense.modprobe.conf` blacklists `acer_wmi`, `linuwu_sense`, `acer_rear_wmi`. `ts-keys` reconnects when the hidraw node goes away (`HidError::Gone`, `LoopTiming::reconnect`) |
| 40 | CoolBoost register `0x10` is overwritten by the EC and has no measurable effect | 2280-2320 | n/a | We do not write EC registers. No CoolBoost feature exists |
| 41 | `balanced` firmware fan table gives up half the range (about 2925 rpm at the CPU limit) | 2322-2340 | handled | It is the reason the smart fan exists; our curve measurements (`fanctl` header) agree: 50 % about 3460 rpm, 100 % about 6050 rpm, the same as his PWM 128 and 255 |
| 42 | On kernel 7.x `linuwu-sense-dkms` no longer builds (`strncpy`) | 2342-2346 | n/a | Not our module. `truesense_wmi.c` uses `strim`, `sscanf`, `hex2bin`, no `strncpy` |
| 43 | EC register diff, SMM reverse engineering, debugfs probe | 256-262, 403-424 | n/a | Deliberately not done here; our module offers no raw method access (only the commands in `ts_cmds[]`) |

## 2. The GPU: what was wrong and what is different now

Verdict 38 deserves its own page, because it was the most expensive problem and the findings only point at it.

**Before.** `SystemSensors::ensure_gpu_stream` started `nvidia-smi ... -l 2` as soon as the smart fan, Smart auto or a user power limit
was on, and never ended it. A query every two seconds keeps a runtime-suspended card awake (its suspend delay is 10 to 15 s), so a
laptop with a smart feature on never reached the battery life of one with the card asleep, and the program's start raised system
management interrupts. The averaged `gpu` and `gpuw` values began at 0, `gpu` and `gpuw` read 0 for a sleeping card as well as for an
unread one, and `tgpu` quietly became the CPU temperature whenever the GPU gave none. `cpu_info`, which clients poll, ran
`nvidia-smi` on each call.

**Now.**

* The stream (`ts-system` `sensors.rs`) never starts for a card that is absent or suspended. It reads `power/runtime_status`
  through the same function the Monitor page uses (`nvidia::gpu_power`), which does not wake it.
* It ends within one line (two seconds) when nothing smart needs the GPU; the daemon says so each tick
  (`Sensors::release_gpu_stream`) and asks again (`ensure_gpu_stream`) when something does. The thread is restarted on demand.
* It parks itself, ending `nvidia-smi` so that the card can suspend, after 60 s of idle readings (load at most 5 %, draw at most
  25 W), then waits until the card has been seen asleep and awake again (somebody else woke it) before starting. If the card never
  sleeps within 180 s, something else holds it awake, so reading it costs nothing more: the stream starts again and does not park for
  10 minutes. A card without runtime power management is never parked.
* A failing `nvidia-smi` (not installed, driver not loaded) is retried after 5 s, then 10, 20 ... up to 5 minutes, not every 5 s
  for ever. A machine without an NVIDIA card never tries.
* The readings reset when a session ends, and the first line of a session counts in full, so a card that a game has just woken is not
  read as idle for eight seconds while an average climbs from 0.
* `cpu_info` no longer asks a suspended card; it shows the limits last read from an awake one (`PowerLimits::info`).
* The live state gains two keys, appended after the Python daemon's: `gpu_state` (`suspended`: the 0 load and 0 W are true;
  `live`; `unsampled`: awake, nobody reads it, the 0 is unknown; `absent`) and `tgpu_real`. `tgpu` is a real GPU temperature or 0,
  never the CPU's. The fan curve keeps its CPU fallback.

What is not changed: the firmware's GPU temperature (`sensors`, WMI id `0x0A01`) is still read every tick, because it does not wake the
card. What it reports while the card sleeps is unknown (L3).

## 3. Differences that touch the kernel module or the protocol notes

The module (`kernel/truesense-wmi/truesense_wmi.c`) is unchanged. Compared with his findings:

| Point | His finding | Ours | Action |
|---|---|---|---|
| DMI guard | gate by whole-word `product_name` (19) | `Acer` + `Predator PH16-71`, `force=1` | none |
| Power-mode codes | 0 Quiet, 1 Balanced, 4 Performance, 5 Turbo, 6 Eco; command `(index << 8) \| 0x0B` (14) | identical, `MISC_MODE 0x0B`, accepted 0, 1, 4, 5, 6 | none |
| Rear-light frame | bytes 8 = 3, 9 = 2; the firmware ignores 3, 8, 10-15 (4b, 6) | validated as fixed (`b[8]==3`, `b[9]==2`, `b[10..16]==0`) | none: stricter than needed |
| Rear modes | 0..7 and 0xFF (5) | 0..3 and 7 | live test L6 before widening |
| Method 6 (static colour per zone) | stores but lights nothing (8, 9) | used for the static effect | live test L7 |
| Fan manual 0 | holds the floor (36) | the module accepts 0 (`cpu && (cpu < 20 ...)`); documented in `SAFETY.md` | none; a comment in the module still says "0 = that fan stopped" and could say "held at the firmware's minimum" (not changed: comment only) |
| Fan RPM | not decoded specially | 13-bit field, mask `0x1FFF` | none |
| Keyboard timeout | WMI function does not govern the keyboard; the HID flag does (24) | HID flag acts, WMI verified as fallback | none |
| Mode key event | handled by the driver (14, 25) | not handled (no `.notify`) | live test L2 |
| Resume | lighting restored after resume (22) | boot look re-applied on resume by the daemon | hardened (row 23) |
| Battery threshold | not covered | `get_battery`/`set_battery` 80 % limit | none |
| hidraw access | group `input` on interface 2 (15) | `uaccess` on the keyboard's hidraw nodes | none (row 17) |
| SMM cost | every WMI call is an SMI (12) | five WMI reads per tick | live test L4 |

## 4. Live checks (on the laptop, with the real daemon)

Safe, read-mostly. Do them with `truesense state` open in another terminal.

* **L1, the GPU sleeps with a smart feature on.** Smart fan on, on battery, nothing using the NVIDIA card. Watch
  `cat /sys/bus/pci/devices/0000:01:00.0/power/runtime_status` (use your card's address; `lspci | grep NVIDIA`) every 5 s for two
  minutes and `pgrep -a nvidia-smi`. Expected: `nvidia-smi` runs at most for about a minute after the daemon starts or after a
  program wakes the card; then it ends and `runtime_status` reaches `suspended`; `truesense state` shows `gpu_state=suspended`, `gpu=0`,
  `gpuw=0`. Start a game or `glxgears` on the card: within about two seconds `nvidia-smi` appears, `gpu_state=live`, `gpu` and `gpuw`
  rise. Close it: after about a minute of idle the stream ends again.
* **L2, the mode key.** With the module loaded and `acer_wmi` blacklisted, press the physical mode key, then `truesense power` (or
  `get_mode` through `/dev/truesense`) and `dmesg | tail`. Observe: does the mode change, does the key's LED change, is there any kernel
  message. This decides whether we need a WMI event handler like his `acer_thermal_profile_change`.
* **L3, the firmware's GPU temperature while the card sleeps.** With the card `suspended`, run `truesense sensors`. Observe the GPU
  temperature: 0, stale, or a real number. If 0, `tgpu=0` and `tgpu_real=false` in the live state is expected, and the fan curve follows
  the CPU temperature; if a number, `tgpu_real=true`.
* **L4, cost of the daemon's reads.** Read the SMI counter (`sudo rdmsr -d 0x34`, needs `msr`) before and after 60 s with the daemon
  running idle and again with `systemctl stop truesense`. Expected: the difference is the daemon's cost (about 150 SMIs per minute at the
  idle pace); if it is large, the reads can be thinned (for example reading the fan RPM only while a fan is manual).
* **L5, resume.** Set a fixed smart duty (heavy load), suspend and resume. Expected: within two seconds the fans are back where the daemon
  wants them and the keyboard and the bar show the boot look; the journal has `boot look (resume from suspend)` once.
* **L6, rear-light effects after boot or after brightness 0.** Reboot, then `truesense rear set wave` (or a Breathing/Neon frame) before
  anything else. If the bar stays dark until a Breathing frame, the wake quirk applies and one Breathing frame is needed first. Modes 4
  to 6 and 0xFF would need a module patch to try; do not do that without the module's debugfs access.
* **L7, three-part static colour.** `truesense rear set static --zones ff0000,00ff00,0000ff` (left, middle, right);
  look at the left, middle and right segments. Observe whether all three change.
* **L8, keyboard frame rate.** Run a per-key effect and watch for flicker, dropped frames or `dmesg` USB errors at 5 frames/s (now); only
  then try a shorter `FRAME_INTERVAL`.
* **L9, boot look.** Save a boot look, reboot, and check that the keyboard and the bar come up with it within about 20 s, with at most
  one warning in the journal.
* **L10, CPU and GPU coupling.** With a CPU-only load at 85 to 92 C, `truesense state` should NOT say `activity=hot` unless the CPU
  reaches 92 C (`tgpu_real=false` case) and the fan duties should follow the CPU curve; with a GPU load the GPU fan should rise too. If the
  GPU fan lags while the CPU is the one that is hot, raise `couple` in the active curve (default 15, range 0 to 50) and compare the sustained
  package power, never the temperature (his method, 1011-1018).
* **L11, the CPU ceiling after a stop.** `ExecStopPost` and `CapWriter::uncap` write `cpuinfo_max_freq` to every `scaling_max_freq`. His
  measurement (1135-1189) shows that `cpuinfo_max_freq` is the base clock while `no_turbo` is 1. With another manager having set
  `no_turbo=1`, stopping the service could leave the ceiling at the base clock after turbo comes back (`powermgr::ceiling_low` and the doctor
  catch it later). Check: `echo 1 | sudo tee /sys/devices/system/cpu/intel_pstate/no_turbo`, `systemctl restart truesense`, set
  `no_turbo` back to 0, and read `cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_max_freq`. If it is the base clock, write a very large
  value instead of `cpuinfo_max_freq` while `no_turbo` is 1.
* **L12, the thermal governor on this chassis.** His measurements (1011-1018, 1423-1434) show the CPU sitting at 91 to 93 C under load by
  design. Our governor (`[cpu] gov_enabled`, default on) lowers the clock cap from 88 C under smart control. Run an all-core load for five
  minutes with the smart fan on and read the `cap` live key: if it falls to 2800 MHz while the firmware alone would have held about 2600
  MHz at the same power, the governor is only costing single-thread speed and its `gov_hi`/`gov_lo` could be raised for this model.

## 5. Keyboard effects: his hardware-confirmed list against ours

Sources, all read-only: his `src/hardware/keyboard_rgb.rs` (`Effect`, `opcode`, `uses_color`, `uses_speed`, `uses_direction`, and the
tests `colour_speed_and_direction_match_the_spec_table` and `all_six_wave_directions_reach_the_helper`), `src/hardware/effects/kb.rs`
(what each firmware effect looks like, "seen on a PH16-71 (2026-10-15)"), `tools/effects-reference.js`, his README compatibility tables and
`PH16-71-FINDINGS.md` section 7 ("The effect table for this generation is wider than the palette path's, at 14 entries").

**Result: no effect is missing.** His 14 for the PH16-71's Chicony keyboard (`04F2:0117`) are exactly our 14, with the same codes:

| His `Effect` | His opcode | Our name | Our code | Takes colour | Takes speed | Takes direction |
|---|---|---|---|---|---|---|
| Off (Static at brightness 0) | 0x01 | off | 0 | no | no | no |
| Static | 0x01 | static | 1 | yes | no | no |
| Breathing | 0x02 | breathing | 2 | yes | yes | no |
| Wave | 0x03 | wave | 3 | no | yes | **yes, six values** |
| Snake | 0x05 | snake | 5 | yes | yes | no |
| Ripple | 0x06 | ripple | 6 | yes | yes | no |
| Rainbow | 0x08 | neon | 8 | no | yes | no |
| Rain | 0x0A | raindrop | 10 | yes | yes | no |
| Lightning | 0x12 | lightning | 18 | yes | yes | no |
| Spot | 0x25 | spot | 37 | yes | yes | no |
| Stars | 0x26 | star | 38 | yes | yes | no |
| Fireball | 0x27 | fireball | 39 | yes | yes | no |
| Snow | 0x28 | snow | 40 | yes | yes | no |
| Heartbeat | 0x29 | heartbeat | 41 | yes | yes | no |

His colour rule (`!matches!(Off | Wave | Rainbow)`) is our `NO_COLOR` (`off`, `wave`, `neon`); his speed rule (not Off and not Static) is what
the Lighting page already does. The names differ in two places only (his Rainbow is our neon; his Rain is our raindrop). The one real
difference is `off`: he expresses it as Static at brightness 0, we send the `off` effect packet and use brightness 0 for "dark", as the
earlier editions verified. **Direction**: he lists it for the Wave alone and tested that for all 14, so the Lighting page now offers the
Direction control for the Wave only (it used to enable it for nine effects as a guess); every other effect is sent direction 0.

**Listed by him, not confirmed on a PH16-71, therefore not offered:**

* The 2024+ Sunrex keyboard (`05af:*`, PH16-72 and later) lists Off, Static, Breathing, Wave, Snake, Neon, Spot, Star, Rainbow, five Slash
  variants, Zoom, Row Wave and Swiping. His README marks all of it "implemented, awaiting confirmation on real hardware", and it is a different
  controller and protocol (not `04F2:0117`), so none of its codes can be sent to ours.
* The ENEK5130 chip (PHN16S-71, PHN16-73, AN16S-61): Breathing and Neon, plus Wave, Shifting and Zoom "where the controller advertises
  them"; the AN16S-61 report names six more native modes (hardware off, a boot blink the EC fires itself, four animations) that he has not
  wired in. Again another chip.
* The Helios 300 Chicony keyboard (PH317-56, the same USB id as ours): a fixed 7-colour palette and 12 effects over an older `08 00 .. BE`
  packet family. His code gates on the model list, because the id is shared; ours does the same by DMI and refuses other models.

No code of the 14 has an unexplained hole: codes between them (4, 7, 9, 11 to 17, 19 to 36) were not tested by either of us, and are not
sent. The software sweeps (`wave-clockwise`, `wave-counter-clockwise`, per-key animations) and the themes cover the looks the firmware cannot do in every colour.

## 6. Wave directions

The Wave's direction byte (`08 02 03 <speed> <brightness> 01 <byte>`): the byte is `logical + 1`, and TrueSense stores the logical number.
He confirmed all six on a PH16-71 (2026-10-15); `ts-domain` `keyboard::Direction` and the test `logical_4_is_wire_byte_5_clockwise_and_5_is_6_counter_clockwise`
pin it:

| Logical (stored, CLI number) | Wire byte | Direction |
|---|---|---|
| 0 | 1 | left to right |
| 1 | 2 | right to left |
| 2 | 3 | bottom to top |
| 3 | 4 | top to bottom |
| 4 | 5 | clockwise |
| 5 | 6 | counter-clockwise |

`truesense keyboard effect wave --direction clockwise` (or `4`). A value above 5 is refused for the Wave; the other effects keep the old
0 to 254 rule. The rear light bar: our modes are the kernel module's (2 neon, 3 wave), the byte is 0 or 1 (the module refuses 2), and his
measurement on the same bar was that his modes 3 (Rainbow) and 4 (Wave) follow a direction and Ripple does not. By code that is our wave
(3); his mode 2 is the one code we call neon and he does not list among the directional ones, so the Direction control of the bar now stays
on the wave only. Which of 0 and 1 is left to right on the bar is not confirmed anywhere (he sends 1 and 2 on another path), so the
bar's control keeps its two unlabelled-by-hardware names; live check L13 settles it.

* **L13, wave directions.** `truesense keyboard effect wave --direction clockwise`, then `counter-clockwise`, `bottom-to-top`,
  `top-to-bottom`, `right-to-left`, `left-to-right`: each should move the rainbow the way its name says (clockwise and counter-clockwise
  are the two reference-confirmed ones, bytes 5 and 6). Then `truesense rear set wave --direction 0` and `--direction 1`: note which way
  the bar's wave travels for each, and whether `rear set neon --direction 0|1` looks the same both ways.

