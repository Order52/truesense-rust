//! The read-only panels under the dashboard's rows: temperatures, fans, load, power and what you
//! did. Drawn in that priority order, in two columns when the screen is wide; whatever does not fit
//! is left out, never squeezed.

use super::data::{Hist, Mode, Sensors};
use super::style::{Glyphs, St, clip, width};
use super::view::{Painter, round_even};

/// From this width the panels go side by side.
const TWO_COLUMNS: i32 = 96;

/// The temperature's look: the hotter, the louder the colour.
pub(crate) fn temp_style(t: Option<i64>) -> St {
    match t {
        None => St::Dim,
        Some(t) if t >= 85 => St::Bad,
        Some(t) if t >= 72 => St::Warn,
        Some(_) => St::Good,
    }
}

/// The last `width` readings as a sparkline scaled between `lo` and `hi`, right-aligned.
pub(crate) fn spark(g: &Glyphs, values: &[i64], width: usize, lo: i64, hi: i64) -> String {
    let tail = &values[values.len().saturating_sub(width)..];
    let top = g.spark.len() - 1;
    let line: String = tail
        .iter()
        .map(|v| {
            let f = if hi > lo { ((*v - lo) as f64 / (hi - lo) as f64).clamp(0.0, 1.0) } else { 0.0 };
            g.spark[(f * top as f64).round_ties_even() as usize]
        })
        .collect();
    format!("{}{line}", " ".repeat(width.saturating_sub(tail.len())))
}

fn rjust(s: &str, n: i32) -> String {
    let k = width(s);
    if k < n { format!("{}{s}", " ".repeat(usize::try_from(n - k).unwrap_or(0))) } else { s.to_owned() }
}

/// A gauge's history: the readings, the range the sparkline spans and the width of the number.
struct Trail {
    hist: Vec<i64>,
    lo: i64,
    hi: i64,
    width: i32,
}

fn or_dots(s: &str) -> &str {
    if s.is_empty() { "…" } else { s }
}

impl Painter<'_, '_> {
    #[allow(clippy::too_many_arguments)]
    fn gauge(
        &mut self,
        y: i32,
        x: i32,
        w: i32,
        label: &str,
        val: Option<i64>,
        vmax: i64,
        unit: &str,
        st: St,
        trail: Option<Trail>,
    ) {
        self.cv.put(y, x, &format!("{label:<4}"), St::Dim);
        let wd = trail.as_ref().map_or(6, |t| if t.width > 0 { t.width } else { 6 });
        let text = rjust(&val.map_or_else(|| "…".to_owned(), |v| format!("{v}{unit}")), wd);
        let spark_room = trail.is_some() && w >= 40;
        let extra = if spark_room { 12 } else { 0 };
        let bw = (w - 5 - width(&text) - 2 - extra).max(6);
        let f = match val {
            Some(v) if v != 0 => (v as f64 / vmax as f64).clamp(0.0, 1.0),
            _ => 0.0,
        };
        let n = round_even(f * f64::from(bw));
        self.cv.put(y, x + 5, &self.g.full.repeat(usize::try_from(n).unwrap_or(0)), st);
        self.cv.put(y, x + 5 + n, &self.g.empty.repeat(usize::try_from(bw - n).unwrap_or(0)), St::BarOff);
        self.cv.put(y, x + 6 + bw, &text, if val.is_some() { st } else { St::Dim });
        if let (true, Some(t)) = (spark_room, trail) {
            self.cv.put(y, x + 7 + bw + width(&text), &spark(self.g, &t.hist, 10, t.lo, t.hi), st);
        }
    }

    fn sensor(&self, pick: fn(&Sensors) -> i64) -> Option<i64> {
        self.m.data.sensors().map(|s| pick(&s))
    }

