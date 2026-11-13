//! keyboard: the keyboard backlight of the Predator Helios 16.
//!
//! The keyboard is the Chicony `04F2:0117` controller; its lighting interface (interface 3,
//! usage page `0xFF02`) takes 8-byte feature reports and 64-byte output reports through hidraw
//! (see `docs/PROTOCOL.md`). Every packet is seven data bytes plus the checksum
//! `0xFF - (sum of the seven) & 0xFF`.
//!
//! # How the device is held
//!
//! Opening the interface costs about 50 ms on the PH16-71 (the USB transfers themselves take
//! 0.3 ms), so a [`Keyboard`] opens it **once**, lazily, and keeps the handle. Every sequence of
//! transfers runs under one lock, so a brightness read can never land in the middle of a frame
//! upload. A stale handle (the keyboard re-enumerates after suspend) is dropped and the whole
//! sequence is retried once on a fresh one; a second failure is returned and nothing stale is kept.
//! [`Keyboard::prewarm`] pays the 50 ms in the background.
//!
//! # What is remembered
//!
//! The hardware cannot be asked which effect is running, so every look this module applies is
//! remembered in the user state (`keyboard`, and `kb_restore` for the look before `off`) as a
//! [`State`]. Brightness, speed and colour changes re-send the remembered effect with the new value.
//!
//! The device itself is behind [`HidDevice`] / [`HidOpener`]: the real hidraw node
//! ([`HidrawOpener`]), the simulator ([`SimHid`], `TRUESENSE_SIM=1`) and the recording fakes of
//! the tests all drive the same code.

mod direction;
mod hidraw;
mod sim;
mod state;

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use ts_foundation::sync::locked;
use ts_foundation::userstate::{self, Store};
use ts_platform::firmware::FirmwareError;
use ts_platform::hw::{HwError, KbTimeoutFlag, Sleeper};

use crate::keymap::{self, Color, KeyMap, KeymapError};

pub use direction::{Direction, DirectionError, allowed as direction_choices, takes_direction};
pub use hidraw::{HidrawOpener, find_node, is_lighting_interface};
pub use sim::SimHid;
pub use state::{State, mode};

/// The length of every feature report: seven data bytes and a checksum.
pub(crate) const PACKET_LEN: usize = 8;
/// The length of one per-key output report: 16 cells of 4 bytes.
pub(crate) const OUTPUT_LEN: usize = 64;

/// The keyboard's brightness range is 0 to this.
pub const MAX_BRIGHT: i64 = 50;
/// The slowest animation speed of the UI scale.
///
/// The keyboard stores the speed byte 0 (fastest) to 11 (slowest) and reads it back (byte 3 of
/// the `88` report); 12 and above are refused and become 6 (measured on the PH16-71). The UI
/// scale runs the other way round, 1 = slowest to 12 = fastest, and the byte is
/// `SPEED_MAX - speed`. (Before 0.3.10 the scale was 1..11 with byte = 10 - speed: the two
/// slowest steps were unreachable and the top setting sent 0xFF, which the keyboard turns into a
/// medium speed.)
pub const SPEED_MIN: i64 = 1;
/// The fastest animation speed of the UI scale; see [`SPEED_MIN`].
pub const SPEED_MAX: i64 = 12;

/// The colour that fills in what a caller (or a saved look) left out.
pub const DEFAULT_COLOR: &str = "ffffff";
/// The speed used when none was given or stored.
pub const DEFAULT_SPEED: i64 = 5;
/// The effect brightness used by callers that were not given one.
pub const DEFAULT_EFFECT_BRIGHT: i64 = 30;
/// The brightness used instead of 0 when a picture is uploaded: never upload into darkness.
pub const DEFAULT_BRIGHTNESS: i64 = 25;
/// The colour of the operating mode (the stock cyan).
pub(crate) const OPERATING_COLOR: &str = "00aec7";
/// The brightness of the operating mode (0x32: full).
pub(crate) const OPERATING_BRIGHTNESS: i64 = 0x32;
/// The brightness of the factory look.
pub(crate) const FACTORY_BRIGHTNESS: i64 = 12;

