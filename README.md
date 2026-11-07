# TrueSense (Rust edition)

Hardware control for the Acer Predator Helios 16 (PH16-71): fans, power modes, battery, GPU mode, lighting, undervolt, Fn keys and audio helpers.

**Status: in development (0.5.0), not released.** Every layer of the Python edition is ported and wired into one static program, with about 3,000 tests, and the Qt app is built on top of it. It was installed and checked on the PH16-71 on 2026-11-07; what that covered and what is still unverified is in [docs/STATUS.md](docs/STATUS.md). The plan is in [docs/RUST-MIGRATION.md](docs/RUST-MIGRATION.md), the map in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

The three editions of the project, each in its own repository:

| Edition | Repository | State |
|---|---|---|
| Python 0.3.x | [Order52/truesense-python](https://github.com/Order52/truesense-python) | finished, the reference for behaviour |
| Go 0.4.0 | [Order52/truesense-go](https://github.com/Order52/truesense-go) | finished port, verified on the laptop |
| Rust | this repository | ported, in verification (not released) |

Two programs come out of this repository. **`truesense`** is the static one (daemon, command line, terminal UI, a built-in web window as the fallback, Fn-key and audio services): no libraries to install. **`truesense-qt`** is the native Qt 6 window (`crates/ts-qt`, see [docs/QT-APP.md](docs/QT-APP.md)): optional, dynamic, it needs Qt 6 (`qt6-base`, `qt6-declarative`, `qt6-svg`) at run time. `truesense gui` opens the Qt window when it is installed and the built-in web window when it is not, so nothing depends on Qt; how it is installed, built and released is in [docs/INSTALL.md](docs/INSTALL.md) and [docs/RELEASING.md](docs/RELEASING.md).

The kernel module (`kernel/truesense-wmi`, C, DKMS) and the control-socket protocol are shared by all three and do not change.

## Documentation

| Page | What it is |
|---|---|
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | the map of one request, the layers, alerts and mode bindings |
| [docs/CODE-AUDIT.md](docs/CODE-AUDIT.md) | crate-by-crate inventory, dependency direction, `unsafe`, what the audits found |
| [docs/STATUS.md](docs/STATUS.md) | what is verified, how, and what is not |
| [docs/QT-APP.md](docs/QT-APP.md) | the Qt app: build, page contract, components |
| [docs/INSTALL.md](docs/INSTALL.md), [docs/RELEASING.md](docs/RELEASING.md) | installing, updating, releasing |
| [docs/SAFETY.md](docs/SAFETY.md), [docs/RESILIENCE.md](docs/RESILIENCE.md) | what protects the laptop and how it recovers |
| [docs/PROTOCOL.md](docs/PROTOCOL.md), [docs/PH16-71-NOTES.md](docs/PH16-71-NOTES.md) | the kernel interface and what is known about this model |
| [docs/PERFORMANCE.md](docs/PERFORMANCE.md) | measured costs |

Developer commands: `make ci` (rustfmt, clippy, tests, rustdoc), `make qt-ci` (the Qt app), `make static-ci` (scripts, units, installer tests and the link check of these pages), `make help`.
