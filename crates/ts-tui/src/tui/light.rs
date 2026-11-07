//! The keyboard backlight and the light bar as one screen edits them.
//!
//! The hardware cannot say which keyboard effect runs, and one write sets a whole look, so the
//! screen keeps a working copy: what the hardware (or the last look this tool wrote) says,
//! overlaid with the changes made here that nothing has confirmed yet. That is how changing the
//! colour keeps the effect and the speed, and how two quick changes of different settings do not
//! undo each other.

use super::data::{Data, KbLook, ReadKey, RearLook, SlowValue};

/// The least brightness a light comes back at when an effect or a colour is chosen while it is
/// off (keyboard scale 0-50, light bar 0-100): a barely visible 10 % would look broken.
pub(crate) const KB_FLOOR: i64 = 10;
pub(crate) const REAR_FLOOR: i64 = 40;

/// Which light.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Which {
    Keyboard,
    Rear,
}

impl Which {
    pub(crate) fn floor(self) -> i64 {
        match self {
            Which::Keyboard => KB_FLOOR,
            Which::Rear => REAR_FLOOR,
        }
    }
}

/// The changes made on this screen that the hardware has not confirmed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Edits {
    /// The effect (`name` of the keyboard, `mode` of the light bar).
    pub name: Option<String>,
    pub color: Option<String>,
    pub speed: Option<i64>,
    pub brightness: Option<i64>,
    pub direction: Option<i64>,
}

impl Edits {
    pub(crate) fn merge(&mut self, other: &Edits) {
        if other.name.is_some() {
            self.name.clone_from(&other.name);
        }
        if other.color.is_some() {
            self.color.clone_from(&other.color);
        }
        if other.speed.is_some() {
            self.speed = other.speed;
        }
        if other.brightness.is_some() {
            self.brightness = other.brightness;
        }
        if other.direction.is_some() {
            self.direction = other.direction;
        }
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        *self == Edits::default()
    }
}

/// A complete setting: the hardware's, then the edits laid over it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Light {
    pub name: String,
    pub color: String,
    pub speed: i64,
    pub brightness: i64,
    pub direction: i64,
}

impl Light {
    fn overlay(mut self, e: &Edits) -> Self {
        if let Some(v) = &e.name {
            self.name.clone_from(v);
        }
        if let Some(v) = &e.color {
            self.color.clone_from(v);
        }
        if let Some(v) = e.speed {
            self.speed = v;
        }
        if let Some(v) = e.brightness {
            self.brightness = v;
        }
        if let Some(v) = e.direction {
            self.direction = v;
        }
        self
    }

    /// The keyboard: the last look this tool wrote, the firmware's own brightness (Fn+F11/F12
    /// change it too, and the last look would show the old one), then the unconfirmed changes.
    pub(crate) fn keyboard(data: &Data, edits: &Edits) -> Self {
        let mut s =
            Light { name: "static".into(), color: "00aec7".into(), speed: 5, brightness: 25, direction: 0 };
        if let Some(SlowValue::KbLast(last)) = data.get(ReadKey::KbLast) {
            if !last.name.is_empty() {
                s.name.clone_from(&last.name);
            }
            if !last.color.is_empty() {
                s.color.clone_from(&last.color);
            }
            if last.speed > 0 {
                s.speed = last.speed;
            }
            if last.name != "off" {
                s.brightness = last.brightness;
                s.direction = last.direction;
            }
        }
        if let Some(now) = data.int(ReadKey::KbNow) {
            s.brightness = now;
        }
        s.overlay(edits)
    }

    /// The light bar: what the firmware reports, then the unconfirmed changes.
    pub(crate) fn rear(data: &Data, edits: &Edits) -> Self {
        let mut s =
            Light { name: "static".into(), color: "00aec7".into(), speed: 3, brightness: 100, direction: 1 };
        if let Some(SlowValue::Rear(r)) = data.get(ReadKey::Rear) {
            if !r.mode.is_empty() {
                s.name.clone_from(&r.mode);
            }
            if !r.color.is_empty() {
                s.color.clone_from(&r.color);
            }
            if r.speed > 0 {
                s.speed = r.speed;
            }
            s.brightness = r.brightness;
            s.direction = r.direction;
        }
        s.overlay(edits)
    }

