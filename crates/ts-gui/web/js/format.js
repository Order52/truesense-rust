// Pure formatting helpers: no DOM, no network, unit-tested with `node --test`.

export const clamp = (v, lo, hi) => Math.min(hi, Math.max(lo, v));

export const capitalize = (s) => (s ? s[0].toUpperCase() + s.slice(1) : "");

const isNum = (v) => typeof v === "number" && Number.isFinite(v);

export const fmtTemp = (v) => (isNum(v) ? `${Math.round(v)} °C` : "–");
export const fmtRpm = (v) => (isNum(v) ? `${Math.round(v)} RPM` : "–");
export const fmtPct = (v) => (isNum(v) ? `${Math.round(v)} %` : "–");
export const fmtWatt = (v) => (isNum(v) ? `${Math.round(v * 10) / 10} W` : "–");
export const fmtSigned = (v, unit) => `${v > 0 ? "+" : v < 0 ? "−" : ""}${Math.abs(v)}${unit ? " " + unit : ""}`;

/** "2 min", "45 s": how long a hold or a delay still lasts. */
export function fmtDuration(seconds) {
  const s = Math.max(0, Math.round(seconds));
  if (s < 90) return `${s} s`;
  const m = Math.round(s / 60);
  return m < 90 ? `${m} min` : `${Math.round(m / 6) / 10} h`;
}

/** The text on the Mode control row for every auto mode. */
export const AUTO_HINT = {
  off: "You choose the mode. A mode you pick stays until you change it.",
  native: "Like PredatorSense on Windows: Balanced on power, Eco on battery, nothing else.",
  smart:
    "Switches by heat, load and battery: Performance and Turbo when gaming or heavy work (backs off if it overheats), Eco on battery. A mode you pick yourself ends by itself when the situation changes.",
};

export const FAN_HINT = {
  smart: "Follows your fan curve: quiet when cool, faster as it heats. Edit it under Fan curves.",
  auto: "The firmware's own control, exactly like Auto in PredatorSense on Windows (the fans stop when cool).",
  max: "Both fans at full speed until you pick another mode.",
  manual:
    "The sliders go from 20 to 100 %. Turn fans off asks first and only works while cool; the fans return to Auto by themselves if it gets warm. On this laptop the firmware may keep them at a minimum speed, and TrueSense tells you what they really do.",
};

/** One line about a saved lighting look, like `truesense look show`. */
export function describeLook(look) {
  const k = (look && look.keyboard) || null;
  const r = (look && look.rear) || null;
  const kd = k
    ? `keyboard ${k.name ?? k.mode} brightness ${k.brightness ?? "?"}` + (k.speed ? ` speed ${k.speed}` : "")
    : "keyboard unchanged";
  const rd = r ? `rear ${r.mode} brightness ${r.brightness}` : "rear unchanged";
  return `${kd}; ${rd}`;
}

/** What a whole-state profile changes, one line per area (port of profiles.describe). */
export function describeProfile(d) {
  const out = [];
  const p = d.power || {}, f = d.fan || {}, k = d.keyboard || {}, r = d.rear || {}, b = d.battery || {}, a = d.audio || {};
  const join = (xs) => xs.filter(Boolean).join(", ");
  if ("mode" in p || "auto" in p)
    out.push("Power: " + join(["mode" in p ? capitalize(p.mode) + " mode (held for a while)" : "", "auto" in p ? "mode control " + p.auto : ""]));
  if ("mode" in f || "curve" in f) {
    const names = { smart: "smart curve", auto: "firmware Auto", max: "maximum", manual: `manual ${f.cpu ?? "?"} % / ${f.gpu ?? "?"} %` };
    const txt = "mode" in f ? (names[f.mode] ?? f.mode ?? "") : "";
    out.push("Fans: " + join([txt, "curve" in f ? "curve " + f.curve : ""]));
  }
  if (Object.keys(k).length) {
    const bits = [];
    if ("effect" in k) bits.push(k.effect + " effect");
    if ("profile" in k) bits.push("key set " + k.profile);
    if ("on" in k) bits.push("backlight " + (k.on ? "on" : "off"));
    if ("timeout" in k) bits.push("light off after 30 s " + (k.timeout ? "on" : "off"));
    out.push("Keyboard: " + bits.join(", "));
  }
  if (Object.keys(r).length) out.push("Rear light: " + (r.effect ? `${r.effect} ${r.color ?? ""}`.trim() : r.on ? "on" : "off"));
  if (Object.keys(b).length)
    out.push("Battery: " + join(["limit" in b ? "80 % limit " + (b.limit ? "on" : "off") : "", "usb_charge" in b ? `USB charging while off ${b.usb_charge}` : ""]));
  if (d.display && Object.keys(d.display).length) out.push("Screen: overdrive " + (d.display.overdrive ? "on" : "off"));
  if (d.boot && Object.keys(d.boot).length) out.push("Boot sound " + (d.boot.sound ? "on" : "off"));
  if (Object.keys(a).length)
    out.push(
      "Audio: " + join([a.output ?? "", "voice_headset" in a ? "headset noise reduction " + a.voice_headset : "", "voice_array" in a ? "built-in noise reduction " + a.voice_array : ""]),
    );
  return out.length ? out : ["(nothing)"];
}

/** A profile name as the CLI accepts it: letters, digits, . _ - (max 40). */
export const validName = (s) => /^[A-Za-z0-9._-]{1,40}$/.test(s || "");

export const NAME_HELP = "Names use letters, digits, . _ - (up to 40).";

/** "name  (yours)" labels for a select. */
export const withSource = (name, source) => (source ? `${name}  (${source})` : name);
