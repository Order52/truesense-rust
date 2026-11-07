# TrueSense Qt app: design, reference study and reuse map

> **Update (2026-11-07):** the licence question in section 6 was decided: the whole repository is GPL-3.0-only (see `LICENSE` and `THIRD_PARTY.md`). References to "TrueSense is MIT" below describe the situation when this document was written.

Status: proposal, not yet reviewed by the user. No code was changed to write it.
Written against: truesense-rust 0.5.0 (`/home/sloppy/Documents/truesense-rust`) and the GTK4 app
`predator-sense` by cleyton1986 (clone under the session scratchpad, `ref/psense`, v0.4.0-preview in its Cargo files).

## Licence note for readers of this file (read first)

The original rule for this study was "learn, do not copy". The user has since obtained the reference author's
approval to reuse his work, so this document is a **reuse map** (section 5) as well as a design. Boundaries that stay:

1. The **Acer product photos** (`predator-sense-gui/resources/models/*.png`, 9.3 MB) are Acer's, not covered by his
   GPL grant (his README says so). Never import them.
2. Not ours to take even with his approval, because he says they come from others or they carry vendor marks:
   the Dashboard and Temperature icons (his README credits Flaticon authors), `resources/mode/*.png` (5 MB of
   "robot" artwork per mode; origin not stated, style matches Acer's), `predator-icon.svg`, `logo*.png/jpeg`
   (Predator/Acer marks), `find-serial-number.*` (an Acer label picture). Ask the author, or draw our own.
3. The reference is GPL-3.0 and TrueSense is MIT. Section 6 lists what must be decided by the user. This
   document does not decide it.
4. Quotes from the reference in this file are a few words at most; everything else is described in my words.

Verification marks used below: **[V]** = I read the code or ran a count on it; **[U]** = unverified, said honestly.
No program of the reference was run, and the network was not used.

---

## 0. Decisions in one screen

| Topic | Decision | Why (short) |
|---|---|---|
| Navigation | Sidebar (about 12 entries, collapses to a single column with a menu button below ~720 px) | Screenshots: the old 480 px column works for 10 controls, but fan curve, keyboard preview, theme gallery and live charts need width. Section 4.1. |
| Where behaviour lives | Daemon owns anything continuous (mode bindings, alerts, fan plans, boot look); user key service owns app-drawn animation; the Qt app is a remote control plus viewer | The reference put timers in its GUI process, so closing the window switched off half the app (its own finding 17). We already have a root daemon. |
| Per-mode lighting and fans | "Mode bindings", stored as self-contained copies in the daemon (like `fan_curve_data` and the boot look) | Section 4.3. |
| Favourites | New user-level list, kinds look, fan curve, Smart profile, whole-setup profile; star on every library list; Home strip; PREDATOR-key actions | The reference has no favourites. Section 2.8. |
| Themes | Import his 32-theme catalogue and effect engine as a pure crate `ts-fx`, plus user theme files in the same schema | Section 5, R1. |
| i18n | Qt `.ts` files, `qsTrId` ids, import his 8 translations by script, add Arabic and RTL | Section 4.7. |
| Packaging | Qt app is a separate dynamic binary `truesense-qt`; the main `truesense` stays static musl | Qt cannot be static here; `ts-gui` docs already say so. |
| Licence of the combined work | **Not decided here** | Section 6. |

---

## 1. What the reference does better than TrueSense today (ranked by user value)

Ranking is for a PH16-71 owner who wants to see and steer the laptop. Facts were read in the code, not taken from his README.

### 1.1 Features

1. **Live monitoring pages.** Dashboard (model, CPU, GPU, RAM, disks, network, OS, BIOS), Temperatures (six gauges: CPU, GPU,
   system, two SSDs, Wi-Fi), Usage (tabs CPU, GPU, Memory, Storage; per-core bars; top processes by CPU or memory with PID, user,
   state), Network (rates, peaks, session totals, local IP), GPU (temperature, utilisation, VRAM, power, core and memory clock,
   P-state, PCIe link, VBIOS, TGP slider), Graphs (history with min and max). TrueSense shows CPU/GPU temperature, fan RPM, load
   and watts as one sparkline row on Home and nothing else. This is the largest gap. [V: `ui/dashboard_page.rs`, `usage_page.rs`
   2076 lines, `network_page.rs`, `gpu_page.rs`, `monitor_page.rs`, `hardware/procs.rs`, `sysinfo.rs`, `storage.rs`, `gpu.rs`, `nvml.rs`]
2. **Lighting and fans per power mode, with a "Changed / Discard / Undo" editing model.** A strip "Lighting for: Quiet, Eco,
   Balanced, Performance, Turbo" and the same strip "Fans for". You edit a mode while another is active; "Show on my laptop"
   previews it; changes apply live and are saved; destructive steps offer Undo in a toast instead of a confirm dialog. [V: screenshots
   5, 7, 17; `ui/lighting/mode_select.rs`, `mode_bar.rs`, `toasts.rs`]
3. **Themes, the app-drawn effect engine, and idle blanking.** 32 built-in themes, each one look for keyboard, light bar and
   logo, with colourways and a Custom colour set, drawn at 30 frames/s on the per-key keyboard and 5 on the bar by a renderer thread
   with an ordering gate. A flash guard keeps every light under 3 flashes per second (WCAG 2.3.1). Lights go dark after idle.
   Theme looks that react to keys read the built-in keyboard (needs the `input` group; fails closed on lock screen). [V: `hardware/effects/*`,
   `lighting_renderer/*`, `key_input/*`, `idle.rs`, `blanking.rs`]
4. **Model detection and honest "not available" states.** `capabilities.rs` probes sysfs and devices, never a model list, and the
   Settings page prints a checklist "Features supported on this model" (RGB keyboard, fan read, fan control, profiles, EC controls,
   NVIDIA GPU, battery limit). The GPU page says plainly that TGP is fixed by the vBIOS when it is. [V: `capabilities.rs`, screenshots 9, 12]
5. **Battery health.** Voltage, current, power, cycles, health percentage (full over design capacity), technology, maker, model,
   temperature, charge history. TrueSense reads only percent and status. [V: `ui/battery_page.rs`; our `ts-system/src/sensors.rs` `Battery`
   has two fields]
6. **Tray, background residency, temperature alert.** Tray icon process (StatusNotifierItem), minimise on close default on, start
   hidden at login, a desktop notification when CPU or GPU passes a threshold (slider 70 to 100 C) with 5 C hysteresis and one per
   minute. Its finding 36 is a lesson we keep: do not use the "critical" urgency, a desktop never dismisses it. [V: `tray.rs`,
   `installer/src/tray.rs`, `hardware/alerts.rs`, `notify.rs`, findings 17 and 36]
7. **Nine languages** (English, Portuguese, Spanish, Chinese, Japanese, Russian, German, Italian, Turkish), 829 keys in English and in Portuguese (the other seven tables each span about 838 lines). No Arabic,
   no right-to-left handling (searched `ui/` and `i18n.rs`: none). Language change restarts the app. Coverage is uneven: the lighting
   page has 14 lookups, the usage page 39. [V: `i18n.rs` counts]
8. **Settings backup/export/import and honest failure on a broken config.** A config that does not parse is never overwritten; the
   error is logged even when debug logging is off; import validates first and copies the old file aside. [V: findings 35, `config.rs`]
9. **New tuning features we lack**: CPU temperature ceiling (TCC offset through `intel_tcc_cooling`, range read from the cooling
   device, boot restore), Eco mode (volume and brightness capped to 40 %, restored exactly), firmware profile calibration (writes each
   profile index and ranks them by measured PL1), per-profile GPU power limit, desktop power-profile indicator sync, LCD overdrive.
   [V: `temp_limit.rs`, `eco_mode.rs`, `thermal_profile.rs`, `ppd.rs`; `grep -rli tcc` finds nothing in our crates except the simulator]
10. **Tools tab** (a hub so the sidebar does not grow): GameSync UI, Macros (xdotool, X11 only), Audio EQ (10 bands through
    EasyEffects GSettings), audio-reactive lighting (`parec`), AI assistant (Ollama, opt-in), GRUB splash, Drivers and Manuals page
    (serial number and a link). Value for us: GameSync UI (our engine is better), Audio EQ idea, Drivers page. The rest: section 3.3.
11. **Installer and onboarding.** An interactive Rust installer with menu, a "setup" wizard page for the kernel module, a model
    table in the README with tested and untested marks per feature, and a guard against a dead instance that still owns the D-Bus
    name. [V: `ui/setup_page.rs`, `single_instance.rs`, README]

### 1.2 UX structure

- Reference: custom-drawn dark "gaming" chrome, **sidebar of 13** (Dashboard, Temperatures, Usage, Network, Lighting, Mode, Fan
  Control, Battery, GPU, Graphs, Tools, Drivers and Manuals, Settings) and, inside pages, tabs (Usage: 4, Lighting: Control and Themes,
  Tools: 5). It opens maximised every time. Brand colour follows the detected brand (cyan Predator, orange Nitro). [V: `window.rs`
  1447-1740, `brand_theme.rs`]
- TrueSense Python/Qt: one centred column of about 480 px, Home = controls + a 2x3 grid of page buttons, each page opens with a Back
  button; Lighting has four tab buttons. Readable, friendly, but the grid of buttons is a menu that hides what is going on. [V:
  `/home/sloppy/Documents/truesense/docs/images/home.png`, `lighting-profiles.png`, `fan-curves.png`]
- How live data is drawn in the reference: dashed circular gauges (temperatures, GPU), thin bars (per core, processes), area-filled
  history charts with min and max, a flame graphic on the CPU temperature, spinning fan graphics driven by RPM, a draggable
  six-bar step chart for fan curves, a per-key keyboard preview with glow. Static drawings are cached as pixels (`still_drawing.rs`).
  TrueSense: text values and 110x22 px sparklines. Our curve editor (drag points, per-fan, hysteresis, down-rate) is richer than his
  six bars; what we lack is the presentation.

### 1.3 Engineering practices worth copying (all [V])

| Practice | Where | Use for us |
|---|---|---|
| View-models without the toolkit: `*_model.rs` files (control, frame, themes, colour, paint, preview scene, fan control, curve geometry) with 0 GTK references and tests | `ui/lighting/*_model.rs`, `ui/fan_control_model.rs`, `fan_curve_geometry.rs` | Keep UI decisions in Qt-free Rust so they are testable (`ts-ui-model`, 4.9) |
| Timers exist only while the widget is mapped and the window is visible; minimised or covered counts as hidden | `ui/ticks.rs`, `app_state.rs` | Idle CPU ~0; our rule: no QML `Timer`/animation runs while `visible` is false |
| Config cache keyed by a generation counter bumped on each save | `config.rs` | Cheap re-read on timers; our `ConfigFiles` re-reads from disk each time, fine for CLI, wasteful for a 1 Hz UI |
| "Unreadable config is never saved over"; migration markers; tolerant per-entry reads (one bad theme-memory entry is dropped, not the file) | `config.rs`, `lighting_model.rs` | We already refuse to write over invalid TOML (`write_user`); add the visible banner and tolerant per-entry reads for new stores (5.2, R5) |
| Programmatic refresh must not fire user handlers (buttons for selection, `syncing` flag) | finding 27 | Same bug class in QML (`onToggled` vs `onCheckedChanged`); rule for page code |
| Blank flag records what hardware did, only after an `Ok` write; failed writes retry with back-off | `blanking.rs` | Rule for any state we mirror |
| Reconciler writes only on change, back-off to 30 s so a dead path never spams (a pkexec prompt every 3 s was a real bug) | finding 37 | Our daemon steps already use `warn_every`; keep the rule for bindings |
| Golden tests: frames of every effect compared byte for byte with the JS original; fixtures for config shapes (10 files incl. truncated and wrong-type) | `tests/data/*` (1.2 MB + 40 KB) | Same style as our Python-comparison tests |
| Panic hook that never prints messages on threads that see key presses | `quiet_panics.rs` | Needed if we ever import `key_input` |
| Log file rotation (5 MB x 3) and an `error_always` path for failures that disable the log setting | `applog.rs`, finding 35 | Our logs go to the journal; the lesson is "never gate an error about the config on the config" |

What the reference does **worse** and we must not copy: one flat `AppConfig` of about 100 fields (config.rs 2343 lines); global
`static Mutex`/`AtomicBool` state in many modules (hard to test, `.lock().unwrap()` everywhere); a Rust-match `i18n.rs` of 8084 lines
and 549 KB; everything privileged through a pkexec helper session owned by the GUI; the whole app in one process; `Predator PH315-54`
art shipped as a fallback.

---

## 2. How the reference handles profiles, themes and "favourites"

All paths below are under `predator-sense-gui/src` and [V] unless marked.

### 2.1 Where things are stored

- **One file**: `~/.config/predator-sense/config.json`, pretty JSON, `AppConfig` with about 100 flat fields, `serde(default)` on each so
  old files load. Written atomically (temp file plus replace, follows symlinks). Written only by the GUI process (the UI thread);
  the hotkey service reads it.
- `~/.config/predator-sense/profiles/NAME.json`: legacy lighting profiles (`LightingProfile`: RGB config plus static zones). One file
  per name, name sanitised, written with a plain `fs::write` (not atomic).
- `~/.config/predator-sense/macros/NAME.json`: macros, no trigger field by design.
- `~/.config/predator-sense/mode_key.json`: optional override for the mode-key HID report.
- Privileged side: a helper session (pkexec) and the hotkey service; thermal profile calibration and TCC ceiling have their own
  records so boot can restore them.

### 2.2 Named lighting presets and per-mode lighting (the closest thing to profiles)

- `lighting_modes`: map from the five power modes to `ModeLighting { preset: Option<name>, changed: bool, setup, theme_memory }`.
- `LightingSetup` is either Manual (each light's own effect, or the per-key picture) or a Theme (id, colourway index or Custom,
  setting values). Brightness per light is part of the setup.
- `lighting_presets`: list of named copies of a setup; unique, case-sensitive names. A mode that loaded a preset remembers the name
  and flips `changed` when the shown setup differs; Save writes back, Discard restores, Undo toast.
- `theme_memory` (per mode, by theme id): going back to a theme restores its colours and settings. Presets carry none.
- Legacy shape still read: `lighting_schemes` (name, keyboard, light bar), `mode_bindings` (mode id to scheme name),
  `selected_scheme`. **This is the "mode bindings" structure**; it was superseded by `lighting_modes`.
- Migration: `migrate_lighting_once` runs once, guarded by marker `lighting_migrated`; an unreadable file is neither migrated nor saved;
  a missing file gets marked defaults; migration never switches a feature on. 17 tests in `lighting_model/migration.rs`, fixtures in
  `tests/data/lighting-config` (10 files).

### 2.3 Themes

- Built in only: `effects/themes/catalogue.json`, 32 themes (first 12 ranked "top"), parsed with `deny_unknown_fields`. Each: id, name,
  mood sentence, colourways (named palettes), labels for the Custom palette slots, one look per light (keyboard effect plus params;
  bar hardware effect plus colour indexes; logo), a few user settings (choice, number, toggle), and a `suits` list that the UI does not
  use (his comment says so).
- User choice: `Values { colourway, custom colours, settings }`. There are no user-authored themes; the user's own work is a Custom
  colour set and "Save as preset".
- Rules per hardware kind (`themes/kinds.rs`): per-key, four-zone, one-colour, 7-colour palette keyboards; three-zone and one-colour
  bars; logo. The app draws only where measured (PH16-71 keyboard and bar), elsewhere a firmware look is the nearest match.

### 2.4 Per-mode fan plans

- `fan_plans: Vec<{mode, plan}>`, plan = `automatic | curve{steps:[u8;6]} | fixed{percent} | max` (serde tag `kind`).
- Applied by a single 3-second **reconciler in the GUI process**; every other caller writes intent to config and waits. Writes only
  on change, back-off to one attempt per 30 s on failure. A fixed 0 % is treated as a firmware hand-off, because on this chassis a
  manual 0 holds a floor of about 1670 RPM, while firmware auto reaches 0 RPM when cool.
- Migration seeds each mode from the old global setting (Performance and Turbo become Max, others Automatic) after a bug where
  Turbo stopped raising fans (finding 37).

### 2.5 Game profiles (GameSync)

- `game_profiles: Vec<{name, executable, profile}>` and `game_sync_enabled`. A 5 s tick scans `/proc/*/exe` (full path or basename),
  saves the previous mode, restores after three consecutive misses (about 15 s) so launcher hand-offs do not flap. One entry
  point (`set_profile`) so no new hardware path.

### 2.6 Macros

- `Macro { name, steps: [{key, delay_ms, delay_only}] }`, recorded only while the Macros page is open, played only from a button,
  via `xdotool` (X11 only; inert on Wayland, which his findings 16 admit). No hotkey binding, deliberately.

### 2.7 Power-source rules and keys

- `profile_ac`, `profile_battery`, `power_source_action` (`keep | move-if-disallowed | always-set`), `mode_cycle_ac`, `mode_cycle_battery`
  (which modes the physical mode key steps through, per source), `mode_default`, `auto_eco_enabled` plus threshold (Quiet forced
  below 15 %). Finding 29 is the useful part: an earlier continuous rule fought the user and a third writer (power-profiles-daemon)
  raced both; the fix is "only the plug event moves the mode, once, to the nearest allowed mode".
- `predator_key_action`: `app | command | none` plus one command string. One action for the PREDATOR key. TrueSense has four gestures,
  Fn+Esc and a 30-action vocabulary (`ts-domain/src/actions/parse.rs`).

### 2.8 Favourites

**There is no favourites concept.** What exists: `saved_colors` (a list of colours the user keeps, "Keep this colour" in the
lighting page), named lighting presets, `last_profile`, and the per-mode "which preset is loaded". Nothing can be starred, there is
no quick-access strip and no key action list. [V: searched `config.rs`, `window.rs`, `dashboard_page.rs` for favourite/star: none]

### 2.9 Who applies what, and when

| Thing | Applied by | When | Consequence |
|---|---|---|---|
| Lighting follows the power mode | GUI process (`hardware/lighting.rs`: apply_mode, restore_saved) | mode change seen by GUI, start, resume (CLOCK_BOOTTIME drift) | stops if the GUI is quit |
| App-drawn themes | GUI process renderer thread | continuous | same |
| Fan plans, game sync, auto Eco, idle blank, alerts | GUI process timers | 3 s / 5 s / 1 s | same; finding 17 |
| Mode key and PREDATOR key, ENE lighting restore, thermal profile and TCC re-apply at boot | hotkey service (user unit) and boot helper | event | works with the GUI closed |
| Privileged writes | helper through pkexec, or hidraw when udev grants the session | per write | auth dialog if the helper is gone |

---

## 3. What TrueSense already has that is equal or better (do not regress)

All [V] from the files named.

### 3.1 Better than the reference

| TrueSense | Where | Reference equivalent |
|---|---|---|
| Daemon as root owns continuous behaviour (mode decisions, fans, boot look and resume look, charge-once, power managers) so nothing depends on a window | `ts-daemon/src/daemon/step.rs`, `boot.rs` | GUI timers |
| **Smart auto**: classifier (idle, browsing, heavy, gaming, hot), EMA smoothing, dwell, hold on manual pick (30 min), minimum gap, long Turbo retry pause; named profiles, built-in read-only | `ts-decision/src/autoprofile.rs`, `smartprofile.rs` | a plug-event rule and an Eco threshold |
| **Game rules** with process-name, file-name and Wine `.exe` matching, Turbo backs off when hot, Eco forced when the battery is low | `ts-decision/src/gamerules.rs` | GameSync, 15 s restore delay, `/proc/exe` only |
| **Fan curves**: full point curves per fan with hysteresis, down-rate, predict, couple, battery shift; built-in and user libraries; stop only with confirmation, cool machine and watchdog | `ts-decision/src/curves.rs`, `fanctl.rs`, `docs/SAFETY.md` | six temperature bars; Automatic/Curve/Fixed/Max |
| **Undervolt** CPU (core, cache) and GPU (offset, cap) with crash guard, boot profile, read-back | `ts-domain/src/undervolt`, `gpuundervolt`, `ts-decision/src/crashguard.rs` | none |
| **PREDATOR key and Fn keys**: tap, double, triple, long, Fn+Esc, binding profiles, ~30 actions including `exec:` and `look:` | `ts-keys`, `ts-domain/src/actions` | one action |
| **Whole-setup profiles** (power, fans, lighting, battery, audio, display, boot) in TOML, applied through the action runner | `ts-domain/src/profiles/bundle.rs` | lighting presets only |
| **Looks** (keyboard plus rear light, boot look applied by the daemon before login and after resume), designs, per-key painting from images, software per-key animations | `ts-domain/src/looks`, `designs`, `perkeyfx` | presets and themes (richer visually, section 5) |
| **Power-manager takeover** (stops competing managers and keeps them stopped) | `ts-domain/src/powermgr` | opt-in write to the desktop daemon, which his finding 29 shows still races |
| Audio: speakers/headphones, jack, mic, voice modes, DTS, repair | `ts-domain/src/audio`, api `audio` | EQ only |
| Safety rules and the command bus contract with refusals (`confirm`, `force`) enforced below the UI | `docs/SAFETY.md`, `crates/ts-gui/src/gui/COMMANDS.md` | UI-side checks |
| Hermetic tests (1904) and a firmware simulator (`TRUESENSE_SIM=1`) | `docs/STATUS.md` | tests next to code, some globals |
| Doctor, self-test, service repair, update with checksums and strict tar | `ts-system` | setup page, installer |
| Atomic files, SO_PEERCRED socket auth, refusal to overwrite an invalid config, unknown config keys preserved | `ts-foundation`, `ts-ipc` | atomic config write only |

### 3.2 Equal

Battery limit 80 %, calibration, USB charge stop, charge-to-100-once (ours also auto-restores the limit); touchpad toggle; LCD overdrive;
keyboard backlight timeout; boot sound; GPU mode switch; first-login startup service; model probe and doctor.

### 3.3 Reference features we deliberately do **not** adopt (and why)

- **Macros** (xdotool, X11 only): keystroke synthesis on a hardware-control app; inert on Wayland, which is what a CachyOS user
  likely runs. Skip; if ever added, keep his rule (record and play only from the page, no trigger).
- **AI assistant** (Ollama): pulls an HTTP client with TLS (`ureq`) for a feature outside the product. Skip for now.
- **"React to typing" lighting** (reads the built-in keyboard evdev nodes, 4600 lines `key_input/*`): privacy-sensitive, needs the
  `input` group. Defer until everything else is done and then keep his fail-closed gate.
- **Public-IP lookup** (calls a third-party service from the Network page). Do not import; local IP only.
- **GRUB splash, Drivers and Manuals** page with Acer art: Drivers page idea is fine as a link plus serial number from DMI; no Acer pictures.

---

## 4. Improvement plan for the Qt app

### 4.1 Navigation: sidebar, with a narrow-window single column

Evidence from the screenshots:

- `home.png` (our Python app): 480 px wide; Home mixes live controls with a 2x3 grid of page buttons and Quit; every other page needs
  Back. Fine for a tool you open, change one thing in and close.
- `fan-curves.png`, `lighting-profiles.png`: the curve chart is squeezed into 400 px; Lighting hides four tabs behind icon buttons.
- Reference screenshots 5, 7, 17: lighting preview, effect grid, colour picker and presets sit side by side; theme gallery needs a grid.
  Monitoring pages (screens 2, 3, 9, 10) are only useful with room for several charts at once.

Decision: **sidebar**, because the app is no longer "set one thing and close": monitoring, per-mode editing and favourites want
width and one-click page changes. Keep what users liked in the old app:

- default window about 1000 x 680, remembers size, never forced maximised (the reference opens maximised each time);
- below ~720 px the sidebar collapses to icons, below ~520 px to a menu button, the page becomes one column like today;
- Home keeps the big mode tiles and the "Smart/Native/Manual" control so the old muscle memory works;
- text over icons: every sidebar item has icon plus label; no hidden Tools hub (sidebar entries instead of tabs of unrelated things).

Entries (12), grouped under small headings so it stays scannable:

```
Now        Home | Monitor
Control    Performance | Cooling | Lighting | Keys
Hardware   Battery and screen | Audio
Library    Profiles and favourites
System     Health | Settings
```

### 4.2 Page list: what each shows and which command it uses

"New" = needs a command or field that does not exist yet (section 4.4). Existing commands are the names in `COMMANDS.md` [V].

| Page | Shows | Existing commands | New |
|---|---|---|---|
| **Home** | power-mode tiles (5), Mode control Manual/Native/Smart + Smart profile + hold countdown, GPU mode, fan mode tiles (Smart, Auto, Max, Manual), four live tiles (CPU, GPU, fan RPM, battery) with sparklines, Favourites strip, one-line daemon/key-service health banner | `status get`, stream, `power set/auto/smart/release`, `gpu set`, `fan auto/max/smart/set/off`, `profile list/apply` | `favourite list/apply` |
| **Monitor** (tabs Overview, CPU, GPU, Memory and disks, Network) | Overview = six temperature gauges (CPU, GPU, board, NVMe, Wi-Fi if present) with min/max; CPU = total, per core, frequency, power, top processes; GPU = temperature, utilisation, VRAM, power, clocks, P-state, PCIe, limits; Memory/disks; Network = rates, peaks, session totals, interface | stream (`sensors`, `load`, `power`) | `monitor usage`, `monitor gpu`, `monitor net`, `monitor disks`, `system info` |
| **Performance** (tabs Mode and Smart, Game rules, Power limits, Tuning) | Smart profile editor (all `SMART_SETTING_KEYS`), game rule list with live "running now", CPU PL1/PL2 with lock bit, GPU power limit read-out, CPU temperature ceiling, CPU and GPU undervolt (existing safety dialogs), managers takeover | `power smart/game/managers`, `cpu info/limits/undervolt`, `gpu undervolt` | `cpu ceiling get/set` (4.4), GPU limit write only if verified safe on this model [U] |
| **Cooling** | two live fan dials (RPM, duty register, temperature), plan selector per mode (Native, Smart curve, Fixed, Max) with the curve editor, behaviour sliders, curve library with stars | `fan *`, `fan curve list/current/show/use/copy/save/delete` | `mode bind` for "Fans for" (4.3) |
| **Lighting** (tabs Keyboard, Per-key, Rear bar, Themes, Looks) | strip "Lighting for [mode]" with the Changed badge; live keyboard preview; effect grid with icons; colour row with kept colours; brightness for each light; idle off; boot look; gallery of themes with colourway chips | `keyboard *`, `rear *`, `look *`, `startup *` | `theme list/show/apply` (4.5), `mode bind` |
| **Keys** | gestures table (tap, double, triple, long, Fn+Esc) with action pickers, test button, timing, binding profiles; favourite actions | `keys profile`, `keys test` | action names `favourite:N`, `favourite-next` |
| **Battery and screen** | gauge, status, voltage, current, power, cycles, health %, design vs full, temperature, 80 % limit, charge to 100 once, USB charge stop, calibration, overdrive, boot sound, keyboard timeout, charge history chart (kept in memory for the session; persisted history is out of scope) | `battery *`, `display overdrive`, `boot sound`, `keyboard timeout` | `battery info` |
| **Audio** | existing page (output, speaker filter, DTS, jack, mic, voice, repair) | `audio *` | EQ later (7.14) |
| **Profiles and favourites** | whole-setup profiles with description and Apply, save current, delete; the Favourites list with reorder, kind icons | `profile list/show/apply/next/save/delete/current` | `favourite *` |
| **Health** | doctor results with fix text, self-test, service check/repair, daemon log tail | `doctor run`, `selftest run`, `service *`, `config validate` | none |
| **Settings** | model and "features supported on this machine" checklist, language, theme (dark/light/system), tray options, alerts, startup, autostart, update, reset-all, settings export/import | `gui autostart`, `update *`, `startup *`, `touchpad set`, `config show/reset-all` | `alerts get/set`, `config export/import`, `ui` settings stored in the app's own file |

Rules for every page: a capability the machine lacks shows a greyed control with one sentence ("TGP is fixed by this laptop's firmware"),
never a missing control and never an error toast (the capability list comes from `ts-system::models` probe, extended with sysfs facts, 4.4).
Anything the daemon refuses shows the refusal text as it is (COMMANDS.md rule). Selection controls are buttons that set state on click;
refresh code never emits user signals (finding 27 above).

### 4.3 Mode bindings: per power mode a look and a fan curve

**Who can apply a look as root?** [V] The daemon already drives the keyboard and the rear light: `boot_look_worker` calls `self.looks.apply(&look)` under
the daemon lock, retries for 20 s, and runs at start and after each resume (`ts-daemon/src/daemon/step.rs` 118-130 and 470-505).
`operating_lighting` already re-sends a colour when the power mode changes (`step.rs` 403-416, opt-in `operating_follow`). The udev rule
also grants the logged-in user hidraw access (`packaging/70-truesense.rules`), so both root and user can write. The boot look is stored
**self-contained** in `/var/lib/truesense/boot-look.json` because the daemon cannot read a user's looks. Looks live per user in
`~/.config/truesense/looks/NAME.json`, so bindings must copy the data. The daemon already does the same for curves (`fan_curve_data`
in its state file, `cmd_fan_curve` in `handlers.rs` ~253-295).

Data model (new):

```
daemon state (state file): mode_bindings = {
  "<mode>": {                          # eco | quiet | balanced | performance | turbo
    "look":      { "name": "late-night", "data": <look object> } | null,
    "fan_curve": { "name": "quiet-ish",  "data": <curve object> } | null
  }
}
user state (user.json):    mode_bindings_view = { "<mode>": { "look": name, "curve": name } }   # for the UI and "Changed"
```

- Set: the api resolves the named look and curve from the user's stores and sends the data (as `fan curve use` with `data` already does).
  Saving a look or curve that a binding names re-sends it (`look save` and `fan curve save` call `mode_bindings refresh name`), otherwise the
  daemon copy goes stale. The UI shows "Changed: not yet saved to the binding" using the user-state view and the live look.
- Applied by the **daemon**, on the edge of a power-mode change only (same detection as `operating_lighting`), never continuously:
  the look through the boot-look worker path (retry, lock rules), the curve through the same code as `fan_curve use` with data.
  Debounce 5 s and never in the first tick after boot so Smart auto's own dwell times are not undone by a flapping keyboard.
- Precedence for lights: binding for the mode, else `operating_follow` colours, else leave. Precedence for fans: a manual duty or `fan off`
  set by the user stays until the next mode change *that has a binding for that mode*; no binding means the user's choice is never touched.
  This is a decision for the user [DECISION 1]: should any mode change end manual fans? Recommended no.
- Animated themes (software frames, 4.5) can only run in a user process. The key service (user unit, already owns `perkeyfx::Runner`)
  polls the daemon `state` (cheap, once a second) and, when the mode changed, starts the bound look's animation. So: static looks at boot
  and with no one logged in; animations only while the key service runs. Say so in the UI next to the binding.
- Interaction with Smart auto: bindings follow whatever sets the mode (user pick, Smart auto, game rule, key). The decision engine
  needs no change.
- Migration: none needed (new keys). A daemon that does not know `mode_bindings` ignores it.
- Tests: pure function `bindings::edge(prev, now, bindings) -> Vec<Effect>` in `ts-decision` (no clock, injected debounce), daemon
  handler tests with the existing fakes (`handler_tests.rs`, `loop_tests.rs`), api tests that stale copies are refreshed on save,
  socket integration test in `ts-api` suite.

### 4.4 New commands and daemon features (specific)

| Area | Verb and args | Result | Lives in |
|---|---|---|---|
| `mode` | `bind {mode, look?, curve?}`, `unbind {mode, what?}`, `bindings` | map as above | api `mode.rs` (new), ipc cmd `mode_bindings` (18th daemon command), `ts-decision::bindings` |
| `favourite` | `list`, `add {kind, name}`, `remove {kind, name}`, `move {kind, name, to}`, `apply {kind, name}` | list of `{kind, name, ok}`; apply returns what the underlying command returns | api `favourite.rs` (new); user state `favourites: [{kind,name}]`; kinds `look`, `fan-curve`, `smart-profile`, `profile`; **never** undervolt or anything that needs `confirm` |
| `theme` | `list`, `show {id}`, `apply {id, colourway?, custom?, settings?, mode?}` | the stored values | api `theme.rs` over `ts-fx` |
| `monitor` | `usage` (per-core, top N by cpu or mem, memory, swap), `gpu` (clocks, VRAM, P-state, PCIe, limits, power), `net` (iface, rates, totals, local IP), `disks` | JSON | `ts-system` modules, api `monitor.rs` |
| `system` | `info` (model, BIOS, CPU name, cores, RAM, GPU name and driver, disks, OS, kernel, serial number from DMI if readable) | JSON | `ts-system::sysinfo` |
| `battery` | `info` (voltage, current, power, cycles, design and full capacity, health %, technology, maker, model, temperature) | JSON; fields the kernel does not give are `null` | `ts-system::sensors` extension |
| `alerts` | `get`, `set {enabled, temp_c 70..100, notify_mode_changes}` | settings | daemon reads from config (4.8) |
| `cpu` | `ceiling {get, set {celsius}, auto}` | range and state | daemon cmd; root write to `intel_tcc_cooling`; clamps like his range logic; needs `confirm` below 70 C (decision for the user, he calls it a hazard of staying throttled) |
| `capabilities` | `get` | list of `{id, supported, why}` for the Settings checklist and greyed controls | api `status.rs` over `ts-system::models` |
| `config` | `export`, `import {path}` | validates before replacing, copies the old aside (his rules, findings 35) | api `window.rs` |
| `gui` | `show` already exists | | reuse for tray and second launch |

Also new in the stream (still cheap reads): `battery` summary and `gpu` with `vram`, `clock` when a page asks for it (subscription
flag), because the one-second tick must stay cheap on idle (see 4.10).

### 4.5 Themes and effects for lighting (our own designs on top of his engine)

- Import his engine and catalogue as `ts-fx` (section 5, R1) and render themes in the user key service by replacing the 5 frames/s
  `perkeyfx::Runner` with a renderer host (R3). The existing `keyboard fx` modes stay as a fallback and for the CLI.
- A `Look` gains an optional `theme` member `{id, colourway|"custom", custom[], settings{}}` next to `keyboard` and `rear`. A look
  with `theme` set draws; the static parts remain as the firmware fallback written at apply time (that is also what the daemon writes at boot).
  Additive JSON; **[U]** whether `looks::Look` ignores unknown members (check `Look`'s serde attributes before relying on it).
- **User themes**: `~/.config/truesense/themes/*.json` in the same schema as the catalogue, parsed strictly (`deny_unknown_fields`),
  no code, broken file skipped with a Health warning. This is the "our own designs" system; the first user-visible additions are ours,
  made for the rear bar's real modes (static, breathing, neon, wave, twinkling), because his bar looks name effects (for instance
  "ripple") that our rear light does not have [U: needs a mapping table checked on the laptop].
- Our keyboard cells are `column * 6 + row` in 128 cells (`docs/PROTOCOL.md`); his layout is 103 keys by role (`effects/layout.rs`).
  A layout adapter (key name to cell) is part of R1 and needs a test per key.
- Safety additions kept from his engine: the flash guard (3 flashes/s) stays on; `Reduced motion` setting and "Pause animations on battery"
  are honoured; **never** run frames when the daemon says the laptop is suspending.

### 4.6 Favourites in detail

- Anything starrable: a look, a fan curve, a Smart profile, a whole-setup profile. Excluded: undervolt and power limits (they need
  `confirm` and a danger line; one-tap apply of those is wrong).
- UI: a star toggle in every library list (Lighting > Looks, Cooling curves, Performance > Smart, Profiles); Home shows the
  list as chips (name, kind icon, one-click apply, toast with Undo for the thing it replaced when we can compute it); the Profiles and
  favourites page reorders them.
- PREDATOR key and Fn+Esc: two new actions, `favourite:N` (1-based) and `favourite-next` (cycles all favourites, notification shows
  the name). Add them to `FIXED_NAMES`/`ARG_NAMES` and `CATALOGUE` in `ts-domain/src/actions/parse.rs`; the existing test that every
  action appears in the catalogue enforces the documentation. Keys run as the user so they can read `user.json` directly.
- Storage: user state `favourites`; migration none; unknown kinds are skipped, not an error (so a newer file works in an older build).
- Dangling names (deleted look) show greyed with a remove button; `apply` of one fails with a normal error.

### 4.7 i18n and Arabic

- Mechanism: Qt Linguist. QML uses `qsTrId("page_id_text") //% "English text"` so extraction (`lupdate`) keeps the English next to the
  id; ids are stable keys, so his 829 keys can be converted mechanically (R9): his catalogue key becomes our id, his English value
  our `//%` text, each other language a `.ts` file. Plurals with `%n`; numbers and dates through `QLocale`.
- Arabic: no reference catalogue exists; we write `truesense_ar.ts`. Layout: `LayoutMirroring.enabled: Qt.application.layoutDirection === Qt.RightToLeft`,
  `LayoutMirroring.childrenInherit: true`; icons that point (back, chevrons) mirror, charts and keyboard preview never mirror
  (physical layout). Digits: default to Western digits, setting for Arabic-Indic. Font: fall back to a system Arabic face; no bundled font.
  Test: a pseudo-locale with long strings plus an RTL run of the screenshot tests.
- Strings now: do not touch the web front end (it is being retired). Daemon, api and CLI messages stay English because the CLI contract
  and 1904 tests compare them with the Python edition; the Qt app translates by error `code` where one exists and shows the English text
  otherwise (document this limit in Settings help). All new Qt strings go through `qsTrId` from day one, including the ones imported pages bring.
- CI: `lupdate -no-obsolete` clean check, a report of untranslated ids per language, and a test that every `qsTrId` id in QML exists in the English `.ts`.

### 4.8 Tray, residency, notifications and alerts

- Tray: `QSystemTrayIcon` (his GTK tray process uses a StatusNotifierItem library; we do not need it). Menu: Open, power mode (5 radio),
  fan mode, first five favourites, keyboard light on/off, Quit. Tooltip: mode, CPU and GPU temperature. Left click toggles the window.
  If `QSystemTrayIcon::isSystemTrayAvailable()` is false (some GNOME setups), closing the window quits and Settings says why.
- Closing the window destroys the QML engine and keeps only the application object and the tray (memory); reopening must still meet the
  first-frame budget. Default "minimise on close" is **on**, and, unlike the reference, turning it off disables nothing (the daemon does the
  work). Autostart uses `--background` (his finding 17: login must not pop a window).
- Single instance and raise: reuse `ts-gui::instance` (flock in the runtime directory, released by the kernel on death, so the zombie-owner bug
  of his single_instance.rs cannot occur) and the existing `gui show`.
- Alerts live in the **daemon**, not in the app, so they work with no window: threshold 70 to 100 C (default 90), re-arm 5 C below, at most one per
  minute across sensors, normal urgency, through the existing `notice` and OSD path (`osd.rs` already runs helpers as the session user). Settings
  in `[alerts]` of the config; *how fresh the daemon's copy of the config is I did not verify* [U: it reads `self.cfg` as loaded; add a reload or
  an ipc `set`].
