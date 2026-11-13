# Performance: what costs what

Numbers for TrueSense 0.5.0 (Rust), measured with `tools/bench/run.sh` in the firmware simulator on the Acer Predator Helios 16 (PH16-71, CachyOS, 24 threads), before and after the efficiency work of this branch. The point of this page: where CPU, wake-ups and memory really go, so effort goes where it pays, and an honest account of what the simulator can and cannot show.

## The short version

TrueSense is **not CPU-bound**. What it costs on a laptop is *wake-ups* (every time a process leaves idle, the CPU leaves its sleep state) and *firmware calls* (every WMI call is a system management interrupt that stalls every core for milliseconds, see `docs/PH16-71-NOTES.md`, row 12). A daemon that reads a few numbers a second does not need CPU; it needs to touch the machine rarely. The work in this branch is almost all about that:

| What | Before | After |
|---|---|---|
| Daemon: sensor reads per tick (real `hwmon`) | 17 `coretemp` inputs, 16 core wake-ups, 1.6 ms | the package input: 1 read, 12 microseconds |
| Daemon: firmware calls per tick | 5 (`sensors`), plus the mode and fan reads | 3.4 on average (`sensors_temps`; the fan speeds, which are only displayed, on one tick in five) |
| Key service: wake-ups per second | 26 | 0.3 |
| Audio watcher: wake-ups per second, helper programs per second | 68 and 3.3 | 1.0 and 0.4 |
| `truesense status` (helpers at 12/4 ms) | 48 ms | 22 ms |
| Qt window, idle, visible: main thread wake-ups | 5.5 per second | 3.8 |
| Qt window hidden | 0 | 0 |

## Measured results

Idle windows of 60 s, one process each, everything in simulation (`TRUESENSE_SIM=1`): the daemon with the smart fan and the smart auto profile on, the key service on a fake Fn-key interface, the audio watcher on fake `pactl` and `amixer` that only count their calls and answer, the Qt app offscreen with the software renderer and a simulated daemon. A fake CPU package sensor moves every second so that live values really change. CPU is the run time of all threads as a percentage of one core (`schedstat`), wake-ups are the change of `voluntary_ctxt_switches` summed over the threads, memory is `smaps_rollup`.

| Process | CPU before | CPU after | Wake-ups/s before | after | Threads | RSS | Own memory (anon) |
|---|---|---|---|---|---|---|---|
| Daemon | 0.32 % ¹ | 0.17 % | 18.3 ¹ | 2.3 | 4 | 5.9 MB | 2.2 MB |
| Key service | 0.08 % | **0.002 %** | 26.0 | **0.3** | 4 to 5 | 6.5 MB | 2.5 MB |
| Audio watcher | 0.32 % ² | **0.02 %** | 67.8 ² | **1.0** | 2 to 3 | 4.2 MB | 0.4 MB |
| Qt app, window visible, idle | 0.54 % | 0.45 % | 11.8 | 8.4 | 14 | 99 MB | 29 MB |
| Qt app, window hidden | 0.00 % | 0.00 % | 0.0 | 0.0 | 6 | 94 MB | 24 MB |

¹ Measured on this laptop's real `hwmon`. The benchmark's own fake sensor has one input (the package), so its "before" daemon does not read 17 files and shows 0.19 % and 2.3 wake-ups; the 16 extra wake-ups per tick are the model-specific-register reads of the real `coretemp`, inter-processor interrupts that wake sleeping cores, which a temporary file system cannot reproduce. The per-read cost on the real tree is in the table below. On a real, idle machine those interrupts hit cores in deep sleep, so the saving there is larger than the simulation shows.

² The audio watcher is measured after the login repair (20 s and 90 s after start) has run, in a 60 s window that holds six safety passes (2 `amixer` and 1 or 2 `pactl` calls each, every 10 s).

Start-up (whole command: fork, exec, run, exit; median of 20 runs; `status` and `doctor` against a simulated daemon with helper programs that pause for an assumed 12 ms (`pactl`), 4 ms (`amixer`) and 3 ms (`systemctl`)):

| Command | Before | After | Helper programs per run |
|---|---|---|---|
| `truesense --version` | 0.6 ms | 0.4 ms | 0 |
| `truesense fan status` | 2.9 ms | 2.8 ms | 0 |
| `truesense status` | 48 ms | **22 ms** | 3 |
| `truesense doctor` | 77 ms | 73 ms (best 61) | 16 |
| `truesense-fast fan smart` | 1.3 ms | 1.3 ms | 0 |
| terminal UI, first frame | 6.6 ms | 6.7 ms | 0 |
| Qt app, first frame and exit (offscreen) | 152 ms | 160 ms | 0 |

