//! What each setting writes when its chosen value is sent, and what each button does.
//!
//! This is the only place that turns a row and a value into an [`Op`]. The labels here are the
//! sentences the status line says ("Power mode: Turbo (held 30 min)"), so what the user reads is
//! exactly what was asked of the hardware.

use super::backend::{FanAction, FanReq, Op, VoiceInput};
use super::data::{FanMode, Mode};
use super::light::{Edits, Light, Which};
use super::model::{HOLD_MINUTES, Modal, Model, OkText, RowRef, RunOpts, Undo};
use super::row::{Id, Value};

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

impl Model {
    /// Writes the chosen value of a row.
    pub(super) fn put(&mut self, r: RowRef, v: Value) {
        let id = self.row(r).id.clone();
        let text = v.as_str().to_owned();
        let on = v.as_bool();
        let n = v.as_int().unwrap_or(0);
        let plain = RunOpts::default;
        match id {
            Id::PowerMode => {
                let label = Mode::from_name(&text).map_or_else(|| text.clone(), |m| m.label().to_owned());
                self.run(
                    &format!("Power mode: {label} (held {HOLD_MINUTES} min)"),
                    Op::PowerMode { mode: text, hold_minutes: HOLD_MINUTES },
                    RunOpts { key: Some("power"), ..plain() },
                );
            }
            Id::AutoMode => self.run(
                &format!("Automatic power mode: {text}"),
                Op::AutoMode(text),
                RunOpts { key: Some("auto"), ..plain() },
            ),
            Id::FanMode | Id::DashFans => self.put_fan_mode(&text),
            Id::FanCpu => self.put_duty(n, true),
            Id::FanGpu => self.put_duty(n, false),
            Id::Curve => self.run(
                &format!("Fan curve: {text}"),
                Op::Curve(text),
                RunOpts { key: Some("curve"), ..plain() },
            ),
            Id::SmartProfile => self.run(
                &format!("Smart profile: {text}"),
                Op::SmartProfile(text),
                RunOpts { key: Some("smartp"), ..plain() },
            ),
            Id::BatteryLimit => {
                self.run(&format!("Battery limit: {}", on_off(on)), Op::BatteryLimit(on), plain());
            }
            Id::Calibration => {
                self.run(&format!("Battery calibration: {}", on_off(on)), Op::Calibration(on), plain());
            }
            Id::UsbCharge => self.run(&format!("USB charging: {n}"), Op::UsbCharge(n), plain()),
            Id::GpuMode => self.run(
                &format!("GPU mode: {text} (reboot to apply)"),
                Op::GpuMode(text),
                RunOpts { key: Some("gpu"), ..plain() },
            ),
            Id::Overdrive => {
                self.run(&format!("Panel overdrive: {}", on_off(on)), Op::Overdrive(on), plain())
            }
            Id::BootSound => self.run(&format!("Boot sound: {}", on_off(on)), Op::BootSound(on), plain()),
            Id::KbTimeout => {
                self.run(&format!("Keyboard timeout: {}", on_off(on)), Op::KbTimeout(on), plain())
            }
            Id::KbEffect => self.kb_put(Edits { name: Some(text), ..Edits::default() }),
            Id::KbColor => self.kb_put(Edits { color: Some(text), ..Edits::default() }),
            Id::KbBrightness => self.kb_put(Edits { brightness: Some(n), ..Edits::default() }),
            Id::KbSpeed => self.kb_put(Edits { speed: Some(n), ..Edits::default() }),
            Id::KbDirection => self.kb_put(Edits { direction: Some(n), ..Edits::default() }),
            Id::KbSaved => self.run(&format!("Lighting: {text}"), Op::SavedLighting(text), plain()),
            Id::RearEffect => self.rear_put(Edits { name: Some(text), ..Edits::default() }),
            Id::RearColor => self.rear_put(Edits { color: Some(text), ..Edits::default() }),
            Id::RearBrightness => self.rear_put(Edits { brightness: Some(n), ..Edits::default() }),
            Id::RearSpeed => self.rear_put(Edits { speed: Some(n), ..Edits::default() }),
            Id::Output => self.run(&format!("Output: {text}"), Op::Output(text), plain()),
            Id::SpeakerFilter => {
                self.run(&format!("Speaker noise filter {}", on_off(on)), Op::SpeakerFilter(on), plain())
            }
            Id::Dts => self.run(&format!("DTS {}", on_off(on)), Op::Dts(on), plain()),
            Id::DtsMode => {
                self.run(&format!("DTS sound: {text} on every output"), Op::DtsMode(text), plain())
            }
            Id::MicMute => self.run(
                &format!("Microphones {}", if on { "muted" } else { "live" }),
                Op::MicMute(on),
                plain(),
            ),
            Id::MicVolume => self.run(&format!("Microphone volume: {n} %"), Op::MicVolume(n), plain()),
            Id::VoiceHeadset => self.run(
                &format!("Headset mic: {text}"),
                Op::Voice { input: VoiceInput::Headset, mode: text },
                plain(),
            ),
            Id::VoiceArray => self.run(
                &format!("Built-in mics: {text}"),
                Op::Voice { input: VoiceInput::Array, mode: text },
                plain(),
            ),
            // Rows that are read-only or buttons are never chosen from.
            _ => {}
        }
    }