- Other notifications (all off by default except the first two): fan stop watchdog fired and 80 % limit restored (exist today), power mode changed
  (opt-in), update available, key service or daemon not running.

### 4.9 Engineering plan for the app

- Crates: `ts-ui-model` (Qt-free view-models and formatting, depends on `ts-api` types and `ts-fx`; tested like the rest), `ts-qt` (binary
  `truesense-qt`: Qt bridge, QML, resources). The bridge exposes one object, `Bus`, with `call(area, verb, argsJson) -> Promise-like`
  and a `live` signal, wrapping `ts_gui::gui::{Commands, ApiCommands}` in process (COMMANDS.md contract and its contract test stay the
  single source). Calls run on a worker pool, never on the GUI thread.
- Qt binding: **[U]** the shell agent must choose and verify between `cxx-qt` and a thin C++ shim; I did not check crate versions or
  whether either builds on this toolchain (Rust 1.85+, edition 2024). Decide in the first week with a spike that opens a window, calls
  `status get` on the simulator and shows the result.
- State: one `LiveStore` (last stream frame, history ring buffers of 120 samples per series) feeds pages; pages hold no hardware state of
  their own. Page code never writes from refresh.
- Tests: view-model unit tests in `ts-ui-model`; QML component tests with Qt Quick Test against the firmware simulator
  (`TRUESENSE_SIM=1`); screenshot golden tests for each page in LTR and RTL at 480, 1000 and 1600 px using the Qt offscreen platform
  (`QT_QPA_PLATFORM=offscreen`) [U: needs a CI image with Qt 6]; the existing contract test that every command the UI uses is in COMMANDS.md.
