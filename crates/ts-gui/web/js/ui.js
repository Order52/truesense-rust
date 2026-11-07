// Small DOM helpers and widgets. Everything is built with createElement and textContent,
// never from markup strings, so nothing from the hardware or a config file can become markup.

import { icon } from "./icons.js";

const PROPS = new Set(["value", "checked", "disabled", "hidden", "selected", "open", "readOnly", "multiple"]);

/** Create an element: h("div", {class: "x", onClick: fn}, child, ...). Strings become text. */
export function h(tag, props = {}, ...kids) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(props || {})) {
    if (v === undefined || v === null || v === false) continue;
    if (k === "class") el.className = v;
    else if (k === "text") el.textContent = v;
    else if (k.startsWith("on") && typeof v === "function") el.addEventListener(k.slice(2).toLowerCase(), v);
    else if (PROPS.has(k)) el[k] = v;
    else el.setAttribute(k, v === true ? "" : String(v));
  }
  append(el, kids);
  return el;
}

export function append(el, kids) {
  for (const k of kids.flat(Infinity)) {
    if (k === null || k === undefined || k === false) continue;
    el.append(k instanceof Node ? k : document.createTextNode(String(k)));
  }
  return el;
}

let uid = 0;
export const nextId = (p = "id") => `${p}-${++uid}`;

const now = () => performance.now();

/** A control that ignores programmatic updates for a moment after the user touched it, so a
 * refresh arriving just after a click cannot flip it back before the hardware has caught up. */
function touchGuard(ms = 2500) {
  let at = -1e9;
  return { touch: () => (at = now()), recent: () => now() - at < ms };
}

// ---- cards and rows ------------------------------------------------------------------------------------------------------------------

export function card(title, { icon: ic = "", right = false, id = "" } = {}) {
  const rightEl = right ? h("span", { class: "card-right", "aria-live": "off" }) : null;
  const head = title || ic || right ? h("header", { class: "card-head" }, ic ? icon(ic, 20) : null, title ? h("h2", { text: title }) : null, rightEl) : null;
  const body = h("div", { class: "card-body" });
  const el = h("section", { class: "card", id: id || undefined }, head, body);
  return {
    el,
    body,
    add: (...kids) => (append(body, kids), body),
    setRight: (t) => {
      if (rightEl) rightEl.textContent = t;
    },
  };
}

export const hint = (text, cls = "") => h("p", { class: `hint ${cls}`.trim(), text });

export const sep = () => h("hr", { class: "sep" });

/** A settings row: the name on the left, the controls on the right (under the name when narrow). */
export function row(label, controls, { sub = "", id = "" } = {}) {
  const ctl = h("div", { class: "row-ctl" }, controls);
  const lab = h("div", { class: "row-label", id: id || undefined, text: label });
  const first = ctl.querySelector("input, select, button, [role=radiogroup], [role=switch]");
  if (first && !first.getAttribute("aria-label") && !first.getAttribute("aria-labelledby") && first.tagName !== "BUTTON") {
    lab.id = lab.id || nextId("lbl");
    first.setAttribute("aria-labelledby", lab.id);
  }
  return h("div", { class: "row-wrap" }, h("div", { class: "row" }, lab, ctl), sub ? hint(sub) : null);
}

// ---- buttons -------------------------------------------------------------------------------------------------------------------------

/**
 * @param {string} label
 * @param {{onClick?: () => (void|Promise<unknown>), kind?: "primary"|"danger"|"quiet", icon?: string, title?: string, disabled?: boolean}} [o]
 */
export function button(label, { onClick, kind = "", icon: ic = "", title = "", disabled = false } = {}) {
  const b = h("button", { type: "button", class: `btn ${kind}`.trim(), title: title || undefined, disabled }, ic ? icon(ic, 18) : null, label ? h("span", { text: label }) : null);
  if (!label && title) b.setAttribute("aria-label", title);
  b.addEventListener("click", async () => {
    if (!onClick || b.dataset.busy) return;
    b.dataset.busy = "1";
    b.setAttribute("aria-busy", "true");
    try {
      await onClick();
    } finally {
      delete b.dataset.busy;
      b.removeAttribute("aria-busy");
    }
  });
  return b;
}

// ---- switch --------------------------------------------------------------------------------------------------------------------------