    /// What a button does.
    pub(super) fn button(&mut self, r: RowRef) {
        let id = self.row(r).id.clone();
        match id {
            Id::ReleaseHold => {
                self.run("Hold released: automatic power mode is back", Op::ReleaseHold, RunOpts::default())
            }
            Id::ChargeFull => {
                // Read now, on the screen's own state: the write runs later on the worker.
                let active = self.data.bool(super::data::ReadKey::ChargeFull).unwrap_or(false);
                self.run("Charging to 100 % once", Op::ChargeFull { start: !active }, RunOpts::default());
            }
            Id::KbCustom => self.prompt_colour(Which::Keyboard),
            Id::RearCustom => self.prompt_colour(Which::Rear),
            Id::FixSound => self.run(
                "Repairing the sound output",
                Op::RepairSound,
                RunOpts { ok: OkText::Repair, ..RunOpts::default() },
            ),
            Id::DtsEverywhere => self.run(
                "DTS on every output",
                Op::DtsEverywhere,
                RunOpts { ok: OkText::Dts, ..RunOpts::default() },
            ),
            Id::Profile(name) => self.run(
                &format!("Profile {name}"),
                Op::Profile(name.clone()),
                RunOpts { ok: OkText::Text(format!("Profile {name} applied")), ..RunOpts::default() },
            ),
            Id::Look(name) => self.run(&format!("Look {name}"), Op::Look(name), RunOpts::default()),
            Id::Favourite(kind, name) => self.run(
                &format!("Favourite {}", kind.label()),
                Op::Favourite(kind, name.clone()),
                RunOpts {
                    ok: OkText::Text(format!("{} {name} applied", kind.label())),
                    ..RunOpts::default()
                },
            ),
            Id::OpenWindow => self.run("Opening the window", Op::OpenWindow, RunOpts::default()),
            Id::RunDoctor => self.run(
                "Checking everything",
                Op::RunDoctor,
                RunOpts { ok: OkText::Doctor, ..RunOpts::default() },
            ),
            _ => {}
        }
    }

    /// The fan mode row. `off` has been asked about before it gets here; `manual` starts both fans
    /// at what they do now and never stops one (a stop has its own question, on the sliders).
    fn put_fan_mode(&mut self, mode: &str) {
        let key = RunOpts { key: Some("fan"), ..RunOpts::default() };
        match mode {
            "off" => {
                self.run("Fans off", Op::Fan(FanReq { confirm: true, ..FanReq::mode(FanAction::Off) }), key)
            }
            "manual" => {
                let (cpu, gpu) = self.fan_hand(None, None, false);
                self.run(
                    &format!("Fans: manual, CPU {cpu} % GPU {gpu} %"),
                    Op::Fan(FanReq { action: FanAction::Manual, cpu, gpu, confirm: false }),
                    key,
                );
            }
            other => {
                let action = match other {
                    "smart" => FanAction::Smart,
                    "max" => FanAction::Max,
                    _ => FanAction::Auto,
                };
                self.run(&format!("Fans: {other}"), Op::Fan(FanReq::mode(action)), key);
            }
        }
    }

    /// A fan slider. Only the fan that was moved gets the new duty, and only a duty of 0 (which
    /// the slider asked about) is sent with the confirmation.
    fn put_duty(&mut self, n: i64, cpu_fan: bool) {
        let (cpu, gpu) =
            if cpu_fan { self.fan_hand(Some(n), None, true) } else { self.fan_hand(None, Some(n), true) };
        let which = if cpu_fan { "CPU" } else { "GPU" };
        let what = if n == 0 { "stopped".to_owned() } else { format!("{n} %") };
        self.run(
            &format!("{which} fan {what}"),
            Op::Fan(FanReq { action: FanAction::Manual, cpu, gpu, confirm: n == 0 }),
            RunOpts { key: Some("fan"), ..RunOpts::default() },
        );
    }

