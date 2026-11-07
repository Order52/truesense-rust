// Adapted from predator-sense by cleyton1986 (GPL-3.0), used with the author's permission.
//! The built-in lighting themes: the catalogue effects.js declares as `THEMES`, as typed
//! data.
//!
//! - [`catalogue`]: the 32 themes, their looks per light, colourways, Custom slot labels and
//!   settings, read from `catalogue.json`, which gen-golden.js cuts out of effects.js.
//! - [`Theme::apply`] with [`Values`] (the chosen colourway or Custom colours and the
//!   settings), [`Theme::setting_value`] and [`Theme::look_for`]: effects.js's
//!   applySettings, settingValue and lookFor, held to effects.js by `parity.rs`.
//! - [`Hardware`] (the kinds of keyboard, bar and logo a machine has), [`Theme::lights`],
//!   [`Theme::hide_reason`], [`gallery`] and [`hidden_note`]: what each light shows and which
//!   themes are offered, ported from the reference design and held to it by
//!   `kinds_parity.rs`, except that software looks run only on the lights where app-drawn
//!   frames are measured ([`LightKind::app_drawn_measured`]).
//! - [`Theme::text_in`]: the catalogue's words in a language, by stable key, English from the
//!   catalogue and other languages from `text/<code>.json`.
//! - [`Theme::problems`] and [`Theme::keyboard_effect`]: the checks a theme that is not built
//!   in must pass, and the effect that draws a theme on the PH16-71 keyboard.

mod catalogue;
#[cfg(test)]
mod integrity;
mod kinds;
#[cfg(test)]
mod kinds_parity;
mod looks;
#[cfg(test)]
mod parity;
mod resolve;
mod text;
mod validate;

pub use catalogue::{
    Colourway, Field, Look, SettingDef, SettingKind, Target, Theme, builtin, catalogue, find, parse,
};
pub use kinds::{
    BarKind, Hardware, HiddenSummary, HideReason, KeyboardKind, LightKind, LogoKind, NearestKind, PALETTE7,
    ThemeLights, gallery, hidden_note, nearest7,
};
pub use looks::LightLook;
pub use resolve::{ColourwayChoice, SettingValue, Values};
pub use text::{Lang, TextItem};
pub use validate::{MAX_NAME, is_valid_id};