    fn temps(&mut self, y: i32, x: i32, w: i32) {
        let d = &self.m.data;
        let (tc, tg, ts) =
            (self.sensor(|s| s.cpu_temp), self.sensor(|s| s.gpu_temp), self.sensor(|s| s.sys_temp));
        let svc = d.service();
        self.cv.frame((y, x, 6, w), "Temperatures", St::Dim, St::Accent);
        let trail = |h: Hist| Some(Trail { hist: d.history(h), lo: 30, hi: 100, width: 0 });
        self.gauge(y + 1, x + 2, w - 4, "CPU", tc, 100, " °C", temp_style(tc), trail(Hist::TempCpu));
        self.gauge(y + 2, x + 2, w - 4, "GPU", tg, 100, " °C", temp_style(tg), trail(Hist::TempGpu));
        self.gauge(y + 3, x + 2, w - 4, "SYS", ts, 100, " °C", temp_style(ts), None);
        self.cv.put(y + 4, x + 2, "Now  ", St::Dim);
        let st = if svc.activity.is_empty() { St::Dim } else { St::Accent };
        let mut now = if svc.mode_reason.is_empty() { svc.activity.clone() } else { svc.mode_reason.clone() };
        if now.is_empty() {
            now = if d.daemon() { "…".to_owned() } else { "needs the TrueSense service".to_owned() };
        }
        self.cv.put(y + 4, x + 8, &clip(&now, (w - 12).max(0)), st);
    }

    fn fans(&mut self, y: i32, x: i32, w: i32) {
        let d = &self.m.data;
        self.cv.frame((y, x, 5, w), "Fans", St::Dim, St::Accent);
        let trail = |h: Hist| Some(Trail { hist: d.history(h), lo: 0, hi: 6500, width: 8 });
        let (r1, r2) = (self.sensor(|s| s.cpu_fan_rpm), self.sensor(|s| s.gpu_fan_rpm));
        self.gauge(y + 1, x + 2, w - 4, "CPU", r1, 6500, " rpm", St::Bar, trail(Hist::Rpm1));
        self.gauge(y + 2, x + 2, w - 4, "GPU", r2, 6500, " rpm", St::Bar, trail(Hist::Rpm2));
        let (cd, gd, mode) = d.fan().map_or_else(
            || ("…".to_owned(), "…".to_owned(), "…".to_owned()),
            |f| (f.cpu_duty.to_string(), f.gpu_duty.to_string(), f.mode.name().to_owned()),
        );
        self.cv.put(y + 3, x + 2, &clip(&format!("duty CPU {cd} %  GPU {gd} %  ·  {mode}"), w - 4), St::Dim);
    }

    fn load(&mut self, y: i32, x: i32, w: i32) {
        let d = &self.m.data;
        let svc = d.service();
        self.cv.frame((y, x, 5, w), "Load", St::Dim, St::Accent);
        let trail = |h: Hist| Some(Trail { hist: d.history(h), lo: 0, hi: 100, width: 0 });
        self.gauge(y + 1, x + 2, w - 4, "CPU", svc.cpu_load, 100, " %", St::Bar, trail(Hist::LoadCpu));
        self.gauge(y + 2, x + 2, w - 4, "GPU", svc.gpu_load, 100, " %", St::Bar, trail(Hist::LoadGpu));
        let watts = svc.gpu_watts.map_or_else(|| "…".to_owned(), |v| format!("{v} W"));
        self.cv.put(y + 3, x + 2, &format!("GPU power {watts}"), St::Dim);
    }

    fn power(&mut self, y: i32, x: i32, w: i32) {
        let d = &self.m.data;
        let svc = d.service();
        self.cv.frame((y, x, 5, w), "Power", St::Dim, St::Accent);
        self.cv.put(y + 1, x + 2, "Mode", St::Dim);
        let mode = Mode::from_name(d.power_mode());
        let (label, short, st) = match mode {
            Some(m) => (m.label().to_owned(), m.label().to_owned(), St::Mode(m)),
            None => (or_dots(d.power_mode()).to_owned(), "…".to_owned(), St::Text),
        };
        self.cv.put(y + 1, x + 9, &label, st);
        let (src, sst) = match d.ac() {
            Some(true) => ("on charger", St::Good),
            Some(false) => ("on battery", St::Warn),
            None => ("…", St::Warn),
        };
        self.cv.put(y + 1, x + 10 + width(&short) + 2, src, sst);
        self.cv.put(y + 2, x + 2, "Auto", St::Dim);
        let auto = format!("{} · profile {}", or_dots(&svc.auto_mode), or_dots(&svc.smart_profile));
        self.cv.put(y + 2, x + 9, &clip(&auto, w - 12), St::Text);
        if let Some(b) = d.battery() {
            let st = if b.percent > 30 { St::Good } else { St::Bad };
            self.gauge(y + 3, x + 2, w - 4, "Batt", Some(b.percent), 100, " %", st, None);
        }
    }

