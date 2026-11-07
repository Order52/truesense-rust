# The window's command bus

Everything the web window does is one of these commands. The page sends
`POST /api/{area}/{verb}` with a JSON object as the body and gets `{"ok":true,"result":...}` or
`{"ok":false,"error":{"message":"...","code":"..."}}`. `gui.Run` forwards each command to
`Commands.Do(ctx, area, verb, args)` (args is the raw JSON object, `{}` when the page sent
nothing) and the live values come from `Commands.Stream`. The implementation runs as the
calling user (the daemon's uid check still applies) and is written on top of the application
layer by the CLI agent: **this table is its specification**. A test
(`contract_test.go`) fails if the front-end calls a command that is not in this table or
that the simulated laptop (`internal/gui/guitest`) does not know.

## Conventions

- **area / verb** are the names of the Python command line (`truesense <area> <verb>`); where the
  CLI has no verb for a command it takes the verb given here (`status get`, `touchpad set`, ...).
- **args** are JSON members named like the CLI's long options, dashes as underscores
  (`--long-ms` -> `long_ms`, `--fn-esc` -> `fn_esc`). Numbers are JSON numbers, flags are booleans.
  `action` is the CLI's positional action (`on|off|toggle|status`, `list|show|use|...`).
  A command ignores members it does not know and fails with a message for a missing or bad one.
- **result** is what `truesense --json <area> <verb>` prints where such a command exists; "new" marks
  a verb, argument or field the CLI does not have yet (everything that is new is additive).
- **Boolean settings** (`on|off|toggle|status`) return the new state as a JSON boolean.
- **Profile libraries** (fan curves, Smart auto profiles, key bindings, CPU and GPU undervolt, looks)
  share one protocol: `action` = `list` (array of names, built-in first) | `current` | `show`
  (the profile, **plus `"builtin": true|false`**, new) | `use` | `copy` (`name`, optional `new`; result:
  the new name) | `save` | `delete`. Built-in profiles are read-only (`save`/`delete` fail).
- **Errors**: `Do` returns an error whose text is shown to the user as it is (so write it for a person,
  like the CLI's `error: ...` line). Wrap `gui.ErrUnknownCommand` for an (area, verb) that does not exist;
  an error that implements `Code() string` sends that code (for example `needs-daemon`).
- **Long commands** (`update apply`, `selftest run`, `doctor run`, `audio repair`) may take minutes; the
  server allows 10 minutes per command and cancels `ctx` when the page goes away.
- A command that changes a **fan to a stop**, an **undervolt** or a **power limit beyond the danger line**
  must be refused by the implementation unless the args carry `confirm: true` (and `force: true` beyond
  the danger line), exactly like `--confirm` / `--force`: the page asks the user first, but the page is
  not trusted to be the only guard.
- Five commands are answered by the gui package itself and never reach `Commands`: `gui show` (a second `truesense gui` asks the open window to go to a page: `{"page": "curves"}`, answered `{"shown": bool, "windows": n}`; the page gets a `show` event on the stream), `gui ping`
  (`{"version": "..."}`), `gui quit` (stops the window server), `gui launch` (mints a one-time window code,
  used by `gui.Resume`) and `gui session` (the page trades the one-time code in its URL for the session
  token; the only call that needs no token). The session token is never in a URL: the window URL holds a
  code that works once and expires after 5 minutes, because a URL on a browser's command line can be read
  by the other users of the computer.

## The live stream (`Commands.Stream`, `GET /api/stream`)

`emit` is called about once a second with one JSON object (only while a page is open):

```json
{"t": 1730973600,
 "sensors": {"cpu_temp": 54, "gpu_temp": 49, "sys_temp": 46, "cpu_fan_rpm": 2150, "gpu_fan_rpm": 2090},
 "power_mode": "balanced", "ac": true, "battery_pct": 87,
 "fans": {"mode": "auto", "duty_register_pct": {"cpu": 40, "gpu": 40}},
 "load": {"cpu_pct": 12, "gpu_pct": 3},
 "power": {"cpu_w": 28.5, "gpu_w": 6.1},
 "daemon": {"live": {...}, "settings": {...}, "hold_left_s": 0}}
```

- `sensors`, `power_mode`, `fans` are `hw.Sensors`, the power mode and `hw.FanState` (as in `truesense --json status`).
- `ac` (bool) and `battery_pct` (number) may be `null` when unknown; `load` and `power` are the daemon's
  `cpu`, `gpu` and `gpuw` live values (CPU package power from RAPL if available) and each member may be `null`.
- `daemon` is exactly the daemon's `state` answer (`{"live": ..., "settings": ..., "hold_left_s": ...}`,
  see `ipc.call("state")`) or `null` when the daemon does not run. The page reads
  `settings.auto_mode|auto_profile|smart_fan|operating_follow`, `live.smart_profile|fan_curve|mode_reason`.
- Cheap reads only (the daemon's cached state where it has it): this runs every second.

## Commands

| area | verb | args | result | replaces (Python) |
|---|---|---|---|---|
| `status` | `get` | none | `truesense --json status` (`power_mode`, `sensors`, `fans`, `daemon` = the daemon `state` or the string `"ERROR: ..."`, `battery_limit`, `usb_charge_stop`, `overdrive`, `boot_sound`, `kb_backlight_timeout`, `gpu_mode`, `rear`, `touchpad`, `audio_output`, `jack`, `voice`) **plus** new: `ac` (bool), `battery_pct`, `kb_brightness` (0..50), `kb_look` (the keyboard look, as `keyboard status` `look`) | `cli.h_status`, `Window.full_refresh` |
| `version` | `get` | none | `{"truesense": "0.4.0"}` | `cli` area `version` |
| `power` | `status` | none | `{"mode": "balanced", "supported": ["quiet", ...], "auto_profile": true, "hold_left_s": 0}` | `cli.h_power` status |
| `power` | `set` | `{"mode": "turbo"}` | the mode name; the choice is held 30 min against the automatic profile | `control.power(mode, 30)` |
| `power` | `auto` | `{"action": "off"\|"native"\|"smart"}` (also `on` = smart, `toggle`, `status`) | the mode control now in force (`off`, `native` or `smart`) | `daemon set auto_mode` |
| `power` | `smart` | `{"action": "list"\|"current"\|"show"\|"use"\|"copy"\|"save"\|"delete", "name"?, "new"?, "profile"?}`. `profile` (new) = the full settings object of `smartprofiles.DEFAULTS` for `save`, and for `use` of a profile that is not saved yet; `use` of a saved profile needs only `name` | list: names; current: `{"name", "profile"}`; show: the profile + `builtin`; use: `{"name"}`; copy: new name; save: the saved profile; delete: name | `cli.h_power` smart, `smartprofiles.*`, `ipc smart_profile` |
| `power` | `release` | none | message string; ends a hold | `ipc hold minutes=None` |
| `power` | `game` | `{"action": "list"\|"add"\|"remove"\|"clear", "name"?, "mode"?}` | `{"rules": {"program": "mode"}, "running": null\|"mode"}` | `ipc game_rules` |
| `power` | `managers` | `{"action": "list"\|"stop"\|"restore"}` | list: `{"on": bool, "found": [{"unit", "active", "enabled"}]}`; stop: `{"stopped": [], "failed": [], "cpu": {...}}`; restore: `{"restored": []}` | `ipc power_managers` (status, takeover, restore) |
| `fan` | `auto` | none | message; also ends the smart curve | `control.fan("auto")` |
| `fan` | `max` | none | message | `control.fan("max")` |
| `fan` | `smart` | `{"action": "on"\|"off"\|"toggle"\|"status"}` | bool: the smart curve is on | `daemon set smart_fan` |
| `fan` | `set` | `{"cpu": 20..100\|0, "gpu"?: 20..100\|0, "confirm"?: bool}` (gpu defaults to cpu; 0 = stop that fan and needs `confirm`, the daemon and a cool laptop) | message | `control.fan("manual", cpu, gpu)` |
| `fan` | `off` | `{"confirm": true}` | message; both fans stopped (refused unless cool, with the watchdog) | `control.fan("off", confirm=True)` |
| `fan` | `curve` | `{"action": "list"\|"current"\|"show"\|"use"\|"copy"\|"save"\|"delete", "name"?, "new"?, "curve"?}`. `curve` (new) = `{"cpu": [[t, pct], ...], "gpu": [...], "hysteresis", "down_rate", "predict", "couple", "battery_shift"}` for `save`, and for `use` of an edited curve not saved yet (then `name` is only a label, the page sends `custom` for a built-in base); the implementation validates it like `curves.validate` | list: names; current: `{"name", "curve"}`; show: the curve (+ `native`, `description`) + `builtin`; use: `{"name"}`; copy: new name | `cli.h_fan` curve, `curves.*`, `ipc fan_curve` |
| `gpu` | `set` | `{"mode": "optimus"\|"nvidia"\|"auto"}` | the mode; takes effect after a reboot | `hw.set_gpu_mode` |
| `gpu` | `undervolt` | `{"action": "get"\|"list"\|"show"\|"use"\|"reset"\|"copy"\|"save"\|"delete"\|"boot"\|"apply", "name"?, "new"?, "offset"?, "cap"?, "confirm"?, "force"?}`. `apply` (new) = apply `offset`/`cap` given in the args (the editor's values, not a saved profile) and needs `confirm`; `boot` with `name` `"off"` clears the boot profile, otherwise sets it from `offset`/`cap` (new: values in the args, else the saved profile `name`) and needs `confirm`; `save` takes `offset` and `cap` | get: `{"offset_mhz", "range_mhz": [lo, hi], "applied": {"name", "offset", "cap"}\|null, "boot": {...}\|null}` or `{"error"}`; apply and reset: `{"offset_mhz", "cap_mhz"}`; boot off: null; library verbs as above (`show`: `{"description", "offset", "cap", "builtin"}`) | `cli.h_gpu` undervolt, `gpuundervolt.*`, `ipc gpu_undervolt` |
| `cpu` | `info` | none | the daemon's `cpu_info`: `{"pl1_w", "pl2_w", "rapl_enabled", "msr_lock_bit", "nvidia_power_limit_w", "user": {"pl1", "pl2"}\|null, "auto": {"normal_w", "gaming_w"}}` | `ipc cpu_info` |
| `cpu` | `limits` | `{"pl1": W, "pl2": W, "auto"?: bool, "force"?: bool}` (`auto: true` goes back to automatic) | `{"pl1_w", "pl2_w"}` | `ipc cpu_limits` |
| `cpu` | `undervolt` | as `gpu undervolt` with `core` and `cache` (millivolts, 0 to -120) instead of `offset` and `cap` | get: `{"core_mv", "cache_mv", "applied": {"name", "core", "cache"}\|null, "boot": {...}\|null, "pending": bool}` or `{"error"}`; apply and reset: `{"core_mv", "cache_mv"}`; `show`: `{"description", "core", "cache", "builtin"}` | `cli.h_cpu` undervolt, `undervolt.*`, `ipc undervolt` |
| `battery` | `status` | none | `{"limit_80", "calibration", "usb_charge_stop"}` | `cli.h_battery` status |
| `battery` | `limit` | `{"action": "on"\|"off"\|"toggle"\|"status"}` | bool | `hw.set_battery_limit` |
| `battery` | `calibration` | `{"action": ...}` | bool | `hw.set_calibration` |
| `battery` | `usb-charge` | `{"level": 0\|10\|20\|30}` | the level | `hw.set_usb_charge` |
| `battery` | `full` | `{"action": "get"\|"start"\|"cancel"}` | get: `{"active", "limit", "capacity", "ac"}`; start/cancel: `{"active"}` | `ipc battery_full` |
| `battery` | `info` | **new**: none | `{"present": bool, "ac_online": bool\|null, "battery": {"name", "present", "percent", "capacity_level", "status", "technology", "manufacturer", "model", "voltage_v", "voltage_min_design_v", "current_ma", "power_w", "charge_now_mah", "charge_full_mah", "charge_full_design_mah", "energy_now_wh", "energy_full_wh", "energy_full_design_wh", "cycle_count", "health_pct", "temperature_c"} \| null, "others": [battery, ...], "charge_limit": {"end_threshold_pct", "start_threshold_pct", "health_mode", "calibration_mode", "firmware_limit_80", "limited"}}`. Every field the kernel does not give is `null`, never an error; `current_ma` and `power_w` are magnitudes (the direction is `status`); `health_pct` is full over design capacity; `limited` summarises all limit sources | `ts-system::battery`, api `battery_info` |
| `display` | `overdrive` | `{"action": ...}` | bool | `hw.set_overdrive` |
| `boot` | `sound` | `{"action": ...}` | bool | `hw.set_boot_sound` |
| `keyboard` | `status` | none | `{"look", "brightness", "max", "backlight_timeout", "lighting_profiles"}` (`look` is `{}` or `{"mode": "effect", "name", "color", "speed", "brightness"}` / `{"mode": "perkey", ...}`) | `cli.h_keyboard` status |
| `keyboard` | `on` / `off` | none | message | `actions.run("keyboard:on")` |
| `keyboard` | `brightness` | `{"level": 0..50}` | the level | `keyboard.set_brightness` |
| `keyboard` | `effect` | `{"name", "color": "rrggbb", "speed": 1..12, "brightness": 0..50, "direction"?: 0\|1}` | message | `keyboard.set_effect` |
| `keyboard` | `color` | `{"color": "rrggbb"}` | message; stays in the running effect | `keyboard.set_color` |
| `keyboard` | `speed` | `{"level": 1..12}` | message | `keyboard.set_speed` |
| `keyboard` | `operating` | `{"action": ...}` | bool: the colour follows the power mode | `daemon set operating_follow` |
| `keyboard` | `timeout` | `{"action": ...}` | bool: backlight off after 30 s | `hw.set_kb_timeout` |
| `keyboard` | `restore` | none | message; the stock cyan per-key state | `keyboard.restore_default` |
| `keyboard` | `layout` | none | `{"keys": {"name": [{"cell", "col", "row"}]}, "groups": [...]}` (the front-end uses the key names and cells) | `keymap.NAMES` |
| `keyboard` | `cells` | none | **new**: `{"cells": [128 x "rrggbb"]}`, the colours of the per-key state (cells 126 and 127 are not keys) | `keyboard.current_cells` |
| `keyboard` | `set-cells` | **new**: `{"cells": [128 x "rrggbb"], "brightness"?: 0..50}` | message | `keyboard.per_key(cells, brightness)` |
| `keyboard` | `fx` | `{"mode": "off"\|"pulse"\|...\|"mood", "speed": 1..10, "cells"?: [cell, ...], "base"?: [128 x "rrggbb"]}`. New: `cells` (the keys to animate; absent = every key) and `base` (the colours under the animation) as numbers/colours instead of key names | `{"mode", ...}` (the stored settings) | `perkeyfx.set_fx(mode, speed, cells, base)` |
| `keyboard` | `design` | `{"action": "list"\|"show"\|"save", "name"?, "cells"?}`. New: `show` and `cells` on `save` | list: `{"name": {"info", "source": "built-in"\|"yours"}}`; show: `{"info", "source", "cells": [128 x "rrggbb"]}`; save: the file path | `designs.load_all`, `designs.cells_of`, `designs.save` |
| `keyboard` | `profile` | `{"action": "list"\|"load"\|"save"\|"delete", "name"?, "cells"?}` (the per-key sets). New: `cells` on `save` (default: the colours now on the keyboard) | list: names; others: message | `keyprofiles.command` |
| `rear` | `status` | none | `{"mode", "speed", "brightness", "direction", "color", "raw"}` plus new `zones`: three colours or `null` | `hw.rear_get` (+ `userstate rear_zones`) |
| `rear` | `on` / `off` | none | message | `actions.run("rear:on")` |
| `rear` | `set` | `{"effect": "static"\|"breathing"\|"neon"\|"wave"\|"twinkling", "color"?, "speed": 1..5, "brightness": 0..100, "direction"?: 0\|1, "zones"?: [c, c, c], "verify"?: bool}`. New: `verify` (false = do not read back; used while a slider drags) | the new rear state | `hw.rear_set` |
| `look` | `list` | none | names (`native` first) | `looks.names` |
| `look` | `show` | `{"name"}` | `{"keyboard": {...}\|null, "rear": {...}\|null, "builtin": bool}` | `looks.load` |
| `look` | `save` | `{"name", "look"?}`. New: `look` = the whole look object (the page saves per-key colours it holds); absent = what is on the lights now | the saved look | `looks.save(name, look)` |
| `look` | `apply` | `{"name"}` | list of what was done | `looks.apply` |
| `look` | `delete` | `{"name"}` | name | `looks.delete` |
| `look` | `copy` | **new**: `{"name", "new"?}` | the new name (`NAME-copy`, `-2`, ... if taken) | `looks.copy` |
| `mode` | `bindings` | none | `{"mode": "turbo"\|null, "bindings": {"turbo": {"look": {"name", "data"}\|null, "fan_curve": {"name", "data"}\|null}}}`: the power mode now and what each bound mode applies (a mode without a binding is absent; `data` is the daemon's self-contained copy) | **new**, `ipc mode_bindings get` |
| `mode` | `bind` | `{"mode": "eco"\|"quiet"\|"balanced"\|"performance"\|"turbo", "look"?: NAME, "curve"?: NAME}` (at least one; a part left out keeps what the mode had; names of saved or built-in looks and curves, not the native curve) | as `bindings`. The look and curve are copied into the daemon, which applies them once when the power mode changes (by Smart auto, the user, the PREDATOR key or a game rule): the look always, the curve only while the fans are in Smart mode (a manual fan setting is never ended by a mode change). Saving a look or curve that a binding names refreshes the copy | **new**, `ipc mode_bindings set` |
| `mode` | `unbind` | `{"mode", "what"?: "look"\|"curve"\|"all"}` (default `all`) | as `bindings`; nothing is applied or undone on the machine | **new**, `ipc mode_bindings clear` |
| `mode` | `apply` | `{"mode"?}` (default: the current mode) | `{"mode", "results": [{"target": "look"\|"fan_curve", "name", "status": "applied"\|"skipped"\|"failed", "detail"}]}`; a part that is held back (fans not in Smart mode, a manual fan setting) is `skipped`, not an error | **new**, `ipc mode_bindings apply_now` |
| `startup` | `show` | none | `{"profile", "keyboard", "rear"}` | `config.load()["startup"]` |
| `startup` | `set` | `{"target": "keyboard"\|"rear"\|"profile", "value"}` (values as `startup set`; `none` clears the profile) | the value | `startup.set_option` |
| `startup` | `apply` | none | list of what was done | `startup.apply` |
| `startup` | `boot` | `{"what"?: "current"\|"clear"\|NAME}`; none = read | the boot look (or null); `current` = the lights now, `clear` = nothing at boot | `ipc boot_look` |
| `keys` | `profile` | `{"action": "list"\|"current"\|"show"\|"use"\|"copy"\|"save"\|"delete", "name"?, "new"?, "tap"?, "double"?, "triple"?, "long"?, "fn_esc"?, "long_ms"?, "multi_ms"?}` | list: names; current: `{"profile", "tap", "double", "triple", "long", "fn_esc", "long_ms", "multi_ms", ...}`; show: the profile + `description` + `builtin`; save: the saved profile (members not given keep the old or default value); use: the name | `keybinds.*` |
| `keys` | `test` | `{"action": "power:turbo"}` | message string | `actions.run` |
| `profile` | `list` | none | `[{"name", "source": "shipped"\|"yours", "description"}]` | `profiles.names` |
| `profile` | `show` | `{"name"}` | the profile as a JSON object (`power`, `fan`, `keyboard`, `rear`, `battery`, `display`, `boot`, `audio`, `description`) | `profiles.load` |
| `profile` | `apply` | `{"name"}` | message | `profiles.apply` |
| `profile` | `next` | none | message | `profiles.next_name` + apply |
| `profile` | `save` | `{"name", "description"?}` | the file path; saves the current state | `profiles.save(name, profiles.capture())` |
| `profile` | `delete` | `{"name"}` | message | `profiles.delete` |
| `profile` | `current` | **new**: none | `{"name": "gaming"\|null}`, the profile applied last | `userstate.get("profile")` |
| `favourite` | `list` | **new**: none | `[{"position", "kind": "look"\|"fan_curve"\|"smart_profile"\|"profile", "name", "exists"}]` in the user's order, positions from 1; `exists: false` = the item was deleted since (shown greyed; apply fails with the library's message) | favourites file beside the user config (`~/.config/truesense/favourites.json`) |
| `favourite` | `toggle` | **new**: `{"kind", "name"}` (`kind` also accepts `curve`, `smart`) | `{"starred": bool}`; starring needs the item to exist, un-starring never does; refused when 24 are starred | `favourites.toggle` |
| `favourite` | `remove` | **new**: `{"kind", "name"}` | `{"removed": bool}` | `favourites.remove` |
| `favourite` | `move` | **new**: `{"from", "to"}` (positions from 1; the others shift) | the new list, as `list` | `favourites.move_to` |
| `favourite` | `apply` | **new**: `{"position"}` or `{"kind", "name"}` | message (`Favourite 2/4: Look calm` for a position, `Look: calm` for an item); applies through the library's own operation (`look apply`, `fan curve use`, `power smart use`, `profile apply`) | `look.apply`, `ipc fan_curve`, `ipc smart_profile`, `profiles.apply` |
| `audio` | `output` | `{"action": "status"\|"speakers"\|"headphones"}` | `{"active", "jack_plugged"}` | `audio.output_status`, `audio.set_output` |
| `audio` | `speaker-filter` | `{"action": "on"\|"off"\|"status"}` | bool | `audio.speaker_filter` |
| `audio` | `dts` | `{"action": "status"\|"on"\|"off"\|"mode"\|"all", "name"?}`. Result is new in `--json`: the CLI prints text | `{"on": bool, "mode": "Music"\|null, "devices": [{"label", "kind", "profile", "current"}]}`; `all`: `{"done": [string]}` | `audio.dts_status`, `dts_power`, `dts_mode`, `dts_devices`, `dts_cover` |
| `audio` | `jack` | `{"use"?: "auto"\|"headphones"\|"headset"}` | `{"headphones", "headset_mic", "raw", "setting", "microphone_in_use"}` | `audio.jack_status`, `config.write_user` |
| `audio` | `mic` | `{"action": "status"\|"mute"\|"unmute"\|"volume"\|"boost", "value"?}` | status: `{"default", "muted", "sources", "volume"}` plus new `boost` (0..3); volume and boost: the value | `audio.mic_state`, `mic_set_mute`, `mic_volume`, `headset_boost` |
| `audio` | `voice` | `{"action": "status"\|"set"\|"notify", "a"?, "b"?}` (`set`: `a` = `headset`\|`array`, `b` = `off`\|`conference`\|`personal`; `notify`: `a` = `on`\|`off`) | status: `{"modes": {"headset", "array"}, "jack", "default_source", "notify"}` | `audio.voice_modes`, `voice_set`, `apply_mic_policy` |
| `audio` | `sound` | **new**: none | `{"ok": true\|false\|null, "problem", "sinks": []}`: is there a sound output (null: no card) | `audio.sound_state` |
| `audio` | `repair` | none (JSON result is new: the CLI prints text) | `{"fixed", "changed", "steps": [string]}` | `audio.repair` |
| `touchpad` | `get` | none | bool | `touchpad.enabled` |
| `touchpad` | `set` | `{"action": "on"\|"off"\|"toggle"}` | bool | `ipc touchpad` |
| `capabilities` | `get` | **new**: none | `{"model": {"vendor", "product", "bios", "tier": "supported"\|"unverified"\|"unsupported", "message"}, "simulated": bool, "firmware_answered": "12/14", "items": [{"id", "label", "supported": bool, "why": string\|null, "source", "detail": string\|null}]}`. One item per feature in a fixed order; ids: `power_modes`, `fan_control`, `fan_rpm`, `overdrive`, `battery_limit`, `battery_calibration`, `usb_charge`, `gpu_mode`, `rear_light`, `keyboard_backlight`, `per_key_rgb`, `boot_sound`, `keyboard_timeout`, `touchpad`, `cpu_undervolt`, `gpu_undervolt`, `power_limits`, `gpu_power_limit`, `fn_keys`, `dts`, `rnnoise`, `daemon`. `why` is one English sentence, set exactly when `supported` is false. The probe is read-only: only the firmware's read commands are sent and nothing is written | `ts-api::api::capabilities` |
| `gui` | `autostart` | **new**: `{"action": "on"\|"off"\|"status"}` | bool; writes or removes `~/.config/autostart/truesense.desktop` (`Exec=truesense gui`) | `Window.set_autostart` |
| `gui` | `ping` | handled by the gui package | `{"version"}` | |
| `gui` | `quit` | handled by the gui package | true; the server stops | `Window.close` |
| `update` | `check` | `{"force"?: bool}` | `{"current", "latest", "available", "error", "info": {"tag", "url"}}` | `update.check` |
| `update` | `apply` | `{"yes": true}` (the page asked the user already) | `{"version", "returncode", "sha256", "verified"}`; the download is verified before the installer runs as root; may take minutes | `update.apply` |
| `update` | `auto` | `{"action": "on"\|"off"\|"status"}` | bool | `cli.h_update` auto |
| `doctor` | `run` | none (new verb) | `{"version", "kernel", "ok", "failures", "warnings", "checks": [{"check", "status": "OK"\|"WARN"\|"FAIL"\|"INFO", "detail", "fix"}]}` | `doctor.run(as_json=True)` |
| `service` | `check` | none | `[{"unit", "scope", "enabled", "active", "ok", "fix"}]` | `services.check` |
| `service` | `logs` | none | `{"text": "..."}` (the last 60 lines of the daemon's journal; new in `--json`) | `Window.show_log` |
| `service` | `repair` | none | `[{"step", "ok"}]`; the user part only (a window cannot answer a sudo prompt): it says in a step when the system part needs `sudo truesense service repair` | `services.repair_user` |
| `selftest` | `run` | `{"skip_fans": true}` (new verb) | `{"ok": bool, "output": "..."}`; touches no fan when `skip_fans` | `selftest.run` |
| `config` | `show` | none | the merged configuration object | `config.load` |
| `config` | `validate` | none | array of problems (empty = valid) | `config.validate` |
| `alerts` | `get` | none | **new**: the `[alerts]` settings in force: `{"enabled", "cpu_limit_c", "gpu_limit_c", "repeat_minutes", "debounce_samples", "battery_low_pct", "throttling", "sensor_lost"}`; an error when a configuration file does not parse. The alerts are raised by the root daemon (so they work with no window), as desktop notifications, never with critical urgency | `truesense --json alerts show` |
| `alerts` | `set` | **new**: any of the members above (`enabled`, `throttling` and `sensor_lost` booleans, the others whole numbers); absent members stay as they are, a limit outside 70..100 C, `repeat_minutes` outside 1..240, `debounce_samples` outside 1..30 or `battery_low_pct` outside 0 (off) and 5..50 is refused with the allowed range and nothing is written | the settings now in force; the daemon re-reads the file within about ten seconds | `truesense alerts set` |
| `config` | `reset-all` | **new**: none | `{"steps": [{"step", "ok", "error"?}]}`; every step runs even if one fails: fan curve `native`, Smart auto profile `native`, fans `auto`, mode control `native`, key bindings `native`, power limits automatic, CPU undervolt boot cleared and reset, GPU undervolt boot cleared and reset, keyboard stock look, rear light stock (`breathing 3723c7 speed 3 brightness 100`); saved profiles are kept | `Window.reset_all` |
