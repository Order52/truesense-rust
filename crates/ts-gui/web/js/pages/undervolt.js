// Undervolt: CPU and GPU profiles, CPU power limits, with safety zones and confirmations.

import { call, tryCall } from "../api.js";
import { h, card, row, hint, button, select, slider, field, numberField, switchRow, riskLine, tabs, run, toast, confirmDialog, errText } from "../ui.js";
import { cpuRisk, gpuRisk, plRisk, CPU_MAX_MV, GPU_OFFSET_RANGE, GPU_CAP_RANGE } from "../risk.js";
import { validName, NAME_HELP, fmtSigned } from "../format.js";

/** Ask before changing voltages or limits. Past the safety line the warning is stronger;
 * beyond the danger line the word APPLY must be typed. Returns {go, force}. */
async function confirmRisk(title, what, risk, footer = "A reboot always returns voltages to stock. Save your work first.") {
  const body = [what, risk.why, footer];
  if (risk.level === "danger") {
    const go = await confirmDialog({ title, body: [...body, "This is beyond the danger line."], confirm: "Apply", danger: true, typeWord: "APPLY" });
    return { go, force: true };
  }
  return { go: await confirmDialog({ title, body, confirm: "Apply", danger: risk.level === "caution" }), force: false };
}

/** The profile card of one processor: list, sliders, risk line, save, apply, boot switch. */
function profilePanel(cfg) {
  const el = h("div", { class: "stack" });
  let names = [];
  let name = "native";
  let locked = true;
  const nowCard = card("Right now", { right: true });
  const state = hint(cfg.reading);
  nowCard.add(state);

  const sel = select([], { label: "Profile", onChange: (n) => load(n) });
  const copyBtn = button("Copy to edit", { onClick: copy });
  const delBtn = button("Delete", { kind: "danger", onClick: del });
  const sliders = cfg.sliders.map((s) => slider({ ...s, onInput: () => paintRisk() }));
  const risk = riskLine();
  const nameIn = field({ placeholder: "name", maxlength: 40, label: "Name to save as", width: "180px" });
  const saveBtn = button("Save", { onClick: save });
  const msg = hint("");
  const prof = card("Profile");
  prof.add(
    row("Profile", [sel.el, button("Reset", { onClick: reset }), button("Apply", { kind: "primary", onClick: apply })]),
    row("Edit", [copyBtn, delBtn]),
    ...sliders.map((s) => s.el),
    risk.el,
    row("Save as", [nameIn.el, saveBtn], { sub: NAME_HELP }),
    msg,
    row("Default", button("Reset to default (native)", { onClick: reset })),
  );
  const boot = switchRow("Apply the selected profile at boot", { onChange: bootSet }, "Off by default. Guarded: if the laptop does not stay up for 10 minutes after applying it, the next boot skips it and turns this off.");
  const bootCard = card("At boot");
  bootCard.add(boot.row);
  el.append(nowCard.el, prof.el, bootCard.el);
  if (cfg.intro) nowCard.body.prepend(hint(cfg.intro));

  const values = () => cfg.toValues(sliders.map((s) => s.get()));
  const paintRisk = () => risk.set(cfg.risk(values()));
  const fail = (e) => {
    msg.textContent = errText(e);
    toast(errText(e), "error");
  };

  async function loadList(select_) {
    names = await cfg.tryRpc({ action: "list" }, []);
    sel.setOptions(names, select_ ?? name);
    await load(sel.get());
  }
  async function load(n) {
    try {
      const p = await cfg.rpc({ action: "show", name: n });
      name = n;
      locked = Boolean(p.builtin);
      cfg.fromProfile(p).forEach((v, i) => {
        sliders[i].set(v, true);
        sliders[i].setDisabled(locked);
      });
      copyBtn.hidden = !locked;
      saveBtn.disabled = locked;
      delBtn.disabled = locked;
      msg.textContent = p.description || (locked ? "Built-in profiles can't be edited. Press Copy to edit." : "");
      paintRisk();
    } catch (e) {
      toast(errText(e), "error");
    }
  }
  async function apply() {
    const v = values();
    if (cfg.isStock(v)) return reset();
    const { go, force } = await confirmRisk(cfg.applyTitle, cfg.applyQuestion(name, v), cfg.risk(v));
    if (!go) return;
    const r = await run(cfg.rpc({ action: "apply", ...cfg.applyArgs(v), name, confirm: true, force }), (x) => cfg.applied(x));
    if (r !== undefined) refresh();
  }
  async function reset() {
    await run(cfg.rpc({ action: "boot", name: "off" }));
    const r = await run(cfg.rpc({ action: "reset" }), cfg.resetText);
    if (r !== undefined) {
      loadList("native");
      refresh();
    }
  }
  async function bootSet(on) {
    if (!on) {
      const r = await run(cfg.rpc({ action: "boot", name: "off" }), `${cfg.label} at boot: off`);
      if (r === undefined) boot.set(true, true);
      return;
    }
    const v = values();
    const { go, force } = await confirmRisk(`${cfg.label} at boot`, `Apply '${name}' (${cfg.describe(v)}) every time the laptop starts?\n\nIf it does not stay up for 10 minutes after applying it, the next boot skips it and this switches off.`, cfg.risk(v));
    if (!go) return boot.set(false, true);
    const r = await run(cfg.rpc({ action: "boot", ...cfg.applyArgs(v), name, confirm: true, force }), `${cfg.label} at boot: ${name}`);
    if (r === undefined) boot.set(false, true);
  }
  async function save() {
    const n = nameIn.get().trim() || name;
    if (!validName(n)) return fail(new Error(NAME_HELP));
    const r = await run(cfg.rpc({ action: "save", name: n, ...cfg.saveArgs(values()) }), `Saved ${cfg.label} profile '${n}'`);
    if (r !== undefined) {
      nameIn.set("");
      loadList(n);
    }
  }
  async function copy() {
    const base = nameIn.get().trim() || `${name}-copy`;
    if (!validName(base)) return fail(new Error(NAME_HELP));
    let n = base, i = 2;
    while (names.includes(n)) n = `${base}-${i++}`;
    const r = await run(cfg.rpc({ action: "copy", name, new: n }), `Copied '${name}' to '${n}'`);
    if (r !== undefined) {
      nameIn.set("");
      await loadList(typeof r === "string" ? r : n);
      msg.textContent = `Copied to '${n}'. Change it, then Save and Apply.`;
    }
  }
  async function del() {
    if (!(await confirmDialog({ title: `Delete ${cfg.label} profile`, body: `Delete the profile '${name}'?`, confirm: "Delete", danger: true }))) return;
    if ((await run(cfg.rpc({ action: "delete", name }), `Deleted ${cfg.label} profile '${name}'`)) !== undefined) loadList("native");
  }
  async function refresh() {
    const r = await cfg.tryRpc({ action: "get" }, { error: "could not ask the service" });
    const t = cfg.show(r);
    nowCard.setRight(t.right);
    state.textContent = t.text;
    boot.set(Boolean(r && r.boot), true);
  }
  loadList("native");
  refresh();
  return { el, refresh, addRight: (node) => el.append(node) };
}

