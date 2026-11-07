// Smart auto profiles: how Smart auto chooses the power mode, and game rules.

import { call, tryCall } from "../api.js";
import { h, card, row, hint, button, select, slider, field, switchRow, run, toast, confirmDialog, errText } from "../ui.js";
import { capitalize, validName, NAME_HELP } from "../format.js";

const MODES = ["eco", "quiet", "balanced", "performance", "turbo"];
const LABELS = {
  ac_idle: "On power, idle", ac_browsing: "On power, everyday use", ac_busy: "On power, gaming or heavy work", battery_idle: "On battery, idle",
  battery_browsing: "On battery, everyday use", battery_busy: "On battery, gaming or heavy work", battery_hot: "On battery, too hot",
};
// key: [min, max, label] (the daemon validates the same ranges)
const NUMS = {
  turbo_after_s: [5, 300, "Heavy load for this long before Turbo (s)"], turbo_max_cpu: [60, 95, "No Turbo above this CPU temperature (°C)"],
  turbo_max_gpu: [55, 85, "No Turbo above this GPU temperature (°C)"], backoff_hold_s: [10, 600, "After overheating, wait before Turbo again (s)"],
  idle_after_s: [10, 600, "Idle for this long before the idle mode (s)"], battery_low_pct: [5, 50, "Below this battery level, force Eco (%)"],
  up_dwell_s: [2, 120, "Wait before moving to a faster mode (s)"], down_dwell_s: [2, 300, "Wait before moving to a slower mode (s)"],
  busy_hold_s: [0, 600, "Stay in the gaming / heavy mode this long after the load drops (s)"], min_change_s: [2, 600, "At least this long between automatic mode changes (s)"],
  turbo_margin_c: [0, 20, "Enter Turbo only this far below its temperature limits (°C)"], turbo_retry_s: [10, 900, "After Turbo reaches its limit, wait before trying it again (s)"],
};
const modeOptions = MODES.map((m) => ({ value: m, label: capitalize(m) }));