    fn activity(&mut self, y: i32, x: i32, w: i32, room: i32) {
        let log = &self.m.log;
        let n = (room - 2).min(12).min(i32::try_from(log.len()).unwrap_or(0)).max(1);
        self.cv.frame((y, x, n + 2, w), "What you did", St::Dim, St::Accent);
        if log.is_empty() {
            self.cv.put(y + 1, x + 2, "Nothing yet: your changes are listed here.", St::Dim);
        }
        let recent = &log[log.len().saturating_sub(usize::try_from(n).unwrap_or(1))..];
        for (i, e) in recent.iter().enumerate() {
            let row = y + 1 + i32::try_from(i).unwrap_or(0);
            self.cv.put(row, x + 2, &e.at, St::Dim);
            let st = if e.st == St::Good { St::Good } else { St::Bad };
            self.cv.put(row, x + 11, &clip(&e.text, w - 14), st);
        }
    }

    /// Draws the panels in the `h` rows from `y`.
    pub(super) fn dashboard(&mut self, y: i32, h: i32, w: i32) {
        let two = w >= TWO_COLUMNS;
        let colw = if two { (w - 3) / 2 } else { w - 2 };
        let mut yy = y;
        // (height, which panel): the order is the priority.
        let panels: [(i32, u8); 4] = [(6, 0), (5, 1), (5, 2), (5, 3)];
        let draw = |p: &mut Self, panel: u8, y: i32, x: i32, w: i32| match panel {
            0 => p.temps(y, x, w),
            1 => p.fans(y, x, w),
            2 => p.load(y, x, w),
            _ => p.power(y, x, w),
        };
        if two {
            for pair in [[panels[0], panels[1]], [panels[2], panels[3]]] {
                let ph = pair[0].0.max(pair[1].0);
                if yy + ph > y + h {
                    break;
                }
                draw(self, pair[0].1, yy, 1, colw);
                draw(self, pair[1].1, yy, 1 + colw + 1, colw);
                yy += ph;
            }
        } else {
            for (ph, panel) in panels {
                if yy + ph > y + h {
                    break;
                }
                draw(self, panel, yy, 1, colw);
                yy += ph;
            }
        }
        if y + h - yy >= 5 {
            self.activity(yy, 1, w - 2, y + h - yy);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn the_sparkline_scales_between_lo_and_hi_and_is_right_aligned() {
        let g = Glyphs::new(false);
        assert_eq!(spark(&g, &[0, 50, 100], 5, 0, 100), "   =%");
        assert_eq!(spark(&g, &[], 4, 0, 100), "    ");
        assert_eq!(spark(&g, &[1, 2, 3, 4, 5, 6], 3, 0, 100).chars().count(), 3);
        assert_eq!(spark(&g, &[7, 7], 2, 5, 5), "  ", "an empty range draws the lowest bar");
        assert_eq!(spark(&g, &[-50, 500], 2, 0, 100), " %", "values outside the range are clamped");
    }

    #[test]
    fn temperatures_are_coloured_by_how_hot_they_are() {
        assert_eq!(temp_style(None), St::Dim);
        assert_eq!(temp_style(Some(50)), St::Good);
        assert_eq!(temp_style(Some(72)), St::Warn);
        assert_eq!(temp_style(Some(84)), St::Warn);
        assert_eq!(temp_style(Some(85)), St::Bad);
    }
}