    pub(crate) fn as_kb(&self) -> KbLook {
        KbLook {
            name: self.name.clone(),
            color: self.color.clone(),
            speed: self.speed,
            brightness: self.brightness,
            direction: self.direction,
        }
    }

    pub(crate) fn as_rear(&self) -> RearLook {
        RearLook {
            mode: self.name.clone(),
            color: self.color.clone(),
            speed: self.speed,
            brightness: self.brightness,
            direction: self.direction,
        }
    }
}

/// Six hex digits, either case.
pub(crate) fn is_hex_colour(s: &str) -> bool {
    s.len() == 6 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::tui::data::SlowReading;

    fn with(key: ReadKey, v: SlowValue) -> Data {
        let mut d = Data::default();
        d.apply_slow(SlowReading { key, value: Ok(v) });
        d
    }

    #[test]
    fn the_keyboard_shown_is_the_last_look_with_the_firmwares_own_brightness_and_the_edits_on_top() {
        let last =
            KbLook { name: "wave".into(), color: "ff0000".into(), speed: 7, brightness: 20, direction: 2 };
        let mut d = with(ReadKey::KbLast, SlowValue::KbLast(last));
        d.apply_slow(SlowReading { key: ReadKey::KbNow, value: Ok(SlowValue::Int(35)) });
        let s = Light::keyboard(&d, &Edits::default());
        assert_eq!(
            (s.name.as_str(), s.color.as_str(), s.speed, s.brightness, s.direction),
            ("wave", "ff0000", 7, 35, 2)
        );
        let edits = Edits { color: Some("00ff40".into()), brightness: Some(5), ..Edits::default() };
        let s = Light::keyboard(&d, &edits);
        assert_eq!((s.color.as_str(), s.brightness, s.speed), ("00ff40", 5, 7));
    }

    #[test]
    fn an_off_keyboard_shows_as_off_with_the_defaults_for_the_rest() {
        let last = KbLook { name: "off".into(), ..KbLook::default() };
        let d = with(ReadKey::KbLast, SlowValue::KbLast(last));
        let mut d = d;
        d.apply_slow(SlowReading { key: ReadKey::KbNow, value: Ok(SlowValue::Int(0)) });
        let s = Light::keyboard(&d, &Edits::default());
        assert_eq!((s.name.as_str(), s.speed, s.brightness, s.color.as_str()), ("off", 5, 0, "00aec7"));
    }

    #[test]
    fn an_unread_light_has_usable_defaults() {
        let d = Data::default();
        let k = Light::keyboard(&d, &Edits::default());
        assert_eq!((k.name.as_str(), k.brightness, k.speed), ("static", 25, 5));
        let r = Light::rear(&d, &Edits::default());
        assert_eq!((r.name.as_str(), r.brightness, r.speed, r.direction), ("static", 100, 3, 1));
    }

    #[test]
    fn the_light_bar_takes_what_the_firmware_reports() {
        let rear =
            RearLook { mode: "wave".into(), color: "ffffff".into(), speed: 4, brightness: 0, direction: 0 };
        let d = with(ReadKey::Rear, SlowValue::Rear(rear));
        let r = Light::rear(&d, &Edits::default());
        assert_eq!(
            (r.name.as_str(), r.brightness, r.direction, r.speed),
            ("wave", 0, 0, 4),
            "0 is a real brightness"
        );
    }

    #[test]
    fn edits_merge_field_by_field_and_hex_colours_are_checked() {
        let mut a = Edits { speed: Some(3), ..Edits::default() };
        a.merge(&Edits { color: Some("ff8800".into()), ..Edits::default() });
        assert_eq!((a.speed, a.color.as_deref()), (Some(3), Some("ff8800")));
        assert!(!a.is_empty() && Edits::default().is_empty());
        for (text, ok) in [
            ("ff8800", true),
            ("FF8800", true),
            ("ff880", false),
            ("ff88000", false),
            ("gg8800", false),
            ("", false),
            ("#ff880", false),
        ] {
            assert_eq!(is_hex_colour(text), ok, "{text}");
        }
    }
}
