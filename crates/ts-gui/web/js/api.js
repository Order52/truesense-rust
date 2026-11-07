// The client of the command bus: every action of the page is one POST /api/{area}/{verb}
// (see COMMANDS.md), the live state arrives on one server-sent-event stream. The window URL
// carries a one-time launch code in its fragment (browsers never send fragments to a server);
// the page trades it for the session token, which goes in an Authorization header on every
// request and is never part of a URL.

import { createSSEParser } from "./sse.js";
import { createLatest } from "./latest.js";

let token = "";

/**
 * Takes the launch code and the first page from the URL fragment, removes them from the
 * address bar, and exchanges the code for the session token (kept for this tab only, so a
 * reload still works).
 * @returns {Promise<{page: string, hasToken: boolean, linkUsed: boolean}>}
 */
export async function initSession() {
  const frag = new URLSearchParams(location.hash.replace(/^#/, ""));
  const page = frag.get("page") || "";
  const code = frag.get("code") || "";
  let fresh = "";
  if (code) {
    history.replaceState(null, "", location.pathname + (page ? `#/${page}` : "#/home"));
    try {
      const res = await fetch("/api/gui/session", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ code }), cache: "no-store" });
      const body = await res.json();
      if (body && body.ok && body.result && body.result.token) fresh = body.result.token;
    } catch {
      /* the server is gone: handled as "no token" below */
    }
  }
  try {
    if (fresh) sessionStorage.setItem("truesense-token", fresh);
    token = fresh || sessionStorage.getItem("truesense-token") || "";
  } catch {
    token = fresh; // storage can be unavailable: the token then only lives until a reload
  }
  return { page, hasToken: Boolean(token), linkUsed: Boolean(code) && !fresh };
}

export class ApiError extends Error {
  constructor(message, { status = 0, code = "" } = {}) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
  }
}

/**
 * Runs one command and returns its result; throws ApiError with a message fit to show.
 * @param {string} area
 * @param {string} verb
 * @param {object} [args]
 */
export async function call(area, verb, args = {}, { signal } = {}) {
  let res;
  try {
    res = await fetch(`/api/${area}/${verb}`, {
      method: "POST",
      headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
      body: JSON.stringify(args),
      cache: "no-store",
      signal,
    });
  } catch (e) {
    if (e && e.name === "AbortError") throw e;
    throw new ApiError("Cannot reach TrueSense. Its window server has stopped: run `truesense gui` again.", { code: "offline" });
  }
  let body = null;
  try {
    body = await res.json();
  } catch {
    /* not JSON: handled below */
  }
  if (res.status === 401)
    throw new ApiError("This window belongs to an earlier run of TrueSense. Close it and run `truesense gui` again.", { status: 401, code: "unauthorised" });
  if (!res.ok || !body || body.ok !== true) {
    const msg = (body && body.error && body.error.message) || `The request failed (HTTP ${res.status}).`;
    throw new ApiError(msg, { status: res.status, code: (body && body.error && body.error.code) || "" });
  }
  return body.result;
}

/** Like call, but a failure gives `fallback` instead of an exception (for reads that may not exist yet). */
export async function tryCall(area, verb, args, fallback = null) {
  try {
    return await call(area, verb, args);
  } catch (e) {
    if (e instanceof ApiError && e.code === "unauthorised") throw e;
    return fallback;
  }
}

const latests = new Map();
let errorSink = () => {};

/** Where errors of live (slider) writes go: the app sets this to its toast. */
export const onLiveError = (fn) => {
  errorSink = fn;
};

/**
 * "Newest value wins": applies `fn` for the control `key` one at a time; while one write
 * runs, only the newest request is kept.
 */
export function live(key, fn) {
  let l = latests.get(key);
  if (!l) {
    l = createLatest((e) => errorSink(e));
    latests.set(key, l);
  }
  l.submit(fn);
}

/**
 * Follows /api/stream. onState gets each parsed state; onConn gets "online", "offline" or
 * "stale" (the token is not accepted: the window is from an older run); onShow gets a page id
 * when a second `truesense gui` asks this window to show that page.
 * @returns {() => void} stop
 */
export function startStream(onState, onConn, onShow) {
  let stopped = false;
  let ctl = null;
  (async () => {
    let delay = 500;
    while (!stopped) {
      ctl = new AbortController();
      try {
        const res = await fetch("/api/stream", { headers: { Authorization: `Bearer ${token}` }, cache: "no-store", signal: ctl.signal });
        if (res.status === 401) {
          onConn("stale");
          return;
        }
        if (!res.ok || !res.body) throw new Error(`HTTP ${res.status}`);
        onConn("online");
        delay = 500;
        const reader = res.body.getReader();
        const dec = new TextDecoder();
        const parser = createSSEParser((ev) => {
          try {
            if (ev.event === "show") {
              if (onShow) onShow(JSON.parse(ev.data));
              return;
            }
            if (ev.event !== "state") return;
            onState(JSON.parse(ev.data));
          } catch {
            /* a malformed message is skipped */
          }
        });
        for (;;) {
          const { done, value } = await reader.read();
          if (done) break;
          parser.push(dec.decode(value, { stream: true }));
        }
      } catch (e) {
        if (stopped) return;
        void e;
      }
      if (stopped) return;
      onConn("offline");
      await new Promise((r) => setTimeout(r, delay));
      delay = Math.min(delay * 2, 5000);
    }
  })();
  return () => {
    stopped = true;
    if (ctl) ctl.abort();
  };
}
