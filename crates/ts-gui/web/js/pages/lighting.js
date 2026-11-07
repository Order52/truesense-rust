// Lighting: keyboard effects, per-key editor, rear bar, saved profiles (one tab at a time).

import { call, tryCall, live } from "../api.js";
import { h, card, row, sep, button, select, slider, switchRow, seg, colorField, tabs, run } from "../ui.js";
import { capitalize } from "../format.js";
import { createPerKey } from "./perkey.js";
import { createLooksTab } from "./looks-tab.js";

const KB_EFFECTS = ["static", "breathing", "wave", "snake", "ripple", "neon", "raindrop", "lightning", "spot", "star", "fireball", "snow", "heartbeat"];
const NO_COLOR = new Set(["off", "wave", "neon"]); // effects whose colour the hardware ignores
const REAR_EFFECTS = ["static", "breathing", "neon", "wave", "twinkling"];
const SPEED_MIN = 1, SPEED_MAX = 12;
const REAR_DEFAULT = { effect: "breathing", color: "3723c7", speed: 3, brightness: 100, direction: 1 };

function keyboardTab() {
  const el = h("div", { class: "stack" });
  const on = switchRow("Backlight", { onChange: (v) => run(v ? call("keyboard", "on", {}) : call("keyboard", "off", {}), (r) => (typeof r === "string" ? r : `Backlight ${v ? "on" : "off"}`)) });
  const level = slider({ label: "Brightness", min: 0, max: 50, step: 5, value: 25, onInput: (v) => live("kb-level", () => call("keyboard", "brightness", { level: v })) });
  const fx = select(KB_EFFECTS.map((e) => ({ value: e, label: capitalize(e) })), {
    label: "Lighting mode",
    onChange: async (name) => {
      await run(call("keyboard", "effect", { name, color: color.get(), speed: speed.get() || 5, brightness: Math.max(level.get(), name === "neon" ? 50 : 10) }), `Effect: ${capitalize(name)}`);
      paintEnabled(name);
    },
  });
  const color = colorField({ value: "00aec7", label: "Effect colour", onInput: (hex) => live("kb-color", () => call("keyboard", "color", { color: hex })) });
  const speed = slider({ label: "Effect speed", min: SPEED_MIN, max: SPEED_MAX, step: 1, value: 5, onInput: (v) => live("kb-speed", () => call("keyboard", "speed", { level: v })) });
  const follow = switchRow("Colour follows the power mode", { onChange: (v) => run(call("keyboard", "operating", { action: v ? "on" : "off" }), `Keyboard colour follows the power mode: ${v ? "on" : "off"}`) },
    "Eco green, Quiet blue, Balanced cyan, Performance orange, Turbo red. Off leaves the colours you picked.");
  const c1 = card("");
  c1.add(on.row, sep(), level.el, sep(), row("Lighting mode", fx.el), row("Colour", color.el), speed.el, sep(), follow.row, row("Default", button("Reset to default", { onClick: () => run(call("keyboard", "restore", {}), "Keyboard back to the stock look") })));
  el.append(c1.el);

  /** Grey out what the current look cannot use: wave and neon have no colour; static and per-key looks have no animation speed. */
  function paintEnabled(name) {
    color.setDisabled(!name || NO_COLOR.has(name));
    speed.setDisabled(!name || name === "static" || name === "off");
  }
  async function refresh() {
    const st = await tryCall("keyboard", "status", {}, null);
    if (!st) return;
    if (typeof st.brightness === "number") {
      level.set(st.brightness);
      on.set(st.brightness > 0);
    }
    const look = st.look || {};
    const name = look.mode === "effect" ? look.name : null;
    if (name && KB_EFFECTS.includes(name)) {
      fx.set(name);
      color.set(look.color || "00aec7");
      if (look.speed) speed.set(look.speed);
    }
    paintEnabled(name);
    const op = await tryCall("keyboard", "operating", { action: "status" }, null);
    if (typeof op === "boolean") follow.set(op);
  }
  refresh();
  return { el, refresh };
}

