// Settings: start at login, what the lights do at login, touchpad, power managers, updates, reset.

import { call, tryCall } from "../api.js";
import { h, card, row, hint, button, select, switchRow, run, toast, confirmDialog, errText } from "../ui.js";

const STOCK = [["last", "Last used"], ["default", "Stock look"], ["none", "Leave as the firmware has it"], ["off", "Dark"]];
const REPO = "https://github.com/Order52/truesense-rust";

export default function render() {
  const page = h("div", { class: "page" });
  let upd = null;

  // ---- window ----
  const autostart = switchRow("Open this window at login", { onChange: (on) => run(call("gui", "autostart", { action: on ? "on" : "off" }), on ? "Window opens at login" : "Window does not open at login").then((r) => r === undefined && autostart.set(!on, true)) },
    "Writes ~/.config/autostart/truesense.desktop. Hyprland needs `exec-once = truesense gui` (or uwsm) to run autostart entries.");
  const c1 = card("Start up");
  c1.add(autostart.row, hint("Lighting at login is set under At login below, and at boot in Lighting, Profiles. The PREDATOR key has its own page."));

  // ---- at login ----
  const suKb = select([], { label: "Keyboard lighting", onChange: (v) => pick("keyboard", v, suKb) });
  const suRear = select([], { label: "Rear light bar", onChange: (v) => pick("rear", v, suRear) });
  const suProf = select([], { label: "Profile applied first", onChange: (v) => pick("profile", v, suProf) });
  const c2 = card("At login");
  c2.add(
    hint("What the lights and a profile do when you log in (the truesense-startup service). Boot lighting, before login, is in Lighting, Profiles."),
    row("Keyboard lighting", suKb.el),
    row("Rear light bar", suRear.el),
    row("Profile (applied first)", suProf.el),
    row("Try it now", button("Apply the login setup", { onClick: () => run(call("startup", "apply", {}), (r) => (Array.isArray(r) && r.length ? `Login setup applied:\n${r.join("\n")}` : "Login setup applied")) })),
  );

  // ---- touchpad ----
  const tp = switchRow("Touchpad on", { onChange: (on) => run(call("touchpad", "set", { action: on ? "on" : "off" }), `Touchpad ${on ? "on" : "off"}`) }, "The same as Fn+F10.");
  const c3 = card("Touchpad");
  c3.add(tp.row);

  // ---- power managers ----
  const pm = switchRow("TrueSense manages power", { onChange: pmSet }, "Stops and disables other power managers that fight it (auto-cpufreq, TLP, tuned, power-profiles-daemon) and puts the CPU back to sane defaults, turbo on. Off puts them back exactly as they were.");
  const pmMsg = hint("");
  const c4 = card("Power management");
  c4.add(pm.row, pmMsg);

  // ---- updates ----
  const version = h("span", { text: "…" });
  const updMsg = hint("Not checked yet.");
  const notes = h("a", { class: "btn", hidden: true, target: "_blank", rel: "noopener noreferrer" }, "What's new");
  const updBtn = button("Update…", { kind: "primary", onClick: updateNow });
  updBtn.hidden = true;
  const auto = switchRow("Check for updates automatically", { onChange: (on) => run(call("update", "auto", { action: on ? "on" : "off" }), `Automatic update checks: ${on ? "on" : "off"}`) },
    "Once a day it asks GitHub for the newest release number (nothing else is sent) and shows a notification; nothing is installed without you pressing Update.");
  const c5 = card("Updates");
  c5.add(row("Installed version", [version, button("Check now", { onClick: () => check(true) }), notes, updBtn]), updMsg, auto.row);

  // ---- about / reset ----
  const c6 = card("About");
  c6.add(
    row("TrueSense", [h("img", { src: "logo.svg", alt: "", width: 40, height: 40 })]),
    row("Credits and licence", [
      h("a", { class: "btn", href: `${REPO}/blob/main/docs/CREDITS.md`, target: "_blank", rel: "noopener noreferrer" }, "Credits"),
      h("a", { class: "btn", href: REPO, target: "_blank", rel: "noopener noreferrer" }, "Source"),
    ]),
  );
  const c8 = card("System");
  c8.add(
    row("System check, services, self-test, log", h("a", { class: "btn", href: "#/health" }, "Open health check")),
    row("One-click bundles of settings", h("a", { class: "btn", href: "#/profiles" }, "Profiles")),
  );
  const c7 = card("Reset");
  c7.add(
    row("Everything back to defaults", button("Reset all…", { kind: "danger", onClick: resetAll })),
    hint("Back to how a fresh install behaves, like PredatorSense on Windows: Native auto mode control, firmware fans, power limits left to the firmware, lighting stock, undervolt off, PREDATOR key `default`. Your saved profiles are kept."),
  );
  page.append(c1.el, c2.el, c3.el, c4.el, c5.el, c8.el, c6.el, c7.el);

  async function pick(target, value, sel) {
    const r = await run(call("startup", "set", { target, value }), () => `At login, ${target}: ${sel.el.selectedOptions[0]?.text ?? value}`);
    if (r === undefined) loadLogin();
  }
  async function loadLogin() {
    const [cfg, looks, sets, profs] = await Promise.all([tryCall("startup", "show", {}, {}), tryCall("look", "list", {}, []), tryCall("keyboard", "profile", { action: "list" }, []), tryCall("profile", "list", {}, [])]);
    const base = STOCK.map(([v, l]) => ({ value: v, label: l }));
    suKb.setOptions([...base, ...looks.map((n) => ({ value: `look:${n}`, label: `Look: ${n}` })), ...sets.map((n) => ({ value: `profile:${n}`, label: `Key set: ${n}` }))], cfg.keyboard || "last");
    suRear.setOptions([...base, ...looks.map((n) => ({ value: `look:${n}`, label: `Look: ${n}` }))], cfg.rear || "last");
    suProf.setOptions([{ value: "none", label: "None" }, ...profs.map((p) => ({ value: p.name, label: p.name }))], cfg.profile || "none");
    suKb.set(cfg.keyboard || "last", true);
    suRear.set(cfg.rear || "last", true);
    suProf.set(cfg.profile || "none", true);
  }
  async function loadPm() {
    try {
      const r = await call("power", "managers", { action: "list" });
      pm.set(Boolean(r.on), true);
      const found = (r.found || []).map((m) => m.unit + (m.active ? " (running)" : " (enabled)"));
      pmMsg.textContent = found.length ? `Another power manager is present: ${found.join(", ")}${r.on ? ". It will be stopped." : ". Turn this on to stop it."}` : "No other power manager is running or enabled.";
    } catch (e) {
      pmMsg.textContent = `Power managers: could not ask the service (${errText(e)})`;
    }
  }
  async function pmSet(on) {
    try {
      const r = await call("power", "managers", { action: on ? "stop" : "restore" });
      toast(on ? `Stopped: ${(r.stopped || []).join(", ") || "nothing was running"}${r.failed && r.failed.length ? `. Could not stop: ${r.failed.join(", ")}` : ""}` : `Restored: ${(r.restored || []).join(", ") || "nothing to restore"}`, "ok");
    } catch (e) {
      toast(errText(e), "error");
    }
    loadPm();
  }

  async function check(force) {
    updMsg.textContent = "Checking…";
    try {
      upd = await call("update", "check", { force });
    } catch (e) {
      upd = { current: "", latest: null, available: false, error: errText(e), info: null };
    }
    const info = upd.info || {};
    updBtn.hidden = !upd.available;
    const link = typeof info.url === "string" && info.url.startsWith("https://github.com/") ? info.url : "";
    notes.hidden = !link;
    if (link) notes.href = link;
    let txt = upd.available ? `TrueSense ${upd.latest} is available (you have ${upd.current}).` : upd.latest ? `You have the newest version (${upd.current}).` : "Could not find the newest version.";
    if (upd.error) txt += `  ${upd.error}`;
    updMsg.textContent = txt;
    if (upd.current) version.textContent = upd.current;
  }
  async function updateNow() {
    if (!upd || !upd.available) return check(true);
    const ok = await confirmDialog({
      title: "Update TrueSense",
      body: [`Update TrueSense from ${upd.current} to ${upd.latest}?`, "This downloads the release from GitHub, checks its checksum and runs its installer with administrator rights: you will be asked for your password. The service restarts for a moment (the fans go back to the firmware meanwhile)."],
      confirm: "Update",
    });
    if (!ok) return;
    updMsg.textContent = "Downloading and installing… (enter your password when asked)";
    try {
      const r = await call("update", "apply", { yes: true });
      updMsg.textContent = r.returncode === 0 ? `Updated to ${r.version}. Close this window and run \`truesense gui\` again to use it.` : `The update did not finish: the installer exited with code ${r.returncode} (see /var/log/truesense-install.log)`;
      toast(r.returncode === 0 ? `Updated to ${r.version}` : "The update did not finish", r.returncode === 0 ? "ok" : "error");
    } catch (e) {
      updMsg.textContent = `The update did not finish: ${errText(e)}`;
      toast(errText(e), "error");
    }
  }
  async function resetAll() {
    const ok = await confirmDialog({
      title: "Reset to defaults",
      body: ["Reset the fan curve, Smart auto profile, PREDATOR key, power limits, lighting and undervolt to their defaults?", "Your saved profiles are not deleted."],
      confirm: "Reset all",
      danger: true,
    });
    if (!ok) return;
    try {
      const r = await call("config", "reset-all", {});
      const bad = (r.steps || []).filter((s) => !s.ok);
      if (bad.length) toast(`Reset done, but ${bad.length} step(s) failed: ${bad.map((s) => `${s.step} (${s.error})`).join("; ")}`, "error", 14000);
      else toast("Everything is back to its defaults", "ok");
    } catch (e) {
      toast(errText(e), "error");
    }
  }

  (async () => {
    const [as, tpState, v, a] = await Promise.all([tryCall("gui", "autostart", { action: "status" }, null), tryCall("touchpad", "get", {}, null), tryCall("version", "get", {}, null), tryCall("update", "auto", { action: "status" }, null)]);
    if (typeof as === "boolean") autostart.set(as, true);
    if (typeof tpState === "boolean") tp.set(tpState, true);
    if (v && v.truesense) version.textContent = v.truesense;
    if (typeof a === "boolean") auto.set(a, true);
  })();
  loadLogin();
  loadPm();
  return { el: page, refresh: () => (loadLogin(), loadPm()) };
}
