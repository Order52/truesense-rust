// PREDATOR key: what each press does (profiles of bindings, actions, timing).

import { call, tryCall } from "../api.js";
import { h, card, row, hint, button, select, slider, field, run, toast, confirmDialog, errText } from "../ui.js";
import { ACTION_TYPES, FAN_CHOICES, AUTO_CHOICES, FAVOURITE_CHOICES, POWER_MODES, typeOf, parseSpec, buildSpec, defaultsFor } from "../actions.js";
import { capitalize, validName, NAME_HELP } from "../format.js";

const GESTURES = [["tap", "Tap"], ["double", "Double tap"], ["triple", "Triple tap"], ["long", "Long press"], ["fn_esc", "Fn + Esc"]];

/** One gesture: pick WHAT it does, then its detail. */
function actionEditor(gestureLabel, lists) {
  let st = parseSpec("none");
  const type = select(ACTION_TYPES.map((t) => ({ value: t.id, label: t.label })), { label: `${gestureLabel}: action`, onChange: (id) => ((st = defaultsFor(id, lists())), paint()) });
  const choice = select([], { label: `${gestureLabel}: choice`, onChange: (v) => ((st.choice = v), paint()) });
  const percent = slider({ label: "Fan speed", min: 20, max: 100, step: 5, value: 40, fmt: (v) => `${v} %`, compact: true, onInput: (v) => (st.percent = v) });
  const text = field({ label: `${gestureLabel}: text or program`, onInput: (v) => (st.text = v) });
  const test = button("Test", { onClick: () => run(call("keys", "test", { action: buildSpec(st) }), (r) => (typeof r === "string" && r ? r : "ok")) });
  const detail = h("div", { class: "row-ctl" }, choice.el, text.el);
  const el = h("div", { class: "stack" }, h("div", { class: "row-ctl" }, type.el, test), detail, percent.el);
  el.firstChild.style.justifyContent = "flex-start";
  type.el.style.flex = "1 1 220px";
  text.el.style.flex = "1 1 240px";
  detail.style.justifyContent = "flex-start";
  el.style.gap = "8px";

  function paint() {
    const t = typeOf(st.id);
    type.set(st.id, true);
    const k = t.kind;
    const opts = { mode: POWER_MODES, auto: AUTO_CHOICES, fan: FAN_CHOICES, profile: lists().profiles, look: lists().looks, favourite: FAVOURITE_CHOICES }[k];
    choice.el.hidden = !opts;
    if (opts) {
      choice.setOptions(opts.map((o) => ({ value: o, label: k === "mode" || k === "auto" ? capitalize(o) : o })), st.choice);
      if (!st.choice || !opts.includes(st.choice)) st.choice = opts[0] ?? "";
      choice.set(st.choice, true);
    }
    percent.el.hidden = !(k === "fan" && st.choice === "manual %");
    percent.set(st.percent, true);
    text.el.hidden = !["cycle", "command", "text"].includes(k);
    text.el.placeholder = { cycle: "eco,balanced,turbo", command: "e.g. notify-send hello, or /home/me/script.sh", text: "message" }[k] || "";
    text.set(st.text);
    detail.hidden = choice.el.hidden && text.el.hidden;
  }
  paint();
  return {
    el,
    get: () => buildSpec(st),
    set(spec) {
      st = parseSpec(spec);
      paint();
    },
    repaint: paint,
    setDisabled(d) {
      [type, choice, text].forEach((c) => c.setDisabled(d));
      percent.setDisabled(d);
    },
  };
}

