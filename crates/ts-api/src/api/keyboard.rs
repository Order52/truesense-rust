//! The keyboard backlight: effects, brightness, per-key pictures, designs, animations and the
//! saved per-key lighting profiles.
//!
//! The keyboard controller cannot be asked which effect it runs, so everything that is put on it
//! is remembered in the user state (`ts_domain::keyboard`); [`Api::keyboard_status`] reports that
//! memory as `look`.
#![forbid(unsafe_code)]

use std::fmt;
use std::path::{Path, PathBuf};

use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};
use serde_json::json;
use ts_domain::designs;
use ts_domain::keyboard::{
    DEFAULT_COLOR, DEFAULT_EFFECT_BRIGHT, DEFAULT_SPEED, Direction, MAX_BRIGHT, State, effect_names,
};
use ts_domain::keymap::{self, Color, KeymapError, NUM_CELLS, ROWS};
use ts_domain::keyprofiles::CommandOutput;
use ts_domain::perkeyfx::{self, Settings as FxSettings};
use ts_foundation::userstate::keys;

use super::{Api, ApiError, ApiResult};

/// A map that keeps the order its entries were added in, which is the order the Python edition
/// printed them (a `BTreeMap` would sort them).
#[derive(Debug, Clone, PartialEq)]
pub struct Ordered<V>(pub Vec<(String, V)>);

impl<V: Serialize> Serialize for Ordered<V> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (k, v) in &self.0 {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

/// The backlight at a glance. `look` is what this tool last put on the keyboard (the hardware
/// cannot be asked): an empty object when unknown.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct KeyboardStatus {
    /// What was last put on the keyboard.
    pub look: State,
    /// The keyboard's own brightness.
    pub brightness: u8,
    /// The largest brightness.
    pub max: i64,
    /// "Backlight off after 30 s of no typing"; `None` when it cannot be told.
    pub backlight_timeout: Option<bool>,
    /// The saved per-key lighting profiles.
    pub lighting_profiles: Vec<String>,
}

/// The key and group names the lighting commands accept, sorted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KeyNames {
    /// Single keys.
    pub keys: Vec<String>,
    /// Named groups (`wasd`, `arrows`, ...).
    pub groups: Vec<String>,
}

/// One lighting cell of a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct KeyCell {
    /// The cell number, 0..=127.
    pub cell: usize,
    /// Its column, `cell / 6`.
    pub col: usize,
    /// Its row, `cell % 6`.
    pub row: usize,
}

/// Where each key's lighting cells are, for other tools.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct KeyLayout {
    /// Each key and its cells, in the order of the capture.
    pub keys: Ordered<Vec<KeyCell>>,
    /// The group names, sorted.
    pub groups: Vec<String>,
}

/// A hardware effect as the command line gives it. A `brightness` of `None` keeps the keyboard's
/// own level (30 when it is dark; 50 for neon).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectRequest {
    /// The effect (`wave`, `breathing`, ...) or the pseudo effect `operatingmode`.
    pub name: String,
    /// The colour `RRGGBB`.
    pub color: String,
    /// Animation speed, 1 (slowest) to 12 (fastest).
    pub speed: i64,
    /// Brightness 0..=50, or the keyboard's own.
    pub brightness: Option<i64>,
    /// Direction, 0 to 5 for the wave (see [`wave_directions`]); the other effects ignore it.
    pub direction: i64,
}

/// One direction of the hardware wave, for a client that lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct WaveDirection {
    /// The number `keyboard effect` stores and takes, 0 to 5.
    pub value: i64,
    /// The name every client accepts (`clockwise`, `left-to-right`, ...).
    pub id: &'static str,
    /// The byte the keyboard receives.
    pub wire: u8,
}

/// The six wave directions in display order (reference-confirmed on a PH16-71).
pub fn wave_directions() -> Vec<WaveDirection> {
    Direction::ALL
        .iter()
        .map(|d| WaveDirection { value: d.logical(), id: d.id(), wire: d.wire_byte() })
        .collect()
}

