// Battery and Screen: charge limit, charge to full once, USB charging, calibration, overdrive, boot sound.

import { call, tryCall } from "../api.js";
import { store } from "../state.js";
import { h, card, row, sep, hint, button, seg, switchRow, run, confirmDialog } from "../ui.js";

const USB_LEVELS = [0, 10, 20, 30];

export default function render() {
  const page = h("div", { class: "page" });
  let fullActive = false;

  const bat = card("Battery", { right: true });
  const limit = switchRow("80 % charge limit", { onChange: (on) => run(call("battery", "limit", { action: on ? "on" : "off" }), (r) => `80 % limit: ${r ? "on" : "off"}`).then(refresh) });
  const fullBtn = button("Charge to 100 % once", { onClick: fullToggle });
  const fullMsg = hint("");
  const usb = seg(USB_LEVELS.map((l) => ({ value: String(l), label: l ? `${l} %` : "Off" })), {
    label: "USB charging while off",
    onChange: (v) => run(call("battery", "usb-charge", { level: Number(v) }), (r) => `USB charge stops at ${r} %`),
  });
  const calib = switchRow("Calibration", { onChange: calibPicked }, "Needs AC power and takes hours. Switch off to cancel.");
  bat.add(
    limit.row,
    sep(),
    row("Need a full battery", fullBtn),
    fullMsg,
    hint("The firmware offers two levels: 80 % or 100 %. Charge to 100 % once lifts the limit for one charge; the 80 % limit comes back by itself when you unplug, when the battery has been full for 5 minutes, or after 24 hours."),
    sep(),
    row("USB charging while off", usb.el),
    sep(),
    calib.row,
  );

  const screen = card("Screen and boot");
  const od = switchRow("LCD overdrive", { onChange: (on) => run(call("display", "overdrive", { action: on ? "on" : "off" }), (r) => `Overdrive: ${r ? "on" : "off"}`) });
  const bootSound = switchRow("Boot animation and sound", { onChange: (on) => run(call("boot", "sound", { action: on ? "on" : "off" }), (r) => `Boot sound: ${r ? "on" : "off"}`) });
  const kbTimeout = switchRow("Keyboard light off after 30 s", { onChange: (on) => run(call("keyboard", "timeout", { action: on ? "on" : "off" }), "Saved") });
  screen.add(od.row, sep(), bootSound.row, sep(), kbTimeout.row);
  page.append(bat.el, screen.el);

  async function calibPicked(on) {
    if (on) {
      const ok = await confirmDialog({
        title: "Battery calibration",
        body: ["Start a battery calibration?", "It needs the charger connected and takes several hours; the battery is charged and discharged fully. Switch it off here to cancel."],
        confirm: "Start",
      });
      if (!ok) return calib.set(false, true);
    }
    const r = await run(call("battery", "calibration", { action: on ? "on" : "off" }), (x) => `Calibration: ${x ? "on" : "off"}`);
    if (r === undefined) calib.set(!on, true);
  }
  async function fullToggle() {
    await run(call("battery", "full", { action: fullActive ? "cancel" : "start" }), fullActive ? "80 % limit is back on" : "Charging to 100 % once: the 80 % limit comes back by itself");
    refresh();
  }

  function paintFull(r) {
    if (!r) {
      fullBtn.disabled = true;
      fullMsg.textContent = "Needs the TrueSense service.";
      return;
    }
    fullActive = Boolean(r.active);
    fullBtn.querySelector("span").textContent = fullActive ? "Back to the 80 % limit" : "Charge to 100 % once";
    fullBtn.disabled = !(fullActive || (r.limit && r.ac));
    fullMsg.textContent = `Battery ${r.capacity} %. ` + (fullActive ? "Charging to full this time." : !r.ac ? "Plug in the charger to use this." : !r.limit ? "The 80 % limit is off already." : "");
  }

  async function refresh() {
    const [st, b, full] = await Promise.all([tryCall("status", "get", {}, null), tryCall("battery", "status", {}, null), tryCall("battery", "full", { action: "get" }, null)]);
    if (st) {
      if (st.overdrive != null) od.set(Boolean(st.overdrive));
      if (st.boot_sound != null) bootSound.set(Boolean(st.boot_sound));
      if (st.kb_backlight_timeout != null) kbTimeout.set(Boolean(st.kb_backlight_timeout));
    }
    if (b) {
      limit.set(Boolean(b.limit_80));
      calib.set(Boolean(b.calibration));
      usb.set(String(b.usb_charge_stop));
    }
    paintFull(full);
  }
  function paintLive(msg) {
    if (!msg) return;
    bat.setRight(`${msg.battery_pct != null ? Math.round(msg.battery_pct) + " % · " : ""}${msg.ac === true ? "on power" : msg.ac === false ? "on battery" : ""}`);
  }
  const unsub = store.subscribe((st) => paintLive(st.live));
  paintLive(store.get().live);
  refresh();
  return { el: page, refresh, destroy: unsub };
}
