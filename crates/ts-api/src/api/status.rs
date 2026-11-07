//! Status: everything at a glance, the live readings the window polls, and the one-line summary.
#![forbid(unsafe_code)]

use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};
use serde_json::{Value, json};
use ts_domain::keyboard::State as KeyboardState;
use ts_foundation::fsx;
use ts_ipc::wire::Args;
use ts_platform::hw::{Coded, FanState, Reading, Sensors, UsbLevel};

use super::{Api, ApiResult};

/// `truesense status`: every reading at a glance. It serialises as one JSON object in the order
/// the sections were read (the order the Python edition printed), and a reading that failed is the
/// text `ERROR: ...` instead, so one broken part never hides the others.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Status {
    entries: Vec<(String, Value)>,
}

impl Status {
    /// The sections in reading order.
    pub fn entries(&self) -> &[(String, Value)] {
        &self.entries
    }

    /// One section; `None` when there is none.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    fn push(&mut self, key: &str, value: Value) {
        self.entries.push((key.to_owned(), value));
    }

    /// Adds a reading, or its failure as `ERROR: text`.
    fn put<T: Serialize, E: fmt::Display>(&mut self, key: &str, reading: Result<T, E>) {
        let value = match reading {
            Ok(v) => serde_json::to_value(v).unwrap_or_else(|e| Value::String(format!("ERROR: {e}"))),
            Err(e) => Value::String(format!("ERROR: {e}")),
        };
        self.push(key, value);
    }
}

impl Serialize for Status {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.entries.len()))?;
        for (k, v) in &self.entries {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

impl fmt::Display for Status {
    /// Pretty JSON, as the Python edition printed the dictionary.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match serde_json::to_string_pretty(self) {
            Ok(text) => f.write_str(&text),
            Err(e) => write!(f, "ERROR: {e}"),
        }
    }
}

/// The battery as the power supply class reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Supply {
    /// Charge in percent.
    pub percent: i64,
    /// `Charging`, `Discharging`, `Full`, `Not charging` or `Unknown`.
    pub status: String,
}

/// The CPU and GPU load in percent, as the service measures it.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
pub struct LiveLoad {
    /// CPU load.
    pub cpu_pct: Option<f64>,
    /// GPU load.
    pub gpu_pct: Option<f64>,
}

/// The CPU package power (from RAPL, once two readings exist) and the GPU power (the service's
/// reading), in watts.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
pub struct LivePower {
    /// CPU package power.
    pub cpu_w: Option<f64>,
    /// GPU power.
    pub gpu_w: Option<f64>,
}

/// One reading of everything the window shows live. Only cheap reads, because it runs about once
/// a second; a member that cannot be read is `null`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Live {
    /// Seconds since the epoch.
    pub t: i64,
    /// Temperatures and fan speeds.
    pub sensors: Option<Sensors>,
    /// The power mode name.
    pub power_mode: Option<String>,
    /// Whether the charger is in.
    pub ac: Option<bool>,
    /// The battery level; `null` without a battery.
    pub battery_pct: Option<i64>,
    /// The firmware's fan control state.
    pub fans: Option<FanState>,
    /// CPU and GPU load.
    pub load: LiveLoad,
    /// CPU and GPU power.
    pub power: LivePower,
    /// The service's state when it runs.
    pub daemon: Option<super::DaemonState>,
}

/// The overview a status line or a dashboard header shows.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Summary {
    /// The power mode name.
    pub power_mode: Option<String>,
    /// The firmware's fan control (`auto`, `max`, `manual`).
    pub fan_mode: Option<String>,
    /// CPU temperature in degrees C.
    pub cpu_temp: Option<i64>,
    /// GPU temperature in degrees C.
    pub gpu_temp: Option<i64>,
    /// CPU fan speed.
    pub cpu_fan_rpm: Option<i64>,
    /// GPU fan speed.
    pub gpu_fan_rpm: Option<i64>,
    /// Whether the charger is in.
    pub ac: Option<bool>,
    /// The battery level.
    pub battery_pct: Option<i64>,
    /// Whether the service answers.
    pub daemon_running: bool,
    /// Whether the service's smart fan curve is in control.
    pub smart_fan: Option<bool>,
    /// The automatic power mode (`off`, `native`, `smart`).
    pub auto_mode: Option<String>,
}

