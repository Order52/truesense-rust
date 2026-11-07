//! A still picture of a theme for the gallery: the 128 cell colours of one frame of its effect.
//!
//! The gallery shows a small keyboard on every theme card. The picture is drawn by the same
//! engine and the same key map that draw the theme on the keyboard, so it shows what the theme
//! looks like with the chosen colourway, not a guess. It is a pure function of the theme, the
//! user's choices and the time: no clock, no device, and the engine's own default seed, so the
//! same request always gives the same picture (which is what lets a client cache it).
//!
//! The effect is run from time 0 to `t` in steps of [`STEP`] seconds before the frame is taken,
//! because several effects keep a state (rain, fire, ripples) that only exists after a moment.
//! It is never sent to the keyboard, so a theme whose flashing was not measured (which the key
//! service refuses to start) can still be pictured.
#![forbid(unsafe_code)]

use ts_fx::cells::CellPainter;
use ts_fx::engine::Input;

use super::{ThemeSpec, ThemeStore, ThemesError, invalid};
use crate::keymap::{Color, KeyMap};

/// The time step the effect is advanced by, the key service's frame rate (15 a second).
pub const STEP: f64 = 1.0 / 15.0;
/// The moment of a theme's life the gallery pictures when no time is asked for: long enough for the
/// effects that need a moment to fill the keyboard.
pub const DEFAULT_T: f64 = 3.0;
/// The latest moment a picture may ask for, which bounds the work of one request.
pub const MAX_T: f64 = 30.0;

/// The 128 colours of the frame of `spec` at time `t` seconds (`None`: [`DEFAULT_T`]). Speed and
/// brightness of the spec do not change a picture: the speed only moves it along in time and the
/// keyboard's brightness is not a colour.
pub fn preview(store: &ThemeStore, spec: &ThemeSpec, t: Option<f64>) -> Result<Vec<Color>, ThemesError> {
    let t = t.unwrap_or(DEFAULT_T);
    if !t.is_finite() || !(0.0..=MAX_T).contains(&t) {
        return Err(invalid(format!("preview time {t}: 0 to {MAX_T} seconds")));
    }
    spec.check_shape()?;
    let theme = store.get(&spec.name)?;
    let values = spec.values(&theme)?;
    let mut effect = theme
        .keyboard_effect(&values)
        .ok_or_else(|| invalid(format!("theme '{}' names an effect this build does not have", spec.name)))?;
    let painter = CellPainter::new(KeyMap::verified());
    let keys = ts_fx::ph16_71().iter().count();
    let steps = (t / STEP).round() as u32;
    let mut now = 0.0;
    let mut picture = painter.paint(effect.frame(now, 0.0, &Input::default()));
    for step in 1..=steps {
        now += STEP;
        // Themes that react to typing (ripples, comets, sparks) show nothing on an untouched
        // keyboard, so three keys are "pressed" in the last second of the run, at places that
        // depend only on the step: the same request is always the same picture.
        let press = [(step as usize * 37 + 11) % keys.max(1)];
        let pressed: &[usize] = if matches!(steps - step, 4 | 10 | 16) { &press } else { &[] };
        let input = Input { key_presses: pressed, ..Input::default() };
        picture = painter.paint(effect.frame(now, STEP, &input));
    }
    Ok(picture)
}
