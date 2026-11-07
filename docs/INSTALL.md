# Installing TrueSense

Requires: an Acer Predator Helios 16 **PH16-71**, systemd, Linux with the kernel headers of your running kernel (and dkms and a C compiler, to build the small kernel module). TrueSense itself is one static program: no Python, no Qt, no runtime to install.

## One line
```
curl -fsSL https://raw.githubusercontent.com/Order52/truesense/main/tools/get.sh | sudo bash
curl -fsSL https://raw.githubusercontent.com/Order52/truesense/main/tools/get.sh | sudo bash -s -- --no-gui   # extra options
```
`get.sh` finds the newest release, downloads it, **checks it against the release's `SHA256SUMS`** (a mismatch, or a missing list, stops the install), checks that the download really is the version it was asked for, and runs `tools/install.sh`. Pin a version with `TRUESENSE_VERSION=0.5.0`, also require an exact hash with `TRUESENSE_SHA256=<hash>`, and with the GitHub CLI installed add `TRUESENSE_VERIFY_ATTESTATION=1` to check the signed build provenance too. Read the script first if you like: it is short.

## From a clone
```
tools/install.sh --check     # read-only: distribution, model, what is missing (no root)
make build                   # the static program (needs Rust and the musl target, see below); optional: the installer builds it itself when it is missing
sudo tools/install.sh        # installs missing prerequisites, then TrueSense
```
Which program the installer installs, in this order: `TRUESENSE_BIN=/path` if you set it; `bin/truesense` (what a release bundle ships); `target/x86_64-unknown-linux-musl/release/truesense` (what `make build` makes); otherwise it builds the program itself with `cargo build --release --locked`, **as the user who ran `sudo`** (never as root, so no root-owned files appear in your tree; rustup's `~/.cargo/bin` is found). The musl build needs its standard library: `rustup target add x86_64-unknown-linux-musl` (Arch's `rust` package does not ship it: install `rust-musl` or use rustup). If it is missing the installer says so and builds a statically linked glibc program instead, which is just as self-contained. **A program whose version differs from the `version` in this tree's `Cargo.toml` is refused** (a stale build: run `make build` again). The program is found, or built, before anything on your system changes.

## Options
| option | meaning |
|---|---|
| `--check` | report only, change nothing |
| `--no-deps` | never run the package manager, only report what is missing |
| `--no-gui` | accepted for compatibility: the window is part of the program and needs no extra package |
| `--with-audio` | also install the RNNoise plugin where packaged (Arch) |
| `--no-services` | install files, do not enable/start services |
| `--module-only` | only the kernel module |
| `--user=NAME` | desktop user to set up (default: who ran sudo) |
| `--prefix=DIR` | install prefix (default `/usr/local`) |
| `--force` | allow a model other than PH16-71 (unsupported) |
| `--uninstall` | remove (same as `tools/uninstall.sh [--purge]`) |

## What it does for each distribution
| family | prerequisites installed with | headers package |
|---|---|---|
| Arch, CachyOS, Manjaro, EndeavourOS | `pacman -S --needed` | `<pkgbase>-headers` of the running kernel |
| Debian, Ubuntu, Mint, Pop | `apt-get install` | `linux-headers-$(uname -r)` |
| Fedora, Nobara | `dnf install` | `kernel-devel-$(uname -r)` |
| openSUSE | `zypper install` | `kernel-default-devel` |
Anything else: it tells you exactly what to install by hand. A Rust toolchain is **not** a prerequisite (a release bundle carries the program); it is only used when you install from a source tree that has no built program.

Safety: the module is test-built in a scratch folder first (nothing changes if it does not compile); the program files are installed beside the old ones and swapped; if `truesense.service` does not come up within 15 s the previous version is restored (`truesense.prev`). Everything is logged to `/var/log/truesense-install.log`. The installer is safe to re-run.

**Replacing an older edition.** The Python edition (0.3.x) and the Go edition (0.4.x) install to the same paths (`/usr/local/bin/truesense`, the same units, `/etc/truesense`, `/var/lib/truesense`), so this installer upgrades them in place: the new program is swapped in beside the old one, and only **after the new service has started** does the installer delete what is left of the Python tree (`/usr/local/lib/truesense/truesense`, `truesense.prev`). If the new service does not start, the old program is put back and nothing of it was removed. Settings, profiles and state files are kept: the three editions read and write the same formats.

Secure Boot: DKMS modules must be signed; the installer warns if the DKMS key is not enrolled (`sudo mokutil --import /var/lib/dkms/mok.pub`, reboot).

