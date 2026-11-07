# Status of the Rust edition

Version 0.5.0 (released 2026-11-08 from Order52/truesense-rust). This page says what has been verified and how, so that no claim outruns its evidence. Last updated at the release, after the install of the final build on the PH16-71.

## What exists

Every layer of the Python edition (0.3.17) is ported, and the Rust edition adds what the reference study asked for (see [APP-DESIGN.md](APP-DESIGN.md)). Crates, by layer:

| Crate | What |
|---|---|
| `ts-foundation` | config, state, userstate, atomic files, sd_notify, build info, wait backoff |
| `ts-platform` | kernel-module client, firmware simulator, typed hardware features with read-back, command runner |
| `ts-ipc` | control-socket protocol, uid-checked server, client |
| `ts-decision` | fan curves, smart-fan controller and stop guards, automatic power mode, smart profiles, game rules, thermal governor, charge-once, undervolt crash guard, per-mode bindings, alerts |
| `ts-fx` | the lighting effects engine and the 32 themes (GPL-3.0-derived, see [THIRD_PARTY.md](../THIRD_PARTY.md)) |
| `ts-domain` | keyboard HID, lighting looks, themes, per-key effects, audio, power limits, power managers, CPU and GPU undervolt, profiles, key bindings, favourites, actions |
| `ts-system` | models and probe, doctor, selftest, services, update (strict tar extractor), osd, touchpad, sensors, battery, monitor sources |
| `ts-daemon` | the control loop, guarded steps, boot and shutdown, the socket command table, alerts, mode bindings, GPU stream policy |
| `ts-api` | the application layer, capabilities, monitor, the real service builder, a daemon-to-api integration suite |
| `ts-cli` | the whole command tree (ported argparse), the fast status-bar client |
| `ts-tui` | the terminal UI (ratatui), golden snapshots of every page |
| `ts-gui` | the command bus the apps use, and the old local web window (fallback only) |
| `ts-keys` | the PREDATOR/Fn key service, theme renderer, login startup |
| `ts-ui-model` | the Qt-free logic of the app: bus, live store, write coalescing, view-models per page (they read their limits from `ts-decision`, `ts-domain` and `ts-foundation`, see [CODE-AUDIT.md](CODE-AUDIT.md)) |
| `ts-qt` | the Qt 6 / QML app `truesense-qt`: 11 pages, tray, first-run wizard, English catalogue and the `qsTrId` plumbing (Arabic was dropped) |
| `truesense` | the static binary: daemon, CLI, TUI, key service, dispatch by program name |

Gates: `make ci` (rustfmt, clippy with warnings as errors, about 3,000 tests, rustdoc with warnings as errors; the workspace without Qt) and `make qt-ci` (the Qt crates, offscreen, plus about 100 QML component tests). Both pass, and `make ci` also runs on GitHub Actions.

## How it was checked

- **Against the Python edition.** Every module was ported from the Python source with its tests. The command line's help pages and about 400 command transcripts were compared with the Python program's own output; the keyboard's HID transcripts and 275 animation frames were compared byte for byte.
- **Against the Go edition and the reference project.** The Go port (verified on the laptop) and the reference project's PH16-71 findings were used as second opinions: see [PH16-71-NOTES.md](PH16-71-NOTES.md).
- **Between the halves.** An integration suite starts the real daemon on a temporary socket over the firmware simulator and drives it through the real client for every command family.
- **Resilience.** Crash, kill, corrupt-file, full-disk, clock-jump, suspend and upgrade scenarios are tested in simulation: see [RESILIENCE.md](RESILIENCE.md).
- **On the real laptop (2026-11-07).**
  - Read-only: `probe` (0 writes), `sensors`, `doctor` agree with the hardware.
  - Installed with the installer: kernel module 0.5.0 built for both installed kernels and loaded, the daemon, the key and audio services and the units running, `truesense service check` all good.
  - The daemon restored the existing settings (smart fan, auto profile) and the boot lighting.
  - `truesense selftest --skip-fans`: 16 of 16 passed (every other firmware setting, audio, keyboard, daemon pause/restore, doctor).
  - Resident memory: daemon about 6 MB (2 MB private), key service 1.5 MB, audio watcher 3.3 MB.
  - The Qt app on the real display (Hyprland, tiled, 2560x1600 at 165 Hz): opens on the Cooling page with one instance, about 1 % of a core, 16 wake-ups a second and 106 MB (PSS) idle with the window open.

## What has NOT been verified on real hardware

- The fan phase of the self-test (it needs a cool machine), real fan stops with the watchdog, and the crash failsafe (`ExecStopPost`) after a daemon kill.
- Real PREDATOR / Fn key presses with the Rust key service.
- The Qt app's look on a real display beyond the one measurement above (the glitch detector found none on floating windows; tiling and zoom behaviour need the owner's eyes), the tray, and the terminal UI on a real terminal.
- Themes on the real keyboard (frame rate, flicker), per-mode bindings, alerts, the GPU stream parking and the dGPU suspending: the live checks are listed in [PH16-71-NOTES.md](PH16-71-NOTES.md) and [RESILIENCE.md](RESILIENCE.md).
- A real upgrade through the updater from a published release (the first release is this one).
- The animation frames use libm `sin`/`cos`/`pow`; the golden frames match on glibc, the static build uses musl's libm, which could differ in the last bit for a rare cell.

## Licence

The whole repository is GPL-3.0-only (decided 2026-11-07). `ts-fx` is derived from predator-sense by cleyton1986 (GPL-3.0), used with the author's permission, and the binaries link it, so the combination is one GPL-3.0 work; see [THIRD_PARTY.md](../THIRD_PARTY.md). The kernel module keeps its dual MIT / GPL-2.0-only header so that the Linux kernel accepts it.

## Known differences from the Python edition

- Update checksums are always required (Python skipped them for releases before 0.3.16).
- Fan and undervolt safety checks (confirmation, danger lines) run before a request is sent.
- A saved key binding or timing outside its range is refused where Python accepted it.
- The GPU temperature shown is a real GPU reading or 0, never the CPU's; the daemon's GPU stream stops while the card sleeps.