impl fmt::Display for Summary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let num = |v: Option<i64>| v.map_or_else(|| "?".to_owned(), |n| n.to_string());
        write!(
            f,
            "{}   CPU {} C   GPU {} C   fans {} / {} rpm",
            self.power_mode.as_deref().unwrap_or("?"),
            num(self.cpu_temp),
            num(self.gpu_temp),
            num(self.cpu_fan_rpm),
            num(self.gpu_fan_rpm)
        )?;
        if let Some(pct) = self.battery_pct {
            write!(f, "   battery {pct} %{}", if self.ac == Some(true) { " (AC)" } else { "" })?;
        }
        if !self.daemon_running {
            f.write_str("   (service not running)")?;
        }
        Ok(())
    }
}

/// The line `truesense sensors` prints.
pub fn describe_sensors(s: &Sensors) -> String {
    format!(
        "CPU {} C   GPU {} C   board {} C   fans {} / {} rpm",
        s.cpu_temp, s.gpu_temp, s.sys_temp, s.cpu_fan_rpm, s.gpu_fan_rpm
    )
}

/// The USB charge level as the plain number the firmware and the JSON use.
pub(crate) fn usb_percent(level: Reading<UsbLevel>) -> i64 {
    match level {
        Reading::Known(l) => l.code(),
        Reading::Unknown(n) => n,
    }
}

/// Turns the cumulative RAPL energy counter into watts between two readings.
#[derive(Debug, Default)]
pub(crate) struct RaplMeter {
    last: Mutex<Option<(f64, f64)>>,
}

impl RaplMeter {
    /// The watts since the previous call; `None` for the first reading or after the counter wrapped.
    fn watts(&self, microjoules: f64, now: f64) -> Option<f64> {
        let previous = self.last.lock().unwrap_or_else(PoisonError::into_inner).replace((microjoules, now));
        let (before, then) = previous?;
        let dt = now - then;
        (dt > 0.0 && microjoules >= before).then(|| (microjoules - before) / 1e6 / dt)
    }
}

/// The fan registers as the last live reading saw them. Reading them is a firmware call (an SMI that
/// stalls every core for milliseconds) and a window asks once a second. While the daemon runs, its
/// own label of what it wrote to the fans (`fan` in the live state) tells when they can have
/// changed through it, so the registers are read again when that label changes and otherwise every
/// [`FanCache::FRESH_S`] seconds, which still catches a change made behind the daemon's back.
#[derive(Default)]
pub(crate) struct FanCache {
    last: Mutex<Option<(f64, String, FanState)>>,
}

impl FanCache {
    /// How long a reading is reused while the daemon's label stays the same.
    pub(crate) const FRESH_S: f64 = 5.0;

    /// The cached state when `label` (the daemon's, `None` without a daemon) is the one it was read
    /// under and is fresh; otherwise `read()`, remembered only when there is a label.
    pub(crate) fn get(
        &self,
        now: f64,
        label: Option<&str>,
        read: impl FnOnce() -> Option<FanState>,
    ) -> Option<FanState> {
        let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
        if let (Some(label), Some((at, was, state))) = (label, last.as_ref())
            && was == label
            && (0.0..Self::FRESH_S).contains(&(now - at))
        {
            return Some(state.clone());
        }
        let state = read()?;
        *last = label.map(|l| (now, l.to_owned(), state.clone()));
        Some(state)
    }
}

const RAPL_ENERGY: &str = "sys/class/powercap/intel-rapl:0/energy_uj";

