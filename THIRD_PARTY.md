# Third-party code in this repository

## predator-sense (crate `ts-fx`)

| | |
|---|---|
| Source | <https://github.com/cleyton1986/predator-sense>, directory `predator-sense-gui/` (the GTK4 app, v0.4.0-preview in its Cargo files) |
| Author | Cleyton Alves (cleyton1986); contributors named in its README: fawaz7 (lighting page, themes, fan control redesign), jlucaso1 (cover logo) |
| Licence of the source | GPL-3.0 (its `LICENSE`) |
| Permission | The author's approval to reuse his work, obtained by the repository owner; the written record is not in this repository yet |
| Where it is | `crates/ts-fx/` (declared `license = "GPL-3.0-only"`; see `crates/ts-fx/NOTICE.md`) |
| Licence decision | Taken (2026-11-07): the whole repository is `GPL-3.0-only` (see `LICENSE`), because the `truesense` and `truesense-qt` binaries link `ts-fx` and are therefore a combined work. The kernel module keeps its own dual MIT / GPL-2.0-only header |

### What was taken

From `src/hardware/effects/` of the source: the effect engine and its 18 catalogue effects,
the 32-theme catalogue with its types, settings, per-hardware rules and translated words, and
his tests (`golden.rs`, `robust.rs`, `themes/{integrity,parity,kinds_parity}.rs`). From
`tests/data/`: `effects-golden` (golden frames) and `theme-kinds`. From `tools/`:
`gen-golden.js`, `effects-reference.js`. Not taken: anything GTK, his lighting renderer,
configuration and hardware drivers, the Acer product photos and any Acer, Intel or NVIDIA
artwork.

### What was changed

- Paths and module layout: `mod.rs` became `lib.rs`; `super::super::` paths became `crate::`;
  the copy in `gen-golden.js` and `derive.js` points at the new locations; one label in
  `tests/data/effects-golden/themes.json` (`catalogue`) names the new path of `catalogue.json`.
- Lints of this repository: `#![forbid(unsafe_code)]`; documentation on every public item; clippy
  `-D warnings` with `unwrap_used` and `panic` denied (test code is exempt, as elsewhere). The
  fdlibm port (`jsmath.rs`) and index-based effect loops carry targeted `allow`s with the reason.
- Production code: `Resolved::num` and `Resolved::keys` no longer have a `debug_assert!(false)`;
  `catalogue()` yields an empty list instead of `expect`ing its own file; the snake path sorts with
  `total_cmp` instead of `partial_cmp().unwrap()`.
- Cut: the translation language enum of his `i18n` module (now `themes::Lang`, a plain enum) and
  the global language lookup; a test in `themes/integrity.rs` about his page's edit keys; a test in
  `kb.rs` that compared the speed scale with his protocol crate (replaced by explicit values).
- Made public: `LightLook::create`; `Theme` and its types are `Serialize` as well as `Deserialize`
  (the catalogue's own schema, so a user theme can be written back); `Theme::top` and
  `Theme::suits` default when absent.
- Added by TrueSense: `cells.rs`, `themes/validate.rs`, `Theme::keyboard_effect`, `Lang::from_code`.

### Facts and designs studied, no code taken (task of 2026-11-08)

The wave direction table (1 left to right, 2 right to left, 3 bottom to top, 4 top to bottom, 5 clockwise, 6 counter-clockwise
on a PH16-71, confirmed 2026-10-15), the list of the 14 firmware effects with their codes and which of them take a direction, and
the idea of a theme gallery with a thumbnail, colourway chips, search, filters and a star per card come from his `keyboard_rgb.rs`,
`control_model.rs` and themes UI, which were read for their facts and layout. The code that implements them here (`ts-domain`
`keyboard::Direction`, `themes::preview`, `perkeyfx::sweep`, `designs::templates`, the `gallery` view-model and the QML components)
is new, so none of those files carries the adaptation header.

