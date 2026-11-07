// The keyboard picture: an SVG of the real layout where keys can be clicked, boxed, and
// picked with the keyboard. It only draws colours and reports selections; what to do with
// them is the page's business. Geometry maths lives in kbd-layout.js (unit-tested).

import { svg, h } from "./ui.js";
import { buildLayout, keyAt, cellsInBox, keyLabel, neighbour, groupCells } from "./kbd-layout.js";
import { textOn, STOCK_CELLS } from "./color.js";

/**
 * @param {{onSelect?: (cells: Set<number>) => void, onPick?: (hex: string) => void}} cb
 */
export function createBoard({ onSelect, onPick } = {}) {
  let layout = buildLayout({});
  let nameCells = {};
  let cells = [...STOCK_CELLS];
  let sel = new Set();
  let focusName = null;
  const els = new Map(); // key name -> {g, rect, text}

  const band = svg("rect", { class: "band", visibility: "hidden", x: 0, y: 0, width: 0, height: 0 });
  const root = svg("svg", { class: "board", viewBox: "0 0 20.5 6.4", role: "application", tabindex: "0", "aria-label": "Keyboard picture. Arrow keys move between keys, Space selects, Shift with arrows adds, Control A selects all, Escape clears." });
  const wrap = h("div", { class: "board-wrap" }, root);

  const colourOf = (k) => cells[k.cells[0]] ?? "000000";

  function paintKey(k) {
    const e = els.get(k.name);
    if (!e) return;
    const hex = colourOf(k);
    e.rect.setAttribute("fill", `#${hex}`);
    e.text.setAttribute("fill", textOn(hex));
    e.g.classList.toggle("sel", k.cells.some((c) => sel.has(c)));
    e.g.classList.toggle("focus", k.name === focusName && document.activeElement === root);
  }
  const paintAll = () => layout.keys.forEach(paintKey);

  function build() {
    root.replaceChildren();
    els.clear();
    root.setAttribute("viewBox", `0 0 ${layout.width} ${layout.height}`);
    for (const k of layout.keys) {
      const rect = svg("rect", { x: k.x + 0.04, y: k.y + 0.04, width: k.w - 0.08, height: k.h - 0.08, rx: 0.12 });
      const text = svg("text", { x: k.x + k.w / 2, y: k.y + k.h / 2 + 0.12, "text-anchor": "middle" });
      text.textContent = keyLabel(k.name);
      const title = svg("title");
      title.textContent = k.name;
      const g = svg("g", { class: "key", "data-key": k.name }, title, rect, text);
      root.append(g);
      els.set(k.name, { g, rect, text });
    }
    root.append(band);
    paintAll();
  }

  const toLayout = (ev) => {
    const m = root.getScreenCTM();
    if (!m) return { x: -1, y: -1 };
    const p = new DOMPoint(ev.clientX, ev.clientY).matrixTransform(m.inverse());
    return { x: p.x, y: p.y };
  };

  const emit = () => {
    paintAll();
    onSelect && onSelect(new Set(sel));
  };

  // ---- pointer ----
  let drag = null;
  const capture = (id) => {
    try {
      root.setPointerCapture(id); // keeps the drag going outside the board
    } catch {
      /* a pointer that is already gone: the drag still works inside the board */
    }
  };
  root.addEventListener("pointerdown", (ev) => {
    if (ev.button === 2) return;
    root.focus({ preventScroll: true });
    const p = toLayout(ev);
    const k = keyAt(layout.keys, p.x, p.y);
    const additive = ev.ctrlKey || ev.metaKey || ev.shiftKey;
    drag = { start: p, base: additive ? new Set(sel) : new Set(), moved: false, id: ev.pointerId };
    capture(ev.pointerId);
    if (k) {
      focusName = k.name;
      if ((ev.ctrlKey || ev.metaKey) && k.cells.every((c) => sel.has(c))) k.cells.forEach((c) => sel.delete(c));
      else sel = new Set([...drag.base, ...k.cells]);
    } else if (!additive) sel = new Set();
    emit();
  });
  root.addEventListener("pointermove", (ev) => {
    if (!drag) return;
    const p = toLayout(ev);
    if (!drag.moved && Math.hypot(p.x - drag.start.x, p.y - drag.start.y) < 0.18) return;
    drag.moved = true;
    band.setAttribute("visibility", "visible");
    band.setAttribute("x", Math.min(p.x, drag.start.x));
    band.setAttribute("y", Math.min(p.y, drag.start.y));
    band.setAttribute("width", Math.abs(p.x - drag.start.x));
    band.setAttribute("height", Math.abs(p.y - drag.start.y));
    sel = new Set([...drag.base, ...cellsInBox(layout.keys, drag.start, p)]);
    emit();
  });
  const end = () => {
    drag = null;
    band.setAttribute("visibility", "hidden");
  };
  root.addEventListener("pointerup", end);
  root.addEventListener("pointercancel", end);
  root.addEventListener("contextmenu", (ev) => {
    ev.preventDefault();
    const p = toLayout(ev);
    const k = keyAt(layout.keys, p.x, p.y);
    if (k && onPick) onPick(colourOf(k));
  });

  // ---- keyboard ----
  const focusKey = () => layout.keys.find((k) => k.name === focusName) || layout.keys[0];
  root.addEventListener("focus", () => {
    if (!focusName && layout.keys[0]) focusName = layout.keys[0].name;
    paintAll();
  });
  root.addEventListener("blur", paintAll);
  root.addEventListener("keydown", (ev) => {
    const dir = { ArrowLeft: "left", ArrowRight: "right", ArrowUp: "up", ArrowDown: "down" }[ev.key];
    if (dir) {
      ev.preventDefault();
      const from = focusKey();
      const next = neighbour(layout.keys, from, dir);
      if (next) {
        focusName = next.name;
        if (ev.shiftKey) next.cells.forEach((c) => sel.add(c));
      }
      emit();
    } else if (ev.key === " " || ev.key === "Enter") {
      ev.preventDefault();
      const k = focusKey();
      if (!k) return;
      if (k.cells.every((c) => sel.has(c))) k.cells.forEach((c) => sel.delete(c));
      else k.cells.forEach((c) => sel.add(c));
      emit();
    } else if ((ev.ctrlKey || ev.metaKey) && ev.key.toLowerCase() === "a") {
      ev.preventDefault();
      sel = new Set(layout.keys.flatMap((k) => k.cells));
      emit();
    } else if (ev.key === "Escape" && sel.size) {
      ev.preventDefault();
      ev.stopPropagation();
      sel = new Set();
      emit();
    } else if (ev.key === "p" && focusName) {
      const k = focusKey();
      if (k && onPick) onPick(colourOf(k));
    }
  });

  return {
    el: wrap,
    setLayout(nc) {
      nameCells = nc;
      layout = buildLayout(nc);
      build();
    },
    setCells(c) {
      cells = [...c];
      paintAll();
    },
    getCells: () => [...cells],
    selection: () => new Set(sel),
    setSelection(s) {
      sel = new Set(s);
      emit();
    },
    selectGroup(group) {
      if (group === "none") sel = new Set();
      else if (group === "invert") {
        const every = new Set(layout.keys.flatMap((k) => k.cells));
        sel = new Set([...every].filter((c) => !sel.has(c)));
      } else sel = new Set(groupCells(group, nameCells));
      emit();
    },
    selectedNames: () => layout.keys.filter((k) => k.cells.some((c) => sel.has(c))).map((k) => k.name).sort(),
    layout: () => layout,
    allCells: () => layout.keys.flatMap((k) => k.cells),
  };
}