/// Reads a direction given as a name or a number into the number `EffectRequest` carries. A
/// number is passed on as it is (only the effect knows whether it is valid: the wave takes 0 to 5,
/// the others any byte); a name must be one of the six.
pub fn parse_direction(text: &str) -> ApiResult<i64> {
    if let Ok(n) = text.trim().parse::<i64>() {
        return Ok(n);
    }
    Direction::parse(text).map(Direction::logical).map_err(|e| ApiError::invalid(e.to_string()))
}

impl EffectRequest {
    /// An effect with what `keyboard effect NAME` uses for what is not given.
    pub fn new(name: &str) -> Self {
        Self { name: name.to_owned(), color: "00aec7".to_owned(), speed: 5, brightness: None, direction: 0 }
    }
}

/// A key or group and the colour to paint it, from `key=RRGGBB`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaintItem {
    /// A key or group name.
    pub key: String,
    /// The colour `RRGGBB`.
    pub color: String,
}

impl PaintItem {
    /// Parses `key=RRGGBB`.
    pub fn parse(item: &str) -> ApiResult<Self> {
        match item.split_once('=') {
            Some((key, color)) if !color.is_empty() => {
                Ok(Self { key: key.to_owned(), color: color.to_owned() })
            }
            _ => Err(ApiError::invalid(format!("'{item}': use key=RRGGBB"))),
        }
    }

    /// Parses every item.
    pub fn parse_all<S: AsRef<str>>(items: &[S]) -> ApiResult<Vec<Self>> {
        items.iter().map(|i| Self::parse(i.as_ref())).collect()
    }
}

/// One still design of the list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DesignInfo {
    /// What it looks like.
    pub info: String,
    /// `built-in` or `yours`.
    pub source: String,
}

/// A per-key animation: the mode (`off` stops it), its speed 1..=10 and the comma list of keys or
/// groups it covers (none: every key).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnimationRequest {
    /// `off` or an animation name.
    pub mode: String,
    /// 1 (slowest) to 10 (fastest).
    pub speed: i64,
    /// `wasd,arrows,f5`; `None` is every key.
    pub keys: Option<String>,
}

impl fmt::Display for AnimationRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "per-key animation: {}", self.mode)?;
        if self.mode != "off" {
            write!(
                f,
                " on {} (needs the key service: systemctl --user status truesense-keys)",
                self.keys.as_deref().unwrap_or("all keys")
            )?;
        }
        Ok(())
    }
}

/// A keyboard backlight setting as a window or terminal edits it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyboardLook {
    /// The effect name; `off` means the light was switched off; empty means nothing is remembered.
    pub name: String,
    /// The colour, when known.
    pub color: Option<String>,
    /// The speed, when known.
    pub speed: Option<i64>,
    /// The brightness.
    pub brightness: i64,
    /// The direction.
    pub direction: i64,
}

/// The answer of a saved-lighting command: the names for `list`, a message for the others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LightingProfileAnswer {
    /// The profile names.
    Names(Vec<String>),
    /// A line for the user.
    Message(String),
}

impl fmt::Display for LightingProfileAnswer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Names(n) if n.is_empty() => f.write_str("(no lighting profiles)"),
            Self::Names(n) => f.write_str(&n.join("\n")),
            Self::Message(m) => f.write_str(m),
        }
    }
}

/// Colours given as `rrggbb` strings (at most 128) as a full picture.
pub(crate) fn parse_cells(cells: &[String]) -> ApiResult<Vec<Color>> {
    if cells.len() > NUM_CELLS {
        return Err(ApiError::invalid(format!(
            "a picture has at most {NUM_CELLS} cells, not {}",
            cells.len()
        )));
    }
    let mut out = vec![[0u8; 3]; NUM_CELLS];
    for (i, text) in cells.iter().enumerate() {
        out[i] =
            keymap::parse_hex(text).map_err(|e: KeymapError| ApiError::invalid(format!("cell {i}: {e}")))?;
    }
    Ok(out)
}

/// A picture as `rrggbb` strings.
pub(crate) fn hex_cells(cells: &[Color]) -> Vec<String> {
    cells.iter().map(|c| keymap::hex(*c)).collect()
}

impl Api {
    /// Reads the keyboard backlight.
    pub fn keyboard_status(&self) -> ApiResult<KeyboardStatus> {
        Ok(KeyboardStatus {
            look: self.s.keyboard.last(),
            brightness: self.s.keyboard.get_brightness()?,
            max: MAX_BRIGHT,
            backlight_timeout: self.s.hw.get_kb_timeout()?,
            lighting_profiles: self.s.lighting.names(),
        })
    }

