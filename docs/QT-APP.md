# The Qt app: shell, page contract and build

`crates/ts-qt` (binary `truesense-qt`) is the TrueSense window, Qt 6 and QML through cxx-qt 0.10.0.
`crates/ts-ui-model` is everything that is not Qt (command bus, live store, write coalescing,
formatting, routes, capabilities, settings, single instance). Design: `docs/APP-DESIGN.md`.

## Build and test

```
make qt-build      # QMAKE=/usr/bin/qmake6 cargo build -p ts-qt      -> target/debug/truesense-qt
make qt-release    # release build
make qt-test       # cargo test -p ts-qt (offscreen, end to end) + QML component tests (qmltestrunner)
make qt-lint       # rustfmt --check and clippy -D warnings on ts-qt
make qt-ci         # lint + test
make ci            # rustfmt, clippy, tests, rustdoc of the workspace WITHOUT ts-qt (default-members), no Qt needed
```

Run it offscreen for a look: `QT_QPA_PLATFORM=offscreen TRUESENSE_SIM=1 TRUESENSE_QT_TEST=1
TRUESENSE_QT_EXIT_AFTER_MS=2000 TRUESENSE_QT_SHOT=/path/shot.png truesense-qt --page settings`
(with `HOME` and `XDG_RUNTIME_DIR` pointing at a temporary directory, and `DBUS_SESSION_BUS_ADDRESS`
at nothing, so nothing touches the real desktop). Never run the GUI without
`QT_QPA_PLATFORM=offscreen` on a machine where somebody works.

## Installing and releasing

`truesense-qt` is an optional, dynamic program next to the static `truesense`: `truesense gui` hands over to it when it is installed (and opens the web window when it is not). `tools/install.sh` finds or builds it (`TRUESENSE_QT_BIN`, `bin/truesense-qt`, `target/release/truesense-qt` from `make qt-release`, else a build from source as your user), refuses a stale one, installs the Qt 6 run-time packages, swaps it in with a `.prev` rollback copy and installs the launcher entry and the About-box texts; see docs/INSTALL.md ("The Qt window"). The Arch package builds and ships it (it depends on `qt6-base`, `qt6-declarative`, `qt6-svg`), the release workflow attaches it as `truesense-qt-X.Y.Z-linux-x86_64.tar.gz`, and CI runs `make qt-ci` in an Arch container (docs/RELEASING.md). The shell tests that cover this are `tools/shelltest/test_sandbox_qt.sh` (installer and uninstaller in the sandbox), and the Qt cases of `test_installer.sh`, `test_packaging.sh` and `test_get.sh`.

## What a page agent writes

1. **The QML file** `crates/ts-qt/qml/pages/<Name>Page.qml` (the placeholder exists already; replace
   it). The page is listed in `qml/pages/registry.json` (id, group, icon, title id and English title,
   aliases); adding a *new* page means one line there plus the file. `build.rs` finds every
   `qml/**/*.qml` itself, so no Rust or build file is edited.
2. **Root element**: `PageFrame { title: Nav.titleOf(Nav.currentPage) ... }` containing `Card`s.
   Pages are created when shown and destroyed when left: keep state that must survive in Rust or in
   `Ui`/`Live`, not in the page.
3. **Logic worth testing** goes into `crates/ts-ui-model/src/vm/<page>.rs`:
   `pub fn call(verb: &str, args: &serde_json::Value) -> Result<serde_json::Value, String>`, pure
   (no I/O, no clock). `build.rs` of ts-ui-model finds the file. QML calls
   `JSON.parse(Bus.vm("<page>", "<verb>", JSON.stringify(args)))` which gives
   `{ok: true, result}` or `{ok: false, error}`. `vm/fmt.rs` is the example. Only a page that needs
   its own `QObject` adds `crates/ts-qt/src/models/<page>.rs` with a `#[cxx_qt::bridge]` (found by
   `build.rs`, registered in the same QML module).

### Commands (the contract is `crates/ts-gui/src/gui/COMMANDS.md`)

```qml
Api.call("power", "set", { mode: "turbo" }, function (result) { ... })   // one-shot action
Api.call("fan", "off", { confirm: true }, ok, function (err) { ... })     // fail() replaces the toast
Api.write("fan.cpu", "fan", "set", { cpu: v })                            // slider: newest value wins
```

* Callbacks get the decoded JSON result, or `err = {message, code}`.
* Without a `fail` callback an error is shown as a toast with `message` **verbatim** (COMMANDS.md:
  it is written for a person). Codes: `needs-daemon`, `no-permission`, `unknown-command`, `timeout`,
  `bad-args`, `internal` (and `superseded`, silently dropped, for a write a newer one replaced).
  `Api.needsDaemon(err)` is the test for the first.
* `Api.write(key, ...)`: one writer thread does writes in order; a newer write for the same key
  replaces one that has not started. Use it for everything a drag, a key repeat or a colour picker can
  issue many times. `Api.call` is for reads and buttons.
* Dangerous commands (fan stop, undervolt, limits) are refused below the UI without `confirm`
  (and `force`); ask first with `ConfirmDialog`, then send `confirm: true`.
* Refresh code must not send commands: set `current`/`checked`/`value` from `Live`; the components
  emit their signals only for the person's own input.

### Live data

`Live.frame` (the newest frame of COMMANDS.md "The live stream"), `Live.value("sensors.cpu_temp")`
(undefined when absent), `Live.history("sensors.cpu_temp")` (up to 120 samples, `null` = gap),
`Live.serial`, `Live.ok`, `Live.daemonUp`. Ticks about once a second **only while the window is
visible**; a hidden or minimised window polls nothing. Tracked series are `DEFAULT_SERIES` in
`ts-ui-model/src/live.rs`; add a path there if a page needs another history.

