// The PH16-71 keyboard as a picture: rows of keys in the real layout, and the geometry maths
// (hit tests, box selection, arrow-key navigation). Key names and their lighting cells come
// from the server (`keyboard layout`); cell = column * 6 + row, 128 cells, 126 are keys.

// Rows top to bottom: [key, width] or [key, width, absolute x]; "_" is a gap.
const ROW_F = [["esc", 1], ...Array.from({ length: 12 }, (_, i) => [`f${i + 1}`, 0.93]), ["prtsc", 0.93], ["ins", 0.93], ["del", 0.93]];
const row = (keys) => [...keys].map((k) => [k, 1]);
export const KB_MAIN = [
  ROW_F,
  [...row("`1234567890"), ["minus", 1], ["=", 1], ["backspace", 2]],
  [["tab", 1.5], ...row("qwertyuiop"), ["[", 1], ["]", 1], ["\\", 1.5]],
  [["caps", 1.75], ...row("asdfghjkl"), [";", 1], ["'", 1], ["return", 2.25]],
  [["lshift", 2.25], ...row("zxcvbnm"), [",", 1], [".", 1], ["/", 1], ["rshift", 1.75], ["up", 1]],
  [["ctrl", 1.25], ["fn", 1.25], ["windows", 1.25], ["lalt", 1.25], ["space", 4.75], ["ralt", 1], ["options", 1], ["rctrl", 1.25], ["left", 1], ["down", 1], ["right", 1]],
];

// The numpad: [key, column 0..3, width, height]; the media keys sit in the top row.
export const KB_PAD = [
  [["backward", 0, 1, 1], ["playpause", 1, 1, 1], ["forward", 2, 1, 1], ["power", 3, 1, 1]],
  [["predator", 0, 1, 1], ["numlock", 1, 1, 1], ["num/", 2, 1, 1], ["*", 3, 1, 1]],
  [["num7", 0, 1, 1], ["num8", 1, 1, 1], ["num9", 2, 1, 1], ["-", 3, 1, 1]],
  [["num4", 0, 1, 1], ["num5", 1, 1, 1], ["num6", 2, 1, 1], ["+", 3, 1, 1]],
  [["num1", 0, 1, 1], ["num2", 1, 1, 1], ["num3", 2, 1, 1], ["numenter", 3, 1, 2]],
  [["num0", 0, 2, 1], ["num.", 2, 1, 1]],
];
export const PAD_X = 16.5;
const ROW_Y = [0.0, 1.3, 2.3, 3.3, 4.3, 5.3];

export const KEY_LABELS = {
  backspace: "⌫", return: "↵", lshift: "Shift", rshift: "Shift", caps: "Caps", windows: "Win", options: "Opt", lalt: "Alt", ralt: "Alt", ctrl: "Ctrl", rctrl: "Ctrl", space: "",
  up: "↑", down: "↓", left: "←", right: "→", prtsc: "PrtSc", predator: "P", power: "⏻", playpause: "▶", backward: "⏮", forward: "⏭", numlock: "Num", numenter: "↵",
  "num/": "/", "num.": ".", minus: "-", tab: "Tab", esc: "Esc", ins: "Ins", del: "Del", fn: "Fn", num0: "0", num1: "1", num2: "2", num3: "3", num4: "4", num5: "5", num6: "6", num7: "7", num8: "8", num9: "9",
};

export const keyLabel = (name) => KEY_LABELS[name] ?? (name.length === 1 ? name.toUpperCase() : name.slice(0, 3).replace(/^./, (c) => c.toUpperCase()));

/** The groups of the "Select" menu. */
export const KEY_GROUPS = [["All keys", "all"], ["WASD", "wasd"], ["Arrows", "arrows"], ["F1 to F12", "fkeys"], ["Number row", "numrow"], ["Letters", "letters"], ["Numpad", "numpad"]];

/**
 * @param {Record<string, number[]>} nameCells key name -> its cells (from `keyboard layout`)
 * @returns {{keys: {name:string, cells:number[], x:number, y:number, w:number, h:number}[], width:number, height:number}}
 */
