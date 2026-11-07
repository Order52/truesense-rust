// Lighting > Per-key: the keyboard picture, palette, tools, animations, designs and pictures.

import { call, tryCall, live } from "../api.js";
import { createBoard } from "../board.js";
import { h, card, row, hint, button, select, slider, field, colorField, run, toast, errText } from "../ui.js";
import { KEY_GROUPS, xOfCell } from "../kbd-layout.js";
import { PALETTE, STOCK_CELLS, normHex, gradientAt, rainbowAt, cellsFromPixels, pixelsFromCells } from "../color.js";
import { validName, NAME_HELP, withSource } from "../format.js";

// Software animations drawn by the key service (perkeyfx.MODES).
const FX_MODES = [
  ["pulse", "Pulse"], ["wave", "Wave"], ["cycle", "Colour cycle"], ["twinkle", "Twinkle"], ["rainbow", "Rainbow sweep"], ["breathing", "Breathing (base colours)"],
  ["scanner", "Scanner"], ["comet", "Comet"], ["ripple", "Ripples"], ["rain", "Rain"], ["matrix", "Matrix rain"], ["fire", "Fire"], ["plasma", "Plasma"],
  ["aurora", "Aurora"], ["lightning", "Lightning"], ["confetti", "Confetti"], ["starlight", "Starlight"], ["mood", "Mood lamp"],
];

