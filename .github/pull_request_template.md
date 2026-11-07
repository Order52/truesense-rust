## What and why

## How it was checked
- [ ] `make ci` passes (rustfmt, clippy with warnings as errors, every test; new behaviour has a test)
- [ ] Anything that writes to the hardware was only exercised against the simulator (`TRUESENSE_SIM=1`), or I say below what I ran on real hardware
- [ ] `RELEASE_NOTES.md` has a line under "## Unreleased" if users will notice the change
