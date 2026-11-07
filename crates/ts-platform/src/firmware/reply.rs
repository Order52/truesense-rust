//! A parsed reply of the kernel module: `ok key=value ...` or `err message`.

use serde::{Serialize, Serializer, ser::SerializeMap};

use super::FirmwareError;

/// The fields of an `ok` reply, in the order the module printed them (the probe report shows
/// them in that order). A key that appears twice keeps its first position and its last value,
/// like a Python dict, which the first edition used.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Reply {
    fields: Vec<(String, String)>,
}

impl Reply {
    /// Parses one reply line. `command` is only context for the error: an `err` reply becomes
    /// [`FirmwareError::Rejected`] carrying the module's message (`error` when it gave none), and
    /// anything that is neither `ok` nor `err` is [`FirmwareError::BadReply`]. Never panics.
    pub fn parse(command: &str, line: &str) -> Result<Self, FirmwareError> {
        let line = line.trim();
        let (word, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        match word {
            "err" => {
                let reason = rest.trim();
                Err(FirmwareError::Rejected {
                    command: command.to_string(),
                    reason: if reason.is_empty() { "error" } else { reason }.to_string(),
                })
            }
            "ok" => {
                let mut reply = Self::default();
                for token in rest.split_whitespace() {
                    if let Some((key, value)) = token.split_once('=') {
                        reply.set(key, value);
                    }
                }
                Ok(reply)
            }
            _ => Err(FirmwareError::BadReply { detail: format!("unexpected reply {line:?}") }),
        }
    }

    fn set(&mut self, key: &str, value: &str) {
        match self.fields.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = value.to_string(),
            None => self.fields.push((key.to_string(), value.to_string())),
        }
    }

    /// The text of a field.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// A numeric field: decimal (possibly negative: the module prints `-1` for "unknown") or
    /// `0x`-prefixed hexadecimal. A missing or malformed field is a [`FirmwareError::BadReply`].
    pub fn int(&self, key: &str) -> Result<i64, FirmwareError> {
        self.get(key).and_then(parse_int).ok_or_else(|| FirmwareError::BadReply {
            detail: format!("missing/invalid '{key}' in reply {{{}}}", self.render()),
        })
    }

    /// The fields in reply order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.fields.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// The number of fields.
    pub fn len(&self) -> usize {
        self.fields.len()
    }

    /// Whether the reply had no fields (a bare `ok`).
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    fn render(&self) -> String {
        self.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join(", ")
    }
}

fn parse_int(text: &str) -> Option<i64> {
    match text.strip_prefix("0x") {
        Some(hex) => {
            // from_str_radix would also accept a sign here ("0x-5"); a hex field never has one.
            if hex.is_empty() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            i64::from_str_radix(hex, 16).ok()
        }
        None => text.parse().ok(),
    }
}

/// Serialised as a JSON object, fields in reply order (the probe report embeds replies).
impl Serialize for Reply {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.fields.len()))?;
        for (key, value) in &self.fields {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}