    /// Switches the backlight `on`, `off` or `toggle`, as the key would, and answers the message the
    /// key would show.
    pub fn keyboard_power(&self, action: &str) -> ApiResult<String> {
        match action {
            "on" | "off" | "toggle" => self.run_action(&format!("keyboard:{action}")),
            other => Err(ApiError::invalid(format!("keyboard: on | off | toggle (got '{other}')"))),
        }
    }

    /// Puts the stock cyan per-key picture back, at the brightness the keyboard has now.
    pub fn restore_keyboard(&self) -> ApiResult<()> {
        Ok(self.s.keyboard.restore_default()?)
    }

    /// Every key and group name the lighting commands accept, sorted.
    pub fn key_names(&self) -> KeyNames {
        let mut keys: Vec<String> = self.s.keymap.names().iter().map(|(n, _)| n.clone()).collect();
        keys.sort();
        let mut groups: Vec<String> = self.s.keymap.groups().iter().map(|(n, _)| (*n).to_owned()).collect();
        groups.sort();
        KeyNames { keys, groups }
    }

    /// Where each key's lighting cells are.
    pub fn key_layout(&self) -> KeyLayout {
        let keys = self
            .s
            .keymap
            .names()
            .iter()
            .map(|(name, cells)| {
                let cells =
                    cells.iter().map(|c| KeyCell { cell: *c, col: c / ROWS, row: c % ROWS }).collect();
                (name.clone(), cells)
            })
            .collect();
        KeyLayout { keys: Ordered(keys), groups: self.key_names().groups }
    }