- Logging: the app logs to the journal with `qInfo/qWarning`; a config problem is always logged (his lesson).
- Config of the app itself (language, theme, window size, tray options): `~/.config/truesense/ui.toml` through the same atomic writer and the
  never-overwrite-unreadable rule.

### 4.10 Accessibility and performance budget

Accessibility: every control has `Accessible.name` and role; Tab order follows reading order and every control shows a focus ring; contrast
4.5:1 for text in both themes; state never by colour alone (power modes have icon and word); font scale setting (his `font_scale`) plus honouring
the system scale; reduced-motion setting turns off gauges' easing and all non-essential animation; Orca smoke test with AT-SPI [U]; keyboard
shortcuts for sidebar (Ctrl+1..9); all dialogs focus the safe button first (danger confirmations default to Cancel).

Budget (targets, to be measured with a script in CI on the real laptop):

| Item | Target | How |
|---|---|---|
| Start to first frame | < 300 ms warm | window and sidebar shell appear first with the last cached status; first IPC call is asynchronous with 1 s timeout; pages created lazily by `Loader`; no `QtCharts`; Basic Quick Controls style with our own components |
| Idle CPU, window open on Home | < 1 % of one core | one-second stream only; sparklines repaint at 1 Hz; no running animation on hidden pages |
| Idle CPU, tray only | 0.0 % (no timers) | engine destroyed, tray waits on events; the stream is closed (the daemon pushes nothing) |
| Memory, window open on Home | < 120 MB RSS [U: Qt Quick baseline not measured here] | measured, tracked in CI |
| Memory, tray only | < 40 MB RSS [U] | |
| Stream payload | < 2 KB per tick, heavier data on request | subscription flags per page |
| GPU polling | never wakes a suspended discrete GPU | his issue 79 (see 5.2, R6): NVML or `nvidia-smi` calls while the dGPU sleeps cost SMIs that stall every core. Verify that our daemon's `nvidia-smi` stream respects runtime suspend before shipping `monitor gpu` [U] |