/// How far apart the per-key output reports are written. Windows spaces them about 15 ms apart;
/// unpaced writes lose the last report (the last row of keys keeps its old colour).
pub(crate) const FRAME_SPACING: Duration = Duration::from_millis(16);
/// The pause after the last report, so the controller can finish before the packet that commits
/// the upload.
pub const FRAME_SETTLE: Duration = Duration::from_millis(50);

/// The hardware effects with their codes, in the order users see them.
pub const EFFECTS: [(&str, u8); 14] = [
    ("off", 0),
    ("static", 1),
    ("breathing", 2),
    ("wave", 3),
    ("snake", 5),
    ("ripple", 6),
    ("neon", 8),
    ("raindrop", 10),
    ("lightning", 18),
    ("spot", 37),
    ("star", 38),
    ("fireball", 39),
    ("snow", 40),
    ("heartbeat", 41),
];

/// The effects that ignore the colour packet.
const NO_COLOR: [&str; 3] = ["off", "wave", "neon"];

/// The pseudo effect that hands over to [`Keyboard::set_operating_mode`].
const OPERATING_PSEUDO_EFFECT: &str = "operatingmode";

/// A failure of the keyboard.
#[derive(Debug)]
pub enum KeyboardError {
    /// A value was refused before anything was sent; the text says what is allowed (the text is
    /// the Python edition's, so messages reach the user unchanged).
    Invalid(String),
    /// No hidraw node is the keyboard's lighting interface.
    NotFound,
    /// The node exists but this user may not open it.
    Permission {
        /// The device node.
        node: PathBuf,
    },
    /// Talking to the device failed at the operating-system level.
    Io {
        /// What was being done.
        op: &'static str,
        /// The operating-system error.
        source: io::Error,
    },
    /// A key name, group or colour was not usable.
    Keymap(KeymapError),
    /// The remembered per-key colours are damaged.
    BadState(KeymapError),
}

impl std::fmt::Display for KeyboardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => f.write_str(message),
            Self::NotFound => f.write_str("keyboard lighting interface (usage page 0xFF02) not found"),
            Self::Permission { node } => write!(
                f,
                "no permission on {}: add yourself to group plugdev and install the udev rule",
                node.display()
            ),
            Self::Io { op, source } => write!(f, "{op}: {source}"),
            Self::Keymap(e) => e.fmt(f),
            Self::BadState(e) => write!(f, "remembered per-key colours: {e}"),
        }
    }
}

impl std::error::Error for KeyboardError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Keymap(e) | Self::BadState(e) => Some(e),
            Self::Invalid(_) | Self::NotFound | Self::Permission { .. } => None,
        }
    }
}

impl From<KeymapError> for KeyboardError {
    fn from(e: KeymapError) -> Self {
        Self::Keymap(e)
    }
}

fn invalid(text: impl Into<String>) -> KeyboardError {
    KeyboardError::Invalid(text.into())
}

/// The keyboard's vendor HID interface as the controller sees it. The report id (always 0 on
/// this device) is the implementation's business: payloads here are data only. Dropping the value
/// closes the device.
pub trait HidDevice: Send {
    /// Sends one feature report (`HIDIOCSFEATURE`).
    fn set_feature(&mut self, payload: &[u8; PACKET_LEN]) -> io::Result<()>;
    /// Reads a feature report (`HIDIOCGFEATURE`), report id stripped.
    fn get_feature(&mut self) -> io::Result<[u8; PACKET_LEN]>;
    /// Sends one output report of 64 data bytes.
    fn write_output(&mut self, data: &[u8; OUTPUT_LEN]) -> io::Result<()>;
}

/// Opens the [`HidDevice`]. The [`Keyboard`] calls it lazily, and again after a failure.
pub trait HidOpener: Send + Sync {
    /// Finds and opens the keyboard's lighting interface.
    fn open(&self) -> Result<Box<dyn HidDevice>, KeyboardError>;
}

/// The checksum of the first seven bytes of a packet: `0xFF - (sum & 0xFF)`.
pub fn checksum(data: &[u8]) -> u8 {
    0xFF - data.iter().fold(0u8, |sum, b| sum.wrapping_add(*b))
}

