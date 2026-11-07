# TrueSense (Rust edition)

Hardware control for the Acer Predator Helios 16 (PH16-71): fans, power modes, battery, GPU mode, lighting, undervolt, Fn keys and audio helpers.

**Status: just started.** This repository is the Rust port of TrueSense. The plan is in [docs/PLAN.md](docs/PLAN.md); nothing here is released or installable yet.

The three editions of the project, each in its own repository:

| Edition | Repository | State |
|---|---|---|
| Python 0.3.x | [Order52/truesense-python](https://github.com/Order52/truesense-python) | finished, the reference for behaviour |
| Go 0.4.0 | [Order52/truesense-go](https://github.com/Order52/truesense-go) | finished port, verified on the laptop |
| Rust | this repository | not yet started (plan only) |

The kernel module (`kernel/truesense-wmi`, C, DKMS) and the control-socket protocol are shared by all three and do not change.