---

## 5. Reuse map

Convention: **IMPORT** = copy with small edits; **ADAPT** = copy, re-wire to our traits and types; **PORT** = rewrite for Qt/QML using the file as the
spec; **CROSS-CHECK** = use as a test oracle; **SKIP**. Sizes are lines of Rust (production plus inline tests) from my count; "gtk refs" is the count of
toolkit paths before the test module. All paths relative to `predator-sense-gui/` of the reference. Unwrap counts are production code only.

Cleanup needed for every import to pass our gate (`clippy all=deny`, `unwrap_used=deny`, `panic=deny`, `missing_docs=warn`, edition 2024, MSRV 1.85,
`#![forbid(unsafe_code)]` except the allowed places, rustfmt width 110): the reference is edition 2021 with no deny lints [V: Cargo files], so expect:
`Mutex::lock().unwrap()` on global statics (replace with `lock().unwrap_or_else(PoisonError::into_inner)` and, better, injected state), doc comments
for public items, `mem::take`/iterator clippy fixes, and an `unsafe` audit. Add the author's credit header to every imported file (section 6).

### 5.1 Summary table

| ID | Piece | Reference files | Size | Verdict | Target |
|---|---|---|---|---|---|
| R1 | Effect engine, 19 effects, palette, layout, flash guard, fdlibm maths, golden tests | `src/hardware/effects/*` (not `themes`), `tests/data/effects-golden`, `tools/*.js` | ~10,300 lines incl. 2 test modules (the whole `effects/` directory is 14,025 with themes); 1.2 MB data | IMPORT | new crate `ts-fx` |
| R1b | Theme catalogue, resolve, kinds, looks, text | `effects/themes/*`, `tests/data/theme-kinds` | ~3,700 lines; 32 themes | IMPORT + ADAPT | `ts-fx::themes` |
| R2 | Lighting setups, presets, migration, plan | `hardware/lighting_model.rs` and `lighting_model/*` | 5,971 lines, 103 tests | ADAPT | `ts-domain::setups` |
| R3 | Renderer thread, scheduler, audio capture | `hardware/lighting_renderer.rs`, `lighting_renderer/*`, `blanking.rs` | 6,246 lines, ~95 tests | ADAPT | `ts-keys::renderer` |
| R4 | Keyboard wire protocol and other-chip drivers | `protocol/src/lib.rs`, `hidraw/`, `hardware/keyboard_rgb.rs`, `chicony_*`, `hid_rgb.rs`, `magic_rgb.rs`, `light_bar*.rs` | 3,330 + 511 + ~5,500 | CROSS-CHECK now; IMPORT later only if a second model is supported | tests in `ts-domain::keyboard` |
| R5 | Config shapes, mode bindings, fan plans, game profiles | `config.rs` | 2,343 lines | PORT ideas, not code | none |
| R6 | Usage, disks, network, system info, GPU, NVML | `hardware/procs.rs`, `storage.rs`, `sysinfo.rs`, `sensors.rs`, `hwmon.rs`, `gpu.rs`, `nvml.rs`, network sampling in `ui/network_page.rs` | ~3,000 | ADAPT (usage, sysinfo, net, disks); SKIP NVML dlopen | `ts-system::{usage,netstat,sysinfo,disks}` |
| R7 | Capability detection | `hardware/capabilities.rs`, `discrete_gpu.rs`, `display.rs` | 388 + 101 + 180 | CROSS-CHECK | `ts-system::models` |
| R8 | Alerts, game sync, Eco mode, CPU ceiling, audio EQ | `alerts.rs`, `game_sync.rs`, `eco_mode.rs`, `temp_limit.rs`, `audio_eq.rs`, `audio_sync.rs` | 180, 408, 141, 513, 497, 695 | mixed, per item | daemon and `ts-system` |
| R9 | Translations | `i18n.rs` (8 languages x 829 keys), `effects/themes/text.rs` | 8,084 + 539 | IMPORT by script | `.ts` files |
| R10 | View-models | `ui/lighting/*_model.rs`, `frame_model`, `paint_model`, `states`, `preview_scene`, `fan_control_model`, `fan_curve_geometry`, `key_cursor`, `zone_sim` | ~10,000 | ADAPT | `ts-ui-model` |
| R11 | GTK pages, widgets, CSS, tray | `ui/*` rest, `style.css`, `installer/src/tray.rs` | ~33,000 + 48 KB CSS | PORT to QML (spec only) | `ts-qt` |
| R12 | Icons and artwork | `resources/lighting-*-symbolic.svg` (17, 4 KB each) | small | IMPORT the 17 lighting icons; rest in note 2 above | `ts-qt/resources` |
| R13 | Installer, privileged helper, hotkey service | `installer/src/*` | 4,422 + 2,548 + 2,606 | SKIP | none (we have our daemon, keys, installer) |
| R14 | Macros, AI, GRUB splash, key_input | `macro_player.rs`, `ai_*`, `grub_splash.rs`, `key_input/*` | | SKIP now | |