/// An 8-byte packet from its seven data bytes.
pub fn packet(data: [u8; 7]) -> [u8; PACKET_LEN] {
    let mut p = [0u8; PACKET_LEN];
    p[..7].copy_from_slice(&data);
    p[7] = checksum(&data);
    p
}

/// The hardware effect names in display order.
pub fn effect_names() -> Vec<&'static str> {
    EFFECTS.iter().map(|(n, _)| *n).collect()
}

/// The hardware code of an effect.
pub(crate) fn effect_code(name: &str) -> Option<u8> {
    EFFECTS.iter().find(|(n, _)| *n == name).map(|(_, c)| *c)
}

/// Whether the effect takes a colour (`wave` and `neon` do not; `off` has none to take).
pub(crate) fn has_color(name: &str) -> bool {
    !NO_COLOR.contains(&name)
}

/// The colours of the operating-mode lighting that follows the power mode. The vendor's
/// per-mode colours are not known: these defaults are ours, overridable in the config
/// `[keyboard.operating_colors]`.
pub fn operating_colors() -> BTreeMap<String, String> {
    [
        ("eco", "00cc66"),
        ("quiet", "3366ff"),
        ("balanced", "00aec7"),
        ("performance", "ff9900"),
        ("turbo", "ff0000"),
    ]
    .into_iter()
    .map(|(m, c)| (m.to_owned(), c.to_owned()))
    .collect()
}

/// Reads `RRGGBB` for the keyboard (leading `#` allowed).
fn hexc(color: &str) -> Result<Color, KeyboardError> {
    keymap::parse_hex(color).map_err(|_| invalid("colour must be RRGGBB"))
}

/// A brightness as the effect and per-key packets take it: 0 to 50.
fn brightness_byte(level: i64) -> Result<u8, KeyboardError> {
    u8::try_from(level)
        .ok()
        .filter(|b| i64::from(*b) <= MAX_BRIGHT)
        .ok_or_else(|| invalid(format!("brightness 0..{MAX_BRIGHT}")))
}

/// A request packet (command, checksum) and the controller's answer.
fn query(dev: &mut dyn HidDevice, cmd: u8, chk: u8) -> Result<[u8; PACKET_LEN], KeyboardError> {
    dev.set_feature(&[cmd, 0, 0, 0, 0, 0, 0, chk])
        .map_err(|source| KeyboardError::Io { op: "send feature report", source })?;
    dev.get_feature().map_err(|source| KeyboardError::Io { op: "get feature report", source })
}

fn set_feature(dev: &mut dyn HidDevice, p: &[u8; PACKET_LEN]) -> Result<(), KeyboardError> {
    dev.set_feature(p).map_err(|source| KeyboardError::Io { op: "send feature report", source })
}

/// The keyboard backlight controller. It is safe to share between threads.
pub struct Keyboard {
    opener: Arc<dyn HidOpener>,
    state: Arc<Store>,
    sleep: Sleeper,
    /// The kept handle; the lock also serialises every sequence of transfers.
    device: Mutex<Option<Box<dyn HidDevice>>>,
}

impl Keyboard {
    /// A controller that opens its device through `opener` and remembers looks in `state`.
    /// Nothing is opened yet.
    pub fn new(opener: Arc<dyn HidOpener>, state: Arc<Store>) -> Self {
        Self { opener, state, sleep: Arc::new(thread::sleep), device: Mutex::new(None) }
    }

    /// Replaces the function that paces the per-key frames (tests pass a recorder).
    #[must_use]
    pub fn with_sleeper(mut self, sleep: Sleeper) -> Self {
        self.sleep = sleep;
        self
    }

    /// A controller over a fresh simulated keyboard, without frame pacing.
    pub fn simulated(state: Arc<Store>) -> (Keyboard, SimHid) {
        let sim = SimHid::new();
        let keyboard = Keyboard::new(Arc::new(sim.clone()), state).with_sleeper(Arc::new(|_| {}));
        (keyboard, sim)
    }