impl Api {
    /// Reads everything. Every reading is independent: one that fails becomes an `ERROR: ...`
    /// entry and the rest are still shown.
    pub fn status(&self) -> Status {
        // The keyboard takes about 50 ms to open: do it while the firmware reads below run.
        drop(Arc::clone(&self.s.keyboard).prewarm());
        let mut out = Status::default();
        let hw = &self.s.hw;
        // What starts helper programs or waits on a socket (the sound system's `pactl` and `amixer`
        // calls are 20 to 40 ms between them, the daemon's reply a few more) runs beside the
        // firmware reads instead of after them. The firmware calls stay on this thread in their
        // order: the kernel module serialises them anyway. The entries are put together in the
        // order they always had.
        let (daemon, audio_output, jack) = std::thread::scope(|scope| {
            let daemon = scope.spawn(|| self.ask_value("state", Args::new()));
            let audio = scope.spawn(|| self.s.audio.output_status());
            let jack = scope.spawn(|| self.s.audio.jack_status().raw);
            out.put("power_mode", hw.get_mode());
            out.put("sensors", hw.sensors());
            out.put("fans", hw.fan_state());
            // A panic in a helper query is the panic it always was: it goes on up.
            (
                daemon.join().unwrap_or_else(|p| std::panic::resume_unwind(p)),
                audio.join().unwrap_or_else(|p| std::panic::resume_unwind(p)),
                jack.join().unwrap_or_else(|p| std::panic::resume_unwind(p)),
            )
        });
        out.put("daemon", daemon);
        out.put("battery_limit", hw.get_battery_limit());
        out.put("usb_charge_stop", hw.get_usb_charge().map(usb_percent));
        out.put("overdrive", hw.get_overdrive());
        out.put("boot_sound", hw.get_boot_sound());
        out.put("kb_backlight_timeout", hw.get_kb_timeout());
        out.put("gpu_mode", hw.get_gpu_mode());
        out.put("rear", hw.rear_get());
        out.put("touchpad", Ok::<_, std::convert::Infallible>(self.s.touchpad.enabled()));
        out.put("audio_output", audio_output);
        out.put("jack", Ok::<_, std::convert::Infallible>(jack));
        out.put("voice", Ok::<_, std::convert::Infallible>(self.s.audio.voice_modes()));
        out
    }

    /// [`Api::status`] plus what the window shows at a glance: whether the charger is in (`ac`),
    /// the battery level (`battery_pct`, null without a battery), the keyboard's own brightness
    /// (`kb_brightness`) and the look this tool last put on the keyboard (`kb_look`).
    pub fn status_full(&self) -> Status {
        let mut st = self.status();
        st.push("ac", json!(self.s.linux.on_ac()));
        st.push("battery_pct", self.supply().map_or(Value::Null, |b| json!(b.percent)));
        st.push("kb_brightness", self.s.keyboard.get_brightness().map_or(Value::Null, |b| json!(b)));
        let last = self.s.keyboard.last();
        st.push("kb_look", if last == KeyboardState::default() { json!({}) } else { json!(last) });
        st
    }

    /// Every temperature and both fan speeds in one firmware call.
    pub fn sensors(&self) -> ApiResult<Sensors> {
        Ok(self.s.hw.sensors()?)
    }

    /// The first battery; `None` on a machine without one.
    pub fn supply(&self) -> Option<Supply> {
        self.s.sampler.battery().map(|b| Supply { percent: b.percent, status: b.status })
    }

    /// The CPU package power in watts since the previous call (RAPL), when it can be read.
    fn cpu_power(&self) -> Option<f64> {
        let path = Path::new(&self.s.root).join(RAPL_ENERGY);
        let microjoules = fsx::read_float(&path).ok()?;
        self.rapl.watts(microjoules, (self.s.now)())
    }

