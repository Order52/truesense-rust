# Plan: TrueSense in Rust, ported from the Python version

## Goal

The same product as the Python edition (0.3.17), as one memory-safe, static program, with every safety rule of the original kept and the same on-disk and on-socket formats so the three editions can replace each other on a laptop without migration.

## Sources of truth

1. **Python edition** (`Order52/truesense-python`, tag-equivalent commit `1085027`): behaviour and file formats. Where it is silent or buggy, the question is settled by reading the code and, if needed, the hardware.
2. **Go edition** (`Order52/truesense-go`): the Python behaviour already ported, tested and verified on the real laptop (self-test 17/17). It is used to cross-check the port, never copied line by line: the Rust code follows Rust idiom.

## What does not change

- The kernel module (C, DKMS) and its ioctl interface.
- The control socket: JSON lines, `SO_PEERCRED` uid check.
- Config (`/etc/truesense/config.toml`), state files, per-user files, key maps and profile TOML formats.
- The web window's front end (HTML, JS, CSS and the command bus `POST /api/{area}/{verb}`): reused as it is, embedded in the binary.
- The GPU undervolt helper (a small C program that talks to NVML), so the main binary stays static and a driver hang cannot block the daemon.

## Safety invariants (carried over unchanged)

- Fan duty is 0 or 20 to 100. A stop needs confirmation, a cool machine and a watchdog (CPU 70 / GPU 65 degrees C).
- Fans go back to firmware Auto on any exit or panic.
- Undervolt writes are guarded by the crash-guard flags; writes are read back.
- Updates are verified against `SHA256SUMS`; the tar extractor is strict.
- The probe path is read-only.

## Design

One Cargo workspace, layers with one-way dependencies (the same layering as the Go edition, enforced by crate boundaries instead of convention):

| Crate | Content |
|---|---|
| `ts-core` | config, state, file helpers, version info, error type |
| `ts-hw` | kernel-module client, HID keyboard, sysfs, firmware simulator for tests |
| `ts-domain` | profiles, curves, keyboard looks and effects, audio, power, undervolt |
| `ts-daemon` | decision logic (auto profile, fan control, thermal watchdog), control socket |
| `ts-clients` | CLI, TUI (ratatui), web window server, Fn-key service, startup |
| `truesense` | the binary: thin `main`, dispatch |

Dependencies are kept few and audited: `libc` (or `rustix`), `serde`, `serde_json`, `toml`, `ratatui` with `crossterm`, a small HTTP server crate. No async runtime unless a measured need appears.

Unsafe code is confined to `ts-hw` (ioctl, hidraw) behind safe wrappers, each with a safety comment and tests.

## Milestones

1. **Skeleton and gates**: workspace, `cargo fmt`, `clippy -D warnings`, `cargo test`, `cargo deny`/`audit`, CI, reproducible static build (musl).
2. **Foundation and hardware layer** with the simulator; port the Python tests as the first test suite.
3. **Domain and decision layers**; same results as Python on recorded inputs.
4. **Daemon and control socket**; run side by side with the old daemon on a test socket.
5. **CLI** with the same commands and output as the Python CLI (`cli-spec`), then **TUI**.
6. **Web window** server with the existing front end; **Fn-key service**, audio watcher, startup, doctor, self-test.
7. **Packaging and release**: installer, PKGBUILD, systemd units, update path, SBOM, attestation.
8. **Verification on the laptop**, compared with the Python and Go editions: behaviour, latency, memory.

Nothing is released before milestone 8 passes on the real hardware.

## Open questions

- Native window instead of a browser window: the Rust ecosystem has options (GTK4, Qt via bindings, egui) but every one of them drops the fully static binary. Decide after the port works with the shared web front end.