    /// Runs `body` on the open device under the lock; if it fails, the handle is dropped and the
    /// whole body runs once more on a fresh one. Failing to open is not retried.
    fn call<T>(
        &self,
        mut body: impl FnMut(&mut dyn HidDevice) -> Result<T, KeyboardError>,
    ) -> Result<T, KeyboardError> {
        let mut guard = locked(&self.device);
        let mut attempt = 0;
        loop {
            if guard.is_none() {
                *guard = Some(self.opener.open()?);
            }
            let result = match guard.as_mut() {
                Some(dev) => body(dev.as_mut()),
                None => return Err(invalid("keyboard device is not open")),
            };
            match result {
                Ok(v) => return Ok(v),
                Err(e) => {
                    *guard = None;
                    attempt += 1;
                    if attempt == 2 {
                        return Err(e);
                    }
                }
            }
        }
    }

    /// Releases the device. It is opened again if the keyboard is used again.
    pub fn close(&self) {
        *locked(&self.device) = None;
    }

    /// Opens the device in the background (about 50 ms): call it early in a process that will
    /// talk to the keyboard soon, so that time overlaps other work. Failures are left for the
    /// first real call to report. Join the handle only if you need to wait.
    pub fn prewarm(self: &Arc<Self>) -> thread::JoinHandle<()> {
        let keyboard = Arc::clone(self);
        thread::spawn(move || {
            let _ = keyboard.call(|_| Ok(()));
        })
    }

    fn send(&self, packets: &[[u8; PACKET_LEN]]) -> Result<(), KeyboardError> {
        self.call(|dev| packets.iter().try_for_each(|p| set_feature(dev, p)))
    }