### 5.2 Per item

**R1 Effect engine (`ts-fx`).** [V] Header says: pure, no GTK, no I/O, no global state. 19 effect modules (aurora, beacon, cascade, cycles, daylight,
hearth, kb, lava, outrun, pulse, radar, rainfall, ripple, scanner, silk, spectrum, thermal, tide, warp, plus engine, palette, layout, noise, guard,
hw, sim, jsmath). API: `create(id, params, palette, layout, seed)` then `frame(t, dt, input)` returns one colour per light after easing, flash guard and
byte rounding. Imports: `jsmath.rs` (706 lines) is a line-by-line fdlibm port so results match the JS original bit for bit; that is also the cure for
the musl-libm drift our STATUS.md lists as unverified, for the functions it covers. Layering: depends on serde and serde_json only, so it sits right above
`ts-foundation` and below `ts-domain`; no new dependencies. Safety fit: it never touches hardware; the flash guard (WCAG 2.3.1) is an extra safety
we want. Quality: production code has no unwrap or panic (the 17 unwraps and 20 panic-like macros in `golden.rs`, `parity.rs`, `kinds_parity.rs` are in
`#[cfg(test)]` modules); most effect files have **no inline tests** and are covered only by golden frames (`golden.rs`, 23 tests; `robust.rs`, fuzz-style
robustness). Needed edits: `missing_docs`, clippy, `#![forbid(unsafe_code)]`, replace `expect` in `robust.rs`. Data: import `tests/data/effects-golden`
(1.2 MB) as it is; keep `tools/gen-golden.js` and `effects-reference.js` (228 KB) only as a documented dev tool: regenerating needs `gjs`, which CI
will not have, so freeze the vectors and say so. **Open point:** `effects.js` is described as "the reference design" and a contributor's work; confirm with
the author who wrote it and that his permission covers it (section 6).