export function toggle({ label = "", checked = false, onChange } = {}) {
  const input = h("input", { type: "checkbox", role: "switch", checked, "aria-label": label || undefined });
  const el = h("label", { class: "switch" }, input, h("span", { class: "track", "aria-hidden": "true" }, h("span", { class: "ring" }), h("span", { class: "bar" }), h("span", { class: "thumb" })));
  const g = touchGuard();
  input.addEventListener("change", () => {
    g.touch();
    onChange && onChange(input.checked);
  });
  return {
    el,
    input,
    get: () => input.checked,
    set(v, force = false) {
      if (force || !g.recent()) input.checked = Boolean(v);
    },
    setDisabled: (d) => (input.disabled = d),
  };
}

/** A row with a switch: name (and an optional explanation) and the switch. */
export function switchRow(label, opts = {}, sub = "") {
  const t = toggle({ ...opts, label });
  return { ...t, row: row(label, t.el, { sub }) };
}

// ---- segmented choice (radio group) -----------------------------------------------------------------------------------------------------

/**
 * Choice pills or big icon tiles. Arrow keys move and choose, like a radio group.
 * @param {{value:string,label:string,icon?:string,color?:string,title?:string}[]} options
 */
export function seg(options, { value = null, onChange, tiles = false, label = "", compact = false, tileMin = 0 } = {}) {
  const el = h("div", { class: `seg ${tiles ? "tiles" : "pills"} ${compact ? "compact" : ""}`.trim(), role: "radiogroup", "aria-label": label || undefined });
  if (tileMin) el.style.setProperty("--tile-min", `${tileMin}px`);
  const g = touchGuard();
  let cur = value;
  const btns = options.map((o) => {
    const b = h(
      "button",
      { type: "button", class: "seg-btn", role: "radio", "aria-checked": "false", tabindex: "-1", title: o.title || undefined, "data-value": o.value },
      tiles && o.icon ? icon(o.icon, 22) : null,
      h("span", { class: "seg-label", text: o.label }),
    );
    if (o.color) {
      b.style.setProperty("--seg-color", o.color);
      b.dataset.colored = "1";
    }
    el.append(b);
    return b;
  });
  const paint = () => {
    const idx = options.findIndex((o) => o.value === cur);
    btns.forEach((b, i) => {
      const on = i === idx;
      b.setAttribute("aria-checked", on ? "true" : "false");
      b.tabIndex = on || (idx < 0 && i === 0) ? 0 : -1;
    });
  };
  const choose = (i, user) => {
    const v = options[i].value;
    if (user) g.touch();
    cur = v;
    paint();
    if (user && onChange) onChange(v);
  };
  btns.forEach((b, i) => {
    b.addEventListener("click", () => {
      if (!b.disabled) choose(i, true);
    });
    b.addEventListener("keydown", (e) => {
      const step = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[e.key];
      if (!step) return;
      e.preventDefault();
      let j = i;
      do j = (j + step + btns.length) % btns.length;
      while (btns[j].disabled && j !== i);
      btns[j].focus();
      choose(j, true);
    });
  });
  paint();
  return {
    el,
    get: () => cur,
    set(v, force = false) {
      if (!force && g.recent()) return;
      cur = v;
      paint();
    },
    setDisabled(d) {
      btns.forEach((b) => (b.disabled = Boolean(d)));
      el.toggleAttribute("aria-disabled", Boolean(d));
    },
    setOptionDisabled(val, d) {
      const i = options.findIndex((o) => o.value === val);
      if (i >= 0) btns[i].disabled = Boolean(d);
    },
  };
}

// ---- slider --------------------------------------------------------------------------------------------------------------------------

/**
 * A range slider with its name and value above. onInput fires while dragging (use it with
 * live() so the newest value wins), onChange when the value is committed.
 */
