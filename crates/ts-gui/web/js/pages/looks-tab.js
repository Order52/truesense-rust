// Lighting > Profiles: saved looks (keyboard and rear bar), per-key sets, and the boot look.

import { call, tryCall } from "../api.js";
import { h, card, row, hint, button, select, field, run, toast, confirmDialog, errText } from "../ui.js";
import { describeLook, validName, NAME_HELP } from "../format.js";

export function createLooksTab() {
  const el = h("div", { class: "stack" });
  el.append(hint("A lighting profile remembers the keyboard and the rear bar as they are now. Set the lights the way you like (Keyboard, Per-key, Rear bar), then save them here."));

  // ---- looks ----
  const lookSel = select([], { label: "Lighting profile", onChange: () => lookSelected() });
  const copyBtn = button("Copy to edit", { onClick: lookCopy });
  const delBtn = button("Delete", { kind: "danger", onClick: lookDelete });
  const lookName = field({ placeholder: "name", maxlength: 40, label: "Name to save as", width: "180px" });
  const saveBtn = button("Save", { onClick: lookSave });
  const lookMsg = hint("");
  const c1 = card("Profile", { icon: "manual" });
  c1.add(
    row("Profile", [lookSel.el, button("Apply", { kind: "primary", onClick: lookApply })]),
    row("Edit", [copyBtn, delBtn]),
    row("Save as", [lookName.el, saveBtn], { sub: NAME_HELP }),
    lookMsg,
    row("Default", button("Reset to default", { onClick: lookReset })),
  );

  // ---- per-key sets ----
  const setSel = select([], { label: "Key set" });
  const setName = field({ placeholder: "name", maxlength: 40, label: "Name for the key set", width: "180px" });
  const c2 = card("Per-key sets");
  c2.add(
    hint("Only the 128 key colours (no effect, no rear bar), used by `truesense keyboard profile` and by the login setting in Settings."),
    row("Key set", [setSel.el, button("Apply", { kind: "primary", onClick: setApply }), button("Delete", { kind: "danger", onClick: setDelete })]),
    row("Save the keys now as", [setName.el, button("Save", { onClick: setSave })]),
  );

  // ---- boot ----
  const bootSel = select([], { label: "Boot lighting profile" });
  const bootMsg = hint("Checking what is applied at boot…");
  const c3 = card("At boot");
  c3.add(
    hint("Applied by the TrueSense service when the laptop starts (before you log in) and again after every resume from suspend."),
    row("Profile", [bootSel.el, button("Use", { kind: "primary", onClick: bootUse })]),
    row("Boot lighting", button("Don't change at boot", { onClick: bootClear })),
    bootMsg,
  );
  el.append(c1.el, c2.el, c3.el);

  let builtin = false;
  async function reload(select_) {
    const names = await tryCall("look", "list", {}, []);
    lookSel.setOptions(names, select_ ?? (names.includes(lookSel.get()) ? lookSel.get() : "native"));
    bootSel.setOptions([{ value: "__current__", label: "What is on now" }, ...names.map((n) => ({ value: n, label: n }))], "__current__");
    lookSelected();
    loadBoot();
    loadSets();
  }
  async function lookSelected() {
    const name = lookSel.get();
    if (!name) return;
    const look = await tryCall("look", "show", { name }, null);
    builtin = Boolean(look && look.builtin);
    copyBtn.hidden = !builtin;
    delBtn.disabled = builtin;
    const d = look ? describeLook(look) : "";
    lookMsg.textContent = builtin ? `Built in, read-only: PredatorSense's own lighting. Press Copy to edit to make your own.   ${d}` : d;
  }
  async function lookSave() {
    const name = lookName.get().trim() || lookSel.get();
    if (!validName(name)) return toast(`${NAME_HELP}`, "error");
    const r = await run(call("look", "save", { name }), `Saved lighting profile '${name}'`);
    if (r !== undefined) {
      lookName.set("");
      reload(name);
    }
  }
  async function lookCopy() {
    const src = lookSel.get();
    const n = lookName.get().trim();
    if (n && !validName(n)) return toast(NAME_HELP, "error");
    const r = await run(call("look", "copy", n ? { name: src, new: n } : { name: src }), (x) => `Copied '${src}' to '${x}'. Change the lights, then Save to update it.`);
    if (r !== undefined) {
      lookName.set("");
      reload(typeof r === "string" ? r : undefined);
    }
  }
  async function lookDelete() {
    const name = lookSel.get();
    if (!(await confirmDialog({ title: "Delete lighting profile", body: `Delete the profile '${name}'?`, confirm: "Delete", danger: true }))) return;
    const r = await run(call("look", "delete", { name }), `Deleted lighting profile '${name}'`);
    if (r !== undefined) reload("native");
  }
  function lookApply() {
    return run(call("look", "apply", { name: lookSel.get() }), `Lighting profile '${lookSel.get()}' applied`);
  }
  async function lookReset() {
    await run(call("look", "apply", { name: "native" }), "Lighting back to the default (native)");
    reload("native");
  }

  async function loadSets(sel) {
    const r = await tryCall("keyboard", "profile", { action: "list" }, []);
    setSel.setOptions(r.length ? r : [{ value: "", label: "No key sets yet" }], sel ?? (r.includes(setSel.get()) ? setSel.get() : r[0] ?? ""));
  }
  function setApply() {
    return setSel.get() && run(call("keyboard", "profile", { action: "load", name: setSel.get() }), `Key set '${setSel.get()}' applied`);
  }
  async function setSave() {
    const name = setName.get().trim();
    if (!/^[A-Za-z0-9_-]{1,40}$/.test(name)) return toast("Key set names: letters, digits, - and _", "error");
    const cells = (await tryCall("keyboard", "cells", {}, null))?.cells;
    if (!cells) return toast("Could not read the key colours", "error");
    const r = await run(call("keyboard", "profile", { action: "save", name, cells }), `Saved key set '${name}'`);
    if (r !== undefined) loadSets(name);
  }
  async function setDelete() {
    const name = setSel.get();
    if (!name) return;
    if (!(await confirmDialog({ title: "Delete key set", body: `Delete the key set '${name}'?`, confirm: "Delete", danger: true }))) return;
    if ((await run(call("keyboard", "profile", { action: "delete", name }), `Deleted key set '${name}'`)) !== undefined) loadSets();
  }

  async function loadBoot() {
    try {
      const cur = await call("startup", "boot", {});
      bootMsg.textContent = cur ? `At boot: ${describeLook(cur)}` : "At boot: the lighting is left alone";
    } catch (e) {
      bootMsg.textContent = /unknown command/.test(errText(e)) ? "At boot: needs the updated service (re-run the installer)" : `At boot: unknown (${errText(e)})`;
    }
  }
  async function bootUse() {
    const picked = bootSel.get();
    const what = picked === "__current__" ? "current" : picked;
    const r = await run(call("startup", "boot", { what }), (look) => `At boot: ${look ? describeLook(look) : "set"}`);
    if (r !== undefined) loadBoot();
  }
  async function bootClear() {
    const r = await run(call("startup", "boot", { what: "clear" }), "Lighting is left alone at boot");
    if (r !== undefined) loadBoot();
  }

  reload();
  return { el, refresh: () => reload() };
}
