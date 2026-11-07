// History of the live values for the little graphs on the home page.

/** A fixed-size history: the newest `size` samples. */
export function createHistory(size = 90) {
  const values = [];
  return {
    push(v) {
      values.push(typeof v === "number" && Number.isFinite(v) ? v : null);
      if (values.length > size) values.shift();
    },
    values: () => values.slice(),
    last: () => (values.length ? values[values.length - 1] : null),
  };
}

/**
 * SVG path data of a sparkline in a w x h box. min/max default to the data range, widened
 * to at least `minSpan` so a flat line does not look like a wild swing.
 */
export function sparkPath(values, w, h, { min, max, minSpan = 10 } = {}) {
  const pts = values.map((v, i) => [i, v]).filter(([, v]) => v !== null);
  if (pts.length < 2) return "";
  const vs = pts.map(([, v]) => v);
  let lo = min ?? Math.min(...vs), hi = max ?? Math.max(...vs);
  if (hi - lo < minSpan) {
    const mid = (hi + lo) / 2;
    lo = min ?? mid - minSpan / 2;
    hi = max ?? mid + minSpan / 2;
  }
  const n = Math.max(values.length - 1, 1);
  const pad = 2;
  return pts
    .map(([i, v], k) => {
      const x = (i / n) * w;
      const y = pad + (1 - (v - lo) / (hi - lo || 1)) * (h - 2 * pad);
      return `${k ? "L" : "M"}${x.toFixed(1)} ${y.toFixed(1)}`;
    })
    .join("");
}
