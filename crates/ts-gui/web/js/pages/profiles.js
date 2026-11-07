// Profiles: one-click bundles of settings (power, fans, lights, battery, screen, audio).

import { call, tryCall } from "../api.js";
import { h, card, row, hint, button, select, field, run, toast, confirmDialog, errText } from "../ui.js";
import { describeProfile, validName, NAME_HELP, withSource } from "../format.js";

export default function render() {
  const page = h("div", { class: "page" });
  let list = [];
  page.append(hint("A profile is a one-click bundle: it sets the power mode, fans, rear light, battery limit, screen overdrive, boot sound and audio in one go. It applies once (the power mode is then held like when you pick a tile). A profile only changes what it lists below: anything it does not list stays as it is, so switching from one profile to another can leave things from the earlier one (for example battery-saver's 80 % limit) switched on."));

  const sel = select([], { label: "Profile", onChange: () => showProfile() });
  const now = hint("");
  const choose = card("Choose");
  choose.add(row("Profile", [sel.el, button("Apply", { kind: "primary", onClick: apply }), button("Next", { onClick: next })]), now);

  const info = h("div", { class: "stack" });
  const delBtn = button("Delete", { kind: "danger", onClick: del });
  const fileBox = h("pre", { class: "pre", hidden: true });
  const details = card("What it changes (and only this)");
  details.add(info, row("Details", [button("Show the file", { onClick: showFile }), delBtn]), fileBox);

  const nameIn = field({ placeholder: "name, e.g. my-gaming", maxlength: 40, label: "Profile name", width: "200px", onEnter: () => save() });
  const own = card("Make your own");
  own.add(
    hint("Set everything the way you like first, then save: TrueSense records the current power mode, fans, rear light, battery, screen, boot sound and audio. (Keyboard colours are saved under Lighting, Profiles.)"),
    row("Save the current state as", [nameIn.el, button("Save", { onClick: save })], { sub: NAME_HELP }),
  );
  const differs = card("How this differs");
  differs.add(hint("Smart auto profiles decide HOW the power mode follows load and heat. Fan curves are only the fan shape. Lighting profiles and key sets are only lights. Key bindings are only the PREDATOR key. A profile here sets all of it once; it does not keep watching."));
  page.append(choose.el, details.el, own.el, differs.el);

  async function load(select_) {
    list = await tryCall("profile", "list", {}, []);
    sel.setOptions(list.map((p) => ({ value: p.name, label: withSource(p.name, p.source) })), select_ ?? sel.get());
    await showProfile();
  }
  async function showProfile() {
    const name = sel.get();
    fileBox.hidden = true;
    info.replaceChildren();
    if (!name) {
      info.append(hint("No profiles yet."));
      return;
    }
    const cur = await tryCall("profile", "current", {}, null);
    now.textContent = cur && cur.name ? `Last applied: ${cur.name}` : "No profile applied yet.";
    try {
      const d = await call("profile", "show", { name });
      info.append(d.description ? h("p", { text: d.description }) : null, h("ul", { class: "bullets" }, ...describeProfile(d).map((l) => h("li", { text: l }))));
      delBtn.disabled = (list.find((p) => p.name === name) || {}).source !== "yours";
    } catch (e) {
      info.append(hint(`Could not read this profile: ${errText(e)}`));
    }
  }
  async function showFile() {
    const name = sel.get();
    if (!name) return;
    try {
      fileBox.textContent = JSON.stringify(await call("profile", "show", { name }), null, 2);
      fileBox.hidden = !fileBox.hidden;
    } catch (e) {
      toast(errText(e), "error");
    }
  }
  async function apply() {
    const name = sel.get();
    if (!name) return;
    await run(call("profile", "apply", { name }), (r) => (typeof r === "string" && r ? r : `Profile '${name}' applied`));
    showProfile();
  }
  async function next() {
    await run(call("profile", "next", {}), (r) => (typeof r === "string" && r ? r : "Next profile applied"));
    load();
  }
  async function save() {
    const name = nameIn.get().trim();
    if (!validName(name)) return toast(`Give the profile a name. ${NAME_HELP}`, "error");
    const r = await run(call("profile", "save", { name, description: "saved from the window" }), `Saved profile '${name}' (the current power, fans, rear light, battery, screen, boot sound and audio)`);
    if (r !== undefined) {
      nameIn.set("");
      load(name);
    }
  }
  async function del() {
    const name = sel.get();
    if (!(list.find((p) => p.name === name) || {}).source === "yours") return toast("Shipped profiles cannot be deleted; save a copy under your own name instead", "info");
    if (!(await confirmDialog({ title: "Delete profile", body: `Delete the profile '${name}'?`, confirm: "Delete", danger: true }))) return;
    if ((await run(call("profile", "delete", { name }), `Deleted profile '${name}'`)) !== undefined) load();
  }
  load();
  return { el: page, refresh: () => load(sel.get()) };
}
