# TrueSense (Rust edition)

Hardware control for the Acer Predator Helios 16 (PH16-71): fans, power modes, battery, GPU mode, lighting, undervolt, Fn keys and audio helpers.

**Status: in development (0.5.0).** Every layer of the Python edition is ported and wired into one static program, with 1904 tests. It has not run on the real laptop yet and is not released or installable yet: see [docs/STATUS.md](docs/STATUS.md) for exactly what is verified. The plan is in [docs/RUST-MIGRATION.md](docs/RUST-MIGRATION.md), the map in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

The three editions of the project, each in its own repository:

| Edition | Repository | State |
|---|---|---|
| Python 0.3.x | [Order52/truesense-python](https://github.com/Order52/truesense-python) | finished, the reference for behaviour |
| Go 0.4.0 | [Order52/truesense-go](https://github.com/Order52/truesense-go) | finished port, verified on the laptop |
| Rust | this repository | ported, in verification (not released) |

The kernel module (`kernel/truesense-wmi`, C, DKMS) and the control-socket protocol are shared by all three and do not change.