const cpuPanel = () =>
  profilePanel({
    rpc: (args) => call("cpu", "undervolt", args), tryRpc: (args, fb) => tryCall("cpu", "undervolt", args, fb), label: "CPU undervolt", reading: "Reading the current voltage…",
    applyTitle: "Apply CPU undervolt", resetText: "Undervolt reset to stock (no undervolt at boot)",
    sliders: [
      { label: "Core offset", min: 0, max: CPU_MAX_MV, step: 5, value: 0, fmt: (v) => (v ? `−${v} mV` : "0 mV") },
      { label: "Cache offset", min: 0, max: CPU_MAX_MV, step: 5, value: 0, fmt: (v) => (v ? `−${v} mV` : "0 mV") },
    ],
    toValues: ([c, k]) => ({ core: -c, cache: -k }),
    fromProfile: (p) => [-p.core, -p.cache],
    risk: (v) => cpuRisk(v.core, v.cache),
    isStock: (v) => v.core === 0 && v.cache === 0,
    applyQuestion: (n, v) => `Apply '${n}': core ${v.core} mV, cache ${v.cache} mV?`,
    describe: (v) => `core ${v.core} mV, cache ${v.cache} mV`,
    applyArgs: (v) => ({ core: v.core, cache: v.cache }),
    saveArgs: (v) => ({ core: v.core, cache: v.cache }),
    applied: (r) => `Undervolt applied: core ${r.core_mv} mV, cache ${r.cache_mv} mV`,
    show: (r) =>
      r && "core_mv" in r
        ? { right: `core ${r.core_mv} mV · cache ${r.cache_mv} mV`, text: (r.applied ? `Applied: ${r.applied.name}. ` : "Stock voltage. ") + (r.boot ? `At boot: ${r.boot.name}. ` : "") + (r.pending ? "A boot undervolt is waiting to prove stable." : "") }
        : { right: "not readable", text: String((r && r.error) || "").slice(0, 160) },
  });