export default function render() {
  const page = h("div", { class: "page" });
  let name = "native";
  let locked = true;
  let names = [];
  page.append(hint("How Smart auto chooses the power mode. Built-in profiles can't be edited: copy one, then change the copy."));

  const modeSel = {};
  const c1 = card("Which mode when");
  for (const [k, label] of Object.entries(LABELS)) {
    modeSel[k] = select(modeOptions, { label });
    c1.add(row(label, modeSel[k].el));
  }
  const turbo = switchRow("Allow Turbo on power", {});
  const nums = {};
  const c2 = card("Turbo and timing");
  c2.add(turbo.row);
  for (const [k, [lo, hi, label]] of Object.entries(NUMS)) {
    nums[k] = slider({ label, min: lo, max: hi, step: 1, value: lo });
    c2.add(nums[k].el);
  }

  const sel = select([], { label: "Profile", onChange: (n) => loadProfile(n) });
  const copyBtn = button("Copy to edit", { onClick: copyProfile });
  const delBtn = button("Delete", { kind: "danger", onClick: deleteProfile });
  const nameIn = field({ placeholder: "name", maxlength: 40, label: "Name to save as", width: "180px" });
  const saveBtn = button("Save", { onClick: saveProfile });
  const msg = hint("");
  const c3 = card("Profiles");
  c3.add(
    row("Profile", [sel.el, button("Use", { kind: "primary", onClick: useProfile })]),
    row("Edit", [copyBtn, delBtn]),
    row("Save as", [nameIn.el, saveBtn], { sub: NAME_HELP }),
    row("Default", button("Reset to default", { onClick: resetProfile })),
    msg,
  );

  // ---- game rules ----
  const gName = field({ placeholder: "program, e.g. eldenring.exe or hl2_linux", maxlength: 80, label: "Program name", width: "240px", onEnter: () => addRule() });
  const gMode = select(modeOptions, { label: "Power mode while it runs", value: "performance" });
  gMode.set("performance", true);
  const gList = select([], { label: "Rules" });
  const gMsg = hint("");
  const c4 = card("Game rules");
  c4.add(
    hint("While one of these programs runs, the power mode is at least the one you pick (Native and Smart auto). When it closes, auto takes over again. Turbo still backs off if the laptop gets too hot."),
    row("Program", [gName.el, gMode.el, button("Add", { kind: "primary", onClick: () => addRule() })]),
    row("Rules", [gList.el, button("Remove", { onClick: removeRule })]),
    gMsg,
  );
  page.append(c1.el, c2.el, c3.el, c4.el);

  function fromEditor() {
    const p = Object.fromEntries(Object.entries(modeSel).map(([k, s]) => [k, s.get()]));
    p.ac_turbo = turbo.get();
    for (const [k, s] of Object.entries(nums)) p[k] = s.get();
    return p;
  }
  async function loadList(select_) {
    names = await tryCall("power", "smart", { action: "list" }, []);
    sel.setOptions(names, select_ ?? name);
    await loadProfile(sel.get());
  }
  async function loadProfile(n) {
    try {
      const p = await call("power", "smart", { action: "show", name: n });
      name = n;
      locked = Boolean(p.builtin);
      for (const [k, s] of Object.entries(modeSel)) {
        s.set(p[k], true);
        s.setDisabled(locked);
      }
      turbo.set(p.ac_turbo, true);
      turbo.setDisabled(locked);
      for (const [k, s] of Object.entries(nums)) {
        s.set(p[k], true);
        s.setDisabled(locked);
      }
      copyBtn.hidden = !locked;
      delBtn.disabled = locked;
      saveBtn.disabled = locked;
      msg.textContent = locked ? "Built-in profiles can't be edited. Press Copy to edit to make your own version." : p.description || "";
    } catch (e) {
      toast(errText(e), "error");
    }
  }
  function useProfile() {
    return run(call("power", "smart", locked ? { action: "use", name } : { action: "use", name, profile: fromEditor() }), (r) => `Smart auto profile in use: ${(r && r.name) || name}`);
  }
  async function saveProfile() {
    const n = nameIn.get().trim() || name;
    if (!validName(n)) return toast(NAME_HELP, "error");
    const r = await run(call("power", "smart", { action: "save", name: n, profile: fromEditor() }), `Saved smart profile '${n}'`);
    if (r !== undefined) {
      msg.textContent = `Saved '${n}'. Press Use to run it.`;
      nameIn.set("");
      loadList(n);
    }
  }
  async function resetProfile() {
    await run(call("power", "smart", { action: "use", name: "native" }), "Smart auto reset to the default profile (native)");
    loadList("native");
  }
  async function copyProfile() {
    const base = nameIn.get().trim() || `${name}-copy`;
    if (!validName(base)) return toast(NAME_HELP, "error");
    let n = base, i = 2;
    while (names.includes(n)) n = `${base}-${i++}`;
    const r = await run(call("power", "smart", { action: "copy", name, new: n }), `Copied '${name}' to '${n}'`);
    if (r !== undefined) {
      msg.textContent = `Copied to '${n}'. Change what you like, then Save and Use.`;
      nameIn.set("");
      loadList(typeof r === "string" ? r : n);
    }
  }
  async function deleteProfile() {
    if (!(await confirmDialog({ title: "Delete smart profile", body: `Delete the profile '${name}'?`, confirm: "Delete", danger: true }))) return;
    if ((await run(call("power", "smart", { action: "delete", name }), `Deleted smart profile '${name}'`)) !== undefined) loadList("native");
  }

  function showRules(r) {
    const rules = Object.entries((r && r.rules) || {}).sort(([a], [b]) => a.localeCompare(b));
    gList.setOptions(rules.map(([k, m]) => ({ value: k, label: `${k}  →  ${m}` })));
    gMsg.textContent = rules.length ? (r.running ? `A game rule is active now: ${r.running}.` : "No game rule is active now.") : "No game rules yet.";
  }
  async function rules(args) {
    try {
      showRules(await call("power", "game", args));
    } catch (e) {
      toast(errText(e), "error");
    }
  }
  async function addRule() {
    const n = gName.get().trim();
    if (!n) return;
    await rules({ action: "add", name: n, mode: gMode.get() });
    gName.set("");
  }
  function removeRule() {
    return gList.get() && rules({ action: "remove", name: gList.get() });
  }

  loadList("native").then(async () => {
    const cur = await tryCall("power", "smart", { action: "current" }, null);
    if (cur && cur.name && names.includes(cur.name)) {
      sel.set(cur.name, true);
      loadProfile(cur.name);
    }
  });
  rules({ action: "list" });
  return { el: page };
}