export function buildLayout(nameCells) {
  const seen = new Set();
  const keys = [];
  const add = (name, x, y, w, h) => {
    const cells = nameCells[name] || [];
    if (cells.length && !cells.some((c) => seen.has(c))) {
      keys.push({ name, cells, x, y, w, h });
      cells.forEach((c) => seen.add(c));
    }
  };
  KB_MAIN.forEach((r, ri) => {
    let x = 0;
    for (const item of r) {
      const [name, w] = item;
      const ax = item.length > 2 ? item[2] : x;
      if (name !== "_") add(name, ax, ROW_Y[ri], w, 1);
      if (item.length === 2) x += w;
    }
  });
  KB_PAD.forEach((r, ri) => r.forEach(([name, col, w, h]) => add(name, PAD_X + col, ROW_Y[ri], w, h)));
  // anything the picture does not know still gets a key, in an extra row below
  const extra = Object.entries(nameCells).filter(([n, cs]) => cs.length && !cs.some((c) => seen.has(c)) && n !== "enter" && n !== "win");
  let height = 6.4;
  extra.forEach(([n, cs], i) => {
    if (cs.some((c) => seen.has(c))) return;
    keys.push({ name: n, cells: cs, x: i, y: 6.4, w: 1, h: 1 });
    cs.forEach((c) => seen.add(c));
    height = 7.5;
  });
  return { keys, width: 20.5, height };
}

/** The key under a point given in layout units (keys are inset by 0.04 so neighbours do not touch). */
export function keyAt(keys, x, y) {
  return keys.find((k) => x >= k.x + 0.04 && x <= k.x + k.w - 0.04 && y >= k.y + 0.04 && y <= k.y + k.h - 0.04) || null;
}

/** Cells of every key touched by a box (layout units, any corner order). */
export function cellsInBox(keys, a, b) {
  const x0 = Math.min(a.x, b.x), x1 = Math.max(a.x, b.x), y0 = Math.min(a.y, b.y), y1 = Math.max(a.y, b.y);
  const out = [];
  for (const k of keys) if (k.x + k.w > x0 && k.x < x1 && k.y + k.h > y0 && k.y < y1) out.push(...k.cells);
  return out;
}

const cellsOf = (nameCells, names) => names.flatMap((n) => nameCells[n] || []);

/** Cells of a named group ("wasd", "arrows", ...); "all" is the 126 key cells. */
export function groupCells(group, nameCells) {
  const digits = [..."1234567890"], letters = [..."abcdefghijklmnopqrstuvwxyz"];
  switch (group) {
    case "all": return Array.from({ length: 126 }, (_, i) => i);
    case "wasd": return cellsOf(nameCells, [..."wasd"]);
    case "arrows": return cellsOf(nameCells, ["up", "down", "left", "right"]);
    case "fkeys": return cellsOf(nameCells, Array.from({ length: 12 }, (_, i) => `f${i + 1}`));
    case "numrow": return cellsOf(nameCells, digits);
    case "letters": return cellsOf(nameCells, letters);
    case "numpad": return Object.keys(nameCells).filter((n) => n.startsWith("num")).flatMap((n) => nameCells[n]);
    default: return [];
  }
}

/** Horizontal centre of a cell's key (for gradients and rainbows across the board). */
export function xOfCell(keys, cell) {
  const k = keys.find((kk) => kk.cells.includes(cell));
  return k ? k.x + k.w / 2 : 0;
}

/** The key nearest in a direction ("left" | "right" | "up" | "down") for arrow-key navigation. */
export function neighbour(keys, from, dir) {
  const d = { left: [-1, 0], right: [1, 0], up: [0, -1], down: [0, 1] }[dir];
  const c0 = [from.x + from.w / 2, from.y + from.h / 2];
  let best = null, bestScore = Infinity;
  for (const k of keys) {
    if (k === from) continue;
    const dx = k.x + k.w / 2 - c0[0], dy = k.y + k.h / 2 - c0[1];
    const along = dx * d[0] + dy * d[1], across = Math.abs(dx * d[1] - dy * d[0]);
    if (along < 0.3 || across > along * 1.6 + 0.5) continue;
    const score = along + across * 2;
    if (score < bestScore) { best = k; bestScore = score; }
  }
  return best;
}