**R1b Themes.** [V] `catalogue.json` (32 themes, strict parse), `resolve.rs` (colour and settings resolution), `kinds.rs` and `looks.rs` (what each light shows
per hardware kind; 805 + 138 lines), `text.rs` (translated theme names and moods, 539 lines), `integrity.rs` (cross-checks of the catalogue; test only),
`parity.rs`, `kinds_parity.rs` (compare with tables derived from his JS; test only; the table fixtures are 132 KB). Adapt: our hardware kinds are the PH16-71
per-key keyboard and the three-zone rear bar only, so `kinds.rs` can shrink to what a user with one model needs; keep the enum shape so other models can be
added by data. The bar effect names must be mapped to our rear modes [U]. Quality: fixtures are strict and tested; this is the best-tested part of the repo.

**R2 Lighting setups (`ts-domain::setups`).** [V] 5,971 lines, 103 tests, production unwrap count 0. Value: `LightingSetup` (Manual or Theme), `ModeLighting`
(preset, changed, theme memory), preset operations (load, save, save as new, discard, rename, delete, undo), tolerant reads (one bad entry dropped),
migration with markers. Coupling that must be cut: it imports `config::AppConfig`, `keyboard_rgb::KeyboardState`, `light_bar::LightBarState`, `magic_rgb`
states, `profile::PowerProfile` (5 modules of his hardware layer). Adapt plan: keep the type and operation code, replace the device states with our
`Look` parts (`ts_domain::looks`, `keyboard::State`, `RearSettings`), and replace the `AppConfig` accessors with a small `SetupStore` trait (user JSON
file, atomic). `plan.rs` (1,594 lines, setup to per-light writes) is **rewritten** against our `Keyboard` and `Hw` rear calls, using his tests as
the expectation list. Not imported: the four-zone, ENE, Magic, Chicony palette variants (other models).

**R3 Renderer (`ts-keys::renderer`).** [V] One thread, creates effects, steps them, writes changed frames (keyboard 30/s, bar 5/s), blocks when nothing
animates (zero cost idle). Ordering gate with per-light epochs so a firmware look written right after `show` cannot be overwritten by a late frame;
fails closed if the thread panics; pause for idle, suspend, battery, reduced motion. Cut: the global `renderer()` singleton, `quiet_panics` coupling,
`output.rs` (writes his devices) becomes an `Output` trait implemented over our `Keyboard::frame` (`FrameSink`, the type `perkeyfx::Runner` already uses)
and `Hw::rear_set`. `audio.rs` (1,010 lines, one `unsafe`, `parec` subprocess) is optional: import last and re-audit the unsafe. **Rate and safety:**
his default is 30 frames/s on the keyboard; our `Runner` runs about 5/s and each upload is eight 64-byte HID reports with a 50 ms settle
(`FRAME_SETTLE`, `ts-domain/src/keyboard/mod.rs`). **[U]** whether the PH16-71 controller tolerates 30 uploads/s; he says it is measured on this exact model
("measured" per `LightKind::app_drawn_measured`), but start at our 5/s and raise only after a read-back soak test on the real laptop.

**R4 Keyboard protocol.** [V] His `protocol/src/lib.rs` (3,330 lines, 93 tests) builds the Chicony reports for the same controller as ours (04F2:0117, in
`70-truesense.rules`). Effects agree with ours: 14 effects and the same opcodes (his Rainbow = our `neon`, code 8; his Rain = our `raindrop`, 10).
Use now: turn his byte-level tests into test vectors for `ts-domain::keyboard` (a second independent witness next to the Python transcripts). Do not import
his `hidraw` crate (511 lines, 3 `unsafe`): we have `ts-domain/src/keyboard/hidraw.rs`. Other chips (ENE5130, Sunrex, Darfon, Chicony palette): leave until the user
owns such a laptop; the code is untested on real hardware by his own table.

