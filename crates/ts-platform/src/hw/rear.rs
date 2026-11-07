//! The rear light bar.

use serde::{Serialize, Serializer};
use serde_json::json;

use super::{Coded, Hw, HwError, Reading, RearMode, Rgb};

/// The light bar's three parts as firmware zone bits, left to right as seen from behind the
/// laptop (left confirmed by the owner; middle and right assumed).
pub const REAR_ZONES: [u8; 3] = [1, 2, 4];

/// The light bar as the firmware reports it, or (after an unverified write) as it was set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Rear {
    /// The reply as hex; absent when the value was not read back.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
    /// The effect. An effect code we do not know is shown as the bare number, as the Python
    /// edition did.
    #[serde(serialize_with = "serialize_mode")]
    pub mode: Reading<RearMode>,
    /// Effect speed, 1..5.
    pub speed: u8,
    /// Brightness, 0..100 (0 is off).
    pub brightness: u8,
    /// Effect direction, 0 or 1; absent when the value was not read back.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direction: Option<u8>,
    /// The effect colour.
    pub color: Rgb,
    /// The colours of the three parts, left to right; only after a static effect.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zones: Option<Vec<Rgb>>,
}

fn serialize_mode<S: Serializer>(mode: &Reading<RearMode>, serializer: S) -> Result<S::Ok, S::Error> {
    match mode {
        Reading::Known(m) => serializer.serialize_str(m.name()),
        Reading::Unknown(n) => serializer.serialize_i64(*n),
    }
}

/// A complete light bar setting. Numbers are plain integers and are checked in
/// [`Hw::rear_set`], so that one place owns the messages (the window and the CLI pass the
/// user's numbers through unchanged). Start from [`RearSettings::default`] and change what you
/// need: a brightness of 0 is meaningful, so there is no "unset".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RearSettings {
    /// The effect.
    pub mode: RearMode,
    /// The effect colour (the static effect ignores it and uses `zones` or this colour in all three parts).
    pub color: Rgb,
    /// Effect speed, 1..5.
    pub speed: i64,
    /// Brightness, 0..100.
    pub brightness: i64,
    /// Direction, 0 or 1.
    pub direction: i64,
    /// One colour, or three (left, middle, right); only the static effect can colour the parts
    /// differently.
    pub zones: Option<Vec<Rgb>>,
    /// Read back and confirm. Off for live sliders and colour drags, where the next write
    /// follows at once.
    pub verify: bool,
}

impl Default for RearSettings {
    fn default() -> Self {
        Self {
            mode: RearMode::Static,
            color: Rgb([0xff, 0xff, 0xff]),
            speed: 3,
            brightness: 100,
            direction: 1,
            zones: None,
            verify: true,
        }
    }
}

impl Hw {
    /// Reads the light bar. Byte 0 of the reply is a status byte; the 16-byte layout of
    /// `set_rear` follows it.
    pub fn rear_get(&self) -> Result<Rear, HwError> {
        let reply = self.exec("get_rear")?;
        let raw = reply.get("raw").unwrap_or("");
        let bytes = decode_hex(raw).filter(|b| b.len() >= 9).ok_or_else(|| {
            HwError::Firmware(crate::firmware::FirmwareError::BadReply {
                detail: format!("unexpected rear reply {raw:?}"),
            })
        })?;
        let b = &bytes[1..];
        Ok(Rear {
            raw: Some(raw.to_ascii_lowercase()),
            mode: Reading::from_code(i64::from(b[0])),
            speed: b[1],
            brightness: b[2],
            direction: Some(b[4]),
            color: Rgb([b[5], b[6], b[7]]),
            zones: None,
        })
    }

    /// Writes the light bar. In the static effect the colour is a separate firmware call per
    /// part (the effect packet's colour bytes are ignored), so the three parts are written after
    /// the packet. The colours are remembered, and the result is read back unless `verify` is off.
    pub fn rear_set(&self, settings: &RearSettings) -> Result<Rear, HwError> {
        let mut color = settings.color;
        let zones = match &settings.zones {
            None => None,
            Some(z) => {
                let z = match z.as_slice() {
                    [one] => vec![*one; 3],
                    [_, _, _] => z.clone(),
                    _ => {
                        return Err(HwError::Invalid(
                            "zones: give one colour, or three (left middle right)".to_string(),
                        ));
                    }
                };
                if settings.mode != RearMode::Static {
                    return Err(HwError::Invalid(
                        "different colours per part only work with the static effect".to_string(),
                    ));
                }
                color = z[0];
                Some(z)
            }
        };
        let (speed, brightness, direction) = match (
            u8::try_from(settings.speed).ok().filter(|v| (1..=5).contains(v)),
            u8::try_from(settings.brightness).ok().filter(|v| *v <= 100),
            u8::try_from(settings.direction).ok().filter(|v| *v <= 1),
        ) {
            (Some(s), Some(b), Some(d)) => (s, b, d),
            _ => return Err(HwError::Invalid("speed 1..5, brightness 0..100, direction 0|1".to_string())),
        };
        let mode = settings.mode;
        // mode, speed, brightness, 0, direction, R, G, B, 3, 2, then six zero bytes: the layout
        // the module validates (bytes 8 and 9 are fixed, the rest must be zero).
        let code =
            u8::try_from(mode.code()).map_err(|_| HwError::Invalid(RearMode::INVALID_NAME.to_string()))?;
        let mut packet = vec![code, speed, brightness, 0, direction];
        packet.extend_from_slice(&color.0);
        packet.extend_from_slice(&[3, 2, 0, 0, 0, 0, 0, 0]);
        self.exec(&format!("set_rear {}", encode_hex(&packet)))?;

        let mut painted = None;
        if mode == RearMode::Static {
            let parts = zones.clone().unwrap_or_else(|| vec![color; 3]);
            for (zone, part) in REAR_ZONES.iter().zip(&parts) {
                self.exec(&format!("set_color {part} {zone}"))?;
            }
            self.remember("rear_zones", json!(parts.iter().map(Rgb::to_string).collect::<Vec<_>>()));
            painted = Some(parts);
        }
        self.remember(
            "rear_state",
            json!({
                "mode": mode.name(), "color": color.to_string(), "speed": speed, "brightness": brightness,
                "direction": direction,
                "zones": if mode == RearMode::Static { zones.as_ref().map(|z| z.iter().map(Rgb::to_string).collect::<Vec<_>>()) } else { None },
            }),
        );
        if !settings.verify {
            return Ok(Rear {
                raw: None,
                mode: Reading::Known(mode),
                speed,
                brightness,
                direction: None,
                color,
                zones: painted,
            });
        }
        let read = || self.rear_get().map(|r| (r.mode, r.brightness));
        self.settle("rear light", &(Reading::Known(mode), brightness), read, |(m, b)| format!("({m}, {b})"))?;
        let mut rear = self.rear_get()?;
        rear.zones = painted;
        Ok(rear)
    }

    fn remember(&self, key: &str, value: serde_json::Value) {
        if let Some(memory) = &self.memory {
            memory.remember(key, value);
        }
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Decodes hex without ever panicking on odd length, non-hex or non-ASCII input.
fn decode_hex(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    if bytes.len() % 2 != 0 {
        return None;
    }
    bytes
        .chunks(2)
        .map(|pair| {
            let hi = char::from(pair[0]).to_digit(16)?;
            let lo = char::from(pair[1]).to_digit(16)?;
            u8::try_from(hi * 16 + lo).ok()
        })
        .collect()
}
