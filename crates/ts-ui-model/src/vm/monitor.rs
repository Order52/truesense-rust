//! What the Monitor page decides: which commands to poll and when, how a command result becomes
//! something to draw, how a chart is scaled, how a rate or a size reads.
//!
//! The page (`MonitorPage.qml`) owns no logic beyond layout: it asks [`call`] for a polling `plan`,
//! runs the commands that come back, keeps the results, and gives each result to a view verb
//! (`overview`, `cpu`, `gpu`, `memory`, `network`) that answers a display-ready object. Everything
//! here is pure (no clock, no I/O: the page passes `now_ms`), and every input member may be missing
//! or `null`, because the commands answer `null` for whatever they cannot read (a laptop without a
//! Wi-Fi sensor, a card that is asleep, a first sample that has no rate yet). A missing value is a
//! dash in the text and `null` in the number, never zero.
//!
//! # Polling policy (COMMANDS.md, "Monitor commands")
//!
//! Only the visible tab is polled, and only while the window is visible; a tab that is left stops
//! at once. The cadences are the contract's: `overview` every 2 s, `cpu`, `memory`, `network` and
//! `gpu` without detail every 1 s, `disks` every 5 s, `gpu` with detail every 5 s. A discrete GPU
//! that is asleep is **never** asked for detail: the cheap `gpu` call (firmware temperature and the
//! power state, no process started) tells whether it is awake, and only an `active` card gets the
//! `nvidia-smi` query. A command that failed backs off (doubling, at most 30 s) so a broken
//! sensor never turns into a request storm.
//!
//! Verbs: `plan`, `push`, `stats`, `scale`, `overview`, `facts`, `cpu`, `gpu`, `memory`, `network`.
#![forbid(unsafe_code)]

use serde_json::{Value, json};

use crate::format;
use crate::json::{at, bool_at, f64_at, i64_at, list_at, str_at};

/// How many samples a chart keeps (two minutes at the 1 s cadence).
const HISTORY_LEN: usize = 120;
/// The longest a back-off lasts.
const MAX_BACKOFF_MS: f64 = 30_000.0;
/// A sample older than this many intervals leaves a gap in the chart instead of a straight line.
const GAP_INTERVALS: f64 = 2.5;
/// Longest process name or command line the page is given (a hostile command line can be megabytes).
const MAX_NAME: usize = 48;
const MAX_CMDLINE: usize = 240;

/// The text for a value that is not known.
const DASH: &str = format::UNKNOWN;

/// Runs one verb.
///
/// # Errors
/// The text for an unknown verb.
pub fn call(verb: &str, args: &Value) -> Result<Value, String> {
    Ok(match verb {
        "plan" => plan(args),
        "push" => push(args),
        "stats" => stats(args),
        "scale" => scale(args),
        "overview" => overview(args),
        "facts" => facts(args),
        "cpu" => cpu(args),
        "gpu" => gpu(args),
        "memory" => memory(args),
        "network" => network(args),
        _ => return Err(format!("unknown monitor verb: {verb}")),
    })
}

// ---- small helpers --------------------------------------------------------------------------

fn num_or_null(v: Option<f64>) -> Value {
    v.map_or(Value::Null, |n| json!(n))
}

fn rows<'a>(v: &'a Value, path: &str) -> &'a [Value] {
    list_at(v, path).unwrap_or(&[])
}

fn clamp01(v: f64) -> f64 {
    v.clamp(0.0, 1.0)
}

/// A fraction 0..1 of `value` out of `whole`; `None` when either is unknown or `whole` is not positive.
fn fraction(value: Option<f64>, whole: Option<f64>) -> Option<f64> {
    match (value, whole) {
        (Some(v), Some(w)) if w > 0.0 => Some(clamp01(v / w)),
        _ => None,
    }
}

/// `ok`, `warn`, `bad`, or `unknown` for a missing value: the colour of a gauge or bar. Never the
/// only signal, the number is always shown too.
fn level(value: Option<f64>, warn: f64, crit: f64) -> &'static str {
    match value {
        None => "unknown",
        Some(v) if v >= crit => "bad",
        Some(v) if v >= warn => "warn",
        Some(_) => "ok",
    }
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max { s.to_owned() } else { format!("{}…", s.chars().take(max).collect::<String>()) }
}

/// `1.5 GB` for a count of bytes.
fn bytes_text(v: Option<f64>) -> String {
    format::bytes(v)
}

/// `12.3 MB/s` for bytes per second.
pub fn rate(v: Option<f64>) -> String {
    match v.filter(|n| n.is_finite() && *n >= 0.0) {
        Some(n) => format!("{}/s", format::bytes(Some(n))),
        None => DASH.to_owned(),
    }
}

/// `800 MHz`, `4.50 GHz`.
pub fn mhz(v: Option<f64>) -> String {
    match v.filter(|n| n.is_finite() && *n >= 0.0) {
        None => DASH.to_owned(),
        Some(n) if n < 1000.0 => format!("{} MHz", n.round()),
        Some(n) => format!("{:.2} GHz", n / 1000.0),
    }
}

/// `45 s`, `12 min`, `2 h 05 min`, `3 d 4 h`: how long the machine has been up.
pub fn uptime(seconds: Option<f64>) -> String {
    let Some(s) = seconds.filter(|n| n.is_finite() && *n >= 0.0) else { return DASH.to_owned() };
    let s = s.floor() as u64;
    let (d, h, m) = (s / 86_400, s % 86_400 / 3600, s % 3600 / 60);
    if d > 0 {
        format!("{d} d {h} h")
    } else if h > 0 {
        format!("{h} h {m:02} min")
    } else if m > 0 {
        format!("{m} min")
    } else {
        format!("{s} s")
    }
}

/// A watts value with one decimal (`28.5 W`), a dash when unknown.
fn watts(v: Option<f64>) -> String {
    format::watts(v)
}

/// `7.2 %` below ten (the load of an idle core is worth a decimal), whole numbers above.
fn pct_fine(v: Option<f64>) -> String {
    match v.filter(|n| n.is_finite()) {
        None => DASH.to_owned(),
        Some(n) if n.abs() < 10.0 => format!("{:.1} %", (n * 10.0).round() / 10.0),
        Some(n) => format!("{} %", n.round()),
    }
}

// ---- polling plan ---------------------------------------------------------------------------

/// One command the page may run for a tab: its key (the page's bookkeeping name), the bus command,
/// its arguments, and the shortest time between two calls.
struct Poll {
    key: &'static str,
    verb: &'static str,
    args: Value,
    every_ms: f64,
}

fn polls_for(tab: &str, gpu_state: Option<&str>, top: i64) -> Vec<Poll> {
    let top = top.clamp(1, 25);
    let p = |key, verb, args, every_ms| Poll { key, verb, args, every_ms };
    match tab {
        "overview" => vec![
            p("overview", "overview", json!({}), 2000.0),
            // Cached by the backend except the uptime, so a slow refresh is plenty.
            p("system", "info", json!({}), 30_000.0),
        ],
        "cpu" => vec![p("cpu", "cpu", json!({"top": top}), 1000.0)],
        "gpu" => {
            let mut v = vec![p("gpu_live", "gpu", json!({"detail": false}), 1000.0)];
            // The query is made only for a card that says it is awake. Unknown state (no answer
            // yet, or the power state cannot be read) waits for the cheap call: the first request
            // of the tab must never be the one that wakes the card.
            if gpu_state == Some("active") {
                v.push(p("gpu_detail", "gpu", json!({"detail": true}), 5000.0));
            }
            v
        }
        "memory" => vec![p("memory", "memory", json!({"top": top}), 1000.0), p("disks", "disks", json!({}), 5000.0)],
        "network" => vec![p("network", "network", json!({}), 1000.0)],
        _ => Vec::new(),
    }
}

/// The commands due now. Arguments: `tab`, `active` (window visible), `now_ms`, `last` (key to the
/// time its command was last started), `pending` (key to whether one is still running), `errors`
/// (key to consecutive failures), `gpu_state` (from the newest GPU answer), `top`.
///
/// Answers `{"commands": [{"key", "area": "monitor"|"system", "verb", "args"}], "wait_ms"}`;
/// `wait_ms` is when to ask again (never below 100 or above 1000).
fn plan(args: &Value) -> Value {
    let active = bool_at(args, "active").unwrap_or(false);
    let tab = str_at(args, "tab").unwrap_or("");
    let now = f64_at(args, "now_ms");
    let (Some(now), true) = (now, active) else {
        return json!({"commands": [], "wait_ms": 1000});
    };
    let top = i64_at(args, "top").unwrap_or(8);
    let mut commands = Vec::new();
    let mut soonest = 1000.0_f64;
    for poll in polls_for(tab, str_at(args, "gpu_state"), top) {
        let failures = f64_at(args, &format!("errors.{}", poll.key)).unwrap_or(0.0).clamp(0.0, 8.0);
        let every = (poll.every_ms * 2f64.powf(failures)).min(MAX_BACKOFF_MS.max(poll.every_ms));
        let running = bool_at(args, &format!("pending.{}", poll.key)).unwrap_or(false);
        // A clock that went backwards counts as due: waiting for it to catch up would freeze the page.
        let due_in = f64_at(args, &format!("last.{}", poll.key)).map_or(0.0, |last| if now < last { 0.0 } else { last + every - now });
        if running {
            continue;
        }
        if due_in <= 0.0 {
            commands.push(json!({
                "key": poll.key,
                "area": if poll.verb == "info" { "system" } else { "monitor" },
                "verb": poll.verb,
                "args": poll.args,
            }));
            soonest = soonest.min(poll.every_ms);
        } else {
            soonest = soonest.min(due_in);
        }
    }
    json!({"commands": commands, "wait_ms": soonest.clamp(100.0, 1000.0)})
}

// ---- history, statistics, scale -------------------------------------------------------------

/// Appends one sample to a chart series: `{series, value, now_ms, last_ms, interval_ms, max}`.
/// A value that is missing or not finite is a gap (`null`), and a long silence before it (the tab was
/// not shown, the window was hidden) leaves one gap too, so the line breaks instead of crossing the
/// time nobody measured. The series is cut to the newest `max` samples (default 120).
fn push(args: &Value) -> Value {
    let mut series: Vec<Value> = rows(args, "series").to_vec();
    let max = i64_at(args, "max").map_or(HISTORY_LEN, |n| usize::try_from(n.clamp(2, 2000)).unwrap_or(HISTORY_LEN));
    if let (Some(now), Some(last), Some(every)) = (f64_at(args, "now_ms"), f64_at(args, "last_ms"), f64_at(args, "interval_ms")) {
        let silent = now - last > every.max(1.0) * GAP_INTERVALS || now < last;
        if silent && series.last().is_some_and(|v| !v.is_null()) {
            series.push(Value::Null);
        }
    }
    series.push(num_or_null(f64_at(args, "value")));
    if series.len() > max {
        series.drain(..series.len() - max);
    }
    Value::Array(series)
}