**R5 Config shapes.** [V] Not importable: JSON, flat, global. Take: the structures in section 2 (modes map, presets, fan plan enum, game profile), the
three-way `Missing | Loaded | Unreadable` result and the rule that only `Missing` may be written with defaults, the generation-counter cache, and the
export/import rules. We already have the key rule [V]: `ts_foundation::config::write_user` refuses an existing file that is not valid TOML, leaves it as it is, and keeps keys it does not know (`config.rs` 659-671). What we lack is his visible reporting: the Qt app must show a banner when the config does not parse (today only `config validate` says so) and must log it. Add the "unparseable file stays byte-identical" test if it is not already there [U].

**R6 Usage, disks, network, system info, GPU.**
- `procs.rs` (369 lines, 1 test, 0 unwrap): `/proc/stat` per core, `/proc/meminfo`, top processes by CPU delta or memory. ADAPT to `ts-system::usage` with an
  injected root path like our `Sampler::new(root, clock)` so tests read fixtures. Cap the process list (top 10), parse with bounds.
- `storage.rs` (281, 4 tests, **2 `unsafe`** for `statvfs`): `statvfs` is not in our allowed-unsafe list (`ARCHITECTURE.md`). Either add it with a SAFETY
  comment and test, or call `df`/read `/proc/self/mountinfo` plus a helper; decision for the user [DECISION 2], recommended: parse `df --output` to stay at zero new `unsafe`.
- `sysinfo.rs` (292, 3 tests): DMI product, BIOS, CPU model from `/proc/cpuinfo`, GPU name, OS from os-release, kernel, disk list. ADAPT to `ts-system::sysinfo`
  with an injected root. Overlaps `ts-system::models` (probe); share the DMI read.
- Network: sampling is inside `ui/network_page.rs` (reads `/sys/class/net/*/statistics`, picks the interface by operstate, rates in the page).
  Extract the 40 lines of logic into `ts-system::netstat`; **do not** import `fetch_public_ip` (calls a third party).
- `gpu.rs` (837 lines; 26 production `unwrap` on global mutexes, 16 tests) and `nvml.rs` (832 lines, `dlopen` with `unsafe`): his reason for NVML-via-dlopen is solid
  and measured (every `nvidia-smi` start initialised NVML, which on the PH16-71 raised about 30 SMIs per second and stalled all cores; one initialised session is silent;
  he also never touches the dGPU while it may be runtime-suspended). Our answer without `dlopen` in the Rust binary: extend `tools/nvml/truesense-nvml.c`
  (our small C helper, already used for undervolt) with a streaming "metrics" mode that initialises once and prints one line per second (temperature,
  utilisation, power, clocks, VRAM, P-state, PCIe) and honours RTD3 (skips while suspended); the Rust side only parses lines, as `parse_gpu_line` does today.
  Check **[U]** what the daemon runs today (`Sampler::run_gpu_stream` takes a program name; it may be `nvidia-smi -l`, which is a long-lived process, so the SMI cost
  may already be paid once). Keep `gpu.rs`'s decisions (power range, what is "live data allowed") as the spec.

**R7 Capabilities.** [V] 388 lines, 5 tests, all sysfs probes with no model list. CROSS-CHECK against `ts-system::models`: compare what each probes and add anything
we lack to the Settings checklist (`capabilities get`). Do not import; our model is a kernel-module feature set, his is acer-wmi and facer sysfs.

**R8 Smaller features.**
- `alerts.rs` (180 lines, 3 tests, uses `gio::Application` for the notification, 5 toolkit refs): take only the rule set (clamp 70..100, hysteresis 5, one per minute,
  one fire per crossing) as a pure function in `ts-decision::alerts`; the delivery is our `notice`. ADAPT as logic only. Add his tests as table tests.
- `game_sync.rs` (408, 11 tests, 3 `lock().unwrap()`): superseded by `ts-decision::gamerules`; use the three-miss restore delay as a test case for ours
  (does ours also absorb a launcher disappearing for a few seconds? [U]).
- `eco_mode.rs` (141): volume (`wpctl`) and brightness cap with exact restore; brightness needs root. Nice, small, optional; ADAPT later as a profile action `eco:on|off`.
- `temp_limit.rs` (513, 8 tests; needs the shared `protocol::temp_limit` module too): ADAPT into `ts-system` plus a daemon command, with the range read from the
  cooling device and a refusal below 70 C unless `confirm`. Safety review by us: it can only make the CPU throttle earlier.
- `audio_eq.rs` (497, 7 tests): talks to EasyEffects through GSettings; presets are his own judgment, not Acer values. ADAPT later, optional.
- `audio_sync.rs` (695), `grub_splash.rs`, `ai_*`, `macro_player.rs`: SKIP (3.3).

**R9 Translations.** [V] 829 keys per language in match tables (`t_pt` 65..903, `t_en` 904..1742, then es, zh, ja, ru, de, it, tr, each about 838 lines), placeholders `{0}`,
fallback to the key. Import by a one-off script into `.ts` (R9 converter, part of work chunk W11); keep every placeholder; normalise to `%1`. Check quality [U]: I cannot judge the
non-English text; there is no sign of a native review in the code. Only keys for pages we port will be used; keys for features we skip stay unused. Arabic: new.
The installer's own `installer/src/i18n.rs` (English and Portuguese only) is not needed.

**R10 View-models (`ts-ui-model`).** [V] 0 toolkit refs in: `control_model` (2,402 lines, 27 tests), `frame_model` (1,418, 22), `themes_model` (1,653, 25), `colour_model` (527, 13),
`paint_model` (276, 6), `states` (422, 7), `preview_scene` (2,255, 24), `fan_control_model` (266, 14), `fan_curve_geometry` (286, 14), `key_cursor` (279, 7), `zone_sim` (185, 7); about 10,000 lines together.
They are the decision logic of his pages (which controls show, what a theme panel contains, the preview scene graph, curve hit-testing) and are covered by tests. ADAPT: each reads
`AppConfig` and his hardware state types, so each gets the same cut as R2; they then drive QML through the bridge as plain structs. This is the best way to get his UX behaviour
into Qt with tests carried over.

