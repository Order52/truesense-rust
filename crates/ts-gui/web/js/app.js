// The window: one centred column (header with Back, the page), router, stream, connection state.

import { initSession, startStream, onLiveError, call, tryCall } from "./api.js";
import { store, ingest } from "./state.js";
import { h, toast, errText, button } from "./ui.js";
import { icon } from "./icons.js";
import { PAGES, resolveRoute } from "./pages/index.js";

const session = await initSession();
const root = document.getElementById("app");
root.className = "";
root.replaceChildren();

if (!session.hasToken) {
  root.append(h("p", { class: "boot-text", text: session.linkUsed ? "This window link has already been used or has expired. Run `truesense gui` again." : "This page has no access. Start the window with `truesense gui`, which opens it with a one-time link." }));
  throw new Error("no token");
}

onLiveError((e) => toast(errText(e), "error"));

// ---- shell: a header (logo on Home, Back and the title elsewhere), banners, the page ----
const head = h("header", { class: "head" });
const bannerHost = h("div", { id: "banners" });
const main = h("main", { id: "main", tabindex: "-1" });
const col = h("div", { class: "col" }, head, bannerHost, main);
const skip = h("a", { class: "skip", href: "#main", text: "Skip to content" });
skip.addEventListener("click", (e) => {
  e.preventDefault();
  main.focus();
});
root.append(skip, col);

function paintHead(page) {
  if (page.id === "home") {
    head.replaceChildren(h("img", { src: "logo.svg", alt: "" }), h("h1", { class: "brand", id: "page-title", text: "TrueSense" }));
    return;
  }
  const back = h("a", { class: "btn quiet", href: `#/${page.parent || "home"}`, title: "Back" }, icon("back", 16), h("span", { text: "Back" }));
  head.replaceChildren(back, h("h1", { id: "page-title", text: page.title }));
}

async function quit() {
  try {
    await call("gui", "quit", {});
  } catch {
    /* the server may already be gone */
  }
  root.replaceChildren(h("p", { class: "boot-text", text: "TrueSense has stopped. You can close this window." }));
  stop();
}

// ---- router ----
let current = null;
let routeToken = 0;

async function show() {
  const { page, sub } = resolveRoute(location.hash);
  const mine = ++routeToken;
  if (current && current.destroy) current.destroy();
  current = null;
  col.classList.remove("wide");
  paintHead(page);
  document.title = page.id === "home" ? "TrueSense" : `TrueSense - ${page.title}`;
  main.replaceChildren(h("p", { class: "hint", text: "Loading…" }));
  try {
    const mod = await page.load();
    if (mine !== routeToken) return;
    const view = mod.default({ sub, quit, wide: (on) => col.classList.toggle("wide", Boolean(on)) });
    current = view;
    main.replaceChildren(view.el);
    view.el.classList.add("page", page.id === "home" ? "home" : "sub");
  } catch (e) {
    if (mine !== routeToken) return;
    main.replaceChildren(h("div", { class: "notice error", role: "alert" }, icon("warn", 18), h("span", { text: `This page could not be shown: ${errText(e)}` })));
  }
  window.scrollTo(0, 0);
  if (document.activeElement === document.body || document.activeElement === null) main.focus({ preventScroll: true });
}
window.addEventListener("hashchange", show);
document.addEventListener("visibilitychange", () => {
  if (!document.hidden && current && current.refresh) current.refresh();
});

// ---- connection state ----
let connBanner = null;
function paintConn(st) {
  const msg = {
    offline: ["warn", "Lost the connection to TrueSense. Trying again…"],
    stale: ["error", "This window belongs to an earlier run of TrueSense. Close it and run `truesense gui` again."],
  }[st.conn];
  if (connBanner) connBanner.remove();
  connBanner = null;
  if (msg) {
    connBanner = h("div", { class: `banner ${msg[0]}`, role: "alert" }, icon("warn", 18), h("span", { text: msg[1] }));
    bannerHost.prepend(connBanner);
  }
}
let lastConn = "";
store.subscribe((st) => {
  if (st.conn !== lastConn) {
    lastConn = st.conn;
    paintConn(st);
  }
});

const stop = startStream(ingest, (conn) => store.set({ conn }), (page) => {
  if (typeof page === "string" && /^[a-z][a-z-]{0,23}$/.test(page)) location.hash = `#/${page}`;
});

// ---- start-up extras ----
async function checkUpdate() {
  const auto = await tryCall("update", "auto", { action: "status" }, false);
  if (auto !== true) return;
  const r = await tryCall("update", "check", { force: false }, null);
  if (!r || !r.available) return;
  const banner = h(
    "div",
    { class: "banner", role: "status" },
    icon("info", 18),
    h("span", { text: `TrueSense ${r.latest} is available (you have ${r.current}).` }),
    h("a", { class: "btn primary", href: "#/settings" }, "Update"),
    button("Later", { kind: "quiet", onClick: () => banner.remove() }),
  );
  bannerHost.append(banner);
}
setTimeout(checkUpdate, 6000);

if (!location.hash || location.hash === "#") history.replaceState(null, "", "#/home");
show();