export function createPerKey() {
  const el = h("div", { class: "stack" });
  let history = [];
  let redoStack = [];
  let recent = [];
  let curHex = "ff3b30";
  let brightness = 25;

  const selLabel = h("p", { class: "hint", "aria-live": "polite", text: "No keys selected" });
  const board = createBoard({ onSelect: onSelection, onPick: (hex) => setColour(hex, false) });
  const groupSel = select([{ value: "", label: "Select…" }, ...KEY_GROUPS.map(([l, g]) => ({ value: g, label: l })), { value: "none", label: "Nothing" }, { value: "invert", label: "Invert selection" }], {
    label: "Select keys by group",
    onChange: (g) => {
      if (g) board.selectGroup(g);
      groupSel.set("", true);
    },
  });

  // ---- colour ----
  const picker = colorField({ value: curHex, label: "Selected colour", onInput: (hex) => setColour(hex), onChange: () => {} });
  const hexIn = field({ value: curHex, maxlength: 7, label: "Colour as hex", width: "96px", onInput: () => {}, onEnter: (v) => setColour(v) });
  hexIn.input.addEventListener("change", () => setColour(hexIn.get()));
  const palette = h("div", { class: "palette", role: "group", "aria-label": "Palette" });
  const swatches = PALETTE.map((c) => {
    const b = h("button", { type: "button", class: "swatch", title: `#${c}`, "aria-label": `Colour #${c}` });
    b.style.backgroundColor = `#${c}`;
    b.addEventListener("click", () => setColour(c));
    palette.append(b);
    return [c, b];
  });
  const recentHost = h("div", { class: "recent", "aria-label": "Recent colours", role: "group" });
  const colourCard = card("Colour");
  colourCard.add(row("Selected colour", [picker.el, hexIn.el]), palette, h("div", {}, h("span", { class: "hint", text: "Recent  " }), recentHost));

  // ---- tools ----
  const g1 = colorField({ value: "ff2d55", label: "Gradient start" });
  const g2 = colorField({ value: "5856d6", label: "Gradient end" });
  const undoBtn = button("Undo", { icon: "undo", onClick: undo, disabled: true });
  const redoBtn = button("Redo", { icon: "redo", onClick: redo, disabled: true });
  const toolsCard = card("Tools");
  toolsCard.add(
    row("Gradient across the selection", [g1.el, g2.el, button("Apply", { onClick: applyGradient })]),
    row("Selection", [button("Rainbow", { onClick: applyRainbow }), button("Clear (off)", { onClick: () => setColour("000000") })]),
    row("History", [undoBtn, redoBtn]),
  );

  // ---- animation ----
  const fxSel = select([{ value: "off", label: "Off (static colours)" }, ...FX_MODES.map(([v, l]) => ({ value: v, label: l }))], { label: "Animation mode", onChange: () => fxPush() });
  const fxSpeed = slider({ label: "Speed", min: 1, max: 10, step: 1, value: 5, onChange: () => fxPush(), onInput: () => fxDebounced() });
  let fxTimer = 0;
  const fxDebounced = () => {
    clearTimeout(fxTimer);
    fxTimer = setTimeout(fxPush, 250);
  };
  const animCard = card("Animation");
  animCard.add(
    hint("The keyboard cannot animate single keys by itself, so TrueSense draws it: about 5 frames a second, while the key service runs. Slow effects look best. It animates the selected keys, or every key if none are selected."),
    row("Mode", fxSel.el),
    fxSpeed.el,
  );

  // ---- designs and pictures ----
  const designSel = select([], { label: "Design" });
  const designName = field({ placeholder: "name, e.g. my-design", maxlength: 40, label: "Design name", width: "180px" });
  const fileIn = h("input", { type: "file", accept: "image/png,image/jpeg,image/webp,image/gif,image/bmp", class: "sr-only", tabindex: "-1", "aria-hidden": "true" });
  const preview = h("canvas", { class: "preview", width: 22, height: 6, hidden: true, role: "img", "aria-label": "Your picture on the 22 by 6 keys" });
  const imgMsg = hint("Choose a picture: it is scaled onto the 22 x 6 keys here in the page and sent as colours. Nothing leaves this computer.");
  fileIn.addEventListener("change", async () => {
    const f = fileIn.files && fileIn.files[0];
    fileIn.value = "";
    if (f) await usePicture(f);
  });
  const designCard = card("Designs and pictures");
  designCard.add(
    hint("A design is a still picture of colours (yours live in ~/.config/truesense/designs). A picture file is scaled across the keys."),
    row("Design", [designSel.el, button("Apply", { kind: "primary", onClick: designApply })]),
    row("Save the keys as a design", [designName.el, button("Save", { onClick: designSave })]),
    row("Picture", [button("Choose a picture…", { onClick: () => fileIn.click() }), fileIn]),
    preview,
    imgMsg,
  );

  // ---- whole keyboard ----
  const level = slider({ label: "Brightness", min: 0, max: 50, step: 5, value: brightness, onInput: (v) => ((brightness = v), live("kb-level", () => call("keyboard", "brightness", { level: v }))) });
  const lookName = field({ placeholder: "name, e.g. my-keys", maxlength: 40, label: "Lighting profile name", width: "180px" });
  const wholeCard = card("Whole keyboard");
  wholeCard.add(
    level.el,
    row("Fill and presets", [button("Fill all", { onClick: fillAll }), button("Rainbow", { onClick: rainbowAll })]),
    row("Reset", [button("Reset to stock", { onClick: resetKeys })]),
    row("Save as a lighting profile (listed under Profiles)", [lookName.el, button("Save", { onClick: saveLook })], { sub: NAME_HELP }),
  );

  el.append(
    hint("Click a key, drag a box around several, or Ctrl+click to add. Right-click a key (or press P on it) to pick up its colour. Changes show on the keyboard as you make them."),
    board.el,
    h("div", { class: "row" }, selLabel, groupSel.el),
    h("div", { class: "grid" }, colourCard.el, toolsCard.el, animCard.el, designCard.el, wholeCard.el),
  );

  // ---- logic ----
  function onSelection(set) {
    const names = board.selectedNames();
    selLabel.textContent = !set.size ? "No keys selected" : `${names.length} key${names.length === 1 ? "" : "s"}: ${names.slice(0, 6).join(", ")}${names.length > 6 ? "…" : ""}`;
  }

  function syncHistory() {
    undoBtn.disabled = !history.length;
    redoBtn.disabled = !redoStack.length;
  }

  const fxActive = () => fxSel.get() !== "off";

  function upload() {
    if (fxActive()) return fxPush();
    const cells = board.getCells();
    live("paint", () => call("keyboard", "set-cells", { cells }));
  }

  /** Show new colours on the picture, remember them for undo and send them (newest wins while dragging). */
  function commit(cells) {
    history.push(board.getCells());
    history = history.slice(-40);
    redoStack = [];
    board.setCells(cells);
    syncHistory();
    upload();
  }
  function undo() {
    if (!history.length) return;
    redoStack.push(board.getCells());
    board.setCells(history.pop());
    syncHistory();
    upload();
  }
  function redo() {
    if (!redoStack.length) return;
    history.push(board.getCells());
    board.setCells(redoStack.pop());
    syncHistory();
    upload();
  }

  function setColour(raw, apply = true) {
    const hex = normHex(raw);
    if (!hex) {
      hexIn.set(curHex);
      return;
    }
    curHex = hex;
    picker.set(hex, true);
    hexIn.set(hex);
    swatches.forEach(([c, b]) => b.setAttribute("aria-pressed", String(c === hex)));
    if (!PALETTE.includes(hex) && !recent.includes(hex)) {
      recent = [hex, ...recent].slice(0, 8);
      paintRecent();
    }
    const sel = board.selection();
    if (apply && sel.size) {
      const cells = board.getCells();
      for (const c of sel) cells[c] = hex;
      commit(cells);
    }
  }
  function paintRecent() {
    recentHost.replaceChildren(
      ...recent.map((c) => {
        const b = h("button", { type: "button", class: "swatch", title: `#${c}`, "aria-label": `Recent colour #${c}` });
        b.style.backgroundColor = `#${c}`;
        b.addEventListener("click", () => setColour(c));
        return b;
      }),
    );
  }

  const selectedByX = () => [...board.selection()].sort((a, b) => xOfCell(board.layout().keys, a) - xOfCell(board.layout().keys, b));

  function applyGradient() {
    const s = selectedByX();
    if (s.length < 2) return toast("Select two or more keys for a gradient", "error");
    const xs = s.map((c) => xOfCell(board.layout().keys, c));
    const colours = gradientAt(xs, g1.get(), g2.get());
    const cells = board.getCells();
    s.forEach((c, i) => (cells[c] = colours[i]));
    commit(cells);
  }
  function applyRainbow() {
    const s = selectedByX();
    if (!s.length) return toast("Select some keys first", "error");
    const colours = rainbowAt(s.map((c) => xOfCell(board.layout().keys, c)));
    const cells = board.getCells();
    s.forEach((c, i) => (cells[c] = colours[i]));
    commit(cells);
  }
  function fillAll() {
    const cells = board.getCells();
    board.allCells().forEach((c) => (cells[c] = curHex));
    commit(cells);
  }
  function rainbowAll() {
    const all = board.allCells().sort((a, b) => xOfCell(board.layout().keys, a) - xOfCell(board.layout().keys, b));
    const colours = rainbowAt(all.map((c) => xOfCell(board.layout().keys, c)));
    const cells = board.getCells();
    all.forEach((c, i) => (cells[c] = colours[i]));
    commit(cells);
  }
  async function resetKeys() {
    commit([...STOCK_CELLS]);
    await run(call("keyboard", "restore", {}), "Keyboard back to the stock look");
  }

  function fxPush() {
    const mode = fxSel.get();
    const sel = [...board.selection()].sort((a, b) => a - b);
    const args = { mode, speed: fxSpeed.get(), base: board.getCells() };
    if (sel.length) args.cells = sel;
    run(call("keyboard", "fx", args), mode === "off" ? "Animation off" : `Animation: ${FX_MODES.find((m) => m[0] === mode)[1]} (the key service draws it)`);
  }

  async function loadDesigns(selectName) {
    const d = await tryCall("keyboard", "design", { action: "list" }, {});
    designSel.setOptions(Object.entries(d).sort(([a], [b]) => a.localeCompare(b)).map(([n, v]) => ({ value: n, label: withSource(n, v.source) })), selectName);
  }
  async function designApply() {
    const name = designSel.get();
    if (!name) return;
    try {
      const d = await call("keyboard", "design", { action: "show", name });
      commit(d.cells);
      toast(`Design '${name}' applied`, "ok");
    } catch (e) {
      toast(errText(e), "error");
    }
  }
  async function designSave() {
    const name = designName.get().trim();
    if (!validName(name)) return toast(`Give the design a name. ${NAME_HELP}`, "error");
    const r = await run(call("keyboard", "design", { action: "save", name, cells: board.getCells() }), `Saved design '${name}'`);
    if (r !== undefined) loadDesigns(name);
  }
  async function saveLook() {
    const name = lookName.get().trim();
    if (!validName(name)) return toast(`Give the profile a name. ${NAME_HELP}`, "error");
    const look = { keyboard: { mode: "perkey", brightness: brightness || 25, cells: board.getCells() }, rear: null };
    await run(call("look", "save", { name, look }), `Saved lighting profile '${name}' (Lighting, Profiles)`);
  }

  async function usePicture(file) {
    try {
      const bmp = await createImageBitmap(file);
      const cv = document.createElement("canvas");
      cv.width = 22;
      cv.height = 6;
      const ctx = cv.getContext("2d", { willReadFrequently: true });
      ctx.imageSmoothingQuality = "high";
      ctx.drawImage(bmp, 0, 0, 22, 6);
      const cells = cellsFromPixels(ctx.getImageData(0, 0, 22, 6).data);
      commit(cells);
      preview.hidden = false;
      preview.getContext("2d").putImageData(new ImageData(pixelsFromCells(cells), 22, 6), 0, 0);
      imgMsg.textContent = `Sent '${file.name}' to the keyboard: scaled to 22 x 6 keys (preview above, each block is one key).`;
      toast(`Picture '${file.name}' painted on the keys`, "ok");
    } catch (e) {
      preview.hidden = true;
      imgMsg.textContent = `NOT painted: this file could not be read as a picture (${errText(e)}).`;
      toast("That file could not be read as a picture", "error");
    }
  }

  async function refresh() {
    const [layout, cells, st] = await Promise.all([
      board.layout().keys.length ? null : tryCall("keyboard", "layout", {}, null),
      tryCall("keyboard", "cells", {}, null),
      tryCall("keyboard", "status", {}, null),
    ]);
    if (layout && layout.keys) {
      const nc = {};
      for (const [n, v] of Object.entries(layout.keys)) nc[n] = v.map((x) => (typeof x === "number" ? x : x.cell));
      board.setLayout(nc);
    }
    if (cells && Array.isArray(cells.cells) && !board.selection().size && !history.length) board.setCells(cells.cells);
    if (st && typeof st.brightness === "number") {
      brightness = st.brightness;
      level.set(st.brightness);
    }
  }
  refresh();
  loadDesigns();
  return { el, refresh };
}
