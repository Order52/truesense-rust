# Notice: ts-fx is adapted from predator-sense

This crate is adapted from **predator-sense** by **Cleyton Alves (cleyton1986)**,
<https://github.com/cleyton1986/predator-sense>, licensed GPL-3.0. It is used with the author's
permission. Every source file that came from that project carries the header

    Adapted from predator-sense by cleyton1986 (GPL-3.0), used with the author's permission.

(data files that cannot hold a comment, and the byte-exact reference script, are listed below).

## Licence

Because the code is derived from GPL-3.0 work, this crate is declared `GPL-3.0-only` in its
`Cargo.toml`, not the MIT licence of the rest of the workspace. A program that links it (the
`truesense` binary) is a combined work. This notice records that fact; it does not decide how the
rest of the repository is licensed.

## Contributors named by the project's README

The project's README credits two contributors for parts that this crate uses or follows:

- **fawaz7** (<https://github.com/fawaz7>): the Lighting page, the themes and the Fan Control
  redesign (pull requests #85 to #95). The theme catalogue and its settings, which this crate
  imports, belong to that work.
- **jlucaso1** (<https://github.com/jlucaso1>): the cover logo feature. Only the logo light model
  (the `Role::Logo` layouts and the logo looks of the themes) relates to it.

Who wrote `tools/effects-reference.js` (the JavaScript reference design the Rust engine was
ported from) is not stated in the project; it is kept unchanged.

## What was imported

- The effect engine: `engine`, `guard` (the flash guard), `palette`, `noise`, `jsmath` (a
  line-by-line port of the fdlibm functions SpiderMonkey uses), `layout`, `sim`, `hw`, `kb`, and
  the 18 catalogue effects (`aurora` ... `tide`).
- The theme catalogue (`src/themes/catalogue.json`, 32 themes), its types, resolution of
  colours and settings, the per-hardware rules (`kinds`), the translated words
  (`src/themes/text/*.json`).
- The test suite that holds the port to the reference: `golden.rs`, `robust.rs`,
  `themes/{integrity,parity,kinds_parity}.rs`, the golden frames in `tests/data/effects-golden/`,
  the expected table in `tests/data/theme-kinds/`, and the generators `tools/gen-golden.js`,
  `tests/data/theme-kinds/derive.js` (run with `gjs`, which CI does not have: the vectors are
  frozen).

Files without the header, and why: `tools/effects-reference.js` and
`tests/data/theme-kinds/mockup-kinds.js` (hash-checked copies of the reference, kept byte for
byte), and the JSON and binary data (`catalogue.json`, `text/*.json`, the golden files). All of
them are the author's work under the same permission.

## What is TrueSense's own

`src/cells.rs` (the adapter from the engine's keys to numbered lighting cells),
`src/themes/validate.rs` (checks for user-written themes), and the edits listed in
`../../THIRD_PARTY.md`.

Acer, Predator, PredatorSense and Helios are trademarks of Acer Inc., used only to name the
hardware this code runs on.
