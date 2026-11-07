# Plan: TrueSense in Rust, ported from the Python version

## Goal

The same product as the Python edition (0.3.17), as version 0.5.0 of the Rust edition, as one memory-safe, static program, with every safety rule of the original kept and the same on-disk and on-socket formats so the three editions can replace each other on a laptop without migration.

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

One Cargo workspace (edition 2024, MSRV 1.85). Crates are the layers of the Go edition's architecture; dependencies point **down** this table and Cargo refuses a cycle, so the layering is enforced by the compiler, not by convention. Inside a crate each Go package becomes one module.

| Layer | Crate | Modules |
|---|---|---|
| Entry | `truesense` (binary) | thin `main`: dispatch by subcommand and by `argv[0]` (`truesense-fast`) |
| Clients | `ts-cli`, `ts-tui`, `ts-gui`, `ts-keys`, `ts-api` | `cli` `fast` / `tui` / `gui` / `fnkeys` `startup` / `api` `adapt` (the application layer clients share) |
| Daemon | `ts-daemon`, `ts-ipc` | `daemon` `assembly` / `wire` `server` `client` |
| Decisions | `ts-decision` | `autoprofile` `fanctl` `curves` `smartprofile` `thermal` `chargefull` `crashguard` `gamerules`: pure functions and state machines |
| Domain | `ts-domain` | `keyboard` `looks` `keyprofiles` `designs` `perkeyfx` `keymap` `audio` `power` `powermgr` `undervolt` `gpuundervolt` `profiles` `keybinds` `actions` |
| System | `ts-system` | `models` `doctor` `selftest` `services` `update` `osd` `touchpad` `sensors` |
| Platform | `ts-platform` | `firmware` (kernel-module client and simulator), `hw` (typed features with read-back) |
| Foundation | `ts-foundation` | `config` `state` `userstate` `fsx` `sdnotify` `buildinfo` |

Dependencies are few and pinned in the root `Cargo.toml`: `serde`, `serde_json`, `toml`, `libc`, `sha2`, `flate2` (pure-Rust backend), `ratatui` with `crossterm`, `tiny_http`. No async runtime, no `anyhow`: each crate has its own error enum. Adding a dependency needs a reason written in the pull request.

Conventions:

- `unsafe` only where the kernel needs it (ioctl and hidraw in `ts-platform`, `SO_PEERCRED` and `sd_notify` socket calls in `ts-ipc` and `ts-foundation`), each block with a `// SAFETY:` comment and a test; everything else is `#![forbid(unsafe_code)]`.
- Clippy at `deny` for `all`, `unwrap_used`, `panic`, `todo`, `dbg_macro`; rustfmt; `missing_docs` on public items. Panics never cross the daemon's request or tick boundary (`catch_unwind` at those two places, fans back to Auto).
- Every external effect (files, processes, sockets, the clock) is a trait or an injected closure; logic never reads the real machine in a test.
- Tests live next to the code, are table-driven and hermetic; parsers of untrusted input (socket requests, firmware replies, archives, checksum lists, profiles) get fuzz targets (`cargo fuzz`) and a seed corpus.
- Binary: `x86_64-unknown-linux-musl`, `lto = "fat"`, `codegen-units = 1`, `strip`, reproducible (`--remap-path-prefix`, `SOURCE_DATE_EPOCH`).

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
