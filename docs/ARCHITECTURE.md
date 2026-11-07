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
 ts-domain  ── what the hardware can do: keyboard, looks, themes, per-key effects, audio, power limits,
               undervolt, profiles, key bindings, favourites, actions     (lighting engine: ts-fx)
 ts-system  ── the machine around it: models, doctor, selftest, services, update, osd, touchpad,
               sensors, battery, monitor sources (usage, disks, network, nvidia)
 ts-decision ── pure decisions: fan curves, smart fan, auto profile, smart profiles, game rules,
               thermal governor, alerts, mode bindings, crash guard
        ▲
 ts-daemon (root, truesense.service) ── loop every 1–2 s, decisions from ts-decision
        └ control socket /run/truesense/truesense.sock  (JSON lines, SO_PEERCRED uid check)
                ▲
 ts-ipc::client ── one request, typed errors
        ▲
 ts-api::api::Api ── what every client shares: daemon first, safe hardware fallback; capabilities,
                     monitor, favourites, themes, mode bindings, alerts
        ▲
 ts-cli · ts-tui · ts-gui (the command bus the apps use, and the fallback web window) · ts-keys (hidraw key service)
        ▲
 ts-ui-model ── everything of the app that is not Qt: bus, live store, view-model per page, limits
        ▲
 ts-qt ── the QML skin, the program truesense-qt (the only crate that needs Qt 6)