### Capabilities, errors, confirmations, settings

* A control the laptop lacks stays visible and greyed: `enabled: Caps.usable("rear-light")` and a
  `CapabilityNote { capability: "rear-light"; text: qsTrId("...") }` under it. Ids are in
  `ts-ui-model/src/capability.rs` (`capability::id`), derived from `status get`; the answer of
  `capabilities get` (the application layer's capability probe) overrides them.
* `Toasts.show(text, "info"|"ok"|"error")`; `ConfirmDialog { id: confirm }` and
  `confirm.ask({title, text, confirmText, danger, typeWord}, function () { ... })`.
* `Ui.theme`, `Ui.fontScale`, `Ui.reducedMotion`, `Ui.rtl`; change them with `Ui.set(key, value)`
  (saved in `~/.config/truesense/ui.json`). Navigation: `Nav.go("cooling")`.
* Colours, sizes, fonts: **only** `Theme.*` tokens (`Theme.qml` is the one place with colour
  literals). Respect `Theme.animMs` for animations (0 with reduced motion). No timer or animation may run
  while the page is not shown.

### Translations

Every user string: `//% "English text"` on the line **before** `qsTrId("page_what")` (ids are
`<page>_<what>`). Run `crates/ts-qt/tools/i18n-update.sh` (lupdate; fills `en.ts`, adds unfinished
entries to `ar.ts`) and commit `i18n/*.ts`. `tests/i18n.rs` fails when an id has no English entry or a
catalogue keeps a dead id. Arabic is a stub: untranslated ids show their English text. Right-to-left
is automatic (`LayoutMirroring` from `Ui.rtl`); charts and the keyboard preview must not mirror
(`LayoutMirroring.enabled: false` inside them, as `Sparkline` does). `language: "pseudo"` in ui.json
shows every translated string accented and padded: whatever stays plain bypasses `qsTrId`.

## Components (`qml/components`, all `import org.truesense.app`)

| Component | Use |
|---|---|
| `PageFrame { title }` | scrolling, centred column (max 920 px); children are its content |
| `Card { title; icon; rightText }` | rounded panel with header |
| `SectionHeader { icon; title; rightText }` | heading row |
| `TileRow { model: [{value,label,icon,enabled?}]; current; variant: "filled"\|"outlined"; onActivated }` | exclusive icon tiles, wrap when narrow |
| `Segmented { model: [{value,label}]; current; onActivated }` | pill of choices |
| `ToggleRow { label; sub; checked; onToggled(bool) }` | switch, signal only on user action |
| `SliderRow { label; from; to; stepSize; value; unit; valueText; coalesceMs; onEdited(v); onReleased(v) }` | `edited` throttled during a drag (first move at once, then the newest every `coalesceMs`), final value guaranteed |
| `SelectRow { label; options: ["a", {value,text}]; current; onActivated(value) }` | drop-down |
| `SwatchRow { colors; current; allowCustom; onActivated(color); onCustomRequested }` | colour swatches |
| `SettingRow { label; sub }` | the row layout the others share; put your own controls inside |
| `HelpText` | muted explanation paragraph |
| `PrimaryButton`, `SecondaryButton`, `AppButton { kind: "danger"; busy; iconName }` | buttons (`busy` disables and shows ...) |
| `NavGrid { model: [{id,title,icon}]; onNavigate(id) }` | grid of page buttons |
| `Sparkline { values; minimum; maximum; filled; dot }` | line chart, null = gap, repaints on change only |
| `Gauge { value; from; to; warnAt; critAt; text; caption }` | 270 degree dial |
| `ConfirmDialog` | modal question, Cancel focused, `typeWord` for the danger zone |
| `CapabilityNote { capability; text }` | "not available on this model" line, hidden while usable |
| `Icon { name; size; color }` | tintable SVG from `qml/icons/<name>.svg` (24x24, `stroke="currentColor"`) |
| `Placeholder` | the "coming in the next step" page body |
| `StatusLine`, `ToastHost`, `Sidebar`, `SidebarItem`, `NarrowMenu` | shell parts, not for pages |

Singletons: `Theme`, `Ui`, `Live`, `Caps`, `Nav`, `Api`, `Toasts`, `Fmt` (formatting: `Fmt.temp(v)`,
`rpm`, `percent`, `watts`, `signed`, `duration`, `bytes`, `capitalize`; the twin of
`ts-ui-model/src/format.rs`, both tested against `ts-ui-model/tests/format-vectors.json`).
Only `Main.qml` mentions the Rust `Bus`; everything else goes through these singletons, so
component tests run without Rust (`crates/ts-qt/tests/qml`, `tools/qml-tests.sh`).

## Shell behaviour

Sidebar with 5 groups and 11 pages (Now: Home, Monitor; Control: Performance, Cooling, Lighting,
Keys; Hardware: Battery and screen, Audio; Library: Profiles and favourites; System: Health, Settings).
Width >= 720 px: titles; 520 to 720: icon rail; below 520: a menu button and a header with the page
title. Ctrl+1..9 jump to pages, Ctrl+Q quits. Window size and position are kept in `ui.json`
(atomic write; an unreadable file is reported and never overwritten). Closing hides to the tray when
the desktop has one and `minimise_to_tray` is on, otherwise ends the program. One window per user:
`truesense-qt [--page NAME]` sends `show NAME` over `$XDG_RUNTIME_DIR/truesense-qt.sock` (0600; the
lock file `truesense-qt.lock` decides staleness) to the open window, which navigates and raises
itself, and exits 0. Window class and Wayland app id: `truesense` (`TRUESENSE_APP_ID` for tests).
