// Fan curves: drag points on the graph, tune the behaviour, keep several named curves.

import { call, tryCall } from "../api.js";
import { store } from "../state.js";
import { h, card, row, hint, button, select, slider, field, seg, run, toast, confirmDialog, svg, errText } from "../ui.js";
import { T0, T1, makeScale, linePoints, movePoint, addPoint, removePoint, nudge, validateCurve } from "../curve.js";
import { validName, NAME_HELP, fmtTemp } from "../format.js";


export default function render({ wide }) {
  wide(true);
  const page = h("div", { class: "page" });
  let curve = null; // the curve in the editor (deep copy)
  let name = "native";
  let side = "cpu";
  let locked = true;
  let drag = -1;
  let names = [];

  page.append(hint("Drag a point to move it. Double-click empty space to add one, right-click a point to remove it. With the keyboard: Tab to a point, arrow keys move it, Delete removes it. 0 % means the firmware runs the fans (they stop when cool)."));
  const sideSeg = seg([{ value: "cpu", label: "CPU fan" }, { value: "gpu", label: "GPU fan" }], { value: "cpu", label: "Which fan", onChange: (v) => ((side = v), draw()) });

  // the graph is drawn in real pixels (sized from its box) so text and points keep their size on any screen
  let SCALE = makeScale(560, 280);
  const root = svg("svg", { class: "plot", viewBox: "0 0 560 280", role: "group", "aria-label": "Fan curve graph" });
  const gridG = svg("g");
  const linesG = svg("g");
  const marksG = svg("g");
  const ptsG = svg("g");
  root.append(gridG, linesG, marksG, ptsG);
  const nowLine = hint("");
  const addBtn = button("Add point", { icon: "plus", onClick: addMiddle });
  const plot = h("div", { class: "plot-wrap" }, root);

  function resize() {
    const w = Math.max(300, Math.round(plot.clientWidth));
    const h = Math.max(220, Math.min(380, Math.round(w * 0.56)));
    if (root.getAttribute("viewBox") === `0 0 ${w} ${h}`) return;
    root.setAttribute("viewBox", `0 0 ${w} ${h}`);
    SCALE = makeScale(w, h);
    drawGrid();
    draw();
  }

  function drawGrid() {
    gridG.replaceChildren();
    for (let p = 0; p <= 100; p += 20) {
      gridG.append(svg("line", { class: "grid-line", x1: SCALE.box.l, x2: SCALE.box.r, y1: SCALE.y(p), y2: SCALE.y(p) }));
      const t = svg("text", { class: "axis", x: SCALE.box.l - 6, y: SCALE.y(p) + 4, "text-anchor": "end" });
      t.textContent = `${p}%`;
      gridG.append(t);
    }
    for (let t = T0; t <= T1; t += 10) {
      gridG.append(svg("line", { class: "grid-line", x1: SCALE.x(t), x2: SCALE.x(t), y1: SCALE.box.t, y2: SCALE.box.b }));
      const x = svg("text", { class: "axis", x: SCALE.x(t), y: SCALE.box.b + 16, "text-anchor": "middle" });
      x.textContent = `${t}°`;
      gridG.append(x);
    }
  }
  function draw() {
    linesG.replaceChildren();
    ptsG.replaceChildren();
    if (!curve) return;
    for (const s of ["gpu", "cpu"]) {
      const d = linePoints(curve[s]).map(([t, p], i) => `${i ? "L" : "M"}${SCALE.x(t).toFixed(1)} ${SCALE.y(p).toFixed(1)}`).join("");
      linesG.append(svg("path", { class: `line ${s}${s === side ? "" : " off"}`, d }));
    }
    curve[side].forEach(([t, p], i) => {
      const c = svg("circle", {
        class: `pt ${side}${locked ? " locked" : ""}`, cx: SCALE.x(t), cy: SCALE.y(p), r: 7, tabindex: locked ? undefined : "0", role: "button", "data-i": i,
        "aria-label": `Point ${i + 1}: ${t} °C, ${p === 0 ? "firmware auto" : p + " %"}${locked ? "" : ". Arrow keys move it, Delete removes it."}`,
      });
      if (!locked) {
        c.addEventListener("keydown", (e) => keyOnPoint(e, i));
        c.addEventListener("contextmenu", (e) => {
          e.preventDefault();
          remove(i);
        });
      }
      ptsG.append(c);
    });
    addBtn.disabled = locked;
    drawMarks();
  }

  function drawMarks() {
    marksG.replaceChildren();
    const s = ((store.get().live || {}).sensors) || {};
    for (const k of ["cpu", "gpu"]) {
      const t = s[`${k}_temp`];
      if (typeof t === "number" && t >= T0 && t <= T1) marksG.append(svg("line", { class: `mark ${k}`, x1: SCALE.x(t), x2: SCALE.x(t), y1: SCALE.box.t, y2: SCALE.box.b }));
    }
    const duty = ((store.get().live || {}).fans || {}).duty_register_pct || {};
    nowLine.textContent = `Now: CPU ${fmtTemp(s.cpu_temp)}, GPU ${fmtTemp(s.gpu_temp)}${duty.cpu != null ? ` · fans at ${duty.cpu} % / ${duty.gpu} %` : ""}`;
  }

  const toPlot = (ev) => {
    const m = root.getScreenCTM();
    const p = new DOMPoint(ev.clientX, ev.clientY).matrixTransform(m.inverse());
    return { x: p.x, y: p.y };
  };
  root.addEventListener("pointerdown", (ev) => {
    if (locked || ev.button !== 0) return;
    const hit = ev.target.closest && ev.target.closest(".pt");
    if (!hit) return;
    drag = Number(hit.dataset.i);
    try {
      root.setPointerCapture(ev.pointerId);
    } catch {
      /* the drag still works inside the graph */
    }
    ev.preventDefault();
  });
  root.addEventListener("pointermove", (ev) => {
    if (drag < 0) return;
    const p = toPlot(ev);
    curve[side] = movePoint(curve[side], drag, SCALE.t(p.x), SCALE.p(p.y));
    draw();
  });
  const endDrag = () => {
    drag = -1;
  };
  root.addEventListener("pointerup", endDrag);
  root.addEventListener("pointercancel", endDrag);
  root.addEventListener("dblclick", (ev) => {
    if (locked || (ev.target.closest && ev.target.closest(".pt"))) return;
    const p = toPlot(ev);
    const next = addPoint(curve[side], SCALE.t(p.x));
    if (next) {
      curve[side] = next;
      draw();
    } else toast("Too close to another point, or the curve has 12 points already", "info");
  });

  function keyOnPoint(e, i) {
    const step = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, 5], ArrowDown: [0, -5] }[e.key];
    if (step) {
      e.preventDefault();
      curve[side] = nudge(curve[side], i, step[0], step[1]);
      draw();
      ptsG.children[i]?.focus();
    } else if (e.key === "Delete" || e.key === "Backspace") {
      e.preventDefault();
      remove(i);
    }
  }
  function remove(i) {
    const next = removePoint(curve[side], i);
    if (!next) return toast("A curve keeps at least two points", "info");
    curve[side] = next;
    draw();
    ptsG.children[Math.min(i, next.length - 1)]?.focus();
  }
  function addMiddle() {
    const pts = curve[side];
    let best = 0;
    for (let i = 1; i < pts.length; i++) if (pts[i][0] - pts[i - 1][0] > pts[best + 1][0] - pts[best][0]) best = i - 1;
    const t = Math.round((pts[best][0] + pts[best + 1][0]) / 2);
    const next = addPoint(pts, t);
    if (next) {
      curve[side] = next;
      draw();
    } else toast("There is no room for another point there", "info");
  }

  // ---- behaviour ----
  const predict = slider({ label: "Early boost when heating (°C)", min: 0, max: 15, step: 1, value: 5, fmt: (v) => `${v} °C` });
  const hyst = slider({ label: "Wait before slowing down (°C)", min: 0, max: 15, step: 1, value: 4, fmt: (v) => `${v} °C` });
  const rate = slider({ label: "Slow-down speed (% per second)", min: 0.5, max: 10, step: 0.5, value: 1.5, fmt: (v) => `${v} %/s` });
  const behave = card("Behaviour");
  behave.add(predict.el, hyst.el, rate.el);

  // ---- curves ----
  const sel = select([], { label: "Curve", onChange: (n) => loadCurve(n) });
  const copyBtn = button("Copy to edit", { onClick: copyCurve });
  const delBtn = button("Delete", { kind: "danger", onClick: deleteCurve });
  const nameIn = field({ placeholder: "name", maxlength: 40, label: "Name to save as", width: "180px" });
  const saveBtn = button("Save", { onClick: saveCurve });
  const msg = hint("");
  const list = card("Curves");
  list.add(
    row("Curve", [sel.el, button("Use", { kind: "primary", onClick: useCurve })]),
    row("Edit", [copyBtn, delBtn]),
    row("Save as", [nameIn.el, saveBtn], { sub: NAME_HELP }),
    row("Default", button("Reset to default", { onClick: resetCurve })),
    msg,
  );

  page.append(sideSeg.el, plot, h("div", { class: "row" }, nowLine, addBtn), behave.el, list.el);

  /** The curve as the editor shows it: the points and the settings the daemon knows. */
  function fromEditor() {
    return { cpu: curve.cpu, gpu: curve.gpu, couple: curve.couple, battery_shift: curve.battery_shift, predict: predict.get(), hysteresis: hyst.get(), down_rate: rate.get() };
  }
  const fail = (e) => {
    msg.textContent = errText(e);
    toast(errText(e), "error");
  };

  async function loadList(select_) {
    names = await tryCall("fan", "curve", { action: "list" }, []);
    sel.setOptions(names, select_ ?? name);
    await loadCurve(sel.get());
  }
  async function loadCurve(n) {
    try {
      const c = await call("fan", "curve", { action: "show", name: n });
      name = n;
      curve = { ...c, cpu: c.cpu.map((p) => [...p]), gpu: c.gpu.map((p) => [...p]) };
      locked = Boolean(c.builtin);
      for (const [s, k] of [[predict, "predict"], [hyst, "hysteresis"], [rate, "down_rate"]]) {
        s.set(c[k], true);
        s.setDisabled(locked);
      }
      copyBtn.hidden = !locked;
      delBtn.disabled = locked;
      saveBtn.disabled = locked;
      msg.textContent = locked ? (c.native ? "Native: the firmware controls the fans, exactly like PredatorSense on Windows. There is no curve to edit." : "Built-in curves can't be edited. Press Copy to edit to make your own version.") : c.description || "";
      draw();
    } catch (e) {
      fail(e);
    }
  }
  async function useCurve() {
    const err = !locked && validateCurve(curve);
    if (err) return fail(new Error(err));
    msg.textContent = "";
    const args = locked ? { action: "use", name } : { action: "use", name, curve: fromEditor() };
    await run(call("fan", "curve", args), (r) => `Fan curve in use: ${(r && r.name) || name}`);
  }
  async function saveCurve() {
    const n = nameIn.get().trim() || name;
    if (!validName(n)) return fail(new Error(NAME_HELP));
    const err = validateCurve(curve);
    if (err) return fail(new Error(err));
    const r = await run(call("fan", "curve", { action: "save", name: n, curve: fromEditor() }), `Saved fan curve '${n}'`);
    if (r !== undefined) {
      msg.textContent = `Saved '${n}'. Press Use to run it.`;
      nameIn.set("");
      loadList(n);
    }
  }
  async function resetCurve() {
    await run(call("fan", "curve", { action: "use", name: "native" }), "Fan control reset to the default (native: the firmware)");
    loadList("native");
  }
  async function copyCurve() {
    const base = nameIn.get().trim() || `${name}-copy`;
    if (!validName(base)) return fail(new Error(NAME_HELP));
    let n = base, i = 2;
    while (names.includes(n)) n = `${base}-${i++}`;
    const r = await run(call("fan", "curve", { action: "copy", name, new: n }), `Copied '${name}' to '${n}'`);
    if (r !== undefined) {
      msg.textContent = `Copied to '${n}'. Drag the points to change it, then Save and Use.`;
      nameIn.set("");
      loadList(typeof r === "string" ? r : n);
    }
  }
  async function deleteCurve() {
    if (!(await confirmDialog({ title: "Delete fan curve", body: `Delete the curve '${name}'?`, confirm: "Delete", danger: true }))) return;
    const r = await run(call("fan", "curve", { action: "delete", name }), `Deleted fan curve '${name}'`);
    if (r !== undefined) loadList("native");
  }

  const unsub = store.subscribe(() => drawMarks());
  const watch = new ResizeObserver(resize);
  watch.observe(plot);
  loadList("native").then(async () => {
    // start on the curve in use
    const cur = await tryCall("fan", "curve", { action: "current" }, null);
    if (cur && cur.name && names.includes(cur.name)) {
      sel.set(cur.name, true);
      loadCurve(cur.name);
    }
  });
  return { el: page, destroy: () => (unsub(), watch.disconnect()) };
}
