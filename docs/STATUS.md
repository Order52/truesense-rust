# Status of the Rust edition

Version 0.5.0 (development). Nothing here has been released, and nothing has run on the real laptop yet. This page says what has been verified and how, so that no claim outruns its evidence.

## What exists

Every layer of the Python edition (0.3.17) has been ported, crate by crate, and the pieces are wired into one program:

| Crate | What | Tests |
|---|---|---|
| `ts-foundation` | config, state, userstate, atomic files, sd_notify, build info, wait backoff | 77 |
| `ts-platform` | kernel-module client, firmware simulator, typed hardware features with read-back, command runner | 89 |
| `ts-ipc` | control-socket protocol, uid-checked server, client | 51 |
| `ts-decision` | fan curves, smart-fan controller and stop guards, automatic power mode, smart profiles, game rules, thermal governor, charge-once, undervolt crash guard | 167 |
| `ts-domain` | keyboard HID, lighting, per-key effects, audio, power limits, power managers, CPU and GPU undervolt, profiles, key bindings, actions | 397 |
| `ts-system` | models and probe, doctor, selftest, services, update (strict tar extractor), osd, touchpad, sensors | 282 |
| `ts-daemon` | the control loop, guarded steps, boot and shutdown, the 17-command socket table, assembly | 88 |
| `ts-api` | the application layer, the real service builder, a daemon-to-api integration suite | 288 |
| `ts-cli` | the whole command tree (ported argparse), the fast status-bar client | 93 |
| `ts-tui` | the terminal UI (ratatui), golden snapshots of every page | 148 |
| `ts-gui` | the local web window (the Go edition's front end, embedded) | 139 |
| `ts-keys` | the PREDATOR/Fn key service, login startup | 81 |
| `truesense` | the binary: dispatch by program name and subcommand | 5 |

`make ci` (rustfmt, clippy with warnings as errors, 1904 tests) passes, and so does the same job on GitHub Actions.

## How it was checked

- **Against the Python edition.** Every module was ported from the Python source with its tests. The command line's help pages (all 127) and about 400 command transcripts were compared with the Python program's own output; the keyboard's HID transcripts (about 40 scenarios) and 275 animation frames were compared byte for byte with Python.
- **Against the Go edition.** The Go port, which was verified on the laptop, was used as a second opinion for edge cases and test ideas, not copied.
- **Between the halves.** The daemon and the application layer were written separately from `daemon.py`; an integration suite starts the real daemon on a temporary socket over the firmware simulator and drives it through the real client for every command family. It found no mismatch in command names, argument names or result shapes.
- **As a program.** The static binary was run in simulation: the daemon on a temporary socket, driven by the CLI (power modes and hold, fan smart/manual/off with the confirmation rule, charge, CPU limits, game rules). The daemon shut down cleanly on SIGTERM and removed its socket.

## What has NOT been verified

- Anything on real hardware: the kernel module path, hidraw, real fans, the real keyboard, NVML, MSR undervolt, the Fn keys, audio.
- The terminal UI on a real terminal (it ran only on ratatui's test backend), and the web window in a real browser. The window's live-update stream relies on a connection upgrade that was checked only with Node's `fetch`.
- Packaging: installer, units, release pipeline (in progress).
- The animation frames use libm `sin`/`cos`/`pow`; the golden frames match on glibc, the static build uses musl's libm, which could differ in the last bit for a rare cell.

## Known differences from the Python edition

Listed per module in the commit messages and the agents' reports; the ones users may notice:

- Update checksums are always required (Python skipped them for releases before 0.3.16).
- Fan and undervolt safety checks (confirmation, danger lines) run before a request is sent.
- A saved key binding or timing outside its range is refused where Python accepted it.
- `truesense display`, `boot` and `service` without a verb run their status action instead of crashing.