/// `{min, max, avg, latest, count}` of a series, ignoring gaps; every member `null` for an empty one.
/// The page shows these as a chart's "peak" and "lowest" and clears them with the series.
fn stats(args: &Value) -> Value {
    let values: Vec<f64> = rows(args, "series").iter().filter_map(Value::as_f64).filter(|n| n.is_finite()).collect();
    if values.is_empty() {
        return json!({"min": null, "max": null, "avg": null, "latest": null, "count": 0});
    }
    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let avg = values.iter().sum::<f64>() / values.len() as f64;
    let latest = rows(args, "series").last().and_then(Value::as_f64).filter(|n| n.is_finite());
    json!({"min": min, "max": max, "avg": avg, "latest": num_or_null(latest), "count": values.len()})
}

/// The smallest 1, 2, 2.5, 5 or 10 times a power of ten that is at least `v`.
fn nice_ceiling(v: f64) -> f64 {
    if !v.is_finite() || v <= 0.0 {
        return 1.0;
    }
    let exp = v.log10().floor();
    let base = 10f64.powf(exp);
    let m = v / base;
    let step = [1.0, 2.0, 2.5, 5.0, 10.0].into_iter().find(|s| m <= *s + 1e-9).unwrap_or(10.0);
    step * base
}

/// A zero-based chart scale: `{series: [[..], ..], floor, unit}` gives `{min: 0, max, grid}` where
/// `max` is the next round number above the largest sample and above `floor` (so a quiet network does
/// not draw its noise at full height), and `grid` is four lines `{value, fraction, text}` from 0 to
/// `max`. `unit` (`rate`, `bytes`, `percent`, `temp`, `watts`) picks the label format.
fn scale(args: &Value) -> Value {
    let floor = f64_at(args, "floor").unwrap_or(1.0).max(f64::MIN_POSITIVE);
    let peak = rows(args, "series")
        .iter()
        .flat_map(|s| s.as_array().map(Vec::as_slice).unwrap_or(&[]).iter())
        .filter_map(Value::as_f64)
        .filter(|n| n.is_finite())
        .fold(0.0_f64, f64::max);
    let max = nice_ceiling((peak * 1.05).max(floor));
    let label = |v: f64| -> String {
        match str_at(args, "unit").unwrap_or("") {
            "rate" => rate(Some(v)),
            "bytes" => bytes_text(Some(v)),
            "percent" => format::percent(Some(v)),
            "temp" => format::temp(Some(v)),
            "watts" => format::watts(Some(v)),
            _ => format!("{v}"),
        }
    };
    let grid: Vec<Value> = (0..=3)
        .map(|i| {
            let value = max * f64::from(i) / 3.0;
            json!({"value": value, "fraction": f64::from(i) / 3.0, "text": label(value)})
        })
        .collect();
    json!({"min": 0.0, "max": max, "grid": grid})
}

// ---- overview -------------------------------------------------------------------------------

/// Default warning and critical temperatures per sensor kind. The hardware's own limits can lower
/// them, never raise them: a sensor that reports 100 as "high" must not make 99 look fine.
struct Limits {
    warn: f64,
    crit: f64,
}

fn default_limits(id: &str) -> Limits {
    let (warn, crit) = match id {
        "cpu" => (85.0, 95.0),
        "gpu" => (80.0, 90.0),
        "nvme" => (65.0, 75.0),
        "wifi" => (70.0, 85.0),
        _ => (70.0, 90.0),
    };
    Limits { warn, crit }
}

fn plausible_limit(v: Option<f64>) -> Option<f64> {
    v.filter(|n| *n > 30.0 && *n < 130.0)
}

fn gauge(id: &str, index: usize, sensor: Option<&Value>, badge: &str) -> Value {
    let defaults = default_limits(id);
    let value = sensor.and_then(|s| f64_at(s, "celsius"));
    let crit = plausible_limit(sensor.and_then(|s| f64_at(s, "critical_c"))).map_or(defaults.crit, |c| c.min(defaults.crit));
    let mut warn = plausible_limit(sensor.and_then(|s| f64_at(s, "high_c"))).map_or(defaults.warn, |h| h.min(defaults.warn));
    if warn >= crit {
        warn = crit - 5.0;
    }
    let to = (((crit + 10.0) / 10.0).ceil() * 10.0).max(100.0);
    json!({
        "id": id,
        "index": index,
        "value": num_or_null(value),
        "text": format::temp(value),
        "min_text": format::temp(sensor.and_then(|s| f64_at(s, "session_min_c"))),
        "max_text": format::temp(sensor.and_then(|s| f64_at(s, "session_max_c"))),
        "from": 0, "to": to, "warn_at": warn, "crit_at": crit,
        "level": level(value, warn, crit),
        "badge": badge,
    })
}

/// The temperature sensor to use for the CPU: the package, else the plain CPU one, else the
/// hottest core.
fn cpu_sensor(temps: &[Value]) -> Option<&Value> {
    let kind = |k: &str| temps.iter().find(|t| str_at(t, "kind") == Some(k) && f64_at(t, "celsius").is_some());
    kind("cpu_package").or_else(|| kind("cpu")).or_else(|| {
        temps
            .iter()
            .filter(|t| str_at(t, "kind") == Some("cpu_core"))
            .filter_map(|t| f64_at(t, "celsius").map(|c| (c, t)))
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, t)| t)
    })
}

/// The Overview tab. Input `{overview}` (a `monitor overview` result or `null`). Gauges: CPU, GPU
/// (not when the machine has none), board, every NVMe drive (at most two) and Wi-Fi when the machine
/// has the sensor; minimums and maximums are the backend's session ones (`monitor reset` clears them).
fn overview(args: &Value) -> Value {
    let o = at(args, "overview").cloned().unwrap_or(Value::Null);
    let temps = rows(&o, "temperatures");
    let first = |k: &str| temps.iter().find(|t| str_at(t, "kind") == Some(k));
    let gpu_state = str_at(&o, "gpu_state").unwrap_or("unknown");
    let mut gauges = vec![gauge("cpu", 0, cpu_sensor(temps), "")];
    let gpu = first("gpu");
    if gpu_state != "absent" || gpu.is_some() {
        gauges.push(gauge("gpu", 0, gpu, if gpu_state == "suspended" { "asleep" } else { "" }));
    }
    gauges.push(gauge("board", 0, first("board"), ""));
    for (i, t) in temps.iter().filter(|t| str_at(t, "kind") == Some("nvme")).take(2).enumerate() {
        gauges.push(gauge("nvme", i, Some(t), ""));
    }
    if let Some(w) = first("wifi") {
        gauges.push(gauge("wifi", 0, Some(w), ""));
    }
    let cpu_load = f64_at(&o, "cpu_load_pct");
    let mem = f64_at(&o, "memory_used_pct");
    let swap = f64_at(&o, "swap_used_pct");
    json!({
        "gauges": gauges,
        "gpu_state": gpu_state,
        "has_data": !o.is_null(),
        "cpu_load": {"value": num_or_null(cpu_load), "text": format::percent(cpu_load), "level": level(cpu_load, 85.0, 97.0)},
        "cpu_freq_text": mhz(f64_at(&o, "cpu_freq_mhz")),
        "memory": {"value": num_or_null(mem), "text": format::percent(mem), "level": level(mem, 85.0, 95.0)},
        "swap": {"value": num_or_null(swap), "text": format::percent(swap), "level": level(swap, 50.0, 90.0)},
        "net": {
            "primary": str_at(&o, "net_primary"),
            "rx_text": rate(f64_at(&o, "net_rx_bps")),
            "tx_text": rate(f64_at(&o, "net_tx_bps")),
        },
        "uptime_text": uptime(f64_at(&o, "uptime_s")),
    })
}

/// The system facts of the Overview tab. Input `{system, uptime_s?}` (`system info`; the fresher
/// uptime of an overview may be given). Answers `{rows: [{id, text}]}` with the ids `model`, `bios`,
/// `cpu`, `cores`, `max_clock`, `gpu0`..`gpuN`, `memory`, `os`, `kernel`, `uptime`; unknown is a dash.
fn facts(args: &Value) -> Value {
    let s = at(args, "system").cloned().unwrap_or(Value::Null);
    let join = |parts: &[Option<String>]| -> String {
        let parts: Vec<&str> = parts.iter().flatten().map(String::as_str).filter(|p| !p.trim().is_empty()).collect();
        if parts.is_empty() { DASH.to_owned() } else { parts.join(" · ") }
    };
    let own = |p: &str| str_at(&s, p).map(str::to_owned);
    let cores = match (i64_at(&s, "cpu.cores"), i64_at(&s, "cpu.threads")) {
        (Some(c), Some(t)) => format!("{c} / {t}"),
        (None, Some(t)) => format!("– / {t}"),
        (Some(c), None) => format!("{c} / –"),
        (None, None) => DASH.to_owned(),
    };
    let mut out = vec![
        json!({"id": "model", "text": join(&[own("machine.vendor"), own("machine.product")])}),
        json!({"id": "bios", "text": join(&[own("machine.bios"), own("machine.bios_date")])}),
        json!({"id": "cpu", "text": join(&[own("cpu.model")])}),
        json!({"id": "cores", "text": cores}),
        json!({"id": "max_clock", "text": mhz(f64_at(&s, "cpu.max_mhz"))}),
    ];
    let gpus = rows(&s, "gpus");
    if gpus.is_empty() {
        out.push(json!({"id": "gpu0", "text": DASH}));
    }
    for (i, g) in gpus.iter().enumerate() {
        let name = str_at(g, "name").map(str::to_owned).or_else(|| {
            let vendor = str_at(g, "vendor")?;
            Some(match str_at(g, "device_id") {
                Some(d) => format!("{vendor} {d}"),
                None => vendor.to_owned(),
            })
        });
        let driver = str_at(g, "driver").map(str::to_owned);
        out.push(json!({"id": format!("gpu{i}"), "text": join(&[name, driver])}));
    }
    out.push(json!({"id": "memory", "text": bytes_text(f64_at(&s, "memory_total_bytes"))}));
    out.push(json!({"id": "os", "text": join(&[own("os.pretty_name")])}));
    out.push(json!({"id": "kernel", "text": join(&[own("kernel")])}));
    out.push(json!({"id": "uptime", "text": uptime(f64_at(args, "uptime_s").or_else(|| f64_at(&s, "uptime_s")))}));
    json!({"rows": out})
}

