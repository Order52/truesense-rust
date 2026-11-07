// Fan curve maths (port of curves.py and the Qt editor's rules): temperature -> duty.
// A curve side is a list of [temperature °C, duty %] points sorted by temperature; a duty
// is 0 (let the firmware run the fans) or 20..100 and may only rise with temperature.

export const T0 = 30;
export const T1 = 100;
export const MAX_POINTS = 12;

/** Piecewise-linear duty at temperature t, clamped to the first and last point. */
export function interp(points, t) {
  if (t <= points[0][0]) return points[0][1];
  for (let i = 1; i < points.length; i++) {
    const [t0, p0] = points[i - 1], [t1, p1] = points[i];
    if (t <= t1) return t1 > t0 ? p0 + ((p1 - p0) * (t - t0)) / (t1 - t0) : p1;
  }
  return points[points.length - 1][1];
}

/** Snap a dragged duty: below 12 % means firmware control (0), else a multiple of 5 in 20..100. */
export function snapDuty(d) {
  return d < 12 ? 0 : Math.max(20, Math.min(100, 5 * Math.round(d / 5)));
}

/** Move point i towards (t, d) without crossing its neighbours or breaking "duty only rises". */
export function movePoint(points, i, t, d) {
  const lo = i > 0 ? points[i - 1][0] + 1 : T0;
  const hi = i < points.length - 1 ? points[i + 1][0] - 1 : T1 + 10;
  let duty = snapDuty(d);
  duty = Math.max(duty, i > 0 ? points[i - 1][1] : 0);
  duty = Math.min(duty, i < points.length - 1 ? points[i + 1][1] : 100);
  const next = points.map((p) => [...p]);
  next[i] = [Math.round(Math.max(lo, Math.min(hi, t))), duty];
  return next;
}

/** A new point at (t, d), or null when one is too close or the curve is full. */
export function addPoint(points, t) {
  const tt = Math.round(t);
  if (points.length >= MAX_POINTS || points.some((p) => Math.abs(tt - p[0]) < 2)) return null;
  const at = interp(points, tt);
  const duty = at >= 12 ? Math.max(20, Math.min(100, 5 * Math.round(at / 5))) : 0;
  // the new point sits on the existing line, so the shape does not change
  return [...points.map((p) => [...p]), [tt, duty]].sort((a, b) => a[0] - b[0]);
}

/** Remove point i (a curve keeps at least two). */
export function removePoint(points, i) {
  return points.length > 2 ? points.filter((_, k) => k !== i).map((p) => [...p]) : null;
}

/** Keyboard nudge: arrows move a point one degree, or to the next multiple of 5 % (0 and 20 are one step apart). */
export function nudge(points, i, dt, dd) {
  const [t, d] = points[i];
  let target = d;
  if (dd > 0) target = d < 20 ? 20 : Math.floor(d / 5) * 5 + 5;
  else if (dd < 0) target = d <= 20 ? 0 : Math.ceil(d / 5) * 5 - 5;
  return movePoint(points, i, t + dt, target);
}

/** Validation with the daemon's rules; returns an error message or null. */
export function validateSide(side, points) {
  const cap = side === "cpu" ? 95 : 85;
  if (!Array.isArray(points) || points.length < 2 || points.length > MAX_POINTS) return `${side}: 2 to ${MAX_POINTS} points`;
  const pts = points.map((p) => [Number(p[0]), Number(p[1])]).sort((a, b) => a[0] - b[0]);
  for (const [t, p] of pts)
    if (!(t >= 20 && t <= 110) || !(p === 0 || (p >= 20 && p <= 100))) return `${side}: temperature 20..110 °C, percent 0 (firmware auto) or 20..100`;
  for (let i = 1; i < pts.length; i++) if (pts[i][1] < pts[i - 1][1]) return `${side}: the duty may only rise with temperature`;
  if (new Set(pts.map((p) => p[0])).size !== pts.length) return `${side}: two points share a temperature`;
  if (interp(pts, cap) < 100) return `${side}: must reach 100 % by ${cap} °C (the CPU and GPU throttle above that)`;
  return null;
}

export const validateCurve = (c) => validateSide("cpu", c.cpu) || validateSide("gpu", c.gpu);

/** Plot geometry: maps (°C, %) to pixels and back inside a box with margins for the axes. */
export function makeScale(width, height, m = { l: 40, r: 14, t: 12, b: 30 }) {
  const w = width - m.l - m.r, h = height - m.t - m.b;
  return {
    x: (t) => m.l + ((t - T0) / (T1 - T0)) * w,
    y: (p) => m.t + (1 - p / 100) * h,
    t: (x) => T0 + ((x - m.l) / w) * (T1 - T0),
    p: (y) => (1 - (y - m.t) / h) * 100,
    box: { l: m.l, r: width - m.r, t: m.t, b: height - m.b },
  };
}

/** The polyline of one side across the whole plot (flat before the first and after the last point). */
export function linePoints(points) {
  return [[T0, points[0][1]], ...points, [T1, points[points.length - 1][1]]];
}