    /// The hardware effect names in display order (`off` first).
    pub fn keyboard_effects() -> Vec<&'static str> {
        effect_names()
    }

    /// Puts a hardware effect on the keyboard. The pseudo effect `operatingmode` makes the colour
    /// follow the power mode.
    pub fn set_keyboard_effect(&self, request: &EffectRequest) -> ApiResult<()> {
        let brightness = match request.brightness {
            Some(b) => b,
            None if request.name == "neon" => 50,
            None => match self.s.keyboard.get_brightness().ok().map(i64::from).filter(|b| *b != 0) {
                Some(b) => b,
                None => DEFAULT_EFFECT_BRIGHT,
            },
        };
        self.s.keyboard.set_effect(
            &request.name,
            &request.color,
            request.speed,
            brightness,
            request.direction,
        )?;
        // A per-key animation would draw over the effect: it ends here, after the effect is set,
        // so the key service sees a newer look and does not put the animation's colours back.
        let fx = self.s.fx.get();
        if fx.mode != "off"
            && let Some(t0) = fx.t0
        {
            self.s.fx.end(t0);
        }
        Ok(())
    }

    /// Changes the colour of the current look without changing the effect.
    pub fn set_keyboard_color(&self, color: &str) -> ApiResult<()> {
        Ok(self.s.keyboard.set_color(color)?)
    }

    /// Changes the animation speed (1 slowest to 12 fastest) of the running effect.
    pub fn set_keyboard_speed(&self, level: i64) -> ApiResult<()> {
        Ok(self.s.keyboard.set_speed(level)?)
    }

    /// The keyboard's own brightness (0..=50).
    pub fn keyboard_brightness(&self) -> ApiResult<u8> {
        Ok(self.s.keyboard.get_brightness()?)
    }

    /// Changes the brightness without leaving the current look.
    pub fn set_keyboard_brightness(&self, level: i64) -> ApiResult<()> {
        Ok(self.s.keyboard.set_brightness(level)?)
    }

    /// Colours the named keys and groups and keeps every other key as it is. Answers how many names
    /// were painted; nothing is uploaded when a name or colour is bad.
    pub fn paint_keys(&self, items: &[PaintItem], brightness: Option<i64>) -> ApiResult<usize> {
        let pairs: Vec<(&str, &str)> = items.iter().map(|i| (i.key.as_str(), i.color.as_str())).collect();
        Ok(self.s.keyboard.paint(self.s.keymap, &pairs, brightness)?)
    }

    /// Paints the named keys and groups over a base colour (everything else).
    pub fn set_key_colors(&self, items: &[PaintItem], base: &str, brightness: Option<i64>) -> ApiResult<()> {
        let mut canvas = keymap::canvas(keymap::parse_hex(base)?);
        for item in items {
            let rgb = keymap::parse_hex(&item.color)?;
            for cell in self.s.keymap.cells_for(&item.key)? {
                canvas[cell] = rgb;
            }
        }
        Ok(self.s.keyboard.per_key(&canvas, brightness)?)
    }

    /// Paints every key one colour.
    pub fn fill_keyboard(&self, color: &str, brightness: Option<i64>) -> ApiResult<()> {
        Ok(self.s.keyboard.per_key(&keymap::canvas(keymap::parse_hex(color)?), brightness)?)
    }

    /// Paints a left-to-right blend of two colours.
    pub fn gradient_keyboard(&self, from: &str, to: &str, brightness: Option<i64>) -> ApiResult<()> {
        let picture = keymap::gradient(keymap::parse_hex(from)?, keymap::parse_hex(to)?);
        Ok(self.s.keyboard.per_key(&picture, brightness)?)
    }

    /// Paints a rainbow across the keys.
    pub fn rainbow_keyboard(&self, brightness: Option<i64>) -> ApiResult<()> {
        Ok(self.s.keyboard.per_key(&keymap::rainbow(), brightness)?)
    }

    /// Paints a picture file across the keys (scaled to the 22 x 6 key grid). Needs an image
    /// decoder in this build.
    pub fn image_keyboard(&self, path: &Path, brightness: Option<i64>) -> ApiResult<()> {
        let decoder = self.s.image.as_ref().ok_or_else(|| {
            ApiError::unavailable("painting a picture needs an image decoder, which this build does not have")
        })?;
        let cells = designs::image_cells(decoder.as_ref(), path)?;
        Ok(self.s.keyboard.per_key(&cells, brightness)?)
    }

    /// Lists every design (built-in, and yours which win) with its description and source.
    pub fn designs(&self) -> Ordered<DesignInfo> {
        Ordered(
            self.s
                .designs
                .load_all()
                .into_iter()
                .map(|(n, d)| (n, DesignInfo { info: d.info, source: d.source.name().to_owned() }))
                .collect(),
        )
    }

    /// Paints a design on the keyboard.
    pub fn apply_design(&self, name: &str, brightness: Option<i64>) -> ApiResult<()> {
        if name.is_empty() {
            return Err(ApiError::invalid("give a design name"));
        }
        let design = self.s.designs.get(name)?;
        let cells = design.cells_of(self.s.keymap)?;
        Ok(self.s.keyboard.per_key(&cells, brightness)?)
    }

    /// Stores the picture now on the keyboard as one of your designs and answers its file.
    pub fn save_design(&self, name: &str) -> ApiResult<PathBuf> {
        if name.is_empty() {
            return Err(ApiError::invalid("give a design name"));
        }
        Ok(self.s.designs.save(name, &self.s.keyboard.current_cells())?)
    }

    /// Starts or stops a per-key animation. The key service draws it (about 5 frames a second);
    /// this records the choice, after uploading the picture it starts from.
    pub fn set_keyboard_fx(&self, request: &AnimationRequest) -> ApiResult<FxSettings> {
        let mut cells = Vec::new();
        if let Some(list) = request.keys.as_deref().filter(|k| !k.is_empty()) {
            for key in list.split(',') {
                cells.extend(self.s.keymap.cells_for(key.trim())?.into_iter().map(|c| c as i64));
            }
            cells.sort_unstable();
            cells.dedup();
        }
        self.set_keyboard_fx_cells(&request.mode, request.speed, &cells, None)
    }

    /// [`Api::set_keyboard_fx`] with the keys as cell numbers (none: every key) and the colours under
    /// the animation given (`None`: the picture now on the keyboard, uploaded first). Answers the
    /// settings stored.
    pub fn set_keyboard_fx_cells(
        &self,
        mode: &str,
        speed: i64,
        cells: &[i64],
        base: Option<&[String]>,
    ) -> ApiResult<FxSettings> {
        if mode != "off" {
            // Two loops cannot draw on one keyboard: a running theme gives way.
            self.theme_store().stop()?;
        }
        let under = match base {
            Some(colours) => parse_cells(colours)?,
            None => {
                if mode != "off" {
                    self.s.keyboard.per_key(&self.s.keyboard.current_cells(), None)?;
                }
                Vec::new()
            }
        };
        // Stopping needs no speed, but the settings keep a valid one.
        let speed = if mode == "off" && speed == 0 { perkeyfx::SPEED_MIN } else { speed };
        Ok(self.s.fx.set(mode, speed, cells, &under)?)
    }

    /// Whether the keyboard colour follows the power mode (the service re-sends it on each change).
    pub fn operating_follow(&self) -> ApiResult<bool> {
        self.daemon_bool("operating_follow")
    }

    /// Switches that and answers the value the service holds.
    pub fn set_operating_follow(&self, on: bool) -> ApiResult<bool> {
        self.daemon_set_bool("operating_follow", on)
    }

    /// The saved per-key lighting profiles.
    pub fn lighting_profile_names(&self) -> Vec<String> {
        self.s.lighting.names()
    }

    /// Puts a saved per-key lighting on the keyboard, at the brightness the keyboard has.
    pub fn apply_lighting_profile(&self, name: &str) -> ApiResult<()> {
        Ok(self.s.lighting.apply(name, None)?)
    }

    /// `keyboard profile list|load|save|delete NAME [spec ...]`; for `save` the spec is `fill COLOR`,
    /// `gradient C1 C2` or `rainbow`.
    pub fn lighting_profile(
        &self,
        action: &str,
        name: Option<&str>,
        spec: &[&str],
    ) -> ApiResult<LightingProfileAnswer> {
        Ok(match self.s.lighting.command(action, name, spec)? {
            CommandOutput::Names(names) => LightingProfileAnswer::Names(names),
            CommandOutput::Message(text) => LightingProfileAnswer::Message(text),
        })
    }

    /// Saves the given colours (`None`: the picture now on the keyboard) as a lighting profile and
    /// answers the message the command prints.
    pub fn save_lighting_profile_cells(&self, name: &str, cells: Option<&[String]>) -> ApiResult<String> {
        let picture = match cells {
            Some(c) => parse_cells(c)?,
            None => self.s.keyboard.current_cells(),
        };
        self.s.lighting.save(name, &picture)?;
        Ok(format!("saved lighting profile '{name}'"))
    }

    /// What this tool last put on the keyboard. Name `off` means the light was switched off (by a
    /// key, a command or a window); an empty name means nothing is remembered.
    pub fn keyboard_last(&self) -> KeyboardLook {
        let st = self.s.keyboard.last();
        let off = !self.s.user.get_bool(keys::KEYBOARD_ON, true);
        KeyboardLook {
            name: if off { "off".to_owned() } else { st.name.clone() },
            color: (!st.color.is_empty()).then(|| st.color.clone()),
            speed: st.speed,
            brightness: st.brightness_or(0),
            direction: st.direction_or(0),
        }
    }

    /// Puts a look on the keyboard: name `off` switches the light off keeping the look it had,
    /// anything else is a hardware effect and brings the light back on.
    pub fn apply_keyboard_look(&self, look: &KeyboardLook) -> ApiResult<()> {
        if look.name == "off" {
            self.s.keyboard.off()?;
            return self.remember_keyboard_on(false);
        }
        self.s.keyboard.set_effect(
            &look.name,
            look.color.as_deref().unwrap_or(DEFAULT_COLOR),
            look.speed.unwrap_or(DEFAULT_SPEED),
            look.brightness,
            look.direction,
        )?;
        self.remember_keyboard_on(true)
    }

    fn remember_keyboard_on(&self, on: bool) -> ApiResult<()> {
        Ok(self.s.user.set(keys::KEYBOARD_ON, json!(on))?)
    }

    /// The colours of the per-key picture now (128 of them; the last two are not keys).
    pub fn keyboard_cells(&self) -> Vec<String> {
        hex_cells(&self.s.keyboard.current_cells())
    }

    /// Uploads a per-key picture given as `rrggbb` strings; `None` keeps the keyboard's own level.
    pub fn set_keyboard_cells(&self, cells: &[String], brightness: Option<i64>) -> ApiResult<()> {
        Ok(self.s.keyboard.per_key(&parse_cells(cells)?, brightness)?)
    }
}