## Updating
Updating is the same one click, and it is safe to repeat:
- **From the window**: Settings, Update (it checks once a day if you allow it; one request, no identifiers). **From a terminal**: `truesense update check`, then `sudo truesense update apply`. Or run the one-line install again.
- The updater downloads the release's published file and **verifies its SHA256 against `SHA256SUMS`** before anything runs as root, and refuses a download whose top-level `Cargo.toml` does not declare the version it was asked for. Releases are built by CI (`.github/workflows/release.yml`) from the tag, byte-for-byte reproducibly, with a signed provenance attestation: `gh attestation verify truesense-X.Y.Z.tar.gz --repo Order52/truesense`.
- The installer test-builds the kernel module **before** it changes anything, swaps the program files atomically, restarts the service, and **rolls back to the previous version** if the service does not come up. The fans are handed back to the firmware whenever the service stops.
- After a kernel update, DKMS rebuilds the module by itself; if it ever does not, the health timer rebuilds it within minutes (`truesense service repair` does it now).

## Services
| unit | what | scope |
|---|---|---|
| `truesense.service` | the daemon: fans, power modes, Smart auto (watchdog, restarts itself, gives the fans back to the firmware however it ends) | system |
| `truesense-health.timer` | every 30 s: restart a stalled daemon, rebuild a missing module once per 10 min | system |
| `truesense-keys.service`, `truesense-audio.service`, `truesense-startup.service` | Fn/PREDATOR keys, microphone and sound handling, look at login | user |
| `truesense-update.timer` | daily check for a new release: it only notifies you, once per version; nothing is installed without you | user |

`truesense service check` lists every unit, the kernel module and the control socket (read-only, exit code 1 if something is down). `truesense service repair` fixes what it finds: re-reads the unit files, rebuilds and loads the module, enables and starts what is stopped, restarts a daemon that does not answer. It is idempotent (a second run changes nothing), never touches a setting or a fan, and asks for `sudo` only for the system part. `truesense service logs` shows the journal.

## Other models
TrueSense talks to firmware, so it only writes to laptops it has been **verified** on (today: **Predator PH16-71**, BIOS V1.18; other BIOS versions of the same model are expected to work and `doctor` says when yours is not the verified one).
- **Same model, any distribution or kernel**: supported; CI builds the module weekly against current Arch, Fedora and Ubuntu kernels and opens an issue before an update can break it.
- **A similar Acer Predator/Nitro** (for example PH16-72, PH18-71, PHN16-71): **unverified**. The kernel module does not bind and the installer refuses without `--force`; nothing is written to the firmware. The safe way to help: `sudo modprobe truesense_wmi force=1` (this only lets the module load), then `truesense probe --json`. The probe **only reads**, prints the model, BIOS, which firmware read-outs answer and whether the Acer gaming WMI interface is present, and contains no serial number. Post it with the "Another Predator model" issue form. Once it matches the verified model a release adds the model to the supported list. Undo with `sudo modprobe -r truesense_wmi`.
- **`--force`** installs anyway and can misbehave (wrong fan registers, wrong modes). Use it only after reading `docs/SAFETY.md`.

## Arch package (AUR)
`packaging/arch/PKGBUILD` builds from a checkout (`cd packaging/arch && makepkg -si`); `packaging/aur/PKGBUILD.template` builds from the tagged release tarball, and the maintainer publishes it with `tools/aur-publish.sh X.Y.Z` (needs an AUR account and SSH key). After that: `yay -S truesense`. Official Arch repositories require a Trusted User/package maintainer to adopt the package; it cannot be pushed there directly.

Both build with `makedepends=('rust' 'gcc')`: `cargo build --release --locked` for `x86_64-unknown-linux-gnu` with `-C target-feature=+crt-static`. They use the host target and not musl because Arch's `rust` package ships the host standard library only; the result is the same kind of program (a static-pie executable with no libc to match), which the PKGBUILD verifies before packaging. The program uses no name lookups or `dlopen`, the things a static glibc cannot do. The release bundle's program is the musl build.

## Status
The packaging of the Rust edition is checked in CI on every push: the installer's distribution detection and package names on each family, the kernel module against each distribution's headers, the Arch package, the release bundle's reproducibility. A full install on Debian, Ubuntu, Fedora and openSUSE is **untested on real hardware**, and so is the first installation of the Rust edition itself on the laptop. Reports and fixes welcome.