export default function render() {
  const page = h("div", { class: "page" });
  let name = "default";
  let locked = true;
  let names = [];
  let lists = { profiles: [], looks: [] };
  const getLists = () => lists;
  page.append(hint("What each press of the PREDATOR key does. Pick an action, or run your own program or script. Built-in profiles can't be edited: copy one, then change the copy."));

  const sel = select([], { label: "Bindings profile", onChange: (n) => loadProfile(n) });
  const copyBtn = button("Copy to edit", { onClick: copyProfile });
  const delBtn = button("Delete", { kind: "danger", onClick: deleteProfile });
  const nameIn = field({ placeholder: "name", maxlength: 40, label: "Name to save as", width: "180px" });
  const saveBtn = button("Save", { onClick: saveProfile });
  const msg = hint("");
  const c0 = card("Profile", { icon: "key" });
  c0.add(
    row("Bindings", [sel.el, button("Use", { kind: "primary", onClick: useProfile })]),
    row("Edit", [copyBtn, delBtn]),
    row("Save as", [nameIn.el, saveBtn], { sub: NAME_HELP }),
    row("Default", button("Reset to default", { onClick: resetProfile })),
    msg,
  );

  const editors = {};
  const cards = GESTURES.map(([g, label]) => {
    const c = card(label);
    editors[g] = actionEditor(label, getLists);
    c.add(editors[g].el);
    return c.el;
  });
  const longMs = slider({ label: "Long press after", min: 300, max: 2000, step: 50, value: 800, fmt: (v) => `${v} ms` });
  const multiMs = slider({ label: "Taps count together within", min: 200, max: 1000, step: 50, value: 450, fmt: (v) => `${v} ms` });
  const timing = card("Timing");
  timing.add(longMs.el, multiMs.el);
  page.append(c0.el, h("div", { class: "grid" }, ...cards, timing.el));

  function values() {
    const v = Object.fromEntries(GESTURES.map(([g]) => [g, editors[g].get()]));
    v.long_ms = longMs.get();
    v.multi_ms = multiMs.get();
    return v;
  }
  function paintProfile(p, isLocked, note) {
    locked = isLocked;
    for (const [g] of GESTURES) {
      editors[g].set(p[g]);
      editors[g].setDisabled(locked);
    }
    longMs.set(p.long_ms, true);
    multiMs.set(p.multi_ms, true);
    longMs.setDisabled(locked);
    multiMs.setDisabled(locked);
    copyBtn.hidden = !locked;
    saveBtn.disabled = locked;
    delBtn.disabled = locked;
    msg.textContent = note;
  }
  async function loadProfile(n, current = false) {
    try {
      const p = await call("keys", "profile", { action: current ? "current" : "show", name: n });
      name = n;
      const isLocked = Boolean(p.builtin) && !current;
      paintProfile(p, isLocked, isLocked ? "Built-in profiles can't be edited. Press Copy to edit to make your own version." : current ? "These are your current bindings, not saved as a profile yet." : p.description || "Change the actions, then Save and Use.");
    } catch (e) {
      toast(errText(e), "error");
    }
  }
  async function loadAll(select_) {
    const [n, pr, lk, cur] = await Promise.all([
      tryCall("keys", "profile", { action: "list" }, []),
      tryCall("profile", "list", {}, []),
      tryCall("look", "list", {}, []),
      tryCall("keys", "profile", { action: "current" }, null),
    ]);
    names = n;
    lists = { profiles: pr.map((p) => p.name), looks: lk };
    Object.values(editors).forEach((e) => e.repaint());
    const want = select_ ?? (cur && names.includes(cur.profile) ? cur.profile : null);
    sel.setOptions(names, want ?? names[0]);
    if (want) await loadProfile(want);
    else {
      await loadProfile(names.includes("default") ? "default" : names[0]);
      if (cur) paintProfile(cur, false, "These are your current bindings, not saved as a profile yet.");
    }
  }

  async function useProfile() {
    const n = sel.get();
    if (!locked) {
      const saved = await run(call("keys", "profile", { action: "save", name: n, ...values() }));
      if (saved === undefined) return;
    }
    await run(call("keys", "profile", { action: "use", name: n }), `PREDATOR key: using '${n}' (the key service reloads by itself)`);
  }
  async function saveProfile() {
    const n = nameIn.get().trim() || name;
    if (!validName(n)) return toast(NAME_HELP, "error");
    const r = await run(call("keys", "profile", { action: "save", name: n, ...values() }), `Saved key profile '${n}'`);
    if (r !== undefined) {
      nameIn.set("");
      await loadAll(n);
      msg.textContent = `Saved '${n}'. Press Use to switch the key to it.`;
    }
  }
  async function resetProfile() {
    await run(call("keys", "profile", { action: "use", name: "native" }), "PREDATOR key reset to the default bindings");
    loadAll("native");
  }
  async function copyProfile() {
    const src = sel.get();
    const n = nameIn.get().trim();
    if (n && !validName(n)) return toast(NAME_HELP, "error");
    const r = await run(call("keys", "profile", n ? { action: "copy", name: src, new: n } : { action: "copy", name: src }), (x) => `Copied '${src}' to '${x}'`);
    if (r !== undefined) {
      nameIn.set("");
      await loadAll(typeof r === "string" ? r : undefined);
      msg.textContent = "Copied. Change what you like, then Save and Use.";
    }
  }
  async function deleteProfile() {
    const n = sel.get();
    if (!(await confirmDialog({ title: "Delete key profile", body: `Delete the profile '${n}'?`, confirm: "Delete", danger: true }))) return;
    if ((await run(call("keys", "profile", { action: "delete", name: n }), `Deleted key profile '${n}'`)) !== undefined) loadAll();
  }
  loadAll();
  return { el: page, refresh: () => loadAll(sel.get()) };
}