export function slider({ label, min, max, step = 1, value = min, fmt = (v) => String(v), onInput, onChange, disabled = false, compact = false }) {
  const id = nextId("rng");
  const out = h("output", { class: "slider-value", for: id });
  const input = h("input", { type: "range", id, min, max, step, value, disabled, class: "range" });
  const el = h("div", { class: `slider ${compact ? "compact" : ""}`.trim() }, h("div", { class: "slider-head" }, h("label", { for: id, text: label }), out), input);
  let dragging = false;
  const g = touchGuard(3000);
  const show = () => {
    out.textContent = fmt(Number(input.value));
    // the fill of the track up to the thumb
    input.style.setProperty("--fill-at", `${((input.value - min) / (max - min || 1)) * 100}%`);
  };
  input.addEventListener("pointerdown", () => (dragging = true));
  const up = () => (dragging = false);
  input.addEventListener("pointerup", up);
  input.addEventListener("pointercancel", up);
  input.addEventListener("blur", up);
  input.addEventListener("input", () => {
    g.touch();
    show();
    onInput && onInput(Number(input.value));
  });
  input.addEventListener("change", () => {
    g.touch();
    show();
    onChange && onChange(Number(input.value));
  });
  show();
  return {
    el,
    input,
    get: () => Number(input.value),
    /** A refresh never moves the slider while it is dragged or just used. */
    set(v, force = false) {
      if (!force && (dragging || g.recent())) return;
      input.value = String(v);
      show();
    },
    setRange(lo, hi) {
      input.min = String(lo);
      input.max = String(hi);
      show();
    },
    setDisabled: (d) => (input.disabled = Boolean(d)),
    dragging: () => dragging,
  };
}

// ---- inputs --------------------------------------------------------------------------------------------------------------------------

export function field({ value = "", placeholder = "", maxlength, onInput, onEnter, label = "", width = "", type = "text", inputmode, autocomplete = "off", spellcheck = false } = {}) {
  const input = h("input", { type, class: "input", value, placeholder, maxlength, "aria-label": label || undefined, inputmode, autocomplete, spellcheck: String(spellcheck), autocapitalize: "off" });
  if (width) input.style.width = width;
  input.addEventListener("input", () => onInput && onInput(input.value));
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && onEnter) onEnter(input.value);
  });
  return { el: input, input, get: () => input.value, set: (v) => (input.value = v ?? ""), setDisabled: (d) => (input.disabled = Boolean(d)) };
}

/** A number box with its unit, for the power limits. */
export function numberField({ label, min, max, step = 1, value, unit = "", onInput }) {
  const input = h("input", { type: "number", class: "input num", min, max, step, value, "aria-label": label, inputmode: "decimal" });
  input.addEventListener("input", () => onInput && onInput(input.valueAsNumber));
  const el = h("span", { class: "num-wrap" }, input, unit ? h("span", { class: "unit", text: unit }) : null);
  return { el, input, get: () => input.valueAsNumber, set: (v) => (input.value = String(v)) };
}

/** A drop-down. Options are strings or {value, label}. */
export function select(options, { value = "", onChange, label = "" } = {}) {
  const el = h("select", { class: "select", "aria-label": label || undefined });
  const g = touchGuard();
  let opts = [];
  const fill = (list, keep) => {
    opts = list.map((o) => (typeof o === "string" ? { value: o, label: o } : o));
    el.replaceChildren(...opts.map((o) => h("option", { value: o.value, text: o.label })));
    if (keep !== undefined && opts.some((o) => o.value === keep)) el.value = keep;
  };
  fill(options, value);
  el.addEventListener("change", () => {
    g.touch();
    onChange && onChange(el.value);
  });
  return {
    el,
    get: () => el.value,
    set(v, force = false) {
      if ((force || !g.recent()) && document.activeElement !== el && opts.some((o) => o.value === v)) el.value = v;
    },
    setOptions(list, keep = el.value) {
      fill(list, keep);
    },
    setDisabled: (d) => (el.disabled = Boolean(d)),
    has: (v) => opts.some((o) => o.value === v),
  };
}

/** A colour swatch that opens the system picker; onInput fires live while the colour is dragged. */
export function colorField({ value = "00aec7", label = "Colour", onInput, onChange } = {}) {
  const input = h("input", { type: "color", class: "color", value: `#${value}`, "aria-label": label, title: label });
  const g = touchGuard();
  input.addEventListener("input", () => {
    g.touch();
    onInput && onInput(input.value.slice(1));
  });
  input.addEventListener("change", () => {
    g.touch();
    onChange && onChange(input.value.slice(1));
  });
  return {
    el: input,
    input,
    get: () => input.value.slice(1),
    set(hex, force = false) {
      if (force || !g.recent()) input.value = `#${hex}`;
    },
    setDisabled: (d) => (input.disabled = Boolean(d)),
  };
}

// ---- messages ------------------------------------------------------------------------------------------------------------------------

let toastHost = null;

