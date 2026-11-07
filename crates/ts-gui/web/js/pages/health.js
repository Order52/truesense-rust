// Health check: every part of the stack, the services, a hardware self-test, the log, the configuration.

import { call, tryCall } from "../api.js";
import { h, card, row, hint, button, run, toast, confirmDialog, errText, notice } from "../ui.js";

const BADGE = { OK: "ok", INFO: "", WARN: "warn", FAIL: "fail" };

async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

export default function render() {
  const page = h("div", { class: "page" });
  let report = "";

  const summary = h("div", {});
  const checks = h("div", { class: "checks", "aria-live": "polite" });
  const doctorCard = card("System check", { icon: "health" });
  doctorCard.add(summary, checks, row("Check again", [button("Run check", { kind: "primary", onClick: runDoctor }), button("Copy report", { onClick: copyReport })], { sub: "Reads only: nothing is changed." }));

  const svcRows = h("div", { class: "checks" });
  const svcMsg = hint("");
  const svc = card("Services", { icon: "chip" });
  svc.add(svcRows, svcMsg, row("Repair", button("Repair services", { onClick: repair }), { sub: "Gets the daemon, its health timer, the kernel module and your user services running again. Safe to repeat; touches no setting." }));

  const selftestOut = h("pre", { class: "pre", hidden: true });
  const diag = card("Diagnostics");
  diag.add(
    row("Hardware self-test", button("Run self-test…", { onClick: selfTest }), { sub: "Sets each firmware feature, reads it back and restores it (fans are skipped). Takes about a minute." }),
    row("Service log", button("Show log", { onClick: showLog })),
    row("Configuration", [button("Show", { onClick: showConfig }), button("Check", { onClick: checkConfig })]),
    selftestOut,
  );
  page.append(doctorCard.el, svc.el, diag.el);

  function showOut(text) {
    selftestOut.hidden = false;
    selftestOut.textContent = text;
    selftestOut.scrollIntoView({ block: "nearest" });
  }

  async function runDoctor() {
    summary.replaceChildren(hint("Checking…"));
    try {
      const r = await call("doctor", "run", {});
      const checksList = r.checks || [];
      summary.replaceChildren(notice(r.failures ? `${r.failures} problem(s), ${r.warnings} warning(s). TrueSense ${r.version ?? ""} on kernel ${r.kernel ?? ""}.` : r.warnings ? `OK with ${r.warnings} warning(s). TrueSense ${r.version ?? ""} on kernel ${r.kernel ?? ""}.` : `All good. TrueSense ${r.version ?? ""} on kernel ${r.kernel ?? ""}.`, r.failures ? "error" : r.warnings ? "warn" : "info"));
      checks.replaceChildren(
        ...checksList.map((c) =>
          h("div", { class: "check" }, h("span", { class: `badge ${BADGE[c.status] ?? ""}`, text: c.status }), h("div", {}, h("div", { class: "name", text: c.check }), h("div", { class: "detail", text: c.detail })), c.fix && (c.status === "WARN" || c.status === "FAIL") ? h("div", { class: "fix", text: `Fix: ${c.fix}` }) : null),
        ),
      );
      report = [`TrueSense ${r.version ?? ""} (${r.kernel ?? ""})`, ...checksList.map((c) => `[${c.status}] ${c.check}: ${c.detail}${c.fix && c.status !== "OK" ? `  (fix: ${c.fix})` : ""}`)].join("\n");
    } catch (e) {
      summary.replaceChildren(notice(`The check failed: ${errText(e)}`, "error"));
    }
  }
  async function copyReport() {
    if (!report) await runDoctor();
    toast((await copyText(report)) ? "Report copied to the clipboard" : "Could not copy: select the text and copy it yourself", "info");
  }
  async function loadServices() {
    const rows = await tryCall("service", "check", {}, null);
    if (!rows) return svcMsg.replaceChildren(), (svcMsg.textContent = "Could not read the services.");
    svcRows.replaceChildren(
      ...rows.map((r) => h("div", { class: "check" }, h("span", { class: `badge ${r.ok ? "ok" : "fail"}`, text: r.ok ? "ok" : "!!" }), h("div", {}, h("div", { class: "name", text: r.unit }), h("div", { class: "detail", text: `${r.scope} · ${r.enabled} · ${r.active}` })), r.fix && !r.ok ? h("div", { class: "fix", text: `Fix: ${r.fix}` }) : null)),
    );
    svcMsg.textContent = rows.every((r) => r.ok) ? "Everything that should run is running." : "Something is not running: use Repair services, or run `sudo truesense service repair`.";
  }
  async function repair() {
    try {
      const steps = await call("service", "repair", {});
      toast((steps || []).map((s) => `${s.ok ? "ok" : "!!"} ${s.step}`).join("\n") || "Nothing to repair", (steps || []).every((s) => s.ok) ? "ok" : "error", 12000);
    } catch (e) {
      toast(errText(e), "error");
    }
    loadServices();
  }
  async function selfTest() {
    const ok = await confirmDialog({ title: "Hardware self-test", body: ["This briefly changes power mode, lighting, battery and display settings and restores them. The fans are not touched.", "Run it now?"], confirm: "Run it" });
    if (!ok) return;
    showOut("Self-test running…");
    try {
      const r = await call("selftest", "run", { skip_fans: true });
      showOut(`${r.output || ""}\n${r.ok ? "ALL PASSED" : "SOME CHECKS FAILED"}`.trim());
    } catch (e) {
      showOut(`The self-test failed: ${errText(e)}`);
    }
  }
  async function showLog() {
    try {
      const r = await call("service", "logs", {});
      showOut(r.text || "No log lines.");
    } catch (e) {
      showOut(`Could not read the log: ${errText(e)}`);
    }
  }
  async function showConfig() {
    try {
      showOut(JSON.stringify(await call("config", "show", {}), null, 2));
    } catch (e) {
      showOut(errText(e));
    }
  }
  async function checkConfig() {
    const r = await run(call("config", "validate", {}));
    if (r === undefined) return;
    const problems = Array.isArray(r) ? r : [];
    toast(problems.length ? `Configuration problem: ${problems.join("; ")}` : "Configuration is valid", problems.length ? "error" : "ok");
  }
  runDoctor();
  loadServices();
  return { el: page, refresh: loadServices };
}