function rearTab() {
  const el = h("div", { class: "stack" });
  let lastBrightness = 100;
  const on = switchRow("Light", { onChange: (v) => run(v ? call("rear", "on", {}) : call("rear", "off", {}), (r) => (typeof r === "string" ? r : `Rear light ${v ? "on" : "off"}`)) });
  const fx = seg(REAR_EFFECTS.map((e) => ({ value: e, label: capitalize(e) })), { label: "Rear light effect", onChange: () => apply() });
  const zones = [0, 1, 2].map((i) => colorField({ value: "00aec7", label: ["Left", "Middle", "Right"][i] + " colour", onInput: () => apply() }));
  const color = colorField({ value: "00aec7", label: "Colour", onInput: () => apply() });
  const speed = slider({ label: "Speed", min: 1, max: 5, step: 1, value: 3, onInput: () => apply() });
  const bright = slider({ label: "Brightness", min: 0, max: 100, step: 10, value: 100, fmt: (v) => `${v} %`, onInput: () => apply() });
  const c = card("");
  c.add(
    on.row,
    sep(),
    row("Effect", fx.el),
    sep(),
    row("Static: left, middle, right", zones.map((z) => z.el)),
    row("Colour (other effects)", color.el),
    speed.el,
    bright.el,
    sep(),
    row("Default", button("Reset to default", { onClick: () => run(call("rear", "set", { ...REAR_DEFAULT }), "Rear light back to the stock look").then(refresh) })),
  );
  el.append(c.el);

  /** Apply the rear light as shown. An effect picked while the bar is off turns it on (brightness 0 means off). */
  function apply() {
    const effect = fx.get() || "static";
    let br = bright.get();
    if (br === 0) {
      br = lastBrightness || 100;
      bright.set(br, true);
    }
    lastBrightness = br;
    on.set(true, true);
    const args = { effect, speed: speed.get() || 3, brightness: br, direction: 1, verify: false };
    if (effect === "static") args.zones = zones.map((z) => z.get());
    else args.color = color.get();
    live("rear", () => call("rear", "set", args));
  }
  async function refresh() {
    const r = await tryCall("rear", "status", {}, null);
    if (!r) return;
    on.set(r.brightness > 0);
    if (r.brightness) lastBrightness = r.brightness;
    bright.set(r.brightness);
    speed.set(r.speed);
    fx.set(REAR_EFFECTS.includes(r.mode) ? r.mode : null);
    color.set(r.color);
    const zs = Array.isArray(r.zones) && r.zones.length === 3 ? r.zones : [r.color, r.color, r.color];
    zones.forEach((z, i) => z.set(zs[i]));
  }
  refresh();
  return { el, refresh };
}

export default function render({ sub, wide }) {
  const page = h("div", { class: "page" });
  const kb = keyboardTab();
  const pk = createPerKey();
  const rear = rearTab();
  const looks = createLooksTab();
  const t = tabs(
    [
      { value: "keyboard", label: "Keyboard", icon: "keyboard" },
      { value: "keys", label: "Per-key", icon: "palette" },
      { value: "rear", label: "Rear bar", icon: "bar" },
      { value: "profiles", label: "Profiles", icon: "manual" },
    ],
    { value: sub === "keys" || sub === "rear" || sub === "profiles" ? sub : "keyboard", label: "Lighting section", onChange: () => (paintWide(), refresh()) },
  );
  t.panel("keyboard").append(kb.el);
  t.panel("keys").append(pk.el);
  t.panel("rear").append(rear.el);
  t.panel("profiles").append(looks.el);
  page.append(t.el);
  const paintWide = () => wide(t.value() === "keys");
  paintWide();
  const parts = { keyboard: kb, keys: pk, rear, profiles: looks };
  function refresh() {
    const p = parts[t.value()];
    if (p && p.refresh) p.refresh();
  }
  return { el: page, refresh };
}
