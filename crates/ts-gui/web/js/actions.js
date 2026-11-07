// The PREDATOR-key action vocabulary (see actions.py): `name` or `name:argument`, the same
// text the daemon validates. The editor shows "what" and a detail; these functions convert.

export const POWER_MODES = ["eco", "quiet", "balanced", "performance", "turbo"];

// kind: fixed (no detail) | mode | cycle | auto | fan | profile | look | command | text
export const ACTION_TYPES = [
  { id: "none", label: "Nothing", kind: "fixed", base: "none" },
  { id: "power-next", label: "Next power mode", kind: "fixed", base: "power-next" },
  { id: "power-prev", label: "Previous power mode", kind: "fixed", base: "power-prev" },
  { id: "power", label: "Set a power mode", kind: "mode", base: "power" },
  { id: "power-cycle", label: "Cycle through power modes", kind: "cycle", base: "power-cycle" },
  { id: "auto-next", label: "Mode control: next (Manual / Native / Smart)", kind: "fixed", base: "auto-next" },
  { id: "auto", label: "Mode control: choose", kind: "auto", base: "auto" },
  { id: "fan", label: "Fans: set", kind: "fan", base: "fan" },
  { id: "fan-max-toggle", label: "Fans: toggle Max", kind: "fixed", base: "fan-max-toggle" },
  { id: "smart-fan-toggle", label: "Fans: toggle smart curve", kind: "fixed", base: "smart-fan-toggle" },
  { id: "profile", label: "Apply a profile", kind: "profile", base: "profile" },
  { id: "profile-next", label: "Next profile", kind: "fixed", base: "profile-next" },
  { id: "look", label: "Apply a lighting look", kind: "look", base: "look" },
  { id: "output-toggle", label: "Switch speakers / headphones", kind: "fixed", base: "output-toggle" },
  { id: "output-cycle", label: "Cycle audio outputs", kind: "fixed", base: "output-cycle" },
  { id: "mic-toggle", label: "Mute the microphones", kind: "fixed", base: "mic-toggle" },
  { id: "voice-next", label: "Next voice mode", kind: "fixed", base: "voice-next" },
  { id: "touchpad-toggle", label: "Toggle the touchpad", kind: "fixed", base: "touchpad-toggle" },
  { id: "keyboard:toggle", label: "Keyboard light on/off", kind: "fixed", base: "keyboard:toggle" },
  { id: "keyboard-effect-next", label: "Next keyboard effect", kind: "fixed", base: "keyboard-effect-next" },
  { id: "rear:toggle", label: "Rear light on/off", kind: "fixed", base: "rear:toggle" },
  { id: "rear-effect-next", label: "Next rear effect", kind: "fixed", base: "rear-effect-next" },
  { id: "battery-limit-toggle", label: "80 % battery limit on/off", kind: "fixed", base: "battery-limit-toggle" },
  { id: "overdrive-toggle", label: "LCD overdrive on/off", kind: "fixed", base: "overdrive-toggle" },
  { id: "exec", label: "Run a program or script", kind: "command", base: "exec" },
  { id: "notify", label: "Show a message", kind: "text", base: "notify" },
];

export const FAN_CHOICES = ["smart", "auto", "max", "manual %"];
export const AUTO_CHOICES = ["off", "native", "smart"];

const typeById = (id) => ACTION_TYPES.find((t) => t.id === id);
export const typeOf = (id) => typeById(id) ?? ACTION_TYPES[0];

/** Editor state for an action spec: {id, choice, text, percent}. Unknown specs show as a command. */
export function parseSpec(spec) {
  const s = String(spec ?? "none").trim();
  const i = s.indexOf(":");
  const name = i < 0 ? s : s.slice(0, i).trim();
  const arg = i < 0 ? "" : s.slice(i + 1).trim();
  const state = { id: "none", choice: "", text: "", percent: 40 };
  const fixed = ACTION_TYPES.find((t) => t.kind === "fixed" && t.base === s);
  if (fixed) return { ...state, id: fixed.id };
  const t = ACTION_TYPES.find((x) => x.kind !== "fixed" && x.base === name);
  if (!t) return { ...state, id: "exec", text: s };
  switch (t.kind) {
    case "fan":
      if (/^\d+$/.test(arg)) return { ...state, id: t.id, choice: "manual %", percent: Number(arg) };
      return { ...state, id: t.id, choice: FAN_CHOICES.includes(arg) ? arg : "smart" };
    case "cycle":
    case "command":
    case "text":
      return { ...state, id: t.id, text: arg };
    default:
      return { ...state, id: t.id, choice: arg };
  }
}

/** The action spec for an editor state (what is stored and validated by the daemon). */
export function buildSpec(st) {
  const t = typeOf(st.id);
  switch (t.kind) {
    case "fixed": return t.base;
    case "mode": case "auto": case "profile": case "look": return `${t.base}:${st.choice ?? ""}`;
    case "cycle": return `power-cycle:${String(st.text ?? "").replace(/\s+/g, "")}`;
    case "fan": return st.choice === "manual %" ? `fan:${Math.round(st.percent)}` : `fan:${st.choice || "smart"}`;
    case "command": return `exec:${st.text ?? ""}`;
    default: return `notify:${st.text ?? ""}`;
  }
}

/** Defaults when the user switches the type: a sensible first detail. */
export function defaultsFor(id, lists = {}) {
  const t = typeOf(id);
  const base = { id, choice: "", text: "", percent: 40 };
  switch (t.kind) {
    case "mode": return { ...base, choice: "balanced" };
    case "auto": return { ...base, choice: "native" };
    case "fan": return { ...base, choice: "smart" };
    case "cycle": return { ...base, text: "eco,balanced,turbo" };
    case "profile": return { ...base, choice: (lists.profiles || [])[0] ?? "" };
    case "look": return { ...base, choice: (lists.looks || [])[0] ?? "" };
    default: return base;
  }
}