// ---- processes ------------------------------------------------------------------------------

fn state_id(state: Option<&str>) -> &'static str {
    match state.and_then(|s| s.chars().next()) {
        Some('R') => "running",
        Some('S') => "sleeping",
        Some('D') => "waiting",
        Some('Z') => "zombie",
        Some('T' | 't') => "stopped",
        Some('I') => "idle",
        _ => "other",
    }
}

/// The name to show for a process: its name, else the program in the command line, else `pid N`.
fn process_name(p: &Value, pid: Option<i64>) -> String {
    let named = str_at(p, "name").map(str::trim).filter(|n| !n.is_empty());
    let from_cmd = || {
        str_at(p, "cmdline")
            .and_then(|c| c.split_whitespace().next())
            .map(|c| c.rsplit('/').next().unwrap_or(c))
            .filter(|c| !c.is_empty())
    };
    match named.or_else(from_cmd) {
        Some(n) => clip(n, MAX_NAME),
        None => pid.map_or_else(|| DASH.to_owned(), |p| format!("pid {p}")),
    }
}

/// The ranking of a process list: sorted by `by` (`cpu` or `mem`), biggest first, missing values
/// last (the backend sorts already; this keeps the page right with an old or odd backend), at most
/// `limit` rows. `bar` is the share of the busiest row's value (CPU: of at least one full core, so
/// a list of idle processes does not all look busy).
fn process_rows(list: &[Value], by: &str, limit: usize) -> Vec<Value> {
    let key = if by == "mem" { "mem_bytes" } else { "cpu_pct" };
    let mut items: Vec<(Option<f64>, &Value)> = list.iter().filter(|p| p.is_object()).map(|p| (f64_at(p, key), p)).collect();
    items.sort_by(|a, b| match (a.0, b.0) {
        (Some(x), Some(y)) => y.total_cmp(&x),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    items.truncate(limit);
    let scale_to = items.iter().filter_map(|i| i.0).fold(if by == "mem" { 1.0 } else { 100.0 }, f64::max);
    items
        .into_iter()
        .map(|(v, p)| {
            let pid = i64_at(p, "pid");
            json!({
                "pid": num_or_null(pid.map(|n| n as f64)),
                "name": process_name(p, pid),
                "cmdline": clip(str_at(p, "cmdline").unwrap_or(""), MAX_CMDLINE),
                "user": str_at(p, "user"),
                "state_id": state_id(str_at(p, "state")),
                "threads": num_or_null(f64_at(p, "threads")),
                "cpu_text": if by == "mem" { Value::Null } else { json!(pct_fine(v)) },
                "mem_text": bytes_text(f64_at(p, "mem_bytes")),
                "value": num_or_null(v.map(|n| clamp01(n / scale_to))),
            })
        })
        .collect()
}

// ---- cpu ------------------------------------------------------------------------------------

/// The CPU tab. Input `{cpu, limit?}` (a `monitor cpu` result or `null`; `limit` caps the process
/// list). Cores become bar rows `{label, text, sub, value, level}`.
fn cpu(args: &Value) -> Value {
    let c = at(args, "cpu").cloned().unwrap_or(Value::Null);
    let load = f64_at(&c, "load_pct");
    let cores: Vec<Value> = rows(&c, "cores")
        .iter()
        .map(|core| {
            let l = f64_at(core, "load_pct");
            json!({
                "label": i64_at(core, "id").map_or_else(|| DASH.to_owned(), |n| n.to_string()),
                "text": format::percent(l),
                "sub": mhz(f64_at(core, "freq_mhz")),
                "value": num_or_null(l.map(|n| clamp01(n / 100.0))),
                "level": level(l, 85.0, 97.0),
            })
        })
        .collect();
    let limit = usize::try_from(i64_at(args, "limit").unwrap_or(8).clamp(1, 25)).unwrap_or(8);
    let la = at(&c, "load_avg").cloned();
    let la_text = |k: &str| match la.as_ref().and_then(|l| f64_at(l, k)) {
        Some(n) => format!("{n:.2}"),
        None => DASH.to_owned(),
    };
    let temp = f64_at(&c, "temperature_c");
    let power = f64_at(&c, "power_w");
    json!({
        "has_data": !c.is_null(),
        "load": {"value": num_or_null(load), "text": format::percent(load), "level": level(load, 85.0, 97.0)},
        "freq_avg_text": mhz(f64_at(&c, "freq_avg_mhz")),
        "freq_max_text": mhz(f64_at(&c, "freq_max_mhz")),
        "governor": str_at(&c, "governor"),
        "temp": {"value": num_or_null(temp), "text": format::temp(temp)},
        // RAPL is root-only: an ordinary user gets null, and the page says so instead of 0 W.
        "power": {"value": num_or_null(power), "text": watts(power), "available": power.is_some()},
        "load_avg": {
            "available": la.is_some(),
            "one": la_text("one"), "five": la_text("five"), "fifteen": la_text("fifteen"),
            "running": num_or_null(la.as_ref().and_then(|l| f64_at(l, "running"))),
            "tasks": num_or_null(la.as_ref().and_then(|l| f64_at(l, "tasks"))),
        },
        "cores": cores,
        "core_count": rows(&c, "cores").len(),
        "top": process_rows(rows(&c, "top"), "cpu", limit),
        "top_valid": bool_at(&c, "top_valid").unwrap_or(false),
        "processes": num_or_null(f64_at(&c, "processes")),
    })
}

// ---- gpu ------------------------------------------------------------------------------------

fn gen_width(generation: Option<i64>, width: Option<i64>) -> String {
    match (generation, width) {
        (Some(g), Some(w)) => format!("Gen {g} x{w}"),
        (Some(g), None) => format!("Gen {g}"),
        (None, Some(w)) => format!("x{w}"),
        (None, None) => DASH.to_owned(),
    }
}

fn first_f64(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    a.or(b)
}

/// The GPU tab. Input `{live, detail}`: the newest answers of `monitor gpu` without and with
/// detail (either may be `null`). The cheap answer is the fresher and decides the state; detail
/// counts only while the card is awake, so an old query never makes a sleeping card look busy.
/// `asleep` is true for a runtime-suspended card: the page says so and shows no numbers, which is
/// also what keeps it from asking again.
fn gpu(args: &Value) -> Value {
    let live = at(args, "live").cloned().unwrap_or(Value::Null);
    let detail_answer = at(args, "detail").cloned().unwrap_or(Value::Null);
    let base = if live.is_null() { &detail_answer } else { &live };
    let state = str_at(base, "state").unwrap_or("unknown");
    let awake = matches!(state, "active" | "unknown");
    let d = if awake {
        at(&detail_answer, "detail").or_else(|| at(&live, "detail")).cloned().unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    let temp = f64_at(base, "temperature_c").or_else(|| f64_at(&d, "temperature_c"));
    let util = if awake { first_f64(f64_at(base, "utilization_pct"), f64_at(&d, "utilization_pct")) } else { None };
    let power = if awake { first_f64(f64_at(base, "power_w"), f64_at(&d, "power_w")) } else { None };
    let limit = f64_at(&d, "power_limit_w");
    let vram_total = f64_at(&d, "vram_total_mib");
    let vram_used = f64_at(&d, "vram_used_mib");
    let mib = |v: Option<f64>| v.map(|n| n * 1_048_576.0);
    let gfx = f64_at(&d, "clock_graphics_mhz");
    let mem = f64_at(&d, "clock_memory_mhz");
    let gfx_max = f64_at(&d, "clock_graphics_max_mhz");
    let mem_max = f64_at(&d, "clock_memory_max_mhz");
    let limit_row = |id: &str, key: &str| json!({"id": id, "text": watts(f64_at(&d, key))});
    let (pg, pgm, pw, pwm) = (
        i64_at(&d, "pcie_gen"),
        i64_at(&d, "pcie_gen_max"),
        i64_at(&d, "pcie_width"),
        i64_at(&d, "pcie_width_max"),
    );
    let downgraded = matches!((pg, pgm), (Some(a), Some(b)) if a < b) || matches!((pw, pwm), (Some(a), Some(b)) if a < b);
    let crit = default_limits("gpu");
    json!({
        "has_data": !base.is_null(),
        "state": state,
        "asleep": state == "suspended",
        "absent": state == "absent",
        "note": str_at(base, "note"),
        "error": str_at(&detail_answer, "error").or_else(|| str_at(&live, "error")),
        "name": str_at(base, "name").or_else(|| str_at(&d, "name")).or_else(|| str_at(&detail_answer, "name")),
        "driver": str_at(base, "driver").or_else(|| str_at(&d, "driver")).or_else(|| str_at(&detail_answer, "driver")),
        "vbios": str_at(&d, "vbios"),
        "source": str_at(base, "source"),
        "has_detail": !d.is_null(),
        "detail_cached": bool_at(&detail_answer, "detail_cached").unwrap_or(false),
        "detail_age_text": f64_at(&detail_answer, "detail_age_s").map_or_else(|| DASH.to_owned(), |s| format::duration(s.max(0.0))),
        "temperature": {
            "value": num_or_null(temp), "text": format::temp(temp), "from": 0, "to": 110,
            "warn_at": crit.warn, "crit_at": crit.crit, "level": level(temp, crit.warn, crit.crit),
        },
        "util": {"value": num_or_null(util), "text": format::percent(util)},
        "memory_util_text": format::percent(f64_at(&d, "memory_utilization_pct")),
        "vram": {
            "used_text": bytes_text(mib(vram_used)),
            "total_text": bytes_text(mib(vram_total)),
            "free_text": bytes_text(mib(f64_at(&d, "vram_free_mib"))),
            "value": num_or_null(fraction(vram_used, vram_total)),
            "level": level(fraction(vram_used, vram_total).map(|f| f * 100.0), 85.0, 95.0),
        },
        "power": {
            "value": num_or_null(power), "text": watts(power), "limit_text": watts(limit),
            "fraction": num_or_null(fraction(power, limit)),
        },
        "clocks": {
            "graphics": {"text": mhz(gfx), "max_text": mhz(gfx_max), "value": num_or_null(fraction(gfx, gfx_max))},
            "memory": {"text": mhz(mem), "max_text": mhz(mem_max), "value": num_or_null(fraction(mem, mem_max))},
        },
        "pstate": str_at(&d, "pstate"),
        "pcie": {
            "text": gen_width(pg, pw), "max_text": gen_width(pgm, pwm), "downgraded": downgraded,
        },
        "fan_text": format::percent(f64_at(&d, "fan_pct")),
        "limits": [
            limit_row("current", "power_limit_w"),
            limit_row("default", "power_default_limit_w"),
            limit_row("min", "power_min_limit_w"),
            limit_row("max", "power_max_limit_w"),
        ],
    })
}

// ---- memory and disks -----------------------------------------------------------------------

fn memory_part(total: Option<f64>, used: Option<f64>, pct: Option<f64>, warn: f64, crit: f64) -> Value {
    json!({
        "total_text": bytes_text(total), "used_text": bytes_text(used),
        "pct_text": format::percent(pct),
        "value": num_or_null(pct.map(|p| clamp01(p / 100.0))),
        "level": level(pct, warn, crit),
    })
}

/// Which drive a temperature belongs to: `nvme0` is the controller of `nvme0n1`.
fn drive_temp(drive: &str, nvme: &[Value]) -> Option<f64> {
    nvme.iter()
        .find(|n| {
            // `nvme1` owns `nvme1n1` but not `nvme10n1`.
            str_at(n, "controller")
                .is_some_and(|c| !c.is_empty() && drive.strip_prefix(c).is_some_and(|rest| rest.is_empty() || rest.starts_with('n')))
        })
        .and_then(|n| f64_at(n, "celsius"))
}

/// The Memory and disks tab. Input `{memory, disks, limit?}` (`monitor memory` and `monitor disks`
/// results, either may be `null`). Answers RAM and swap, the biggest processes, the filesystems
/// as bar rows and the physical drives with their temperature where there is one.
fn memory(args: &Value) -> Value {
    let m = at(args, "memory").cloned().unwrap_or(Value::Null);
    let d = at(args, "disks").cloned().unwrap_or(Value::Null);
    let ram = f64_at(&m, "memory.total_bytes");
    let has_ram = at(&m, "memory").is_some();
    let swap_total = f64_at(&m, "memory.swap_total_bytes");
    let limit = usize::try_from(i64_at(args, "limit").unwrap_or(8).clamp(1, 25)).unwrap_or(8);
    let swap_present = swap_total.is_some_and(|t| t > 0.0);
    let swap = if swap_present {
        memory_part(swap_total, f64_at(&m, "memory.swap_used_bytes"), f64_at(&m, "memory.swap_used_pct"), 50.0, 90.0)
    } else {
        Value::Null
    };
    let nvme = rows(&d, "nvme");
    let filesystems: Vec<Value> = rows(&d, "filesystems")
        .iter()
        .map(|f| {
            let pct = f64_at(f, "used_pct");
            let mount = str_at(f, "mount").unwrap_or("");
            json!({
                "label": if mount.is_empty() { DASH } else { mount },
                "text": format::percent(pct),
                "sub": format!("{} / {}", bytes_text(f64_at(f, "used_bytes")), bytes_text(f64_at(f, "total_bytes"))),
                "device": str_at(f, "device"),
                "fstype": str_at(f, "fstype"),
                "avail_text": bytes_text(f64_at(f, "avail_bytes")),
                "value": num_or_null(pct.map(|p| clamp01(p / 100.0))),
                "level": level(pct, 85.0, 95.0),
            })
        })
        .collect();
    let drives: Vec<Value> = rows(&d, "drives")
        .iter()
        .map(|dr| {
            let name = str_at(dr, "name").unwrap_or("");
            json!({
                "name": if name.is_empty() { DASH } else { name },
                "model": str_at(dr, "model"),
                "kind": str_at(dr, "kind").unwrap_or("other"),
                "size_text": bytes_text(f64_at(dr, "size_bytes")),
                "temp_text": format::temp(drive_temp(name, nvme)),
            })
        })
        .collect();
    json!({
        "has_memory": has_ram,
        "has_disks": !d.is_null(),
        "ram": if has_ram {
            let mut v = memory_part(ram, f64_at(&m, "memory.used_bytes"), f64_at(&m, "memory.used_pct"), 85.0, 95.0);
            v["available_text"] = json!(bytes_text(f64_at(&m, "memory.available_bytes")));
            v["free_text"] = json!(bytes_text(f64_at(&m, "memory.free_bytes")));
            v["cache_text"] = json!(bytes_text(f64_at(&m, "memory.cache_bytes")));
            v
        } else { Value::Null },
        "swap": swap,
        "swap_present": swap_present,
        "top": process_rows(rows(&m, "top"), "mem", limit),
        "processes": num_or_null(f64_at(&m, "processes")),
        "filesystems": filesystems,
        "drives": drives,
    })
}

// ---- network --------------------------------------------------------------------------------

fn kind_rank(kind: &str) -> u8 {
    match kind {
        "wifi" | "ethernet" => 0,
        "virtual" | "other" => 1,
        _ => 2,
    }
}

/// The Network tab. Input `{network, rx, tx}`: a `monitor network` result (or `null`) and the two
/// rate histories of the chart. Answers rates, peaks, session totals, the interface in use (its
/// kind and address) and the chart scale (zero-based, never tighter than 100 KB/s so that line noise
/// does not fill the chart). Loopback is left out; virtual interfaces come last.
fn network(args: &Value) -> Value {
    let n = at(args, "network").cloned().unwrap_or(Value::Null);
    let primary = str_at(&n, "primary");
    let addresses = at(&n, "addresses");
    let mut interfaces: Vec<&Value> = rows(&n, "interfaces").iter().filter(|i| str_at(i, "kind") != Some("loopback")).collect();
    interfaces.sort_by_key(|i| (str_at(i, "name") != primary, kind_rank(str_at(i, "kind").unwrap_or("other"))));
    let list: Vec<Value> = interfaces
        .iter()
        .map(|i| {
            let name = str_at(i, "name").unwrap_or("");
            let addr: Vec<&str> = addresses
                .and_then(|a| a.get(name))
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            json!({
                "name": if name.is_empty() { DASH } else { name },
                "kind": str_at(i, "kind").unwrap_or("other"),
                "state": str_at(i, "state"),
                "mac": str_at(i, "mac"),
                "primary": primary == Some(name),
                "speed_text": f64_at(i, "speed_mbps").map_or_else(|| DASH.to_owned(), |s| format!("{} Mbit/s", s.round())),
                "rx_text": rate(f64_at(i, "rx_bps")),
                "tx_text": rate(f64_at(i, "tx_bps")),
                "session_rx_text": bytes_text(f64_at(i, "session_rx_bytes")),
                "session_tx_text": bytes_text(f64_at(i, "session_tx_bytes")),
                "total_rx_text": bytes_text(f64_at(i, "total_rx_bytes")),
                "total_tx_text": bytes_text(f64_at(i, "total_tx_bytes")),
                "addresses": addr,
            })
        })
        .collect();
    let primary_kind = list.iter().find(|i| i["primary"] == json!(true)).and_then(|i| i["kind"].as_str().map(str::to_owned));
    let chart = scale(&json!({"series": [rows(args, "rx"), rows(args, "tx")], "floor": 102_400, "unit": "rate"}));
    json!({
        "has_data": !n.is_null(),
        "primary": primary,
        "primary_kind": primary_kind,
        "primary_address": str_at(&n, "primary_address"),
        "rx": {"text": rate(f64_at(&n, "rx_bps")), "peak_text": rate(f64_at(&n, "peak_rx_bps")), "session_text": bytes_text(f64_at(&n, "session_rx_bytes"))},
        "tx": {"text": rate(f64_at(&n, "tx_bps")), "peak_text": rate(f64_at(&n, "peak_tx_bps")), "session_text": bytes_text(f64_at(&n, "session_tx_bytes"))},
        "interfaces": list,
        "chart": chart,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn v(verb: &str, args: &Value) -> Value {
        call(verb, args).unwrap()
    }

    fn keys(plan: &Value) -> Vec<String> {
        plan["commands"].as_array().unwrap().iter().map(|c| c["key"].as_str().unwrap().to_owned()).collect()
    }

    fn plan_at(tab: &str, now: f64, last: &Value, extra: &Value) -> Value {
        let mut a = json!({"tab": tab, "active": true, "now_ms": now, "last": last});
        for (k, val) in extra.as_object().unwrap() {
            a[k] = val.clone();
        }
        v("plan", &a)
    }

    // ---- plan ----

    #[test]
    fn a_hidden_window_or_an_unknown_tab_polls_nothing() {
        for args in [
            json!({"tab": "cpu", "active": false, "now_ms": 5}),
            json!({"tab": "nope", "active": true, "now_ms": 5}),
            json!({"tab": "cpu", "active": true}),
            json!({}),
        ] {
            assert_eq!(keys(&v("plan", &args)), Vec::<String>::new(), "{args}");
        }
    }

    #[test]
    fn each_tab_polls_only_its_own_commands_at_the_contract_cadence() {
        let none = json!({});
        assert_eq!(keys(&plan_at("overview", 0.0, &none, &none)), ["overview", "system"]);
        assert_eq!(keys(&plan_at("cpu", 0.0, &none, &none)), ["cpu"]);
        assert_eq!(keys(&plan_at("memory", 0.0, &none, &none)), ["memory", "disks"]);
        assert_eq!(keys(&plan_at("network", 0.0, &none, &none)), ["network"]);
        // Overview every 2 s, system info every 30 s.
        let last = json!({"overview": 1000.0, "system": 1000.0});
        assert_eq!(keys(&plan_at("overview", 2999.0, &last, &none)), Vec::<String>::new());
        assert_eq!(keys(&plan_at("overview", 3000.0, &last, &none)), ["overview"]);
        assert_eq!(keys(&plan_at("overview", 31_000.0, &last, &none)), ["overview", "system"]);
        // Disks every 5 s, memory every second.
        let last = json!({"memory": 0.0, "disks": 0.0});
        assert_eq!(keys(&plan_at("memory", 1000.0, &last, &none)), ["memory"]);
        assert_eq!(keys(&plan_at("memory", 5000.0, &last, &none)), ["memory", "disks"]);
    }

    #[test]
    fn commands_carry_their_area_verb_and_arguments() {
        let none = json!({});
        let p = plan_at("overview", 0.0, &none, &none);
        assert_eq!(p["commands"][0], json!({"key": "overview", "area": "monitor", "verb": "overview", "args": {}}));
        assert_eq!(p["commands"][1]["area"], "system");
        assert_eq!(p["commands"][1]["verb"], "info");
        let p = plan_at("cpu", 0.0, &none, &json!({"top": 15}));
        assert_eq!(p["commands"][0]["args"], json!({"top": 15}));
        // The process count is clamped to what the command accepts (1..25) and defaults to 8.
        assert_eq!(plan_at("cpu", 0.0, &none, &json!({"top": 99}))["commands"][0]["args"], json!({"top": 25}));
        assert_eq!(plan_at("cpu", 0.0, &none, &json!({"top": 0}))["commands"][0]["args"], json!({"top": 1}));
        assert_eq!(plan_at("cpu", 0.0, &none, &none)["commands"][0]["args"], json!({"top": 8}));
    }

    #[test]
    fn a_command_that_is_still_running_is_not_started_again() {
        let none = json!({});
        let p = plan_at("cpu", 10_000.0, &none, &json!({"pending": {"cpu": true}}));
        assert!(keys(&p).is_empty());
        let p = plan_at("memory", 10_000.0, &none, &json!({"pending": {"memory": true}}));
        assert_eq!(keys(&p), ["disks"]);
    }

    #[test]
    fn the_gpu_detail_query_is_never_made_for_a_card_that_is_asleep_or_unknown() {
        let none = json!({});
        // No answer yet: only the cheap call, which starts no process.
        let p = plan_at("gpu", 0.0, &none, &none);
        assert_eq!(keys(&p), ["gpu_live"]);
        assert_eq!(p["commands"][0]["args"], json!({"detail": false}));
        for state in ["suspended", "absent", "unknown", ""] {
            let p = plan_at("gpu", 100_000.0, &none, &json!({"gpu_state": state}));
            assert_eq!(keys(&p), ["gpu_live"], "state {state}");
        }
        // Awake: detail every 5 s, the cheap call every second.
        let p = plan_at("gpu", 0.0, &none, &json!({"gpu_state": "active"}));
        assert_eq!(keys(&p), ["gpu_live", "gpu_detail"]);
        assert_eq!(p["commands"][1]["args"], json!({"detail": true}));
        let last = json!({"gpu_live": 0.0, "gpu_detail": 0.0});
        assert_eq!(keys(&plan_at("gpu", 1000.0, &last, &json!({"gpu_state": "active"}))), ["gpu_live"]);
        assert_eq!(keys(&plan_at("gpu", 4999.0, &last, &json!({"gpu_state": "active"}))), ["gpu_live"]);
        assert_eq!(keys(&plan_at("gpu", 5000.0, &last, &json!({"gpu_state": "active"}))), ["gpu_live", "gpu_detail"]);
        // The card went to sleep between two polls: detail stops at once even though it is due.
        assert_eq!(keys(&plan_at("gpu", 5000.0, &last, &json!({"gpu_state": "suspended"}))), ["gpu_live"]);
    }

    #[test]
    fn failures_back_off_and_a_good_answer_resets_them() {
        let last = json!({"cpu": 0.0});
        let at = |now: f64, errors: Value| keys(&plan_at("cpu", now, &last, &json!({"errors": errors})));
        assert_eq!(at(1000.0, json!({})), ["cpu"]);
        assert_eq!(at(1999.0, json!({"cpu": 1})), Vec::<String>::new());
        assert_eq!(at(2000.0, json!({"cpu": 1})), ["cpu"]);
        assert_eq!(at(4000.0, json!({"cpu": 2})), ["cpu"]);
        // However many failures, never slower than 30 s.
        assert_eq!(at(30_000.0, json!({"cpu": 50})), ["cpu"]);
        assert_eq!(at(29_999.0, json!({"cpu": 50})), Vec::<String>::new());
        assert_eq!(at(1000.0, json!({"cpu": -3})), ["cpu"], "a negative count is no back-off");
    }

    #[test]
    fn a_clock_that_went_backwards_is_due_not_frozen() {
        let p = plan_at("cpu", 100.0, &json!({"cpu": 50_000.0}), &json!({}));
        assert_eq!(keys(&p), ["cpu"]);
    }

    #[test]
    fn wait_is_between_100_ms_and_a_second() {
        let w = |p: &Value| p["wait_ms"].as_f64().unwrap();
        assert_eq!(w(&v("plan", &json!({}))), 1000.0);
        let none = json!({});
        assert_eq!(w(&plan_at("cpu", 0.0, &none, &none)), 1000.0);
        assert_eq!(w(&plan_at("cpu", 950.0, &json!({"cpu": 0.0}), &none)), 100.0);
        assert_eq!(w(&plan_at("cpu", 600.0, &json!({"cpu": 0.0}), &none)), 400.0);
    }

    // ---- push, stats, scale ----

    #[test]
    fn push_appends_cuts_and_turns_missing_values_into_gaps() {
        assert_eq!(v("push", &json!({"value": 5})), json!([5.0]));
        assert_eq!(v("push", &json!({"series": [1, 2], "value": null})), json!([1, 2, null]));
        assert_eq!(v("push", &json!({"series": [1], "value": "x"})), json!([1, null]));
        assert_eq!(v("push", &json!({"series": [1, 2, 3], "value": 4, "max": 3})), json!([2, 3, 4.0]));
        assert_eq!(v("push", &json!({"series": [1, 2, 3], "value": 4, "max": 0})).as_array().unwrap().len(), 2, "max is at least 2");
        let long: Vec<i32> = (0..500).collect();
        assert_eq!(v("push", &json!({"series": long, "value": 1})).as_array().unwrap().len(), 120);
    }

    #[test]
    fn a_long_silence_leaves_one_gap_and_never_two() {
        let a = |last: f64, series: Value| v("push", &json!({"series": series, "value": 9, "now_ms": 10_000.0, "last_ms": last, "interval_ms": 1000.0}));
        assert_eq!(a(8000.0, json!([1])), json!([1, 9.0]), "two intervals is not silence");
        assert_eq!(a(7400.0, json!([1])), json!([1, null, 9.0]));
        assert_eq!(a(0.0, json!([1, null])), json!([1, null, 9.0]), "already a gap");
        assert_eq!(a(0.0, json!([])), json!([9.0]), "nothing to separate from");
        assert_eq!(a(20_000.0, json!([1])), json!([1, null, 9.0]), "a clock that went back breaks the line");
    }

    #[test]
    fn stats_ignore_gaps_and_answer_null_for_nothing() {
        let s = v("stats", &json!({"series": [4, null, 10, 1, null]}));
        assert_eq!(s["min"], 1.0);
        assert_eq!(s["max"], 10.0);
        assert_eq!(s["avg"], 5.0);
        assert_eq!(s["count"], 3);
        assert_eq!(s["latest"], Value::Null, "the newest sample is a gap");
        assert_eq!(v("stats", &json!({"series": [3, 7]}))["latest"], 7.0);
        for args in [json!({}), json!({"series": []}), json!({"series": [null, "x"]}), json!({"series": 5})] {
            let s = v("stats", &args);
            assert_eq!(s["min"], Value::Null, "{args}");
            assert_eq!(s["count"], 0);
        }
    }

    #[test]
    fn nice_ceilings_are_one_two_two_and_a_half_five_ten() {
        for (input, want) in [(0.7, 1.0), (1.0, 1.0), (1.2, 2.0), (2.1, 2.5), (3.0, 5.0), (7.0, 10.0), (130.0, 200.0), (102_400.0, 200_000.0)] {
            assert_eq!(nice_ceiling(input), want, "{input}");
        }
        assert_eq!(nice_ceiling(0.0), 1.0);
        assert_eq!(nice_ceiling(f64::NAN), 1.0);
        assert_eq!(nice_ceiling(-4.0), 1.0);
    }

    #[test]
    fn scale_is_zero_based_never_below_the_floor_and_labelled() {
        let s = v("scale", &json!({"series": [[100.0, 300_000.0, null], [50.0]], "floor": 102_400, "unit": "rate"}));
        assert_eq!(s["min"], 0.0);
        assert_eq!(s["max"], 500_000.0);
        let grid = s["grid"].as_array().unwrap();
        assert_eq!(grid.len(), 4);
        assert_eq!(grid[0]["text"], "0 B/s");
        assert_eq!(grid[3]["fraction"], 1.0);
        assert_eq!(grid[3]["text"], "488 KB/s");
        // A quiet line is held to the floor.
        let s = v("scale", &json!({"series": [[10.0, 20.0]], "floor": 100.0, "unit": "percent"}));
        assert_eq!(s["max"], 100.0);
        // Nothing at all, or only gaps, still gives a usable scale.
        for args in [json!({}), json!({"series": [[null], []]}), json!({"series": [["x"]], "floor": -5})] {
            let s = v("scale", &args);
            assert!(s["max"].as_f64().unwrap() > 0.0, "{args}");
            assert_eq!(s["grid"].as_array().unwrap().len(), 4);
        }
        assert_eq!(v("scale", &json!({"series": [[18.0]], "floor": 10, "unit": "temp"}))["grid"][3]["text"], "20 °C");
    }

    // ---- formatting ----

    #[test]
    fn rates_clocks_and_uptime_read_naturally_and_dash_when_unknown() {
        assert_eq!(rate(Some(0.0)), "0 B/s");
        assert_eq!(rate(Some(1536.0)), "1.5 KB/s");
        assert_eq!(rate(Some(12.5 * 1024.0 * 1024.0)), "12.5 MB/s");
        assert_eq!(rate(Some(-1.0)), DASH);
        assert_eq!(rate(None), DASH);
        assert_eq!(rate(Some(f64::INFINITY)), DASH);
        assert_eq!(mhz(Some(800.0)), "800 MHz");
        assert_eq!(mhz(Some(4500.0)), "4.50 GHz");
        assert_eq!(mhz(Some(999.6)), "1000 MHz");
        assert_eq!(mhz(None), DASH);
        assert_eq!(uptime(Some(45.9)), "45 s");
        assert_eq!(uptime(Some(720.0)), "12 min");
        assert_eq!(uptime(Some(7500.0)), "2 h 05 min");
        assert_eq!(uptime(Some(3.0 * 86_400.0 + 4.0 * 3600.0 + 59.0)), "3 d 4 h");
        assert_eq!(uptime(Some(-1.0)), DASH);
        assert_eq!(uptime(None), DASH);
        assert_eq!(pct_fine(Some(7.25)), "7.3 %");
        assert_eq!(pct_fine(Some(42.4)), "42 %");
        assert_eq!(pct_fine(None), DASH);
    }

    // ---- overview ----

    fn temps() -> Value {
        json!({"temperatures": [
            {"kind": "cpu_core", "celsius": 60.0, "label": "Core 0"},
            {"kind": "cpu_core", "celsius": 71.0, "label": "Core 1"},
            {"kind": "cpu_package", "celsius": 66.0, "session_min_c": 41.0, "session_max_c": 78.0, "high_c": 100.0, "critical_c": 100.0},
            {"kind": "board", "celsius": 38.0},
            {"kind": "nvme", "celsius": 44.0, "high_c": 84.8, "critical_c": 85.8},
            {"kind": "nvme", "celsius": 47.0},
            {"kind": "nvme", "celsius": 99.0},
            {"kind": "wifi", "celsius": 52.0},
            {"kind": "gpu", "celsius": 48.0}],
            "gpu_state": "active", "cpu_load_pct": 12.4, "cpu_freq_mhz": 3200, "memory_used_pct": 41.2,
            "swap_used_pct": 0.0, "net_primary": "wlan0", "net_rx_bps": 2048.0, "net_tx_bps": null, "uptime_s": 90061.0})
    }

    fn ids(o: &Value) -> Vec<String> {
        o["gauges"].as_array().unwrap().iter().map(|g| format!("{}{}", g["id"].as_str().unwrap(), g["index"])).collect()
    }

    #[test]
    fn overview_gauges_are_cpu_gpu_board_two_nvme_and_wifi() {
        let o = v("overview", &json!({"overview": temps()}));
        assert_eq!(ids(&o), ["cpu0", "gpu0", "board0", "nvme0", "nvme1", "wifi0"]);
        let cpu = &o["gauges"][0];
        assert_eq!(cpu["text"], "66 °C");
        assert_eq!(cpu["min_text"], "41 °C");
        assert_eq!(cpu["max_text"], "78 °C");
        assert_eq!(cpu["level"], "ok");
        assert_eq!(o["cpu_load"]["text"], "12 %");
        assert_eq!(o["cpu_freq_text"], "3.20 GHz");
        assert_eq!(o["net"]["rx_text"], "2.0 KB/s");
        assert_eq!(o["net"]["tx_text"], DASH);
        assert_eq!(o["uptime_text"], "1 d 1 h");
    }

    #[test]
    fn hardware_limits_lower_the_default_thresholds_and_never_raise_them() {
        let o = v("overview", &json!({"overview": temps()}));
        // CPU: hardware says 100/100, the defaults 85/95 stay.
        assert_eq!(o["gauges"][0]["warn_at"], 85.0);
        assert_eq!(o["gauges"][0]["crit_at"], 95.0);
        // NVMe: 84.8/85.8 are above the defaults 65/75, which stay; the second drive has none.
        assert_eq!(o["gauges"][3]["warn_at"], 65.0);
        // A sensor with a lower critical lowers the defaults.
        let o = v("overview", &json!({"overview": {"temperatures": [{"kind": "cpu_package", "celsius": 60, "high_c": 70, "critical_c": 80}]}}));
        assert_eq!(o["gauges"][0]["warn_at"], 70.0);
        assert_eq!(o["gauges"][0]["crit_at"], 80.0);
        // Nonsense limits (0, 1000) are ignored.
        let o = v("overview", &json!({"overview": {"temperatures": [{"kind": "cpu_package", "celsius": 60, "high_c": 0, "critical_c": 1000}]}}));
        assert_eq!(o["gauges"][0]["warn_at"], 85.0);
        // A critical at or below the warning pulls the warning under it.
        let o = v("overview", &json!({"overview": {"temperatures": [{"kind": "board", "celsius": 60, "high_c": 50, "critical_c": 50}]}}));
        assert!(o["gauges"][2]["warn_at"].as_f64().unwrap() < o["gauges"][2]["crit_at"].as_f64().unwrap());
        assert!(o["gauges"][0]["to"].as_f64().unwrap() >= 100.0);
    }

    #[test]
    fn levels_follow_the_thresholds() {
        let hot = |c: f64| v("overview", &json!({"overview": {"temperatures": [{"kind": "cpu_package", "celsius": c}]}}))["gauges"][0]["level"].clone();
        assert_eq!(hot(84.0), "ok");
        assert_eq!(hot(85.0), "warn");
        assert_eq!(hot(95.0), "bad");
    }

    #[test]
    fn the_cpu_gauge_falls_back_to_the_plain_cpu_sensor_then_the_hottest_core() {
        let o = v("overview", &json!({"overview": {"temperatures": [
            {"kind": "cpu_core", "celsius": 50.0}, {"kind": "cpu_core", "celsius": 72.0}, {"kind": "cpu_core", "celsius": null}]}}));
        assert_eq!(o["gauges"][0]["text"], "72 °C");
        let o = v("overview", &json!({"overview": {"temperatures": [
            {"kind": "cpu_core", "celsius": 72.0}, {"kind": "cpu", "celsius": 55.0}]}}));
        assert_eq!(o["gauges"][0]["text"], "55 °C");
        // A package sensor that cannot be read yet does not hide a readable one.
        let o = v("overview", &json!({"overview": {"temperatures": [
            {"kind": "cpu_package", "celsius": null}, {"kind": "cpu", "celsius": 55.0}]}}));
        assert_eq!(o["gauges"][0]["text"], "55 °C");
    }

    #[test]
    fn a_sleeping_gpu_is_badged_and_an_absent_one_has_no_gauge() {
        let mut t = temps();
        t["gpu_state"] = json!("suspended");
        let o = v("overview", &json!({"overview": t}));
        assert_eq!(o["gauges"][1]["id"], "gpu");
        assert_eq!(o["gauges"][1]["badge"], "asleep");
        assert_eq!(o["gauges"][1]["text"], "48 °C", "the firmware sensor does not wake the card");
        let o = v("overview", &json!({"overview": {"temperatures": [{"kind": "board", "celsius": 30}], "gpu_state": "absent"}}));
        assert_eq!(ids(&o), ["cpu0", "board0"]);
    }

    #[test]
    fn overview_with_nothing_is_dashes_never_zeros() {
        for args in [json!({}), json!({"overview": null}), json!({"overview": {}}), json!({"overview": {"temperatures": null, "gpu_state": null}}), json!({"overview": 5})] {
            let o = v("overview", &args);
            assert_eq!(o["gauges"][0]["text"], DASH, "{args}");
            assert_eq!(o["gauges"][0]["value"], Value::Null);
            assert_eq!(o["gauges"][0]["level"], "unknown");
            assert_eq!(o["gauges"][0]["min_text"], DASH);
            assert_eq!(o["cpu_load"]["text"], DASH);
            assert_eq!(o["memory"]["value"], Value::Null);
            assert_eq!(o["net"]["rx_text"], DASH);
            assert_eq!(o["uptime_text"], DASH);
            assert!(o["gauges"].as_array().unwrap().iter().any(|g| g["id"] == "board"));
        }
        assert_eq!(v("overview", &json!({}))["has_data"], false);
        assert_eq!(v("overview", &json!({"overview": {}}))["has_data"], true);
    }

    // ---- facts ----

    fn text_of(f: &Value, id: &str) -> String {
        f["rows"].as_array().unwrap().iter().find(|r| r["id"] == id).unwrap()["text"].as_str().unwrap().to_owned()
    }

    #[test]
    fn facts_join_what_is_known() {
        let f = v("facts", &json!({"system": {
            "machine": {"vendor": "Acer", "product": "Predator PH16-71", "bios": "V1.12", "bios_date": "2024-05-01"},
            "cpu": {"model": "Intel Core i7-13700HX", "cores": 16, "threads": 24, "max_mhz": 5000},
            "memory_total_bytes": 34_359_738_368u64,
            "gpus": [{"vendor": "10de", "device_id": "28e0", "name": "NVIDIA GeForce RTX 4060 Laptop", "driver": "nvidia 560.35"},
                     {"vendor": "8086", "device_id": "a788", "name": null, "driver": null}],
            "os": {"pretty_name": "Arch Linux"}, "kernel": "6.12.1", "uptime_s": 100.0}, "uptime_s": 3700.0}));
        assert_eq!(text_of(&f, "model"), "Acer · Predator PH16-71");
        assert_eq!(text_of(&f, "bios"), "V1.12 · 2024-05-01");
        assert_eq!(text_of(&f, "cpu"), "Intel Core i7-13700HX");
        assert_eq!(text_of(&f, "cores"), "16 / 24");
        assert_eq!(text_of(&f, "max_clock"), "5.00 GHz");
        assert_eq!(text_of(&f, "gpu0"), "NVIDIA GeForce RTX 4060 Laptop · nvidia 560.35");
        assert_eq!(text_of(&f, "gpu1"), "8086 a788");
        assert_eq!(text_of(&f, "memory"), "32.0 GB");
        assert_eq!(text_of(&f, "os"), "Arch Linux");
        assert_eq!(text_of(&f, "uptime"), "1 h 01 min", "the fresher uptime wins");
    }

    #[test]
    fn facts_with_nothing_are_all_dashes() {
        for args in [json!({}), json!({"system": null}), json!({"system": {"gpus": null, "cpu": {"cores": 4}}})] {
            let f = v("facts", &args);
            assert_eq!(text_of(&f, "model"), DASH);
            assert_eq!(text_of(&f, "gpu0"), DASH);
            assert_eq!(text_of(&f, "kernel"), DASH);
            assert_eq!(text_of(&f, "uptime"), DASH);
            assert_eq!(text_of(&f, "memory"), DASH);
        }
        assert_eq!(text_of(&v("facts", &json!({"system": {"cpu": {"cores": 4}}})), "cores"), "4 / –");
        assert_eq!(text_of(&v("facts", &json!({"system": {"machine": {"vendor": "", "product": "  "}}})), "model"), DASH);
    }

    // ---- cpu and processes ----

    fn cpu_result() -> Value {
        json!({"load_pct": 37.5, "cores": [
            {"id": 0, "load_pct": 5.25, "freq_mhz": 800, "max_freq_mhz": 5000},
            {"id": 1, "load_pct": 99.0, "freq_mhz": 4800, "max_freq_mhz": 5000},
            {"id": 2, "load_pct": null, "freq_mhz": null}],
            "freq_avg_mhz": 2200, "freq_max_mhz": 4800, "governor": "powersave",
            "load_avg": {"one": 1.5, "five": 0.75, "fifteen": 0.5, "running": 2, "tasks": 614},
            "power_w": null, "temperature_c": 61.0, "top_valid": true, "processes": 300,
            "top": [
                {"pid": 10, "name": "idle", "cmdline": "", "user": "me", "state": "S", "cpu_pct": 1.5, "mem_bytes": 1000, "threads": 1},
                {"pid": 11, "name": "", "cmdline": "/usr/bin/firefox --new-window", "user": "me", "state": "R", "cpu_pct": 250.0, "mem_bytes": 2147483648u64, "threads": 90},
                {"pid": 12, "name": "", "cmdline": "", "user": null, "state": null, "cpu_pct": null, "mem_bytes": null, "threads": null},
                "junk"]})
    }

    #[test]
    fn cpu_tab_shows_cores_load_average_and_a_ranked_process_list() {
        let c = v("cpu", &json!({"cpu": cpu_result()}));
        assert_eq!(c["load"]["text"], "38 %");
        assert_eq!(c["freq_avg_text"], "2.20 GHz");
        assert_eq!(c["governor"], "powersave");
        assert_eq!(c["temp"]["text"], "61 °C");
        assert_eq!(c["core_count"], 3);
        assert_eq!(c["cores"][0]["text"], "5 %");
        assert_eq!(c["cores"][0]["sub"], "800 MHz");
        assert_eq!(c["cores"][0]["value"], 0.0525);
        assert_eq!(c["cores"][1]["level"], "bad");
        assert_eq!(c["cores"][2]["value"], Value::Null);
        assert_eq!(c["cores"][2]["text"], DASH);
        assert_eq!(c["cores"][2]["level"], "unknown");
        assert_eq!(c["load_avg"]["one"], "1.50");
        assert_eq!(c["load_avg"]["tasks"], 614.0);
        // RAPL is root-only: no power is "not available", not 0 W.
        assert_eq!(c["power"]["available"], false);
        assert_eq!(c["power"]["text"], DASH);
        let top = c["top"].as_array().unwrap();
        assert_eq!(top.len(), 3, "a member that is not a process is dropped");
        assert_eq!(top[0]["name"], "firefox", "the program of the command line, not its path");
        assert_eq!(top[0]["cpu_text"], "250 %");
        assert_eq!(top[0]["value"], 1.0);
        assert_eq!(top[0]["state_id"], "running");
        assert_eq!(top[0]["mem_text"], "2.0 GB");
        assert_eq!(top[1]["name"], "idle");
        assert_eq!(top[1]["cpu_text"], "1.5 %");
        assert_eq!(top[1]["value"], json!(1.5 / 250.0), "a share of the busiest row (at least one core)");
        assert_eq!(top[2]["name"], "pid 12");
        assert_eq!(top[2]["cpu_text"], DASH, "a process without a value is last");
        assert_eq!(top[2]["value"], Value::Null);
        assert_eq!(top[2]["state_id"], "other");
    }

    #[test]
    fn a_visible_power_reading_is_kept() {
        let c = v("cpu", &json!({"cpu": {"power_w": 28.54}}));
        assert_eq!(c["power"]["available"], true);
        assert_eq!(c["power"]["text"], "28.5 W");
    }

    #[test]
    fn the_process_list_is_cut_and_hostile_names_are_clipped() {
        let long = "x".repeat(10_000);
        let list: Vec<Value> = (0..30).map(|i| json!({"pid": i, "name": long, "cmdline": long, "cpu_pct": i})).collect();
        let c = v("cpu", &json!({"cpu": {"top": list}, "limit": 5}));
        let top = c["top"].as_array().unwrap();
        assert_eq!(top.len(), 5);
        assert_eq!(top[0]["pid"], 29.0, "biggest first even when the backend sent them the other way");
        assert!(top[0]["name"].as_str().unwrap().chars().count() <= MAX_NAME + 1);
        assert!(top[0]["cmdline"].as_str().unwrap().chars().count() <= MAX_CMDLINE + 1);
        assert_eq!(v("cpu", &json!({"cpu": {"top": []}, "limit": 0}))["top"], json!([]));
    }

    #[test]
    fn process_states_have_ids() {
        for (s, id) in [("R", "running"), ("S", "sleeping"), ("D", "waiting"), ("Z", "zombie"), ("T", "stopped"), ("t", "stopped"), ("I", "idle"), ("X", "other"), ("", "other")] {
            assert_eq!(state_id(Some(s)), id, "{s}");
        }
        assert_eq!(state_id(None), "other");
    }

    #[test]
    fn cpu_with_nothing_is_dashes_and_no_rows() {
        for args in [json!({}), json!({"cpu": null}), json!({"cpu": {}}), json!({"cpu": {"cores": null, "top": null, "load_avg": null}}), json!({"cpu": "x"})] {
            let c = v("cpu", &args);
            assert_eq!(c["load"]["text"], DASH, "{args}");
            assert_eq!(c["load"]["value"], Value::Null);
            assert_eq!(c["cores"], json!([]));
            assert_eq!(c["top"], json!([]));
            assert_eq!(c["top_valid"], false);
            assert_eq!(c["load_avg"]["one"], DASH);
            assert_eq!(c["load_avg"]["available"], false);
            assert_eq!(c["governor"], Value::Null);
        }
        assert_eq!(v("cpu", &json!({}))["has_data"], false);
    }

    // ---- gpu ----

    fn detail_answer() -> Value {
        json!({"state": "active", "name": "RTX 4060", "driver": "560.35", "temperature_c": 51.0, "utilization_pct": 23.0,
            "power_w": 38.5, "source": "nvidia_smi", "detail_cached": true, "detail_age_s": 3.0, "next_detail_in_s": 2.0, "error": null,
            "detail": {"name": "RTX 4060", "vbios": "95.07.1", "temperature_c": 52.0, "utilization_pct": 23.0, "memory_utilization_pct": 9.0,
                "vram_total_mib": 8188.0, "vram_used_mib": 1024.0, "vram_free_mib": 7000.0, "power_w": 38.5, "power_limit_w": 80.0,
                "power_default_limit_w": 80.0, "power_min_limit_w": 80.0, "power_max_limit_w": 80.0,
                "clock_graphics_mhz": 1500.0, "clock_memory_mhz": 8001.0, "clock_graphics_max_mhz": 3000.0, "clock_memory_max_mhz": 8001.0,
                "pstate": "P2", "pcie_gen": 3, "pcie_gen_max": 4, "pcie_width": 8, "pcie_width_max": 8, "fan_pct": null}})
    }

    #[test]
    fn gpu_tab_shows_every_detail_of_an_awake_card() {
        let live = json!({"state": "active", "temperature_c": 53.0, "utilization_pct": null, "power_w": null, "source": "firmware", "detail": null});
        let g = v("gpu", &json!({"live": live, "detail": detail_answer()}));
        assert_eq!(g["asleep"], false);
        assert_eq!(g["has_detail"], true);
        assert_eq!(g["temperature"]["text"], "53 °C", "the fresher cheap answer wins");
        assert_eq!(g["util"]["text"], "23 %", "and the query fills what it lacks");
        assert_eq!(g["power"]["text"], "38.5 W");
        assert_eq!(g["power"]["limit_text"], "80 W");
        assert_eq!(g["power"]["fraction"], 38.5 / 80.0);
        assert_eq!(g["vram"]["used_text"], "1.0 GB");
        assert_eq!(g["vram"]["total_text"], "8.0 GB");
        assert_eq!(g["vram"]["value"], 1024.0 / 8188.0);
        assert_eq!(g["clocks"]["graphics"]["text"], "1.50 GHz");
        assert_eq!(g["clocks"]["graphics"]["max_text"], "3.00 GHz");
        assert_eq!(g["clocks"]["graphics"]["value"], 0.5);
        assert_eq!(g["pstate"], "P2");
        assert_eq!(g["pcie"]["text"], "Gen 3 x8");
        assert_eq!(g["pcie"]["max_text"], "Gen 4 x8");
        assert_eq!(g["pcie"]["downgraded"], true);
        assert_eq!(g["fan_text"], DASH);
        assert_eq!(g["vbios"], "95.07.1");
        assert_eq!(g["detail_cached"], true);
        assert_eq!(g["detail_age_text"], "3 s");
        let limits: Vec<&str> = g["limits"].as_array().unwrap().iter().map(|l| l["text"].as_str().unwrap()).collect();
        assert_eq!(limits, ["80 W", "80 W", "80 W", "80 W"]);
        assert_eq!(g["name"], "RTX 4060");
    }

    #[test]
    fn a_sleeping_gpu_shows_no_numbers_even_when_an_old_query_exists() {
        let live = json!({"state": "suspended", "note": "The graphics card is asleep.", "temperature_c": 44.0, "utilization_pct": 0, "power_w": 0, "detail": null});
        let g = v("gpu", &json!({"live": live, "detail": detail_answer()}));
        assert_eq!(g["asleep"], true);
        assert_eq!(g["has_detail"], false, "an old query must not make a sleeping card look busy");
        assert_eq!(g["util"]["text"], DASH);
        assert_eq!(g["power"]["text"], DASH);
        assert_eq!(g["vram"]["used_text"], DASH);
        assert_eq!(g["pcie"]["text"], DASH);
        assert_eq!(g["temperature"]["text"], "44 °C", "the firmware temperature does not wake the card");
        assert_eq!(g["note"], "The graphics card is asleep.");
    }

    #[test]
    fn gpu_states_and_missing_answers() {
        let absent = v("gpu", &json!({"live": {"state": "absent"}}));
        assert_eq!(absent["absent"], true);
        assert_eq!(absent["asleep"], false);
        let unknown = v("gpu", &json!({"live": {"state": "unknown", "temperature_c": 50}}));
        assert_eq!(unknown["asleep"], false);
        assert_eq!(unknown["temperature"]["level"], "ok");
        for args in [json!({}), json!({"live": null, "detail": null}), json!({"live": {}}), json!({"live": 3, "detail": "x"})] {
            let g = v("gpu", &args);
            assert_eq!(g["temperature"]["text"], DASH, "{args}");
            assert_eq!(g["temperature"]["level"], "unknown");
            assert_eq!(g["util"]["value"], Value::Null);
            assert_eq!(g["vram"]["value"], Value::Null);
            assert_eq!(g["clocks"]["graphics"]["value"], Value::Null);
            assert_eq!(g["pstate"], Value::Null);
            assert_eq!(g["has_detail"], false);
        }
        assert_eq!(v("gpu", &json!({}))["has_data"], false);
        // The detail answer alone (the cheap one has not arrived) is used.
        let only = v("gpu", &json!({"detail": detail_answer()}));
        assert_eq!(only["has_detail"], true);
        // A failed query is reported.
        let failed = v("gpu", &json!({"live": {"state": "active"}, "detail": {"state": "active", "error": "nvidia-smi timed out"}}));
        assert_eq!(failed["error"], "nvidia-smi timed out");
        assert_eq!(failed["has_detail"], false);
    }

    #[test]
    fn a_zero_sized_vram_or_limit_is_no_fraction() {
        let g = v("gpu", &json!({"live": {"state": "active", "detail": {"vram_total_mib": 0, "vram_used_mib": 5, "power_w": 5, "power_limit_w": 0}}}));
        assert_eq!(g["vram"]["value"], Value::Null);
        assert_eq!(g["power"]["fraction"], Value::Null);
    }

    // ---- memory and disks ----

    #[test]
    fn memory_tab_has_ram_swap_processes_filesystems_and_drive_temperatures() {
        let m = json!({"memory": {"total_bytes": 34359738368u64, "used_bytes": 8589934592u64, "available_bytes": 25769803776u64,
            "free_bytes": 2147483648u64, "cache_bytes": 12884901888u64, "used_pct": 25.0,
            "swap_total_bytes": 8589934592u64, "swap_used_bytes": 0, "swap_used_pct": 0.0},
            "top": [{"pid": 1, "name": "a", "mem_bytes": 1000}, {"pid": 2, "name": "b", "mem_bytes": 3000}], "processes": 200});
        let d = json!({"filesystems": [
            {"device": "/dev/nvme0n1p2", "fstype": "ext4", "mount": "/", "total_bytes": 1000000000u64, "used_bytes": 960000000u64, "avail_bytes": 40000000u64, "used_pct": 96.0},
            {"device": "x", "fstype": "vfat", "mount": "", "total_bytes": null, "used_pct": null}],
            "drives": [{"name": "nvme0n1", "model": "Samsung", "size_bytes": 1000204886016u64, "kind": "nvme"}, {"name": "sda", "size_bytes": null, "kind": "hdd"}],
            "nvme": [{"controller": "nvme0", "celsius": 41.0}, {"controller": null, "celsius": 99.0}]});
        let r = v("memory", &json!({"memory": m, "disks": d}));
        assert_eq!(r["ram"]["total_text"], "32.0 GB");
        assert_eq!(r["ram"]["used_text"], "8.0 GB");
        assert_eq!(r["ram"]["pct_text"], "25 %");
        assert_eq!(r["ram"]["value"], 0.25);
        assert_eq!(r["ram"]["cache_text"], "12.0 GB");
        assert_eq!(r["swap_present"], true);
        assert_eq!(r["swap"]["pct_text"], "0 %");
        assert_eq!(r["top"][0]["name"], "b");
        assert_eq!(r["top"][0]["mem_text"], "2.9 KB");
        assert_eq!(r["top"][0]["value"], 1.0);
        assert_eq!(r["top"][1]["value"], 1000.0 / 3000.0);
        assert_eq!(r["filesystems"][0]["label"], "/");
        assert_eq!(r["filesystems"][0]["level"], "bad");
        assert_eq!(r["filesystems"][0]["value"], 0.96);
        assert_eq!(r["filesystems"][1]["label"], DASH);
        assert_eq!(r["filesystems"][1]["text"], DASH);
        assert_eq!(r["drives"][0]["temp_text"], "41 °C");
        assert_eq!(r["drives"][1]["temp_text"], DASH, "a temperature of no known controller belongs to no drive");
        assert_eq!(r["drives"][1]["size_text"], DASH);
    }

    #[test]
    fn a_drive_temperature_matches_its_own_controller_only() {
        let nvme = [json!({"controller": "nvme1", "celsius": 40.0}), json!({"controller": "nvme10", "celsius": 50.0})];
        assert_eq!(drive_temp("nvme1n1", &nvme), Some(40.0));
        assert_eq!(drive_temp("nvme10n1", &nvme), Some(50.0));
        assert_eq!(drive_temp("nvme2n1", &nvme), None);
        assert_eq!(drive_temp("sda", &nvme), None);
    }

    #[test]
    fn no_swap_is_said_not_drawn_as_an_empty_bar() {
        let m = json!({"memory": {"total_bytes": 100, "used_bytes": 10, "used_pct": 10.0, "swap_total_bytes": 0, "swap_used_bytes": 0, "swap_used_pct": 0.0}});
        let r = v("memory", &json!({"memory": m}));
        assert_eq!(r["swap_present"], false);
        assert_eq!(r["swap"], Value::Null);
        assert_eq!(r["has_disks"], false);
    }

    #[test]
    fn memory_with_nothing_is_empty() {
        for args in [json!({}), json!({"memory": null, "disks": null}), json!({"memory": {"memory": null, "top": null}, "disks": {"filesystems": null}}), json!({"memory": 4})] {
            let r = v("memory", &args);
            assert_eq!(r["has_memory"], false, "{args}");
            assert_eq!(r["ram"], Value::Null);
            assert_eq!(r["swap_present"], false);
            assert_eq!(r["top"], json!([]));
            assert_eq!(r["filesystems"], json!([]));
            assert_eq!(r["drives"], json!([]));
        }
    }

    // ---- network ----

    fn net_result() -> Value {
        json!({"primary": "wlan0", "rx_bps": 1048576.0, "tx_bps": 2048.0, "peak_rx_bps": 5242880.0, "peak_tx_bps": 4096.0,
            "session_rx_bytes": 1073741824u64, "session_tx_bytes": 1048576,
            "primary_address": "192.168.1.5",
            "addresses": {"wlan0": ["192.168.1.5", "10.0.0.2"], "docker0": ["172.17.0.1"]},
            "interfaces": [
                {"name": "lo", "kind": "loopback", "rx_bps": 5.0},
                {"name": "docker0", "kind": "virtual", "state": "up", "rx_bps": 0.0, "tx_bps": 0.0},
                {"name": "enp3s0", "kind": "ethernet", "state": "down", "speed_mbps": null, "rx_bps": null, "tx_bps": null},
                {"name": "wlan0", "kind": "wifi", "state": "up", "mac": "aa:bb", "speed_mbps": 866, "rx_bps": 1048576.0, "tx_bps": 2048.0,
                 "session_rx_bytes": 1073741824u64, "total_rx_bytes": 2147483648u64}]})
    }

    #[test]
    fn network_tab_orders_interfaces_and_formats_rates_peaks_and_totals() {
        let n = v("network", &json!({"network": net_result(), "rx": [100000.0, 3000000.0], "tx": [5.0]}));
        assert_eq!(n["rx"]["text"], "1.0 MB/s");
        assert_eq!(n["rx"]["peak_text"], "5.0 MB/s");
        assert_eq!(n["rx"]["session_text"], "1.0 GB");
        assert_eq!(n["tx"]["text"], "2.0 KB/s");
        assert_eq!(n["primary"], "wlan0");
        assert_eq!(n["primary_kind"], "wifi");
        assert_eq!(n["primary_address"], "192.168.1.5");
        let names: Vec<&str> = n["interfaces"].as_array().unwrap().iter().map(|i| i["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["wlan0", "enp3s0", "docker0"], "primary first, loopback gone, virtual last");
        let wlan = &n["interfaces"][0];
        assert_eq!(wlan["speed_text"], "866 Mbit/s");
        assert_eq!(wlan["addresses"], json!(["192.168.1.5", "10.0.0.2"]));
        assert_eq!(wlan["primary"], true);
        assert_eq!(wlan["total_rx_text"], "2.0 GB");
        assert_eq!(n["interfaces"][1]["rx_text"], DASH);
        assert_eq!(n["interfaces"][1]["speed_text"], DASH);
        assert_eq!(n["interfaces"][1]["addresses"], json!([]));
        // 3 MB/s sample: the next round number above 3.15 MB/s.
        assert_eq!(n["chart"]["max"], 5_000_000.0);
    }

    #[test]
    fn a_quiet_network_keeps_the_chart_scale_at_its_floor() {
        let n = v("network", &json!({"network": net_result(), "rx": [10.0, 20.0, null], "tx": []}));
        assert_eq!(n["chart"]["max"], 200_000.0);
    }

    #[test]
    fn network_with_nothing_is_dashes_and_no_interfaces() {
        for args in [json!({}), json!({"network": null}), json!({"network": {"interfaces": null, "addresses": 4}}), json!({"network": {"interfaces": [{"kind": null}, 3]}})] {
            let n = v("network", &args);
            assert_eq!(n["rx"]["text"], DASH, "{args}");
            assert_eq!(n["rx"]["peak_text"], DASH);
            assert_eq!(n["tx"]["session_text"], DASH);
            assert_eq!(n["primary"], Value::Null);
            assert_eq!(n["chart"]["grid"].as_array().unwrap().len(), 4);
        }
        assert_eq!(v("network", &json!({}))["has_data"], false);
        assert_eq!(v("network", &json!({}))["interfaces"], json!([]));
    }

    // ---- the views the QML tests draw ----

    /// `tests/monitor-fixtures.json` holds realistic and empty inputs for every view verb and
    /// `tests/monitor-views.json` what this module answers for them. The QML component test of the
    /// Monitor tabs draws the same views, so a change of the contract between the two fails here
    /// until the file is regenerated (see the ignored test below) and the QML is looked at again.
    #[test]
    fn the_fixture_views_are_what_the_module_answers() {
        let fixtures: Value = serde_json::from_str(include_str!("../../tests/monitor-fixtures.json")).unwrap();
        let views: Value = serde_json::from_str(include_str!("../../tests/monitor-views.json")).unwrap();
        let fixtures = fixtures.as_object().unwrap();
        assert!(fixtures.len() >= 12);
        assert_eq!(views.as_object().unwrap().len(), fixtures.len(), "regenerate tests/monitor-views.json");
        for (name, f) in fixtures {
            // Through text, as the file went: 4211.0 and 4211 are different JSON numbers to `==`.
            let answered = call(f["verb"].as_str().unwrap(), &f["args"]).unwrap();
            let got: Value = serde_json::from_str(&serde_json::to_string(&answered).unwrap()).unwrap();
            assert_eq!(got, views[name], "{name}");
        }
    }

    #[test]
    #[ignore = "regenerates tests/monitor-views.json from tests/monitor-fixtures.json"]
    fn regenerate_the_fixture_views() {
        let fixtures: Value = serde_json::from_str(include_str!("../../tests/monitor-fixtures.json")).unwrap();
        let mut out = serde_json::Map::new();
        for (name, f) in fixtures.as_object().unwrap() {
            out.insert(name.clone(), call(f["verb"].as_str().unwrap(), &f["args"]).unwrap());
        }
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/monitor-views.json");
        std::fs::write(path, serde_json::to_string_pretty(&Value::Object(out)).unwrap() + "\n").unwrap();
    }

    // ---- dispatch ----

    #[test]
    fn every_verb_answers_and_an_unknown_verb_is_an_error() {
        for verb in ["plan", "push", "stats", "scale", "overview", "facts", "cpu", "gpu", "memory", "network"] {
            assert!(call(verb, &json!({})).is_ok(), "{verb}");
            assert!(call(verb, &Value::Null).is_ok(), "{verb} with null arguments");
            assert!(call(verb, &json!([1, 2])).is_ok(), "{verb} with an array");
        }
        assert!(call("nope", &json!({})).unwrap_err().contains("nope"));
    }
}
