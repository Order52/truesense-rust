// Pure colour helpers for the keyboard editor: hex strings (rrggbb, lower case) everywhere.

const HEX6 = /^[0-9a-f]{6}$/;

/** Normalises user input ("#FF3B30", "ff3b30") to "ff3b30"; returns null when it is not a colour. */
export function normHex(s) {
  const t = String(s ?? "").trim().replace(/^#/, "").toLowerCase();
  if (/^[0-9a-f]{3}$/.test(t)) return [...t].map((c) => c + c).join("");
  return HEX6.test(t) ? t : null;
}

export const hexToRgb = (h) => [0, 2, 4].map((i) => parseInt(h.slice(i, i + 2), 16));

export const rgbToHex = (rgb) => rgb.map((v) => Math.max(0, Math.min(255, Math.round(v))).toString(16).padStart(2, "0")).join("");

export const lerpRgb = (a, b, t) => a.map((v, i) => Math.round(v + (b[i] - v) * t));

/** hue 0..1 -> rgb, the same ramp as keymap.hsv (full saturation and value). */
export function hsv(h) {
  const hh = ((h % 1) + 1) % 1;
  const i = Math.floor(hh * 6);
  const f = hh * 6 - i;
  const q = 1 - f;
  const [r, g, b] = [[1, f, 0], [q, 1, 0], [0, 1, f], [0, q, 1], [f, 0, 1], [1, 0, q]][i % 6];
  return [Math.round(r * 255), Math.round(g * 255), Math.round(b * 255)];
}

/** Perceived brightness 0..255. */
export const luminance = (hex) => {
  const [r, g, b] = hexToRgb(hex);
  return 0.3 * r + 0.59 * g + 0.11 * b;
};

/** Dark text on light keys, light text on dark ones. */
export const textOn = (hex) => (luminance(hex) > 150 ? "#10131a" : "#f2f5fa");

/** Gradient over keys ordered by x: colours for each x between two ends (ties get the start colour). */
export function gradientAt(xs, fromHex, toHex) {
  const a = hexToRgb(fromHex), b = hexToRgb(toHex);
  const lo = Math.min(...xs), hi = Math.max(...xs);
  return xs.map((x) => rgbToHex(lerpRgb(a, b, hi > lo ? (x - lo) / (hi - lo) : 0)));
}

/** Rainbow over keys by x (85 % of the hue circle so the ends differ). */
export function rainbowAt(xs) {
  const lo = Math.min(...xs), hi = Math.max(...xs);
  return xs.map((x) => rgbToHex(hsv(0.85 * (hi > lo ? (x - lo) / (hi - lo) : 0))));
}

/** The stock look of the keyboard: 126 cyan keys and two dark spare cells. */
export const STOCK_CELLS = [...Array(126).fill("00aec7"), "000000", "000000"];

export const PALETTE = ["ff3b30", "ff9500", "ffcc00", "34c759", "00c7be", "32ade6", "007aff", "5856d6", "af52de", "ff2d55", "ffffff", "c7c7cc", "8e8e93", "3a3a3c", "00aec7", "000000"];

/** Scales an RGBA pixel buffer (22 x 6) to cell colours: cell = column * 6 + row, 128 cells. */
export function cellsFromPixels(data, cols = 22, rows = 6) {
  const cells = Array(128).fill("000000");
  for (let c = 0; c < cols; c++)
    for (let r = 0; r < rows; r++) {
      if (c * rows + r >= 126) continue; // cells 126 and 127 are no keys
      const i = (r * cols + c) * 4;
      const a = data[i + 3] / 255;
      // a transparent pixel is dark, not whatever colour the file stores under it
      cells[c * rows + r] = rgbToHex([data[i] * a, data[i + 1] * a, data[i + 2] * a]);
    }
  return cells;
}

/** Cells (128 hex strings) -> RGBA bytes of a 22 x 6 picture (for the preview). */
export function pixelsFromCells(cells, cols = 22, rows = 6) {
  const out = new Uint8ClampedArray(cols * rows * 4);
  for (let c = 0; c < cols; c++)
    for (let r = 0; r < rows; r++) {
      const [R, G, B] = hexToRgb(cells[c * rows + r] ?? "000000");
      out.set([R, G, B, 255], (r * cols + c) * 4);
    }
  return out;
}