**R11 GTK pages, widgets, CSS, tray.** Spec only: `ui/*_page.rs` (lighting_page 1,786; fan_control_page 1,280; usage_page 2,076; window 3,261; the rest), widgets (`tech_gauge`, `gauge_widget`,
`fan_curve_widget`, `preview.rs` 3,265 lines of Cairo, `effect_icon`, `faceted_card`), `style.css` (48 KB), `installer/src/tray.rs` (SNI, uses ksni). All of it is GTK, libadwaita, Cairo or CSS; nothing carries over except behaviour,
the proportions and the strings. QML equivalents: `Canvas` or `Shape` for gauges and preview, `Repeater` grids for effect and theme cards, `Menu` for the tray (`QSystemTrayIcon`). The 17 lighting SVG icons are
"symbolic" and 4 KB each; import them (author's permission) after checking they use `currentColor` (a QML `ColorOverlay` or SVG `fill` is needed otherwise) [U].

**R13 Installer, helper, hotkey service.** SKIP. The helper is a pkexec privileged-action list for the GUI (4,422 lines, 87 tests); our daemon replaces it. His hotkey service listens to evdev and a hidraw mode key; our `ts-keys` already
reads the vendor HID interface. One idea to check on our side: the mode-key on the PHN16-73 is "dead" on stock kernels (finding 25); on the PH16-71 do all Fn and mode keys reach `ts-keys`? [U].

**Dependencies we would drag in and refuse:** `gtk4`, `libadwaita`, `ureq` (TLS), `ksni`, `rusb`, `dirs` (use our own paths), and `tempfile` in dev only. Everything in R1, R2, R3, R6, R8, R10 imports with serde and serde_json only once the cuts above are done.

---

## 6. Licence and attribution

Facts [V]: the reference declares GPL-3.0 in every Cargo file and in `LICENSE`; the source files have **no per-file copyright or SPDX header** (searched `src`, `installer`, `protocol`, `hidraw`); his README asks
for visible credit (README, About screen or credits) and names contributors: **fawaz7** (Lighting page, themes, Fan Control redesign, PRs 85 to 95), **jlucaso1** (cover logo). The kernel module `facer` and `acpi_ec` in his tree are
other people's GPL work; we import none of it. TrueSense is MIT (`LICENSE`, workspace `license = "MIT"`).

What the combined work means (informational, not legal advice): GPL-3.0 code cannot be relicensed MIT by the people who receive it. If TrueSense ships a binary that contains his code, that binary must be offered under GPL-3.0
terms as a whole, even if our own files stay MIT in the repository. His permission can allow us to use the code, and even to relicense **his** parts, but only if he and every other copyright holder of the parts we take agree.

Open items for the **user** to decide or obtain, in this order:

1. **Keep the approval in writing.** Link or copy of the issue comment or mail in the repo (for example `docs/third-party/predator-sense-permission.md`), with date, the exact scope ("may reuse code, translations, SVG icons, design, in an MIT or GPL project") and whether he allows relicensing.
2. **Contributors.** Much of what we want (themes, lighting page, fan control redesign) was written by fawaz7 and is copyright of him unless assigned; the engine's JS origin (`effects-reference.js`, "reference design") may be a third person's work. His permission alone may not cover them. Ask the author who wrote
   which files (`git log` of his repo shows it; I did not have the history) and get a short yes from each contributor whose files we import. List them in the table of the NOTICE.
3. **Decide the licence of the combined parts.** Options: (a) TrueSense stays MIT for its own code and the imported crates (`ts-fx`, `ts-domain::setups`, renderer, view-models, translations) are GPL-3.0 files inside the repo, with the Qt binary therefore GPL-3.0 (Qt itself is LGPL, compatible); (b) the whole repository moves to GPL-3.0; (c) he and the contributors grant an MIT licence for the files we take. This document does not choose.
4. **Third-party artwork inside his repo**: Flaticon icons (his README credits their authors), Acer pictures, mode artwork, Predator marks: not covered by his approval (note 2 at the top).

Proposed files (to be created by the coordinator when work starts, not now):

- `NOTICE` at the repo root: a short paragraph per third-party source: project name, author, URL, licence, what was taken, which TrueSense files contain it, date of approval and where the written approval is kept; contributors fawaz7 and jlucaso1 with their parts; a sentence that Acer, Predator, PredatorSense, Helios, Nitro are trademarks of Acer Inc. used only to name the hardware.
- `THIRD_PARTY.md` or `docs/third-party/`: one row per imported file (our path, source path, source commit if known, edits made, licence), regenerated by a small script so it cannot rot; plus the copy of his `LICENSE`.
- A header on **every imported file**: `// Derived from predator-sense by Cleyton Alves (GPL-3.0), used with the author's permission. Changes: ...`, because his files carry no header of their own and the copyright holder must be visible.
- An About page entry and the Settings > About text crediting him by name and linking his repo (his README asks for exactly this).
- `Cargo.toml`: if the licence of a new crate is GPL-3.0, set `license = "GPL-3.0-only"` for that crate and keep `cargo deny` aware.
- Translations: keep the language catalogues in their own directory with their own notice, so they can be replaced if their licence status changes.

---

## 7. Work breakdown (parallel chunks after the shared Qt shell)

Order: W0 first (one agent, blocks all UI chunks); W5, W6, W9, W10 and W11 backends do not need the shell and can start now. Each chunk lists crates touched and tests. `make ci` must stay green at every merge.

| ID | Chunk | Touches | Tests | Needs |
|---|---|---|---|---|
| W0 | **Qt shell**: `ts-qt` + `ts-ui-model` skeletons, bridge spike (cxx-qt or shim), `Bus`, sidebar, theme tokens (colours as tokens, dark/light), `LiveStore`, i18n plumbing (`qsTrId`, `.ts` loading, RTL switch), tray stub, single instance via `ts-gui::instance`, offscreen test harness | new `ts-qt`, new `ts-ui-model`, workspace manifest, `Makefile` (`make qt`) | Bus call against `TRUESENSE_SIM=1`; window opens offscreen; pseudo-locale and RTL smoke | decision on binding |
| W1 | **Home** page and live tiles | `ts-qt` pages, `ts-ui-model::home` | view-model tests; screenshot LTR/RTL | W0, W9 (stub ok) |
| W2 | **Monitor backend**: `ts-system::{usage,netstat,sysinfo,disks}` (R6), api `monitor`, `system info`, stream flags, NVML metrics mode in `tools/nvml` | `ts-system`, `ts-api`, `ts-gui` bus table + `COMMANDS.md`, `tools/nvml` | fixture-based parsers (reference-style `/proc` samples), api tests with testing services, contract test | none |
| W2b | **Monitor pages** (gauges, charts, top processes) | `ts-qt` | view-model + screenshots | W0, W2 |
| W3 | **Battery info + capabilities + Settings checklist** | `ts-system::sensors`, `models`, api `battery info`, `capabilities get` | fixture tests with missing fields | none |
| W4 | **Lighting pages** (Keyboard, Per-key, Rear, Looks) ported from the web pages; keyboard preview item; mode strip UI | `ts-qt`, `ts-ui-model::lighting` (R10 adapted) | view-model tests carried over from R10; screenshots | W0, W6 for the strip |
| W5 | **`ts-fx` import** (R1, R1b), layout adapter to our cells, golden vectors | new crate `ts-fx`, workspace | the imported golden suite; one test per key name to cell; clippy clean | none (gate: section 6 answered) |
| W5b | **Renderer host in key service** (R3), `Look.theme`, `theme *` commands, user themes dir | `ts-domain::{setups,looks}`, `ts-keys`, `ts-api`, `COMMANDS.md` | renderer with fake output (ordering, fail-closed), api tests, soak test script for the laptop (manual) | W5 |
| W5c | **Theme gallery UI** | `ts-qt` | screenshots | W0, W5b |
| W6 | **Mode bindings backend** (4.3) + `mode *` commands + daemon cmd 18 + key-service listener | `ts-decision::bindings`, `ts-daemon`, `ts-api`, `ts-keys`, `ts-ipc` tables, `COMMANDS.md`, `docs/PROTOCOL`/ARCHITECTURE | edge function table tests; daemon handler and loop tests; api-to-daemon integration | none |
| W7 | **Cooling page**: curve editor item (spec: `fan_curve_geometry`, tests carried), dials, plan selector | `ts-qt`, `ts-ui-model::cooling` | geometry tests carried from R10; screenshots | W0, W6 |
| W8 | **Performance page** (Smart, game rules, limits, undervolt ports) and **CPU ceiling** backend | `ts-qt`, `ts-system`, `ts-daemon`, `ts-api` | refusal tests (`confirm`, `force`) already exist, add ceiling range tests | W0 |
| W9 | **Favourites**: store, api, `favourite:N` and `favourite-next` actions, CLI verbs, TUI list | `ts-domain::actions`, `ts-api`, `ts-cli`, `ts-tui`, `ts-gui` bus, `COMMANDS.md` | actions catalogue test, runner tests with fakes, api tests, CLI help transcript | none |
| W10 | **Alerts and notifications in the daemon** (R8 rules, `[alerts]` config, reload) | `ts-decision::alerts`, `ts-daemon`, `ts-foundation::config`, `ts-api` | table tests (hysteresis, rate limit), daemon loop test with fake clock | none |
| W11 | **i18n pipeline**: converter from his catalogues to `.ts` (kept as a script under `tools/`), Arabic file, CI checks (ids exist, untranslated report) | `tools/`, `ts-qt/i18n`, CI | script tests on a tiny catalogue, placeholder-preservation test | W0 for CI hook |
| W12 | **Keys, Profiles and favourites, Health, Settings, onboarding wizard (first run: doctor, module state), tray menu** | `ts-qt` | screenshots, view-models | W0, W9, W3 |
| W13 | **Packaging**: `truesense-qt` dynamic build, deb/Arch/AUR deps, `.desktop`, icons, CI job with Qt 6 | `packaging/`, `tools/ci`, `Makefile`, `docs/INSTALL.md` | `tools/shelltest` additions, `verify-units` | W0 |
| W14 | **Later**: Eco mode action, EQ via EasyEffects, audio-reactive lighting (R3 audio), idle blanking, key-reactive lighting (privacy review first) | various | each with fakes | W5b |

Parallel waves: (1) W0 | W2 | W3 | W5 | W6 | W9 | W10 in parallel; (2) W1, W2b, W4, W5b, W7, W8, W11 once W0 and their backend land; (3) W5c, W12, W13; (4) W14.

Minimum useful first release of the Qt app: W0, W1, W2+W2b, W3, W4, W7, W12 (without W5 themes or W6 bindings). Themes and bindings are the headline features; they can ship in the second release without blocking the first.

---

## 8. Things I could not verify, and risks

- Which Qt binding builds on this toolchain (cxx-qt version, MSRV 1.85, edition 2024) and whether QML startup under 300 ms and RSS targets hold: not measured.
- Whether `looks::Look` serde ignores unknown members (needed for `Look.theme`), and whether the Go and Python editions do (they share the files).
- How fresh the daemon's configuration is after a file change (alerts and bindings settings).
- Whether the PH16-71 keyboard tolerates 30 HID frame uploads per second; our own code uses about 5 per second with a 50 ms settle.
- Whether the daemon's GPU stream (`nvidia-smi`) is long-lived and respects dGPU runtime suspend; and whether a GPU power-limit write is possible on this model (his GPU screenshot is a PH315-54 with a fixed limit).
- Who wrote `effects-reference.js` and each file we want; I had no commit history (the clone has none in the scratchpad), so contributor attribution per file is unknown.
- Quality of his non-English translations; 829-key count is from the English and Portuguese tables (`=> "` counts), the other seven were not counted.
- The screenshot set shows a PH315-54 for most pages, PH16-71 only for lighting and fan pages; behaviours of his pages on our model are as he states.
- His tray behaviour under GNOME without an AppIndicator extension; ours depends on `QSystemTrayIcon` availability.
- Nothing in this document was run: no program from his repo, no build of ours.
