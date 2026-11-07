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

## Building

`make ci` runs rustfmt, clippy (warnings are errors) and every test. The release build is `cargo build --release -p truesense --target x86_64-unknown-linux-musl`: one static-pie file of about 6 MB.
