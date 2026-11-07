// 24x24 stroke icons drawn from path data: nothing to download, crisp at any size, coloured
// by the text colour. Built with createElementNS (no markup strings anywhere in the app).

const P = {
  eco: "M6 18c0-8 5-13 13-13 0 8-5 13-13 13zM6 18l8-8",
  quiet: "M20 14.5A8 8 0 1 1 9.5 4a6.5 6.5 0 0 0 10.5 10.5z",
  balanced: "M12 4v16M6 20h12M5 8h14M5 8l-3 7a3 3 0 0 0 6 0zM19 8l-3 7a3 3 0 0 0 6 0z",
  performance: "M12 3a9 9 0 1 0 0 18a9 9 0 1 0 0-18zM12 13.2l3.6-4.4M12 6.6v1.1M6.6 12h1.1M17.4 12h-1.1M8.4 8.4l.8.8M15.6 8.4l-.8.8",
  turbo: "M12 3c1 4 5 5 5 10a5 5 0 0 1-10 0c0-2 1-3 2-4 0 2 1 3 2 3 0-3-1-5 1-9z",
  smart: "M12 3l2 5 5 2-5 2-2 5-2-5-5-2 5-2zM19 16l.8 2.2L22 19l-2.2.8L19 22l-.8-2.2L16 19l2.2-.8z",
  auto: "M12 4a8 8 0 1 0 0 16 8 8 0 0 0 0-16zM12 10a2 2 0 1 0 0 4 2 2 0 0 0 0-4z",
  max: "M12 12c0-4 1-7 4-8 1 3 0 6-4 8zM12 12c4 0 7 1 8 4-3 1-6 0-8-4zM12 12c0 4-1 7-4 8-1-3 0-6 4-8zM12 12c-4 0-7-1-8-4 3-1 6 0 8 4z",
  manual: "M4 8h9M17 8h3M4 16h3M11 16h9M15 5v6M9 13v6",
  fan: "M12 12c0-4 1-7 4-8 1 3 0 6-4 8zM12 12c4 0 7 1 8 4-3 1-6 0-8-4zM12 12c0 4-1 7-4 8-1-3 0-6 4-8zM12 12c-4 0-7-1-8-4 3-1 6 0 8 4z",
  gpu: "M2 7h20v10H2zM6 17v3M18 17v3M12 9.5a2.5 2.5 0 1 0 0 5 2.5 2.5 0 0 0 0-5z",
  optimus: "M3 6h8v6H3zM13 12h8v6h-8zM11 9h2v6M7 12v3h4",
  keyboard: "M2 6h20v12H2zM6 10h.01M10 10h.01M14 10h.01M18 10h.01M7 14h10",
  bar: "M3 10h18v4H3zM6 7v-2M12 7V4M18 7V5M6 17v2M12 17v3M18 17v2",
  battery: "M3 7h15v10H3zM18 10h3v4h-3zM6 10v4M9 10v4",
  audio: "M4 10v4h4l5 4V6l-5 4zM16.5 9a4 4 0 0 1 0 6M19 6.5a8 8 0 0 1 0 11",
  settings: "M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6zM12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9l2.1 2.1M17 17l2.1 2.1M19.1 4.9L17 7M7 17l-2.1 2.1",
  palette: "M12 3a9 9 0 1 0 0 18c1.5 0 2-1 1.5-2-.6-1.2.2-2.5 1.6-2.5H17a4 4 0 0 0 4-4c0-5-4-9.5-9-9.5zM7.5 11h.01M10 7.5h.01M14.5 7.5h.01",
  power: "M12 3v8M6.5 6.5a8 8 0 1 0 11 0",
  home: "M3 11l9-8 9 8M5 10v10h5v-6h4v6h5V10",
  profiles: "M4 5h16M4 12h16M4 19h16M8 3v4M15 10v4M10 17v4",
  health: "M12 21s-8-5-8-11a4.5 4.5 0 0 1 8-2.8A4.5 4.5 0 0 1 20 10c0 6-8 11-8 11zM8 12h2l1.5-3 2 6 1.5-3h1",
  key: "M7 14a4 4 0 1 1 3.9-5H21v3h-3v3h-3v-3h-4.1A4 4 0 0 1 7 14z",
  chip: "M7 7h10v10H7zM9 3v4M15 3v4M9 17v4M15 17v4M3 9h4M3 15h4M17 9h4M17 15h4",
  sliders: "M4 6h10M18 6h2M4 12h4M12 12h8M4 18h12M20 18h0M14 4v4M8 10v4M16 16v4",
  back: "M15 5l-7 7 7 7",
  menu: "M4 7h16M4 12h16M4 17h16",
  close: "M6 6l12 12M18 6L6 18",
  check: "M5 12.5l4.5 4.5L19 7.5",
  warn: "M12 3l10 18H2zM12 10v5M12 18h.01",
  info: "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM12 11v6M12 7.5h.01",
  undo: "M9 7L4 12l5 5M4 12h11a5 5 0 0 1 0 10h-3",
  redo: "M15 7l5 5-5 5M20 12H9a5 5 0 0 0 0 10h3",
  external: "M14 4h6v6M20 4l-9 9M18 14v5H5V6h5",
  refresh: "M20 11a8 8 0 1 0-2.3 5.7M20 4v7h-7",
  trash: "M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13",
  copy: "M8 8h11v12H8zM5 16V4h11",
  save: "M5 4h12l3 3v13H5zM8 4v5h8V4M8 20v-7h8v7",
  plus: "M12 5v14M5 12h14",
  wave: "M2 12c3-6 5-6 8 0s5 6 8 0 3-3 4-2",
  sparkle: "M12 3l1.8 5.2L19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8z",
};

const NS = "http://www.w3.org/2000/svg";

/** An icon element; unknown names draw nothing (the layout does not change). */
export function icon(name, size = 20) {
  const s = document.createElementNS(NS, "svg");
  s.setAttribute("viewBox", "0 0 24 24");
  s.setAttribute("width", String(size));
  s.setAttribute("height", String(size));
  s.setAttribute("fill", "none");
  s.setAttribute("stroke", "currentColor");
  s.setAttribute("stroke-width", "1.7");
  s.setAttribute("stroke-linecap", "round");
  s.setAttribute("stroke-linejoin", "round");
  s.setAttribute("aria-hidden", "true");
  s.setAttribute("focusable", "false");
  s.classList.add("icon");
  const p = document.createElementNS(NS, "path");
  p.setAttribute("d", P[name] || "");
  s.append(p);
  return s;
}