Sizes: `truesense` 7.7 MB (static, one file for the daemon, the command line, the terminal UI and the key and audio services), `truesense-qt` 11.7 MB.

Per tick of the daemon, measured in process on the real `hwmon` (the same tick, simulated firmware):

| | Before | After |
|---|---|---|
| CPU temperature read | 1600 microseconds, 16 voluntary context switches | 12 microseconds, 1 |
| Whole tick (`guarded_step`) | 1380 microseconds, 17.7 switches | 476 microseconds, 1.25 switches |

Firmware commands per 30 steady ticks, pinned by a test (`a_steady_tick_asks_the_firmware_for_no_more_than_its_budget`):

| Command | Calls | Meaning |
|---|---|---|
| `sensors_temps` | 24 | the three temperatures (3 firmware calls) |
| `sensors` | 6 | the three temperatures and both fan speeds (5 firmware calls), one tick in five |
| `get_mode` | 10 | every third tick |
| `get_fan` | at most 2 | every fifteenth tick (the writer's check) |

The Go edition made the same set of calls with `sensors` on every tick (5 firmware calls), so the daemon now asks the firmware for about 30 % fewer calls than Go did. What the Rust edition did *more* than Go, and what this branch removes, was the 17 sensor files per tick (Go read `temp1_input` only).

## Measured on the real laptop (PH16-71, installed 0.5.0, 60 s windows, 2026-11-07)

The simulation tables above cannot show the cost of real firmware calls (kernel time in ACPI/WMI and sysfs), so the installed services were measured on the laptop itself, before and after this pass, next to the Go edition's documented figures:

| Process | Before this pass | After | Go 0.4.0 (documented) |
|---|---|---|---|
| Daemon, CPU of one core | 1.38 % | **0.37 %** | about 0.5 % |
| Daemon, wakeups per second | 16.9 | **2.0** | about 1 tick/s plus helpers |
| Daemon, resident memory | 1.7 MB | 5.9 MB | 14.4 MB |
| Audio watcher, CPU / wakeups per s | 0.35 % / 52 | **0.03 % / 1.2** | (about 14 MB) |
| Key service, CPU / wakeups per s | 0.13 % / 26 | **0.00 % / 0.5** | (about 16 MB) |
| The three together, resident memory | about 7 MB | about 16 MB | 44.6 MB |
| Qt window open on Cooling (Hyprland, software renderer), CPU / wakeups per s / PSS | 2 to 3 % / about 220 / 130 MB | **0.9 % / 16 / 106 MB** | (no app) |

What the regression was: the daemon read all 17 coretemp inputs on every tick (each is an MSR read by inter-processor interrupt that wakes that core) where Go read the package sensor only; the audio watcher and key service polled. All three are fixed, and the full self-test (fan phase included) passes on the hardware.

The Qt window with the software renderer: about 130 MB resident (268-323 MB with OpenGL on the NVIDIA stack), about 1 % of a core while it is visible and idle (3 % before the repaint fixes), nothing while hidden, and closing it now quits the process unless a tray host is really present.

## The honest reading

- **Most of the daemon's cost on the real laptop is kernel time in firmware and sysfs reads**, not CPU in our code. On the real machine the daemon measured about 1.4 % of a core, nearly all of it system time. The simulator has no firmware, so it can only show what we control: the wake-ups and syscalls around the calls, and the number of calls (fixed by a test). Whether the real daemon now lands below the Go edition's 0.5 % is a measurement to take on the laptop (see the checklist at the end).
- **The package sensor is not exactly the hottest core.** Reading only `temp1_input` is what the Go edition did and what the 17-input version replaced. Measured on this machine: with 12 busy threads the package value equalled the hottest core in every sample; under a single-core burst the package value lagged the hottest core, by up to 14 degrees for a sample or two. The firmware's own CPU temperature, which the daemon also takes the maximum with every tick, tracks fast bursts, and the fan curve and the governor see it in the same second. On a CPU whose `coretemp` has no `Package id` input all inputs are still read.
- **The three always-on services are now asleep when nothing happens.** The key service waits in `poll` on the keyboard and an inotify descriptor; the lighting loops sleep 30 s on the process bell and are woken by the same inotify thread or by a stop; the audio watcher waits on one resident `pactl subscribe`. Each had a polling loop (a flag read every 50 ms, a 50 ms sleep slice, a 500 ms look at a file) that was most of its wake-ups.
- **The Qt window's idle cost is rendering.** With no canvas repaint at all, the Home page costs 0.23 % and 1.4 wake-ups a second in the simulator; the four sparkline repaints once a second were half of what was left. They now repaint only when the numbers they would draw changed, and at most every three seconds. What remains is the text and tile updates of a one-second live strip and the software renderer drawing them; a real compositor and a HiDPI screen multiply the pixels, so the live number on the laptop will be higher than the simulator's.
- **Memory** is dominated by the shared program file (the static binary is mapped once for the three services) and, in the window, by Qt itself. The private part of the daemon is 2.2 MB, of the key service 2.5 MB; the services' share of the program file is paid once.

## What was worth fixing, and was fixed

- **Reading every core's temperature every second.** One inter-processor interrupt per core, about 16 wake-ups and 1.6 ms of kernel time a tick on the real tree; now the package input only.
- **Asking the firmware for fan speeds nobody decides on.** The kernel module has a `sensors_temps` command (three firmware calls instead of five); the daemon uses it on four ticks in five. An older module answers "unknown command" once and the full reading is used from then on.
- **Polling threads.** The signal bridge looked at a flag every 50 ms in every foreground service (20 wake-ups a second each); it blocks on a pipe now. The audio clock slept in 50 ms slices; it waits on the process bell (`ts_foundation::wake`).
- **The audio watcher asked the sound system every second.** Two to four `pactl` and `amixer` processes a second, for ever; one resident `pactl subscribe` replaces them, with a safety pass every 10 s and the old one-second pace if the stream cannot start.
- **The key service's timers.** 500 ms loop, 1.5 s config stat, two lighting loops reading the user state twice a second each: replaced by inotify on the two directories involved.
- **A held stdout lock.** `main` held the standard output and error locks for the whole command, so any other thread that printed (the key service's actions and lighting loops) blocked for ever, and a key service with a running theme could not be stopped. Not a performance number, but found while measuring and fixed.
- **Waiting for a helper program** polled the child every 0.1 to 5 ms, about ten wake-ups per `pactl` call; the runner now blocks on the program's output and only polls for the exit after it closed it.
- **`truesense status`** runs the sound and jack queries and the daemon request beside the firmware reads (48 ms to 22 ms with the assumed helper latencies).
- **The window's fan read** is cached against the daemon's own label of what it wrote (5 s at most), so the live strip does not make an SMI every second.
- **The window rebuilt its Home page every second.** Unchanged view-model answers are now the same object, tile and banner repeaters update in place, sparklines repaint only on change and at most every three seconds, the tray menu is rebuilt at most every five seconds.

## What was measured and left alone

- **`Services::system()` costs 0.1 ms** (75 to 103 microseconds in process, 200 runs): nothing was made lazy.
- **The allocator.** musl's allocator is slower than glibc's, but no hot path here is allocation-bound (a daemon tick is 0.5 ms in total, `status` is dominated by helper programs), so no dependency was added.
- **Thread stacks.** Helper threads get 32 to 64 KiB stacks where this branch added them; the existing threads were left: their stacks are virtual memory that is never touched (the private memory above is what counts).
- **The 17 `coretemp` inputs** could still be read when the package value is high (a cheap way to keep the "hottest core" semantics where it matters); not done, because the firmware temperature already covers bursts and Go shipped with the package input alone. Reverting is one function (`LinuxSensors::cpu_temp`).
- **`truesense doctor`** (73 ms, 16 helper programs) already runs its checks on a worker pool; the time is the slowest helper.
- **The Qt live poller** woke about five times per tick against the real service; the connect helper thread is gone (see "Qt app responsiveness"), which saved one. A persistent connection would need a protocol change in the daemon (one request per connection today) and was not made.

## What to check on the real laptop

1. Daemon over 60 s with the smart fan on: CPU (the system-time share especially) and wake-ups; compare with the Go edition's 0.5 %. Also `cat /sys/class/hwmon/hwmon*/name` to confirm there is a `Package id` label (`temp1_label`).
2. After the kernel module is rebuilt and reloaded by the installer (the version string is unchanged): `echo sensors_temps > /dev/truesense; cat /dev/truesense` answers `ok cpu= sys= gpu=`; before the rebuild the daemon uses `sensors` every tick (the old behaviour).
3. Plug and unplug a headset: the `microphone policy:` line should appear within about a second. If it only appears after about ten seconds, `pactl subscribe` does not announce the jack on this system and `FALLBACK` in `ts-domain/src/audio/events.rs` should be shortened.
4. `truesense keys run`: edit a binding in the window (applied at once), start and stop a theme (starts at once), press the PREDATOR key twenty times (all actions run: the old stdout lock dropped them after eight), stop the service (immediate).
5. Window: close it with and without a StatusNotifier host (a panel with a tray, then without): without one, closing must quit the program. The host check reads `IsStatusNotifierHostRegistered` from the session bus.

## Qt app responsiveness

Measured 2026-11-13 with `tools/qtprofile/qtprofile.py respond` (the program's own script, `qml/BenchHook.qml`, run with `TRUESENSE_QT_BENCH=1`): an isolated test window on the real display (Hyprland, eDP-1 2560x1600 at 165 Hz, software renderer, 760x520), simulated firmware, no root service, medians of 3 runs (start: 10 runs), "before" = this branch's first commit (the script only), "after" = the branch. The machine was busy with other builds (load average 6 to 18), so single numbers move by a few milliseconds; the before/after runs alternate. Times are milliseconds from the moment of the act.

**What a person waited for.** Everything the window shows from the live values (the power mode tile, the fan mode, the dials) followed a change only at the next one-second live tick, so a click on a power mode answered in 3 ms and was *shown* 0.5 to 0.75 s later (a tick that had just passed meant a full second):

| Action | Command answered | Shown before | Shown after |
|---|---|---|---|
| Power mode (Performance page tiles) | 3 | 532 to 751 | **3.5** (the tile itself: at the click) |
| Fan max / fan auto (Cooling) | 3 to 7 | 202 / 750 | **6.6 / 5.4** (the tile itself: at the click) |
| Keyboard colour (simulated 187 ms keyboard write) | 187 | not on screen | not on screen (the page shows its own choice at once) |
| Theme apply / stop | 16 / 5 | | |

Two changes did it:

- **A command that changes the machine asks for a live reading at once.** The bus knows which commands only read (`Request::mutates` in `ts-ui-model/src/bus.rs`: verbs such as `status`, `list`, `show`, `preview`, `monitor ...`, or an `action` of `status`/`list`/...); when any other command finishes, worked or not, the bridge pokes the live poller (`Poke` in `poller.rs`). The first poke after a quiet moment ticks at once; pokes close together (a slider drag) fold into at most one extra reading every 250 ms, and a poke while the window is hidden does nothing. Idle cost is unchanged (nothing is poked when nothing is done).
- **Tiles answer the click.** `TileRow { optimistic: true }` shows the picked tile as selected at once and lets the live value take over as soon as it changes; the page's fail callback calls `settle()` to put the real state back (and shows the error), and without any change the pick falls back after 2.5 s. Used for the power mode (Home, Performance) and the fan mode (Home, Cooling: Smart, Auto, Max, Manual at its slider values). Never for a choice that asks first: a model entry with `asks: true` is never shown early, and the GPU mode, fan stop and undervolt paths are not optimistic at all.

**Pages and tabs.** A page is built synchronously when it is shown (the Loader of the shell); that is the time the GUI thread is busy (`sync`) and the first frame follows 2 to 5 ms later. Revisits cost the same as first visits (a page is destroyed when left, which is what keeps a hidden page from running at all):

| Page (after) | first visit: sync / frame / with data | revisit: sync / frame / with data |
|---|---|---|
| Home | (cold start) | 13.5 / 15.4 / 19.3 |
| Monitor | 19.8 / 21.6 / 665 ¹ | 9.8 / 11.5 / 46 |
| Performance | 30.7 / 35.2 / 38 | 19.5 / 23.4 / 26 |
| Cooling | 21.0 / 23.3 / 40 | 13.5 / 15.5 / 35 |
| Lighting | 38.0 / 41.0 / 57 | 16.9 / 21.1 / 40 |
| Keys | 29.7 / 32.1 / 52 | 12.4 / 14.8 / 35 |
| Battery and screen | 18.7 / 21.9 / 28 | 9.7 / 11.2 / 21 |
| Audio | 14.2 / 16.3 / 34 | 15.2 / 18.5 / 22 |
| Profiles and favourites | 15.2 / 17.1 / 30 | 8.5 / 12.6 / 24 |
| Health | 14.2 / 15.9 / 36 | 12.8 / 15.0 / 25 |
| Settings | 18.8 / 23.0 / 30 | 13.5 / 17.6 / 37 |

¹ The Monitor page's first CPU reading needs two samples of `/proc/stat` some time apart; the page shows its frame at once and the numbers when they exist.

The view-model calls (`Vm.call` into Rust) are not the cost: 2 to 27 calls per page, 0.1 to 0.9 ms together (`vm_calls`, `vm_ms` in the script's output). The rest is creating the QML objects; a page cache was considered and not made, because a kept page would keep evaluating its bindings on every live frame while hidden.

Lighting tabs (after; before in brackets where it differed): Per-key 37 ms to build, 76 with its pictures; Rear 18; Looks 17; Keyboard 15 (26). **Themes**: the gallery's 32 pictures were asked for again on every visit and each answer copied the whole picture map and re-evaluated every card (32 answers x 32 cards); they are now kept in `LightingVm` for the run (a saved or deleted theme is drawn again) and the cards are told once per batch of answers. Revisit with all pictures shown: 132 to 96 ms, longest GUI stall 67 to 42 ms (offscreen, busier machine: 440 to 177 ms and 110 to 43 ms).

**Writes while dragging.** Sliders already send at most one value per 60 to 250 ms (`SliderRow.coalesceMs`) and the bus keeps only the newest unstarted write per control (`Bus::write`), so a 190 ms keyboard frame never builds a queue: measured with 30 raw writes 16 ms apart, the last value is on the device when the drag ends. The GUI thread never waits for a write.

**Cold start** (10 runs, the same before and after): the application layer 0.4 ms, the shell's QML complete 87 ms, first frame 135 ms, window mapped (seen by the compositor) 150 ms, every first command answered and the first live frame in 17 ms after the first frame, shown at 33 ms.

**Idle and the live poller.** Idle on the real display, side by side (`qtprofile.py compare`, 20 s): Cooling 0.3 to 0.1 % CPU, 20 to 19 wake-ups a second; Home 0.3 to 0.2 %, 21 to 22; no change in memory. The live tick against the real root service (read-only `state` request, ticking with nothing drawn): 4.85 to 3.8 wake-ups a second: every daemon call started a helper thread for its connect (a guard against a full accept queue); `ts-ipc` now connects without blocking on the calling thread and only falls back to the helper when the connect would wait.

**A trap found on the way.** `TRUESENSE_SIM=1` simulates the firmware only: without `TRUESENSE_SOCKET` a test instance talks to the real root service at `/run/truesense/truesense.sock` (world-writable), so a scripted `power set` or `fan max` changes the real laptop. `tools/qtprofile` (all modes) and `glitch.py` now point `TRUESENSE_SOCKET`, `TRUESENSE_RUN`, `TRUESENSE_STATE` and `TRUESENSE_USERSTATE` at their private directory.

Measure again:

```
make qt-release
tools/qtprofile/qtprofile.py respond --runs 3                       # real display, all phases
tools/qtprofile/qtprofile.py respond --platform offscreen --old OLD-BINARY --phases pages,actions
```

## How to measure again

```
make build && make qt-release          # or point the script at any build
tools/bench/run.sh                     # about 8 minutes: prints the tables above
tools/bench/run.sh --seconds 15 --runs 8 --only daemon,keys,audio    # a quick look
tools/bench/run.sh --json out.json     # the raw numbers as well
```

The script needs only `bash`, `python3` and `unshare` (it refuses to run without a private namespace, because the daemon writes `/tmp/truesense-state`). It runs in a user and mount namespace with its own `/tmp`, `/dev` and `/sys/class/{hidraw,hwmon}`, fake helper programs that count their calls, a temporary `HOME`, and the Qt app offscreen offscreen; nothing outside is touched, and it only ever measures processes it started. `TS_BENCH_VERBOSE=1` prints the wake-ups and CPU of each thread.

Per-tick numbers of the daemon come from calling `guarded_step` in a loop in process (a throwaway example, not kept; the firmware budget is a permanent test). For memory, read `VmRSS` and `RssAnon` from `/proc/<pid>/status`; for wake-ups compare `voluntary_ctxt_switches` of every thread under `/proc/<pid>/task` over a minute.