const gpuPanel = () =>
  profilePanel({
    rpc: (args) => call("gpu", "undervolt", args), tryRpc: (args, fb) => tryCall("gpu", "undervolt", args, fb), label: "GPU undervolt", reading: "Reading the GPU clocks…",
    intro: "GPU undervolt shifts the GPU's voltage/frequency curve up and caps the top clock, so it reaches its speed at a lower voltage. Neither setting survives a reboot.",
    applyTitle: "Apply GPU undervolt", resetText: "GPU undervolt reset to stock (no GPU undervolt at boot)",
    sliders: [
      { label: "Curve offset", min: GPU_OFFSET_RANGE[0], max: GPU_OFFSET_RANGE[1], step: 10, value: 0, fmt: (v) => fmtSigned(v, "MHz") },
      { label: "Clock cap (below 600 = none)", min: 0, max: GPU_CAP_RANGE[1], step: 50, value: 0, fmt: (v) => (v < GPU_CAP_RANGE[0] ? "none" : `${v} MHz`) },
    ],
    toValues: ([o, c]) => ({ offset: o, cap: c >= GPU_CAP_RANGE[0] ? c : 0 }),
    fromProfile: (p) => [p.offset, p.cap],
    risk: (v) => gpuRisk(v.offset, v.cap),
    isStock: (v) => v.offset === 0 && v.cap === 0,
    applyQuestion: (n, v) => `Apply '${n}': curve offset ${fmtSigned(v.offset, "MHz")}, clock cap ${v.cap || "none"} MHz?`,
    describe: (v) => `offset ${fmtSigned(v.offset, "MHz")}, cap ${v.cap || "none"} MHz`,
    applyArgs: (v) => ({ offset: v.offset, cap: v.cap }),
    saveArgs: (v) => ({ offset: v.offset, cap: v.cap }),
    applied: (r) => `GPU undervolt applied: offset ${fmtSigned(r.offset_mhz, "MHz")}, cap ${r.cap_mhz || "none"} MHz`,
    show: (r) =>
      r && "offset_mhz" in r
        ? {
            right: `offset ${fmtSigned(r.offset_mhz, "MHz")}`,
            text: (r.applied ? `Applied: ${r.applied.name} (cap ${r.applied.cap || "none"} MHz). ` : "Not applied by TrueSense. ") + (r.boot ? `At boot: ${r.boot.name}. ` : "") + `Driver range ${r.range_mhz[0]} to ${r.range_mhz[1]} MHz.`,
          }
        : { right: "not readable", text: String((r && r.error) || "").slice(0, 160) },
  });

function limitsCard() {
  const now = card("Power limits", { right: true });
  const pl1 = numberField({ label: "PL1 sustained power in watts", min: 25, max: 120, step: 1, value: 65, unit: "W", onInput: paint });
  const pl2 = numberField({ label: "PL2 burst power in watts", min: 25, max: 120, step: 1, value: 80, unit: "W", onInput: paint });
  const risk = riskLine();
  const state = hint("");
  let initial = false;
  now.add(
    hint("How much power the CPU may draw: PL1 is sustained, PL2 is short bursts. Stock is 80 W. TrueSense keeps your values until you go back to automatic."),
    row("PL1 / PL2", [pl1.el, pl2.el]),
    risk.el,
    row("Limits", [button("Use automatic", { onClick: auto }), button("Apply", { kind: "primary", onClick: apply })]),
    state,
  );
  function paint() {
    risk.set(plRisk(pl1.get(), pl2.get()));
  }
  async function apply() {
    const a = pl1.get(), b = pl2.get();
    if (!(a > 0) || !(b > 0)) return toast("Enter both limits in watts", "error");
    const r = plRisk(a, b);
    if (r.invalid) return toast("PL2 (burst) must not be below PL1 (sustained)", "error");
    let force = false;
    if (r.level !== "safe") {
      const q = await confirmRisk("Apply power limits", `Set PL1 ${a} W and PL2 ${b} W?`, r, "Save your work first.");
      if (!q.go) return;
      force = q.force;
    }
    if ((await run(call("cpu", "limits", { pl1: a, pl2: b, force }), (x) => `Power limits: PL1 ${x.pl1_w} W, PL2 ${x.pl2_w} W`)) !== undefined) refresh();
  }
  async function auto() {
    if ((await run(call("cpu", "limits", { auto: true }), "Power limits are automatic again")) !== undefined) refresh();
  }
  async function refresh() {
    const c = await tryCall("cpu", "info", {}, null);
    if (!c || !c.pl1_w) return now.setRight("not readable");
    now.setRight(`PL1 ${c.pl1_w} W · PL2 ${c.pl2_w} W`);
    state.textContent = c.user ? `Yours: ${c.user.pl1} / ${c.user.pl2} W.` : c.auto ? `Automatic: ${c.auto.normal_w} W, ${c.auto.gaming_w} W when gaming.` : "";
    if (!initial) {
      initial = true;
      pl1.set(c.user ? c.user.pl1 : c.pl1_w);
      pl2.set(c.user ? c.user.pl2 : c.pl2_w);
    }
    paint();
  }
  paint();
  refresh();
  return { el: now.el, refresh };
}

export default function render() {
  const page = h("div", { class: "page" });
  page.append(hint("Lowers CPU or GPU voltage to run cooler. Too much can freeze, reboot or crash games. Nothing is applied unless you confirm, and a reboot always returns to stock. Test a profile under load before relying on it.", "warn"));
  const cpu = cpuPanel();
  const limits = limitsCard();
  const gpu = gpuPanel();
  cpu.addRight(limits.el);
  const t = tabs([{ value: "cpu", label: "CPU", icon: "performance" }, { value: "gpu", label: "GPU", icon: "gpu" }], { label: "Processor", onChange: () => refresh() });
  t.panel("cpu").append(cpu.el);
  t.panel("gpu").append(gpu.el);
  page.append(t.el);
  const refresh = () => (t.value() === "cpu" ? (cpu.refresh(), limits.refresh()) : gpu.refresh());
  return { el: page, refresh };
}
