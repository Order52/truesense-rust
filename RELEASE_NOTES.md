# Release notes

Newest first. Write what changed under "## Unreleased" as you merge changes; `tools/release.sh prepare X.Y.Z --title "..."` turns it into the section of the release, and the release workflow publishes it as the GitHub release text.

## Unreleased

- Qt app on Wayland: the Lighting colour picker (Keyboard and Per-key tabs) opened at the corner of the scrolled tab, often off screen, instead of over the middle of the window (`Overlay is not defined`: those files lacked the Controls import); the Per-key template gallery no longer re-evaluates every card's picture in a binding loop each time one picture arrives; Qt's text-input warning that wrote a journal line on every focus change is silenced (QT_LOGGING_RULES still overrides). The reported "glitch that zooming fixes" was hunted on Hyprland with a new whole-window checker (`tools/qtprofile/stale.py`) and not reproduced; see docs/QT-APP.md, Rendering.
- The keyboard no longer snaps back to a lighting theme: a theme the person replaced (from the app, the command line or a key) is now over for good; before, its record stayed "running" and every start of the key service (each login, reboot or upgrade) drew it again over the chosen look. A theme stopped by a fault is still tried again at the next start.
- The keyboard's own keys work while a theme or a per-key animation runs: every frame sent the old brightness, so Fn+F11/F12 were undone within 66 ms and the next read found only that old level. A key the keyboard handles itself now makes the lighting loops follow its brightness at once (dark included). The key service logs such keys as "handled by the keyboard itself".
- The PREDATOR key answers at once: with no double or triple tap bound (the default) a tap runs as the key comes up instead of waiting the 450 ms multi-tap window for a second press that could mean nothing. Binding a double or triple tap brings the window back, with no restart.
- The tray icon's live refresh was never created (QML: `Cannot assign to non-existent default property`, a `Connections` declared inside `SystemTrayIcon`), so the tray menu only refreshed when opened; a source test now refuses child objects in the tray icon. A window opened by a key press or a service now gets the user's locale (`LANG`, `LC_*`) from the session as well, instead of the C locale.
- The PREDATOR key opens the app again on a desktop that starts its session after the key service (Hyprland, and any compositor that is not a systemd-aware login manager): the service has run since boot with no display in its environment, so the window aborted inside Qt and left a core dump. `exec:` actions and `truesense gui` now find the live session (the user manager's environment, then the sockets in `$XDG_RUNTIME_DIR`), `truesense gui` says so plainly when there is no display at all, and an open window is brought to the front by asking the compositor (Hyprland, Sway) instead of relying on its activation settings. A command that a key started and that then fails is logged by the key service.
- A lighting theme started at login no longer dies when the keyboard's access rights arrive a few seconds after the key service: "no permission" and "interface not found" are waited for (one try a second, up to a minute) before the theme gives way; every other error still stops it at once.
- Home: the Fans card always highlighted "Manual" whatever the fans were doing (the Cooling page was right); it now shows what the machine reports (Smart, Auto, Max or Manual). Found by opening the app on Hyprland; a source test now refuses any `shown...` value that is a constant.
- The update check no longer trusts an answer cached for another repository: after moving from the Go or Python edition, `truesense update check` could say "newest 0.4.0" for a day from the old repository's cache.

## 0.5.0 (2026-11-08): TrueSense in Rust, with a native Qt app

TrueSense is now written in Rust. It replaces the Python (0.3.x) and Go (0.4.x) editions in place, with the same commands, files, control socket and kernel module, and adds a native Qt 6 window.

**One static program, much lighter.** `truesense` (daemon, command line, terminal UI, Fn-key and audio services) is one static file. Measured on the Acer Predator Helios 16 (PH16-71): the daemon uses about 0.4 % of a core and 6 MB, the key service and the audio watcher are close to zero (the Go edition: about 0.5 % and 14 MB for the daemon alone; the three services together 16 MB against 45 MB).

**A native app.** `truesense-qt` is a Qt 6 / QML window with 11 pages: Home, Monitor (CPU, GPU, memory, network, disks, temperatures), Performance (Smart auto profile editor, game rules, power limits, CPU and GPU undervolt), Cooling (fan curve editor, live dials), Lighting, Keys, Battery and screen, Audio, Profiles and favourites, Health and Settings, plus a tray menu and a first-run wizard. It draws with the software renderer by default (about 100 MB, never wakes the discrete GPU) and is built to stay idle when nothing changes: on Hyprland the Cooling page went from about 2 to 3 % CPU and 220 wake-ups a second to under 1 % and 16 (no script canvas, no endless animation; `TRUESENSE_QT_RENDER=opengl|vulkan` chooses another renderer). English only for now.

**Lighting.**
- Six wave directions, including clockwise and counter-clockwise (`truesense keyboard effect wave --direction clockwise`, `truesense keyboard fx wave-clockwise`), and software sweeps for per-key looks.
- A gallery of the 32 themes with live previews and colourways, a gallery of your looks, and eight styled keyboard templates (FPS, MOBA, typing, media, sunset, neon grid, rainbow rows, night).
- Per-power-mode lighting and fan curves ("mode bindings"), applied by the daemon even with the window closed.

**More.**
- Favourites: star looks, themes, fan curves, Smart profiles and whole-setup profiles; PREDATOR-key actions `favourite:N` and `favourite-next`.
- Temperature and battery alerts from the daemon (never urgent-level), `truesense alerts ...`.
- Monitoring data, battery health and a capabilities report (controls this laptop lacks are greyed with a reason).
- The daemon's GPU stream stops while the GPU sleeps; a real GPU reading or 0 is shown, never the CPU's.
- The native window is installable and releasable: `tools/install.sh` installs it next to the static program and keeps the previous one for rollback; releases attach `truesense-qt-0.5.0-linux-x86_64.tar.gz`, which `get.sh` verifies and adds; the Arch package ships it. `truesense gui` falls back to the built-in web window when it is not installed.

**Safer.** Crash, kill, corrupt-file, full-disk, clock-jump and suspend scenarios are tested; a leftover socket of a dead daemon no longer blocks the fan failsafe; a second daemon refuses to start; the installer replaces a loaded kernel module safely (stops the service first) and checks the loaded build; updates are verified against `SHA256SUMS`.

**Builds and licence.** Releases are built by CI from the tag: the bundle is reproducible (the program is built twice and compared), comes with `SHA256SUMS`, a signed build provenance attestation and a CycloneDX SBOM, plus an Arch package built with cargo. The whole repository is GPL-3.0-only (the effects engine and themes are derived from predator-sense by cleyton1986, used with the author's permission; the kernel module keeps its dual MIT / GPL-2.0-only header).

**Install.** `curl -fsSL https://raw.githubusercontent.com/Order52/truesense-rust/main/tools/get.sh | sudo bash`, or `sudo tools/install.sh` from a checkout. The installer removes an older edition's files only after the new service has started. The Go and Python editions' updaters do not know this repository: move over by hand once.