    /// The manual duties for both fans. A fan that was not touched keeps what it does now: never 0
    /// unless it really is stopped and the mode is already Manual, so moving one slider can never
    /// stop the other fan. With `keep_stopped` false even that is lifted to a running duty.
    fn fan_hand(&self, cpu: Option<i64>, gpu: Option<i64>, keep_stopped: bool) -> (i64, i64) {
        let fan = self.data.fan();
        let manual = fan.is_some_and(|f| f.mode == FanMode::Manual);
        let keep = |asked: Option<i64>, now: Option<i64>| {
            if let Some(v) = asked {
                return v;
            }
            if keep_stopped && manual && now == Some(0) {
                return 0;
            }
            let n = now.filter(|n| *n != 0).unwrap_or(40);
            n.clamp(20, 100)
        };
        (keep(cpu, fan.map(|f| f.cpu_duty)), keep(gpu, fan.map(|f| f.gpu_duty)))
    }

    // ---- the lights

    /// Makes sure choosing an effect or a colour never leaves a light dark: if the brightness is 0
    /// it comes back at the last brightness, at least the light's floor (setting the brightness
    /// itself to 0 is how you switch it off).
    fn lit(&mut self, which: Which, mut edit: Edits, dark_ok: bool) -> Edits {
        let current = self.light(which).brightness;
        // What the light was at before this change, so a later effect can bring it back to that
        // (a slider dragged straight down to 0 is one write: the steps in between were never sent).
        if current > 0 {
            self.last_on[which as usize] = current;
        }
        if edit.brightness.is_none() && current == 0 && !dark_ok {
            edit.brightness = Some(self.last_on[which as usize].max(which.floor()));
        }
        edit
    }

    fn light(&self, which: Which) -> Light {
        match which {
            Which::Keyboard => Light::keyboard(&self.data, &self.kb),
            Which::Rear => Light::rear(&self.data, &self.rear),
        }
    }

    /// The keyboard: lays the change over what is remembered and writes the whole look.
    pub(super) fn kb_put(&mut self, edit: Edits) {
        let before = self.kb.clone();
        let dark_ok = edit.name.as_deref() == Some("off");
        let edit = self.lit(Which::Keyboard, edit, dark_ok);
        if let Some(b) = edit.brightness.filter(|b| *b > 0) {
            self.last_on[Which::Keyboard as usize] = b;
        }
        self.kb.merge(&edit);
        let s = self.light(Which::Keyboard);
        let undo = Some(Undo::Keyboard(before));
        let opts = || RunOpts { key: Some("kb"), undo: undo.clone(), ..RunOpts::default() };
        if s.name == "off" {
            let off = super::data::KbLook { name: "off".to_owned(), ..super::data::KbLook::default() };
            self.run("Keyboard effect: off", Op::Keyboard(off), opts());
            return;
        }
        let what =
            if s.brightness == 0 { "Keyboard light off".to_owned() } else { format!("Keyboard: {}", s.name) };
        self.run(&what, Op::Keyboard(s.as_kb()), opts());
    }

    /// The light bar, the same way. The three part colours set in the window stay until a colour is
    /// picked here.
    pub(super) fn rear_put(&mut self, edit: Edits) {
        let before = self.rear.clone();
        let edit = self.lit(Which::Rear, edit, false);
        if let Some(b) = edit.brightness.filter(|b| *b > 0) {
            self.last_on[Which::Rear as usize] = b;
        }
        self.rear.merge(&edit);
        let s = self.light(Which::Rear);
        let what =
            if s.brightness == 0 { "Light bar off".to_owned() } else { format!("Light bar: {}", s.name) };
        let keep_zones = s.name == "static" && self.rear.color.is_none();
        self.run(
            &what,
            Op::Rear { look: s.as_rear(), keep_zones },
            RunOpts { key: Some("rear"), undo: Some(Undo::Rear(before)), ..RunOpts::default() },
        );
    }

    /// Asks for a colour as six hex digits.
    fn prompt_colour(&mut self, which: Which) {
        let current = self.light(which).color;
        self.modal = Some(Modal::Prompt {
            title: "Colour as 6 hex digits (RRGGBB)".to_owned(),
            buf: current,
            fresh: true,
            which,
        });
        self.touch();
    }
}
