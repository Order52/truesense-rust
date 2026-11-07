//! The commands: one function per area, all over the same context.
//!
//! Each handler reads its words from [`Matches`], calls the application layer and prints through
//! [`Cx`], which owns the difference between text and `--json` output.
#![forbid(unsafe_code)]

mod audio;
mod battery;
mod cpu;
mod fan;
mod gpu;
mod keyboard;
mod keys;
mod looks;
mod power;
mod profile;
mod rear;
mod status;
mod system;
mod update;

use std::fmt::Display;

use serde::Serialize;
use ts_api::api::{Api, ApiError, on_off_opt};

use super::Launcher;
use super::argparse::Matches;
use super::out::{Io, dumps, plain_text};

/// Why a command ended without finishing.
pub(super) enum Fail {
    /// `error: text` on standard error, exit code 1 (the only way a command fails).
    Error(String),
}

impl From<ApiError> for Fail {
    fn from(e: ApiError) -> Self {
        Self::Error(e.to_string())
    }
}

/// A command's result: the exit code, or the reason it failed.
pub(super) type Outcome = Result<i32, Fail>;

/// A failure with this text.
pub(super) fn fail(text: impl Display) -> Fail {
    Fail::Error(text.to_string())
}

/// Everything a command needs: the application layer, the launcher, the streams, and whether
/// `--json` was given.
pub(super) struct Cx<'a, 'b> {
    pub(super) api: &'a Api,
    pub(super) launcher: &'a dyn Launcher,
    pub(super) io: &'a mut Io<'b>,
    pub(super) json: bool,
}

impl Cx<'_, '_> {
    /// Prints a line (`print(text)`).
    pub(super) fn say(&mut self, text: &str) {
        let _ = writeln!(self.io.out, "{text}");
    }

    /// Prints a line on standard error.
    pub(super) fn warn(&mut self, text: &str) {
        let _ = writeln!(self.io.err, "{text}");
    }

    /// The Python `emit`: JSON of the value under `--json`; otherwise the human text when there
    /// is one, else indented JSON for a list or dictionary, `on`/`off` for a boolean, nothing for
    /// `null`, and the value itself for text and numbers.
    pub(super) fn emit<T: Serialize + ?Sized>(&mut self, value: &T, human: Option<&str>) {
        let json = dumps(value);
        if self.json {
            self.say(&json);
        } else if let Some(text) = human {
            self.say(text);
        } else if json.starts_with('{') || json.starts_with('[') {
            self.say(&json);
        } else if let Some(text) = plain_text(&json) {
            self.say(&text);
        }
    }

    /// `emit(text)`: a message that is also the JSON value.
    pub(super) fn emit_text(&mut self, text: &str) {
        self.emit(text, None);
    }

    /// The `on / off / toggle / status` pattern of every boolean setting. The current value is
    /// read first; `get` answers `None` when the setting cannot be read.
    pub(super) fn tri(
        &mut self,
        action: Option<&str>,
        label: &str,
        get: impl FnOnce() -> Result<Option<bool>, Fail>,
        set: impl FnOnce(bool) -> Result<bool, Fail>,
    ) -> Outcome {
        let current = get()?;
        let new = match action {
            None | Some("status") => current,
            Some("toggle") => Some(set(!current.unwrap_or(false))?),
            Some(other) => Some(set(other == "on")?),
        };
        let text = format!("{label}: {}", on_off_opt(new));
        self.emit(&new, Some(&text));
        Ok(0)
    }
}

/// Runs the command the matches name.
pub(super) fn dispatch(api: &Api, launcher: &dyn Launcher, io: &mut Io<'_>, json: bool, m: &Matches) -> i32 {
    let mut cx = Cx { api, launcher, io, json };
    let path: Vec<&str> = m.path.iter().map(String::as_str).collect();
    let result = run_path(&mut cx, &path, m);
    match result {
        Ok(code) => code,
        Err(Fail::Error(text)) => {
            cx.warn(&format!("error: {text}"));
            1
        }
    }
}

fn run_path(cx: &mut Cx<'_, '_>, path: &[&str], m: &Matches) -> Outcome {
    let area = path.first().copied().unwrap_or_default();
    let verb = path.get(1).copied();
    match area {
        "status" => status::status(cx),
        "sensors" => status::sensors(cx),
        "power" => power::power(cx, verb, m),
        "mode" => super::modebind::run(cx, verb, m),
        "fan" => fan::fan(cx, verb, m),
        "battery" => battery::battery(cx, verb, m),
        "display" => battery::display(cx, m),
        "boot" => battery::boot(cx, m),
        "touchpad" => battery::touchpad(cx, m),
        "gpu" => gpu::gpu(cx, verb, m),
        "rear" => rear::rear(cx, verb, m),
        "update" => update::update(cx, verb, m),
        "look" => looks::look(cx, verb, m),
        "startup" => looks::startup(cx, verb, m),
        "keyboard" => keyboard::keyboard(cx, verb, m),
        "audio" => audio::audio(cx, verb, m),
        "keys" => keys::keys(cx, verb, m),
        "profile" => profile::profile(cx, verb, m),
        "cpu" => cpu::cpu(cx, verb, m),
        "service" => system::service(cx, verb, m),
        "config" => system::config(cx, verb),
        "doctor" => system::doctor(cx),
        "probe" => system::probe(cx),
        "selftest" => system::selftest(cx, m),
        "completion" => system::completion(cx, m),
        "gui" => system::gui(cx, m),
        "tui" => system::tui(cx, m),
        "daemon" => system::daemon(cx),
        "version" => system::version(cx),
        other => Err(fail(format!("unknown command {other}"))),
    }
}

/// `i64` to `i32` for a value that goes to the daemon; the Python edition passed any integer on
/// and the rating refused what was out of range, here nothing that large is meaningful.
pub(super) fn narrow(value: i64, what: &str) -> Result<i32, Fail> {
    i32::try_from(value).map_err(|_| fail(format!("{what}: {value} is out of range")))
}

/// A JSON object that keeps the order the keys were added in (the Python dictionaries did).
pub(super) struct Obj(pub(super) Vec<(&'static str, serde_json::Value)>);

impl Serialize for Obj {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (k, v) in &self.0 {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}