```

`truesense` (the binary crate) dispatches to the clients by subcommand and by `argv[0]`. `docs/CODE-AUDIT.md` has the crate-by-crate inventory and the dependency check; `docs/QT-APP.md` the app.

Rules that keep it safe:

- Dependencies point down the crate list; Cargo refuses a cycle. Every crate except `ts-qt` is Qt-free, and the default workspace members exclude `ts-qt` so `make ci` needs no Qt.
- Decisions (`ts-decision`) are pure: readings in, a duty or a mode out, with an injected clock.
- Every effect (files, processes, sockets, time) sits behind a trait and is replaced by a fake in tests.
- A limit is written down once, in the layer that enforces it; the layers above read it (for example the Performance and Cooling pages read the ranges and the `risk` bands of `ts-decision` and `ts-domain`).
- A fan duty is 0 or 20–100; stopping a fan needs the daemon, an explicit confirmation, a cool machine and the temperature watchdog; the fans return to firmware Auto on every exit path.
- `unsafe` is denied by a workspace lint and allowed only in the nine modules that need it (ioctl, hidraw poll, `SO_PEERCRED`, signals, `flock`, `access`, inotify, the cxx-qt bridge), each with a `SAFETY` comment and a test.

## Alerts

The root daemon, not a window, tells the desktop user when something needs attention, so it works with every window closed.

- **Rules** (`ts-decision::alerts`, pure, injected clock): CPU or GPU at or above a limit (default 90 C), battery low on battery, the thermal governor holding the CPU clock down (thermal throttling suspected), the CPU temperature unreadable. A condition must hold for N samples (debounce), a temperature or battery episode ends 5 C (points) below its limit (hysteresis), one alert is shown at most once per episode and never twice within `repeat_minutes`, and no two messages of any kind come within a minute. Severity is `info` or `warning`; there is deliberately no critical urgency, which a desktop never dismisses.
- **Wiring** (`ts-daemon::daemon::alerts`): on the existing tick the daemon hands the already smoothed temperatures, the charger and the governor state to the rules (the battery level is read every fifth tick, only on battery). Each episode is logged once to the journal. Messages go through the existing `Notice` port (`ts_system::osd`, which runs `hyprctl` or `notify-send` as the desktop user) from a short-lived helper thread: a slow, failing or panicking notifier costs a journal line and never delays or breaks a tick, so fan control is unaffected. Alerts ignore `[profile] osd`; `[osd] style = "none"` silences them.
- **Settings** (`[alerts]` in the configuration, see `ts_foundation::config::AlertsConfig`): `enabled`, `cpu_limit_c`, `gpu_limit_c`, `repeat_minutes`, `debounce_samples`, `battery_low_pct`, `throttling`, `sensor_lost`. The daemon used to read its configuration once at start; for this section it checks the modification time of the machine file and of the desktop user's `~/.config/truesense/config.toml` every ten seconds and loads them again when they changed (the user's file is found through `/etc/passwd`, because the service has no `HOME`). A file that no longer parses keeps the settings in force and says so in the journal.
- **Clients**: `truesense alerts show|set` and the bus commands `alerts get|set` (`ts-api::api::alerts`) validate and write the section with `ConfigFiles::write_user`; they never talk to the daemon.
## Mode bindings: a look and a fan curve per power mode

Each of the five power modes (eco, quiet, balanced, performance, turbo) may be bound to a lighting look (keyboard and rear bar) and to a fan curve. The daemon applies them when the power mode changes.

**Where the data lives.** In the daemon's state file, key `mode_bindings`, as self-contained copies (`ts_decision::bindings`):

```
{"turbo": {"look": {"name": "red", "data": <look object>}, "fan_curve": {"name": "loud", "data": <curve object>}}}
```

The daemon (root) cannot read the looks and curves in a user's home directory, and a binding has to work before anyone logs in, so the API resolves the named look and curve, validates them and sends the data (the boot look and `fan_curve_data` follow the same rule). The name is only a label; saving a look or a curve that a binding names sends the copy again (`Api::refresh_binding_copies`, called by `save_look*` and `save_fan_curve*`), otherwise the daemon's copy would go stale. A mode without a binding is absent from the map.

**Who applies it.** The daemon applies both parts. A look goes through the same port as the boot look (`BootLookPort::apply`), which already runs as root at start and after every resume: the keyboard (HID) and the rear bar (firmware) are written without the user's session, so no key-service listener is needed for static looks. A curve becomes the active curve the way `fan_curve use` with data does it. What only a user process can do, animated per-key themes (software frames), will be started by the key service from the published mode; `Look` has no such part yet.

**When.** Once, on the change of the power mode, whatever causes it (Smart auto, `power set`, the PREDATOR key, a game rule): the control loop compares the mode it reads with the mode it saw on the previous tick (`ts-daemon/src/daemon/bindings.rs`, called right after the automatic mode has decided). Not on a timer, not in a loop, and not for the first mode seen after the daemon starts. A failed apply is logged and not retried; the next change of mode tries again. A binding applied for a mode also tells `operating_follow` that the mode is handled, so the look is not repainted; a look that failed leaves the mode to `operating_follow`.

**Fan rules.** A mode change never ends a manual fan setting and never switches Smart on. The pure function `ts_decision::bindings::edge(prev, now, fan mode, bindings, look parser)` returns the actions: the look is always applied; the curve only while the fans are in Smart mode (`FanMode::Smart`); with firmware Auto or a manual duty, Max or stop (`FanMode::Firmware`, `FanMode::Manual`) the curve is reported as skipped. A binding never stops a fan: a curve is validated by `curves::validate` (the rules of `fan curve use`, 100 % by the throttle temperatures), a duty under 20 % is firmware Auto in the Smart follower and never a stop, and the native curve (firmware control) cannot be bound. Nothing that fails validation is applied; a damaged copy skips only its own part.

**Commands.** Daemon socket `mode_bindings` with `action` = `get` | `set {mode, look?, fan_curve?}` | `clear {mode, what?}` | `apply_now {mode?}` | `refresh {kind, name, data}`. API: `Api::{mode_bindings, bind_mode, unbind_mode, apply_mode_binding}`. Window bus: `mode bindings|bind|unbind|apply` (`crates/ts-gui/src/gui/COMMANDS.md`). Command line: `truesense mode bindings|bind MODE --look NAME --curve NAME|unbind MODE [--look] [--curve]|apply [MODE]`.

**Limits.** The daemon's own memory of the keyboard look (`last`) is the root user's, not the desktop user's: a look applied by a binding is not what `look save` or the login restore sees as "what is on the lights". The same holds for the boot look.

## Building

`make ci` runs rustfmt, clippy (warnings are errors), every test and rustdoc (warnings are errors); `make qt-ci` does the same for the Qt app (needs Qt 6). The release build is `cargo build --release -p truesense --target x86_64-unknown-linux-musl`: one static-pie file of about 6 MB.