/** A short message at the bottom; errors stay longer. Screen readers hear it (aria-live). */
export function toast(message, kind = "info", ms = 0) {
  if (!toastHost) {
    toastHost = h("div", { class: "toasts", role: "status", "aria-live": "polite" });
    document.body.append(toastHost);
  }
  const t = h("div", { class: `toast ${kind}` }, icon(kind === "error" ? "warn" : kind === "ok" ? "check" : "info", 18), h("span", { text: message }));
  toastHost.append(t);
  while (toastHost.children.length > 4) toastHost.firstChild.remove();
  const life = ms || (kind === "error" ? 9000 : 4500);
  const done = () => t.remove();
  t.addEventListener("click", done);
  setTimeout(done, life);
  return t;
}

export const errText = (e) => (e && e.message) || String(e);

/** Run an action, show `ok` (a string or a function of the result) as a toast, or the error. Returns the result or undefined. */
export async function run(promise, ok) {
  try {
    const r = await promise;
    const msg = typeof ok === "function" ? ok(r) : ok;
    if (msg) toast(msg, "ok");
    return r === undefined ? true : r;
  } catch (e) {
    toast(errText(e), "error");
    return undefined;
  }
}

/**
 * A modal question. Returns true for the confirm button. With `typeWord` the user must type
 * that word first (the danger zone). Focus starts on Cancel; Esc cancels.
 */
export function confirmDialog({ title, body, confirm = "Apply", cancel = "Cancel", danger = false, typeWord = "" }) {
  return new Promise((resolve) => {
    const dlg = h("dialog", { class: "dialog", "aria-labelledby": "dlg-title" });
    const paras = (Array.isArray(body) ? body : [body]).filter(Boolean).map((b) => (b instanceof Node ? b : h("p", { text: b })));
    const word = typeWord ? h("input", { type: "text", class: "input", "aria-label": `Type ${typeWord} to confirm`, autocomplete: "off", spellcheck: "false" }) : null;
    const ok = h("button", { type: "button", class: `btn ${danger ? "danger" : "primary"}`, disabled: Boolean(typeWord) }, h("span", { text: confirm }));
    const no = h("button", { type: "button", class: "btn", autofocus: true }, h("span", { text: cancel }));
    if (word) word.addEventListener("input", () => (ok.disabled = word.value.trim() !== typeWord));
    let result = false;
    ok.addEventListener("click", () => {
      result = true;
      dlg.close();
    });
    no.addEventListener("click", () => dlg.close());
    dlg.addEventListener("close", () => {
      dlg.remove();
      resolve(result);
    });
    dlg.append(h("h2", { id: "dlg-title", text: title }), ...paras);
    if (word) dlg.append(h("p", { class: "hint", text: `Type ${typeWord} to go ahead:` }), word);
    dlg.append(h("div", { class: "dialog-actions" }, no, ok));
    document.body.append(dlg);
    dlg.showModal();
    no.focus();
  });
}

/** A coloured one-line risk message under the sliders. */
export function riskLine() {
  const el = h("p", { class: "risk safe", role: "status" });
  return {
    el,
    set({ level, why }) {
      el.className = `risk ${level}`;
      el.textContent = why;
    },
  };
}

/** A tab strip (big icon tiles) with panels; only the chosen panel is shown. */
export function tabs(items, { value, onChange, label = "" } = {}) {
  const panels = new Map(items.map((i) => [i.value, h("div", { class: "tab-panel", hidden: true, role: "tabpanel" })]));
  const s = seg(items, {
    value: value ?? items[0].value,
    tiles: true,
    label,
    onChange: (v) => {
      show(v);
      onChange && onChange(v);
    },
  });
  const show = (v) => {
    for (const [k, p] of panels) p.hidden = k !== v;
  };
  show(value ?? items[0].value);
  return {
    el: h("div", { class: "tabs" }, s.el, ...panels.values()),
    panel: (v) => panels.get(v),
    select(v) {
      s.set(v, true);
      show(v);
    },
    value: () => s.get(),
  };
}

/** An empty-state or error box inside a page. */
export function notice(text, kind = "info") {
  return h("div", { class: `notice ${kind}`, role: kind === "error" ? "alert" : "status" }, icon(kind === "error" || kind === "warn" ? "warn" : "info", 18), h("span", { text }));
}

export const SVGNS = "http://www.w3.org/2000/svg";
export function svg(tag, attrs = {}, ...kids) {
  const el = document.createElementNS(SVGNS, tag);
  for (const [k, v] of Object.entries(attrs)) if (v !== undefined && v !== null && v !== false) el.setAttribute(k, String(v));
  for (const k of kids.flat()) if (k) el.append(k);
  return el;
}