    /// Reads the window's live values. The service's own readings are used when it runs (it has
    /// them this second already); the hardware is read otherwise.
    pub fn live(&self) -> Live {
        let now = (self.s.now)();
        let state = if self.daemon_available() { self.daemon_state().ok() } else { None };
        let number = |key: &str| state.as_ref().and_then(|d| d.live_f64(key));
        // The daemon's temperatures and rpm are whole numbers; a missing optional one counts as 0.
        let from_daemon = (number("rpm1").is_some() && number("tcpu").is_some()).then(|| Sensors {
            cpu_temp: number("tcpu").map_or(0, |v| v as i64),
            sys_temp: number("chassis").map_or(0, |v| v as i64),
            gpu_temp: number("tgpu").map_or(0, |v| v as i64),
            cpu_fan_rpm: number("rpm1").map_or(0, |v| v as i64),
            gpu_fan_rpm: number("rpm2").map_or(0, |v| v as i64),
        });
        let sensors = from_daemon.or_else(|| self.s.hw.sensors().ok());
        let power_mode = state
            .as_ref()
            .and_then(|d| d.live_str("power_mode"))
            .filter(|m| !m.is_empty() && *m != "?")
            .map(str::to_owned)
            .or_else(|| self.s.hw.get_mode().ok().map(|m| m.to_string()));
        let ac = state.as_ref().and_then(|d| d.live_bool("ac")).unwrap_or_else(|| self.s.linux.on_ac());
        Live {
            t: now as i64,
            sensors,
            power_mode,
            ac: Some(ac),
            battery_pct: self.supply().map(|b| b.percent),
            fans: self
                .fans
                .get(now, state.as_ref().and_then(|d| d.live_str("fan")), || self.s.hw.fan_state().ok()),
            load: LiveLoad { cpu_pct: number("cpu"), gpu_pct: number("gpu") },
            power: LivePower { cpu_w: self.cpu_power(), gpu_w: number("gpuw") },
            daemon: state,
        }
    }

    /// The overview for a status line (from [`Api::live`]).
    pub fn summary(&self) -> Summary {
        let live = self.live();
        let sensors = live.sensors;
        let settings = live.daemon.as_ref();
        Summary {
            power_mode: live.power_mode,
            fan_mode: live.fans.map(|f| f.mode.to_string()),
            cpu_temp: sensors.map(|s| s.cpu_temp),
            gpu_temp: sensors.map(|s| s.gpu_temp),
            cpu_fan_rpm: sensors.map(|s| s.cpu_fan_rpm),
            gpu_fan_rpm: sensors.map(|s| s.gpu_fan_rpm),
            ac: live.ac,
            battery_pct: live.battery_pct,
            daemon_running: settings.is_some(),
            smart_fan: settings.and_then(|d| d.setting_bool("smart_fan")),
            auto_mode: settings.and_then(|d| d.setting_str("auto_mode")).map(str::to_owned),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod fan_cache_tests {
    use std::cell::Cell;

    use ts_platform::hw::{Duty, FanMode};

    use super::*;

    fn state(cpu: i64) -> FanState {
        FanState {
            behavior_raw: "0x41".into(),
            mode: FanMode::Auto,
            duty_register_pct: Duty { cpu, gpu: cpu },
        }
    }

    #[test]
    fn the_registers_are_read_again_when_the_daemons_label_changes_or_the_reading_is_old() {
        let cache = FanCache::default();
        let reads = Cell::new(0);
        let get = |now: f64, label: Option<&str>| {
            cache
                .get(now, label, || {
                    reads.set(reads.get() + 1);
                    Some(state(reads.get()))
                })
                .unwrap()
                .duty_register_pct
                .cpu
        };
        assert_eq!(get(0.0, Some("auto")), 1);
        assert_eq!(get(1.0, Some("auto")), 1, "same label, fresh: the firmware is not asked");
        assert_eq!(get(4.9, Some("auto")), 1);
        assert_eq!(get(5.1, Some("auto")), 2, "old: asked again");
        assert_eq!(get(6.0, Some("manual 60")), 3, "the daemon wrote something: asked again");
        assert_eq!(get(7.0, None), 4, "without a daemon every call asks");
        assert_eq!(get(7.5, None), 5);
        assert_eq!(get(6.0, Some("manual 60")), 6, "a clock that went back never serves a reading");
    }

    #[test]
    fn a_failed_read_is_none_and_keeps_nothing() {
        let cache = FanCache::default();
        assert!(cache.get(0.0, Some("auto"), || None).is_none());
        assert_eq!(cache.get(0.5, Some("auto"), || Some(state(9))).unwrap().duty_register_pct.cpu, 9);
    }
}
