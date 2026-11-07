# Architecture

One static Rust program (`truesense`) on top of one small C kernel module. `truesense-fast` is the same file under another name and does only what a status-bar button needs.

The crate layout and its rules are in [RUST-MIGRATION.md](RUST-MIGRATION.md); this page is the map of one request.

```
 firmware (ACPI-WMI)
        ▲
 truesense_wmi (kernel module, C, DKMS)      /dev/truesense: text protocol (docs/PROTOCOL.md)
        ▲
 ts-platform::firmware  ── Device trait: the kernel device, or the Simulator (TRUESENSE_SIM=1)
 ts-platform::hw        ── typed features: write, read back, return the verified value
        ▲
 ts-daemon (root, truesense.service) ── loop every 1–2 s, decisions from ts-decision
        └ control socket /run/truesense/truesense.sock  (JSON lines, SO_PEERCRED uid check)
                ▲
 ts-ipc::client ── one request, typed errors
        ▲
 ts-api::api::Api ── what every client shares: daemon first, safe hardware fallback
        ▲
 ts-cli · ts-tui · ts-gui (loopback web server + embedded front end) · ts-keys (hidraw key service)
```

Rules that keep it safe:

- Dependencies point down the crate list; Cargo refuses a cycle.
- Decisions (`ts-decision`) are pure: readings in, a duty or a mode out, with an injected clock.
- Every effect (files, processes, sockets, time) sits behind a trait and is replaced by a fake in tests.
- A fan duty is 0 or 20–100; stopping a fan needs the daemon, an explicit confirmation, a cool machine and the temperature watchdog; the fans return to firmware Auto on every exit path.
- `unsafe` is limited to the places the kernel needs (ioctl, hidraw poll, `SO_PEERCRED`, signals, `flock`, `access`), each with a `SAFETY` comment and a test.

## Alerts

The root daemon, not a window, tells the desktop user when something needs attention, so it works with every window closed.

- **Rules** (`ts-decision::alerts`, pure, injected clock): CPU or GPU at or above a limit (default 90 C), battery low on battery, the thermal governor holding the CPU clock down (thermal throttling suspected), the CPU temperature unreadable. A condition must hold for N samples (debounce), a temperature or battery episode ends 5 C (points) below its limit (hysteresis), one alert is shown at most once per episode and never twice within `repeat_minutes`, and no two messages of any kind come within a minute. Severity is `info` or `warning`; there is deliberately no critical urgency, which a desktop never dismisses.
- **Wiring** (`ts-daemon::daemon::alerts`): on the existing tick the daemon hands the already smoothed temperatures, the charger and the governor state to the rules (the battery level is read every fifth tick, only on battery). Each episode is logged once to the journal. Messages go through the existing `Notice` port (`ts_system::osd`, which runs `hyprctl` or `notify-send` as the desktop user) from a short-lived helper thread: a slow, failing or panicking notifier costs a journal line and never delays or breaks a tick, so fan control is unaffected. Alerts ignore `[profile] osd`; `[osd] style = "none"` silences them.
- **Settings** (`[alerts]` in the configuration, see `ts_foundation::config::AlertsConfig`): `enabled`, `cpu_limit_c`, `gpu_limit_c`, `repeat_minutes`, `debounce_samples`, `battery_low_pct`, `throttling`, `sensor_lost`. The daemon used to read its configuration once at start; for this section it checks the modification time of the machine file and of the desktop user's `~/.config/truesense/config.toml` every ten seconds and loads them again when they changed (the user's file is found through `/etc/passwd`, because the service has no `HOME`). A file that no longer parses keeps the settings in force and says so in the journal.
- **Clients**: `truesense alerts show|set` and the bus commands `alerts get|set` (`ts-api::api::alerts`) validate and write the section with `ConfigFiles::write_user`; they never talk to the daemon.

## Building

`make ci` runs rustfmt, clippy (warnings are errors) and every test. The release build is `cargo build --release -p truesense --target x86_64-unknown-linux-musl`: one static-pie file of about 6 MB.