    fn remember(&self, key: &str, state: &State) {
        // A convenience file: failing to write it never fails the light.
        let _ = self.state.set_as(key, state);
        if key == userstate::keys::KEYBOARD {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0.0, |d| d.as_secs_f64());
            let _ = self.state.set_as(userstate::keys::KEYBOARD_SET_AT, &now);
        }
    }

    /// What this tool last put on the keyboard (empty if unknown).
    pub fn last(&self) -> State {
        self.state.get_as(userstate::keys::KEYBOARD).unwrap_or_default()
    }

    /// When the look was last set (seconds since the epoch), if known. It moves on even when the
    /// same look is set again, which [`Keyboard::last`] cannot show.
    pub fn last_set_at(&self) -> Option<f64> {
        self.state.get_as(userstate::keys::KEYBOARD_SET_AT)
    }

    /// Puts a hardware effect on the keyboard. The pseudo effect `operatingmode` hands over to
    /// [`Keyboard::set_operating_mode`] (`color` is then the only argument used). `speed` is
    /// 1 to 12 and `brightness` 0 to 50; a brightness of 0 is a real, dark setting. The colour
    /// is ignored (and not checked) for the effects that take none.
    pub fn set_effect(
        &self,
        name: &str,
        color: &str,
        speed: i64,
        brightness: i64,
        direction: i64,
    ) -> Result<(), KeyboardError> {
        if name == OPERATING_PSEUDO_EFFECT {
            return self.set_operating_mode(color, OPERATING_BRIGHTNESS);
        }
        let code =
            effect_code(name).ok_or_else(|| invalid(format!("effect: {}", effect_names().join(", "))))?;
        let bright = brightness_byte(brightness)
            .ok()
            .filter(|_| (SPEED_MIN..=SPEED_MAX).contains(&speed))
            .ok_or_else(|| invalid(format!("speed {SPEED_MIN}..{SPEED_MAX}, brightness 0..{MAX_BRIGHT}")))?;
        direction::check(name, direction).map_err(|e| invalid(e.to_string()))?;
        // The packet carries direction + 1 in one byte (the Wave's six are `Direction`).
        let direction_byte = u8::try_from(direction)
            .ok()
            .and_then(|d| d.checked_add(1))
            .ok_or_else(|| invalid("direction 0..254"))?;
        let [r, g, b] = if has_color(name) { hexc(color)? } else { [0; 3] };
        let prelude = [0xB1, 0, 0, 0, 0, 0, 0, 0x4E];
        if name == "off" {
            self.send(&[prelude, packet([0x08, 0x02, 0, 0, 0, 0, 0]), packet([0x14, 0, 0, 0, 0, 0, 0])])?;
            self.remember(userstate::keys::KEYBOARD, &State { mode: mode::OFF.into(), ..State::default() });
            return Ok(());
        }
        // `SPEED_MAX - speed` is 0..=11 (checked above): the byte the keyboard accepts.
        let speed_byte = (SPEED_MAX - speed) as u8;
        self.send(&[
            prelude,
            packet([0x14, 0, 0, r, g, b, 0]),
            packet([0x08, 0x02, code, speed_byte, bright, 0x01, direction_byte]),
        ])?;
        self.remember(
            userstate::keys::KEYBOARD,
            &State {
                mode: mode::EFFECT.into(),
                name: name.to_owned(),
                color: state::normalise_color(color),
                speed: Some(speed),
                brightness: Some(brightness),
                direction: Some(direction),
                cells: Vec::new(),
            },
        );
        Ok(())
    }

    /// Selects OPERATINGMODE (`0x22`): the keyboard colour reflects the power mode
    /// (`docs/PROTOCOL.md`: no prelude; colour packet `14 00 01 R G B 00 chk`, effect packet
    /// `08 03 22 05 32 02 01 chk`). `brightness` is 0 to 255; the controller takes 0x32 as full.
    pub fn set_operating_mode(&self, color: &str, brightness: i64) -> Result<(), KeyboardError> {
        let [r, g, b] = hexc(color)?;
        let bright = u8::try_from(brightness).map_err(|_| invalid("brightness 0..255"))?;
        self.send(&[
            packet([0x14, 0, 0x01, r, g, b, 0]),
            packet([0x08, 0x03, 0x22, 0x05, bright, 0x02, 0x01]),
        ])?;
        self.remember(
            userstate::keys::KEYBOARD,
            &State {
                mode: mode::OPERATING.into(),
                color: state::normalise_color(color),
                brightness: Some(brightness),
                ..State::default()
            },
        );
        Ok(())
    }

    /// Operating-mode lighting that tracks the power mode: the keyboard does not do it by itself
    /// (verified), so the colour is re-sent on each change. `colors` overrides
    /// [`operating_colors`]; a mode with no colour at all gets the stock cyan.
    pub fn follow_mode(&self, mode: &str, colors: &BTreeMap<String, String>) -> Result<(), KeyboardError> {
        let mut merged = operating_colors();
        merged.extend(colors.iter().map(|(m, c)| (m.clone(), c.clone())));
        let color = merged.get(mode).map_or(OPERATING_COLOR, String::as_str);
        self.set_operating_mode(color, OPERATING_BRIGHTNESS)
    }

    /// The current keyboard brightness (0 to 50) as the controller reports it: byte 4 of the
    /// answer to the `88` packet. Verified: it tracks writes, including Fn+F11 and Fn+F12.
    pub fn get_brightness(&self) -> Result<u8, KeyboardError> {
        self.call(|dev| Ok(query(dev, 0x88, 0x77)?[4]))
    }

    /// The brightness, or [`DEFAULT_BRIGHTNESS`] when the keyboard is dark.
    fn brightness_or_default(&self) -> Result<i64, KeyboardError> {
        Ok(match self.get_brightness()? {
            0 => DEFAULT_BRIGHTNESS,
            b => i64::from(b),
        })
    }

    /// Uploads a per-key picture (`docs/PROTOCOL.md`): 128 cells of 4 bytes (`00 R G B`) as eight
    /// 64-byte output reports, cell = column * 6 + row. Missing cells are black, cells past 128
    /// are ignored. `brightness` of `None` keeps what the user set with Fn+F11/F12 (and never
    /// uploads into darkness: a dark keyboard gets 25); `Some(0)` really is dark. The picture is
    /// remembered as the current look.
    pub fn per_key(&self, cells: &[Color], brightness: Option<i64>) -> Result<(), KeyboardError> {
        self.upload(cells, brightness, true)
    }

    /// [`Keyboard::per_key`] for animation frames: the picture is not remembered, so the look the
    /// user chose survives the animation.
    pub fn per_key_frame(&self, cells: &[Color], brightness: Option<i64>) -> Result<(), KeyboardError> {
        self.upload(cells, brightness, false)
    }

    fn upload(&self, cells: &[Color], brightness: Option<i64>, remember: bool) -> Result<(), KeyboardError> {
        let brightness = match brightness {
            Some(b) => b,
            None => self.brightness_or_default()?,
        };
        let bright = brightness_byte(brightness)?;
        let mut full = vec![[0u8; 3]; keymap::NUM_CELLS];
        for (slot, cell) in full.iter_mut().zip(cells) {
            *slot = *cell;
        }
        let mut reports = [[0u8; OUTPUT_LEN]; keymap::NUM_CELLS * 4 / OUTPUT_LEN];
        for (i, [r, g, b]) in full.iter().copied().enumerate() {
            let at = (i % 16) * 4;
            reports[i / 16][at..at + 4].copy_from_slice(&[0, r, g, b]);
        }
        self.call(|dev| {
            query(dev, 0x88, 0x77)?;
            set_feature(dev, &[0x12, 0, 0, 0x08, 0, 0, 0, 0xE5])?;
            for report in &reports {
                dev.write_output(report)
                    .map_err(|source| KeyboardError::Io { op: "write per-key report", source })?;
                (self.sleep)(FRAME_SPACING);
            }
            (self.sleep)(FRAME_SETTLE);
            set_feature(dev, &packet([0x08, 0x02, 0x33, 0x05, bright, 0x08, 0x01]))
        })?;
        if remember {
            self.remember(
                userstate::keys::KEYBOARD,
                &State {
                    mode: mode::PERKEY.into(),
                    brightness: Some(brightness),
                    cells: state::cells_hex(&full),
                    ..State::default()
                },
            );
        }
        Ok(())
    }

    /// The factory look of the keyboard: cells 0 to 125 are `#00aec7`, brightness 12.
    pub fn restore_default(&self) -> Result<(), KeyboardError> {
        self.per_key(&factory_cells(), Some(FACTORY_BRIGHTNESS))
    }

    /// Changes the brightness WITHOUT leaving the current look. The per-key brightness packet
    /// (mode `0x33`) would drop a running effect back to static, so the effect is re-sent with the
    /// new level. If this tool did not set the current look (unknown), the per-key packet is used.
    pub fn set_brightness(&self, level: i64) -> Result<(), KeyboardError> {
        let bright = brightness_byte(level)?;
        let s = self.last();
        if s.mode == mode::EFFECT && effect_code(&s.name).is_some() && s.name != "off" {
            // The keyboard's own brightness level first (some effects, spot for one, follow it) ...
            self.send(&[packet([0x08, 0x02, 0x33, 0x05, bright, 0x08, 0x01])])?;
            return self.set_effect(
                &s.name,
                s.color_or(DEFAULT_COLOR),
                s.speed_or(DEFAULT_SPEED),
                level,
                s.direction_or(0),
            );
        }
        if s.mode == mode::OPERATING {
            return self.set_operating_mode(s.color_or(OPERATING_COLOR), level);
        }
        self.send(&[packet([0x08, 0x02, 0x33, 0x05, bright, 0x08, 0x01])])?;
        if s.mode == mode::PERKEY {
            self.remember(userstate::keys::KEYBOARD, &State { brightness: Some(level), ..s });
        }
        Ok(())
    }

    /// Backlight off, keeping the look: brightness 0 in whatever mode is running (the effect
    /// `off` packet did not blank this keyboard). The look and the brightness it had are
    /// remembered (`kb_restore`) for [`Keyboard::on`].
    pub fn off(&self) -> Result<(), KeyboardError> {
        let s = self.last();
        // Like the Python edition this reads the brightness twice: the first read decides whether
        // there is anything to restore, the second is the level that is saved, so a Fn+F11 press
        // in between is honoured. A transfer takes 0.3 ms.
        if s.mode != mode::OFF && self.get_brightness()? > 0 {
            let level = i64::from(self.get_brightness()?);
            self.remember(userstate::keys::KB_RESTORE, &State { brightness: Some(level), ..s });
        }
        self.set_brightness(0)
    }

    /// Backlight back on, in the look it had before [`Keyboard::off`] (brightness 25 if no level
    /// was saved).
    pub fn on(&self) -> Result<(), KeyboardError> {
        let saved: Option<State> = self.state.get_as(userstate::keys::KB_RESTORE);
        let level =
            saved.as_ref().and_then(|s| s.brightness).filter(|b| *b != 0).unwrap_or(DEFAULT_BRIGHTNESS);
        if let Some(s) = saved.filter(|s| !s.mode.is_empty()) {
            self.remember(userstate::keys::KEYBOARD, &s);
        }
        self.set_brightness(level)
    }

    /// Puts a remembered look back on the keyboard. Returns what was done, or `None` if there
    /// was nothing to restore (an unknown effect, a per-key look without colours, mode `off`:
    /// the caller decides about that one).
    pub fn replay(&self, s: &State) -> Result<Option<String>, KeyboardError> {
        if s.mode == mode::EFFECT && effect_code(&s.name).is_some() {
            self.set_effect(
                &s.name,
                s.color_or(DEFAULT_COLOR),
                s.speed_or(DEFAULT_SPEED),
                s.brightness_or(DEFAULT_BRIGHTNESS),
                s.direction_or(0),
            )?;
            return Ok(Some(format!("effect {}", s.name)));
        }
        if s.mode == mode::OPERATING {
            self.set_operating_mode(s.color_or(OPERATING_COLOR), s.brightness_or(OPERATING_BRIGHTNESS))?;
            return Ok(Some("operating mode".to_owned()));
        }
        if s.mode == mode::PERKEY && !s.cells.is_empty() {
            let cells = s.parse_cells().map_err(KeyboardError::BadState)?;
            let level = s.brightness.filter(|b| *b != 0).unwrap_or(DEFAULT_BRIGHTNESS);
            self.per_key(&cells, Some(level))?;
            return Ok(Some("per-key colours".to_owned()));
        }
        Ok(None)
    }

    /// [`Keyboard::replay`] of what was last put on the keyboard.
    pub fn replay_last(&self) -> Result<Option<String>, KeyboardError> {
        self.replay(&self.last())
    }

    /// Changes the animation speed (1 slowest to 12 fastest) of the running effect, keeping its
    /// colour and brightness.
    pub fn set_speed(&self, speed: i64) -> Result<(), KeyboardError> {
        if !(SPEED_MIN..=SPEED_MAX).contains(&speed) {
            return Err(invalid(format!("speed {SPEED_MIN}..{SPEED_MAX}")));
        }
        let s = self.last();
        if s.mode != mode::EFFECT || s.name.is_empty() || s.name == "off" || s.name == "static" {
            return Err(invalid("the current look has no animation speed; pick an effect first"));
        }
        // The Python edition evaluates this default eagerly, so the brightness is read from the
        // keyboard even when the look remembers one. The transcript is kept identical.
        let current = self.brightness_or_default()?;
        self.set_effect(
            &s.name,
            s.color_or(DEFAULT_COLOR),
            speed,
            s.brightness_or(current),
            s.direction_or(0),
        )
    }

    /// Changes the colour of the current look without changing the effect. The effects without a
    /// colour (`wave`, `neon`) cannot take one. A look that is not an effect or the operating mode
    /// (including an unknown one) becomes a plain fill of that colour.
    pub fn set_color(&self, color: &str) -> Result<(), KeyboardError> {
        let rgb = hexc(color)?;
        let color = state::normalise_color(color);
        let s = self.last();
        let level = self.brightness_or_default()?;
        if s.mode == mode::EFFECT && effect_code(&s.name).is_some() {
            if !has_color(&s.name) {
                return Err(invalid(format!(
                    "the {} effect has no colour; pick another effect (or use `keyboard fill`)",
                    s.name
                )));
            }
            return self.set_effect(&s.name, &color, s.speed_or(DEFAULT_SPEED), level, s.direction_or(0));
        }
        if s.mode == mode::OPERATING {
            return self.set_operating_mode(&color, level);
        }
        self.per_key(&keymap::canvas(rgb), Some(level))
    }

    /// The per-key colours this tool last uploaded (128 cells); the stock cyan if none are
    /// remembered or the remembered ones are damaged.
    pub fn current_cells(&self) -> Vec<Color> {
        let s = self.last();
        if s.mode == mode::PERKEY
            && !s.cells.is_empty()
            && let Ok(mut cells) = s.parse_cells()
        {
            cells.resize(keymap::NUM_CELLS, [0; 3]);
            return cells;
        }
        factory_cells()
    }

    /// Colours named keys or groups (`("w", "ff0000"), ("arrows", "00ff00")`) and leaves every
    /// other key as it is. When two names cover the same key the later one wins. Returns how many
    /// names were painted; nothing is uploaded if any name or colour is bad.
    pub fn paint(
        &self,
        map: &KeyMap,
        items: &[(&str, &str)],
        brightness: Option<i64>,
    ) -> Result<usize, KeyboardError> {
        let mut cells = self.current_cells();
        for (key, color) in items {
            let rgb = keymap::parse_hex(color)?;
            for c in map.cells_for(key)? {
                cells[c] = rgb;
            }
        }
        self.per_key(&cells, brightness)?;
        Ok(items.len())
    }

    /// Colours individual cells (`(cell, "rrggbb")`) and keeps every other key as it is.
    pub fn paint_cells(&self, changes: &[(i64, &str)], brightness: Option<i64>) -> Result<(), KeyboardError> {
        let mut cells = self.current_cells();
        for (cell, color) in changes {
            let slot = usize::try_from(*cell)
                .ok()
                .and_then(|c| cells.get_mut(c))
                .ok_or_else(|| invalid(format!("cell '{cell}': 0..{}", keymap::NUM_CELLS - 1)))?;
            *slot = hexc(color)?;
        }
        self.per_key(&cells, brightness)
    }

    /// The keyboard controller's OWN "backlight off after 30 s" flag, which is separate from the
    /// firmware (WMI) flag: it is read by sending `b0 00 .. 4f` and reading byte 2 of the
    /// answer. Its factory state is ON, which is why the lights went dark after 30 s while the
    /// firmware flag read "off". `None` when the controller's answer is not the expected report.
    pub fn hid_timeout_get(&self) -> Result<Option<bool>, KeyboardError> {
        self.call(|dev| {
            let r = query(dev, 0xB0, 0x4F)?;
            Ok((r[0] == 0xB0).then_some(r[2] != 0))
        })
    }

    /// Sets the controller's own backlight-timeout flag: `30 01 <0|1> 00 00 00 00 chk`.
    pub fn hid_timeout_set(&self, on: bool) -> Result<(), KeyboardError> {
        self.send(&[packet([0x30, 0x01, u8::from(on), 0, 0, 0, 0])])
    }

    /// The controller's timeout flag as the value `ts_platform::hw::Hw::with_kb_timeout` takes.
    pub fn timeout_flag(self: &Arc<Self>) -> TimeoutFlag {
        TimeoutFlag { keyboard: Arc::clone(self) }
    }
}

/// The factory picture: the stock cyan on cells 0 to 125, the last two black.
fn factory_cells() -> Vec<Color> {
    keymap::canvas([0x00, 0xAE, 0xC7])
}

/// The keyboard controller's timeout flag for the `hw` layer, which sits below this module and
/// only declares the trait.
pub struct TimeoutFlag {
    keyboard: Arc<Keyboard>,
}

fn to_hw(e: KeyboardError, operation: &str) -> HwError {
    HwError::Firmware(FirmwareError::Io {
        operation: operation.to_owned(),
        source: io::Error::other(e.to_string()),
    })
}

impl KbTimeoutFlag for TimeoutFlag {
    fn get(&self) -> Result<Option<bool>, HwError> {
        self.keyboard.hid_timeout_get().map_err(|e| to_hw(e, "read keyboard timeout flag"))
    }

    fn set(&self, on: bool) -> Result<(), HwError> {
        self.keyboard.hid_timeout_set(on).map_err(|e| to_hw(e, "set keyboard timeout flag"))
    }
}

#[cfg(test)]
mod direction_tests;
#[cfg(test)]
mod tests;
