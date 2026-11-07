// Home: power mode (with Manual / Native / Smart control), GPU mode, fans, live readings.

import { call, tryCall, live } from "../api.js";
import { store, history } from "../state.js";
import { h, card, row, seg, slider, select, button, hint, toggle, toast, run, confirmDialog, svg } from "../ui.js";
import { icon } from "../icons.js";
import { fmtTemp, fmtRpm, fmtPct, fmtWatt, fmtDuration, capitalize, AUTO_HINT, FAN_HINT, clamp } from "../format.js";
import { sparkPath } from "../spark.js";

const MODE_COLORS = { eco: "#5de0a0", quiet: "#5db8ff", balanced: "#58a6ff", performance: "#ffb347", turbo: "#ff5d6c" };
const MODE_TIPS = {
  eco: "Lowest power and heat; for battery and light work",
  quiet: "Low fan noise; reduced speed",
  balanced: "PredatorSense's default: everyday use",
  performance: "More power for demanding work and games",
  turbo: "Maximum power; loud fans and high heat",
};
const MODE_ORDER = ["eco", "quiet", "balanced", "performance", "turbo"];
const FAN_TIPS = {
  smart: "Follows your fan curve by temperature and load",
  auto: "The firmware's own control, exactly like PredatorSense on Windows",
  max: "Both fans at full speed",
  manual: "Set the speed yourself",
};

const store_ = {
  get: (k, d) => {
    try {
      const v = localStorage.getItem(k);
      return v === null ? d : JSON.parse(v);
    } catch {
      return d;
    }
  },
  set: (k, v) => {
    try {
      localStorage.setItem(k, JSON.stringify(v));
    } catch {
      /* private window: the choice just is not remembered */
    }
  },
};

/** One live reading: the name at the left, a small history graph and the value at the right. */
function liveRow(label) {
  const val = h("span", { text: "–" });
  const path = svg("path", { d: "" });
  const sp = svg("svg", { class: "spark", viewBox: "0 0 110 22", preserveAspectRatio: "none", "aria-hidden": "true" }, path);
  const el = h("div", { class: "live-row" }, h("span", { text: label }), h("span", { class: "live-val" }, sp, val));
  return {
    el,
    set(text, hist, opts = {}) {
      val.textContent = text;
      path.setAttribute("d", hist ? sparkPath(hist.values(), 110, 22, opts) : "");
    },
    hot: (on) => el.classList.toggle("hot", on),
  };
}

