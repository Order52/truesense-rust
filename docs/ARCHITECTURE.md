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

`make ci` runs rustfmt, clippy (warnings are errors) and every test. The release build is `cargo build --release -p truesense --target x86_64-unknown-linux-musl`: one static-pie file of about 6 MB.