export default function render({ quit }) {
  const page = h("div", { class: "page" });
  const unsub = [];
  let modes = MODE_ORDER;

  // ---- performance mode ----
  const perf = card("Performance Mode", { icon: "performance", right: true });
  let powerSeg = seg(
    MODE_ORDER.map((m) => ({ value: m, label: capitalize(m), icon: m, color: MODE_COLORS[m], title: MODE_TIPS[m] })),
    { tiles: true, label: "Performance mode", onChange: (m) => run(call("power", "set", { mode: m }), (r) => `Power mode: ${r || m} (held 30 min)`).then((r) => r === undefined && refreshStatus()) },
  );
  const autoHint = hint(AUTO_HINT.off);
  const smartSel = select([], {
    label: "Smart profile",
    onChange: (name) => run(call("power", "smart", { action: "use", name }), (r) => `Smart auto profile in use: ${(r && r.name) || name}`),
  });
  const smartStatus = hint("");
  const smartBox = h("div", { class: "stack", hidden: true, style: undefined }, row("Smart profile", [smartSel.el, h("a", { class: "btn", href: "#/smart" }, "Edit")]), smartStatus);
  smartBox.style.gap = "8px";
  const holdText = hint("");
  const holdBox = h("div", { class: "row", hidden: true }, h("div", { class: "row-label" }, holdText), button("Resume auto", { onClick: () => run(call("power", "release", {}), "Auto is back in charge") }));
  const autoSeg = seg(
    [{ value: "off", label: "Manual" }, { value: "native", label: "Native" }, { value: "smart", label: "Smart" }],
    {
      label: "Mode control",
      onChange: (v) => {
        paintAuto(v);
        run(call("power", "auto", { action: v }), `Mode control: ${v}`);
      },
    },
  );
  function paintAuto(v) {
    autoHint.textContent = AUTO_HINT[v] || "";
    smartBox.hidden = v !== "smart";
  }
  const tilesHost = h("div", {}, powerSeg.el);
  perf.add(tilesHost, row("Mode control", autoSeg.el), autoHint, smartBox, holdBox);

  // ---- GPU mode ----
  const gpuCard = card("GPU Mode", { icon: "gpu", right: true });
  let gpuPrev = "";
  const gpuSeg = seg(
    [{ value: "optimus", label: "Optimus", icon: "optimus" }, { value: "nvidia", label: "Nvidia", icon: "gpu" }, { value: "auto", label: "Auto", icon: "auto" }],
    {
      tiles: true,
      label: "GPU mode",
      onChange: async (mode) => {
        const ok = await confirmDialog({
          title: "Change the GPU mode",
          body: [`Switch the GPU mode to ${capitalize(mode)}?`, "It takes effect after the next reboot. Pick your previous mode again before rebooting to cancel."],
          confirm: "Apply",
        });
        if (!ok) return gpuSeg.set(gpuPrev, true);
        const r = await run(call("gpu", "set", { mode }), (m) => `GPU mode: ${m || mode} (reboot to apply)`);
        if (r === undefined) gpuSeg.set(gpuPrev, true);
        else gpuPrev = mode;
      },
    },
  );
  gpuCard.add(gpuSeg.el, hint("A change needs a reboot to take effect."));

  // ---- fans ----
  const fans = card("Fans", { icon: "fan", right: true });
  const fanSeg = seg(
    ["smart", "auto", "max", "manual"].map((m, i) => ({ value: m, label: capitalize(m), icon: ["smart", "auto", "max", "manual"][i], title: FAN_TIPS[m] })),
    { tiles: true, label: "Fan mode", onChange: (m) => fanPicked(m) },
  );
  const fanHint = hint(FAN_HINT.smart);
  const curveSel = select([], {
    label: "Smart curve",
    onChange: (name) => run(call("fan", "curve", { action: "use", name }), (r) => `Fan curve in use: ${(r && r.name) || name}`),
  });
  const curveBox = h("div", {}, row("Smart curve", [curveSel.el, h("a", { class: "btn", href: "#/curves" }, "Edit")]));
  const linked = toggle({ label: "Control both fans together", checked: store_.get("ts.fanLink", true), onChange: (on) => linkChanged(on) });
  const single = slider({ label: "Fan speed", min: 20, max: 100, step: 5, value: 40, fmt: (v) => `${v} %`, onInput: () => manualLive() });
  const cpuFan = slider({ label: "CPU fan", min: 20, max: 100, step: 5, value: 40, fmt: (v) => `${v} %`, onInput: () => manualLive() });
  const gpuFan = slider({ label: "GPU fan", min: 20, max: 100, step: 5, value: 40, fmt: (v) => `${v} %`, onInput: () => manualLive() });
  const two = h("div", { class: "stack" }, cpuFan.el, gpuFan.el);
  two.style.gap = "10px";
  const offBtn = button("Turn fans off…", { kind: "danger", onClick: fansOff, title: "Asks first; only allowed while cool" });
  const manualBox = h("div", { class: "stack", hidden: true }, row("Control both fans together", linked.el), single.el, two, row("Fans off", offBtn));
  manualBox.style.gap = "12px";
  fans.add(fanSeg.el, curveBox, fanHint, manualBox);

  function linkChanged(on) {
    store_.set("ts.fanLink", on);
    if (on) single.set(Math.max(cpuFan.get(), gpuFan.get()), true);
    else {
      cpuFan.set(single.get(), true);
      gpuFan.set(single.get(), true);
    }
    paintLink();
    if (fanSeg.get() === "manual") manualLive();
  }
  function paintLink() {
    const on = linked.get();
    single.el.hidden = !on;
    two.hidden = on;
  }
  const duties = () => (linked.get() ? [single.get(), single.get()] : [cpuFan.get(), gpuFan.get()]).map((v) => Math.max(20, v));
  function manualLive() {
    const [cpu, gpu] = duties();
    live("fan-duty", () => call("fan", "set", { cpu, gpu }));
  }
  function paintFan(m) {
    fanHint.textContent = FAN_HINT[m] || "";
    manualBox.hidden = m !== "manual";
    curveBox.hidden = m !== "smart";
  }
  async function fanPicked(m) {
    paintFan(m);
    let r;
    if (m === "smart") r = await run(call("fan", "smart", { action: "on" }), "Fans: smart");
    else if (m === "auto") r = await run(call("fan", "auto", {}), "Fans: firmware auto");
    else if (m === "max") r = await run(call("fan", "max", {}), "Fans: MAX");
    else {
      const [cpu, gpu] = duties();
      r = await run(call("fan", "set", { cpu, gpu }), cpu === gpu ? `Fans: manual ${cpu} %` : `Fans: manual CPU ${cpu} % / GPU ${gpu} %`);
    }
    if (r === undefined) refreshStatus();
  }
  async function fansOff() {
    const ok = await confirmDialog({
      title: "Turn the fans off",
      body: [
        "Stop both fans?",
        "With the fans off the laptop can overheat quickly under any load. This is only for quiet, light use.",
        "It is only allowed while the CPU is under 60 °C and the GPU under 55 °C. If the CPU reaches 70 °C or the GPU 65 °C, TrueSense gives the fans back to the firmware automatically.",
        "On this laptop the firmware may keep the fans at a minimum speed. TrueSense checks the fan speed afterwards and tells you what really happened.",
      ],
      confirm: "Turn fans off",
      danger: true,
    });
    if (!ok) return;
    const r = await run(call("fan", "off", { confirm: true }));
    if (r === undefined) return;
    toast("Fans stopped. Checking what they really do…", "info");
    await new Promise((res) => setTimeout(res, 8000));
    const s = (store.get().live || {}).sensors || {};
    const turning = [s.cpu_fan_rpm, s.gpu_fan_rpm].some((v) => v > 500);
    if (turning)
      toast(`The firmware accepted the stop but the fans still turn (CPU ${s.cpu_fan_rpm} rpm, GPU ${s.gpu_fan_rpm} rpm): it keeps a minimum speed in manual mode, so the fans cannot really be stopped on this laptop. They stay under TrueSense's watch; Auto brings them back to the firmware.`, "info", 14000);
    else toast("Fans stopped (confirmed by the fan speed). They come back on Auto automatically if the laptop gets warm.", "ok", 8000);
  }

  // ---- live readings ----
  const liveCard = card("Live", { icon: "chip", right: true });
  const g = { cpu: liveRow("CPU temperature"), gpu: liveRow("GPU temperature"), fan: liveRow("Fans"), cl: liveRow("CPU load"), gl: liveRow("GPU load"), pw: liveRow("Power draw"), bat: liveRow("Battery") };
  liveCard.add(...Object.values(g).map((x) => x.el));

  // ---- where to go ----
  const links = [["Lighting", "keyboard", "lighting"], ["Fan curves", "fan", "curves"], ["PREDATOR key", "key", "predator"], ["Undervolt", "performance", "undervolt"], ["Battery and Screen", "battery", "battery"], ["Audio", "audio", "audio"], ["Settings", "settings", "settings"]];
  const grid = h(
    "div",
    { class: "nav-grid" },
    ...links.map(([t, i, p]) => h("a", { class: "btn", href: `#/${p}` }, icon(i, 18), t)),
    button("Quit", { icon: "power", title: "Stop the TrueSense window server", onClick: quit }),
  );

  page.append(perf.el, gpuCard.el, fans.el, liveCard.el, grid);
  paintLink();

  // ---- data ----
  async function loadModes() {
    const p = await tryCall("power", "status", {}, null);
    if (p && Array.isArray(p.supported)) {
      modes = MODE_ORDER.filter((m) => p.supported.includes(m));
      if (!modes.length) modes = MODE_ORDER;
      const fresh = seg(
        modes.map((m) => ({ value: m, label: capitalize(m), icon: m, color: MODE_COLORS[m], title: MODE_TIPS[m] })),
        { tiles: true, label: "Performance mode", onChange: (m) => run(call("power", "set", { mode: m }), (r) => `Power mode: ${r || m} (held 30 min)`).then((r) => r === undefined && refreshStatus()) },
      );
      fresh.set(p.mode, true);
      tilesHost.replaceChildren(fresh.el);
      powerSeg = fresh;
    }
  }
  async function loadLists() {
    const [sm, cv] = await Promise.all([tryCall("power", "smart", { action: "list" }, []), tryCall("fan", "curve", { action: "list" }, [])]);
    smartSel.setOptions(sm);
    curveSel.setOptions(cv);
    applyLive(store.get().live);
  }
  async function refreshStatus() {
    const st = await tryCall("status", "get", {}, null);
    if (!st) return;
    if (st.gpu_mode) {
      gpuSeg.set(st.gpu_mode, true);
      gpuPrev = st.gpu_mode;
    }
    if (st.power_mode) powerSeg.set(st.power_mode, true);
    const d = st.daemon && typeof st.daemon === "object" ? st.daemon : null;
    if (d) applyDaemon(d, st.fans);
  }

  function applyDaemon(d, fansState) {
    const s = d.settings || {}, lv = d.live || {};
    const am = s.auto_mode || (s.auto_profile ? "smart" : "off");
    autoSeg.set(am);
    paintAuto(autoSeg.get());
    if (lv.smart_profile && smartSel.has(lv.smart_profile)) smartSel.set(lv.smart_profile);
    smartStatus.textContent = lv.mode_reason && am !== "off" ? `Now: ${lv.mode_reason}` : "";
    const hl = d.hold_left_s || 0;
    holdBox.hidden = !(hl && am !== "off");
    holdText.textContent = `Your pick stays until something changes (plug, a game starting or stopping, heat) or ${fmtDuration(hl)} more. Then ${am} auto takes over by itself.`;
    if (lv.fan_curve && curveSel.has(lv.fan_curve)) curveSel.set(lv.fan_curve);
    const fm = fansState && fansState.mode;
    const mode = s.smart_fan ? "smart" : ["auto", "max", "manual"].includes(fm) ? fm : "auto";
    fanSeg.set(mode);
    if (!manualBox.hidden || mode === "manual") paintFan(fanSeg.get());
    if (mode === "manual" && fansState && fansState.duty_register_pct) {
      const c = clamp(fansState.duty_register_pct.cpu, 20, 100), gg = clamp(fansState.duty_register_pct.gpu, 20, 100);
      cpuFan.set(c);
      gpuFan.set(gg);
      single.set(Math.max(c, gg));
    }
  }

  function applyLive(msg) {
    if (!msg) return;
    const s = msg.sensors || {};
    if (msg.power_mode) powerSeg.set(msg.power_mode);
    const rpm = `${fmtRpm(s.cpu_fan_rpm)}`;
    perf.setRight(`CPU: ${fmtTemp(s.cpu_temp)} · ${rpm}`);
    gpuCard.setRight(`GPU: ${fmtTemp(s.gpu_temp)} · ${fmtRpm(s.gpu_fan_rpm)}`);
    fans.setRight(fmtRpm(Math.max(s.cpu_fan_rpm || 0, s.gpu_fan_rpm || 0)));
    if (msg.daemon) applyDaemon(msg.daemon, msg.fans);
    else if (msg.fans) {
      const fm = msg.fans.mode;
      fanSeg.set(["auto", "max", "manual"].includes(fm) ? fm : "auto");
      paintFan(fanSeg.get());
    }
    g.cpu.set(fmtTemp(s.cpu_temp), history.cpuTemp);
    g.cpu.hot(s.cpu_temp >= 85);
    g.gpu.set(fmtTemp(s.gpu_temp), history.gpuTemp);
    g.gpu.hot(s.gpu_temp >= 75);
    g.fan.set(`${fmtRpm(Math.max(s.cpu_fan_rpm || 0, s.gpu_fan_rpm || 0))}`, history.fanRpm, { min: 0, minSpan: 1000 });
    const ld = msg.load || {};
    g.cl.set(fmtPct(ld.cpu_pct), history.cpuLoad, { min: 0, max: 100 });
    g.gl.set(fmtPct(ld.gpu_pct), history.gpuLoad, { min: 0, max: 100 });
    const pw = msg.power || {};
    g.pw.set(fmtWatt((pw.cpu_w || 0) + (pw.gpu_w || 0) || null), history.power, { min: 0, minSpan: 40 });
    g.bat.set(msg.battery_pct == null ? "–" : `${fmtPct(msg.battery_pct)}${msg.ac === true ? " · on power" : msg.ac === false ? " · on battery" : ""}`, null);
    liveCard.setRight(`${msg.ac === true ? "AC" : msg.ac === false ? "Battery" : ""}`);
  }

  unsub.push(store.subscribe((st) => applyLive(st.live)));
  loadModes().then(loadLists).then(refreshStatus);
  return { el: page, destroy: () => unsub.forEach((f) => f()), refresh: refreshStatus };
}
