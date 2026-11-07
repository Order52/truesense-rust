// Audio: sound repair, output, DTS, 3.5 mm jack, microphones and Purified Voice.

import { call, tryCall } from "../api.js";
import { h, card, row, sep, hint, button, seg, select, slider, switchRow, run, toast, errText } from "../ui.js";
import { capitalize } from "../format.js";

const DTS_MODES = ["Music", "Movie", "Voice", "Game1", "Game2", "Game3", "Plain"];
const VOICE = ["off", "conference", "personal"];
const JACK = [
  { value: "auto", label: "Auto", hint: "Uses what the codec detects when you plug something in." },
  { value: "headphones", label: "Headphones", hint: "Output only: the headset microphone is never used." },
  { value: "headset", label: "Headset", hint: "Always use the headset microphone, even if the codec has not detected it (try this if it is missing after boot)." },
];

export default function render() {
  const page = h("div", { class: "page" });

  // ---- sound output ----
  const sndMsg = hint("Checking that the sound card has an output…");
  const fixBtn = button("Fix sound", { onClick: soundFix });
  const c1 = card("Sound output");
  c1.add(sndMsg, row("No sound at all?", fixBtn, { sub: "Restarts the audio session manager (WirePlumber) so it probes the sound card again, and checks that an output exists. Nothing is written to the sound chip." }));

  // ---- output ----
  const out = seg([{ value: "speakers", label: "Speakers" }, { value: "headphones", label: "Headphones" }], { label: "Play sound on", onChange: (v) => run(call("audio", "output", { action: v }), `Output: ${v}`) });
  const spk = switchRow("Noise filter on the speakers", { onChange: (on) => run(call("audio", "speaker-filter", { action: on ? "on" : "off" }), `Speaker noise filter ${on ? "on" : "off"}`) }, "Off by default: it also gates music. For calls on the speakers.");
  const dts = switchRow("Acer DTS sound", { onChange: (on) => run(call("audio", "dts", { action: on ? "on" : "off" }), `DTS ${on ? "on" : "off"}`) }, "This laptop plays on the speakers or the headphone jack, not both at once.");
  const dtsMode = select(DTS_MODES, { label: "DTS sound mode", onChange: (m) => run(call("audio", "dts", { action: "mode", name: m }), `DTS sound: ${m} on every output`) });
  const dtsList = hint("Outputs: …");
  const c2 = card("Output");
  c2.add(
    row("Play sound on", out.el),
    sep(),
    spk.row,
    dts.row,
    row("DTS sound mode", dtsMode.el, { sub: "Applies to every output: speakers, the jack, USB and Bluetooth headsets, controllers. Plain is no processing." }),
    row("A headset still sounds flat?", button("Use DTS on every output", { onClick: dtsAll }), { sub: "New outputs get DTS by themselves when you plug them in. This does it again for all of them now (and adds a headphone EQ to headsets)." }),
    dtsList,
  );

  // ---- jack ----
  const jack = seg(JACK.map((j) => ({ value: j.value, label: j.label })), { label: "I plugged in", onChange: (v) => { paintJackHint(v); run(call("audio", "jack", { use: v }), `3.5 mm jack: ${v}`); } });
  const jackHint = hint("");
  const jackState = hint("Looking at the 3.5 mm jack…");
  const boost = slider({ label: "Headset mic boost", min: 0, max: 3, step: 1, value: 0, onChange: (v) => run(call("audio", "mic", { action: "boost", value: v }), (r) => `Boost: ${r}`) });
  const c3 = card("3.5 mm jack");
  c3.add(row("I plugged in", jack.el), jackHint, jackState, boost.el);
  const paintJackHint = (v) => (jackHint.textContent = (JACK.find((j) => j.value === v) || {}).hint || "");

  // ---- microphones ----
  const micVol = slider({ label: "Microphone volume", min: 0, max: 150, step: 5, value: 100, fmt: (v) => `${v} %`, onChange: (v) => run(call("audio", "mic", { action: "volume", value: v }), (r) => `Microphone volume: ${r} %`) });
  const mute = switchRow("Mute every microphone", { onChange: (on) => run(call("audio", "mic", { action: on ? "mute" : "unmute" }), on ? "Microphones muted" : "Microphones live") });
  const vOpts = VOICE.map((v) => ({ value: v, label: capitalize(v) }));
  const vHead = seg(vOpts, { label: "Headset noise reduction", onChange: (v) => run(call("audio", "voice", { action: "set", a: "headset", b: v }), `Headset mic: ${v}`) });
  const vArr = seg(vOpts, { label: "Built-in noise reduction", onChange: (v) => run(call("audio", "voice", { action: "set", a: "array", b: v }), `Built-in mics: ${v}`) });
  const vNote = switchRow("Notify when the microphone changes", { onChange: (on) => run(call("audio", "voice", { action: "notify", a: on ? "on" : "off" }), "Saved") });
  const c4 = card("Microphone");
  c4.add(micVol.el, mute.row, sep(), row("Headset (AI noise reduction)", vHead.el), row("Built-in (AI noise reduction)", vArr.el), sep(), vNote.row);
  page.append(c1.el, c2.el, c3.el, c4.el);

  async function soundFix() {
    try {
      const r = await call("audio", "repair", {});
      toast(
        r.fixed && r.changed ? `Sound output repaired: ${r.steps.join("; ")}` : r.fixed ? "The sound output is there: nothing to repair" : `Could not repair the sound output (${r.steps.join("; ")}). Check the kernel log for 'sof' errors and try a reboot.`,
        r.fixed ? "ok" : "error",
        r.fixed ? 0 : 12000,
      );
    } catch (e) {
      toast(errText(e), "error");
    }
    refresh(true);
  }
  async function dtsAll() {
    const r = await run(call("audio", "dts", { action: "all" }), (x) => `DTS: ${(x && x.done && x.done.join("; ")) || "on for every output"}`);
    if (r !== undefined) refresh(true);
  }

  async function refresh(slow = true) {
    const [snd, o, f, d, j, m, v] = await Promise.all([
      tryCall("audio", "sound", {}, null),
      tryCall("audio", "output", { action: "status" }, null),
      tryCall("audio", "speaker-filter", { action: "status" }, null),
      slow ? tryCall("audio", "dts", { action: "status" }, null) : null,
      tryCall("audio", "jack", {}, null),
      tryCall("audio", "mic", { action: "status" }, null),
      tryCall("audio", "voice", { action: "status" }, null),
    ]);
    if (snd) sndMsg.textContent = snd.ok ? "The sound card's output is present." : snd.ok === null ? "No sound card was found." : `No sound output: ${snd.problem || "unknown"}. Press Fix sound.`;
    if (o) out.set(o.active);
    if (typeof f === "boolean") spk.set(f);
    if (d) {
      dts.set(Boolean(d.on));
      const devs = d.devices || [];
      dtsList.textContent = devs.length ? "Now: " + devs.map((x) => `${x.label}: ${x.profile}`).join(" · ") : "";
      const cur = devs.find((x) => x.current) || devs[0];
      if (cur && DTS_MODES.includes(cur.profile)) dtsMode.set(cur.profile);
    }
    if (j) {
      jack.set(j.setting || "auto");
      paintJackHint(j.setting || "auto");
      jackState.textContent = "Detected now: " + (j.headset_mic ? "headset with microphone" : j.headphones ? "headphones" : "nothing plugged in");
    }
    if (m) {
      mute.set(Boolean(m.muted));
      if (m.volume != null) micVol.set(Math.round(m.volume));
      if (m.boost != null) boost.set(m.boost);
    }
    if (v) {
      const modes = v.modes || {};
      vHead.set(VOICE.includes(modes.headset) ? modes.headset : "off");
      vArr.set(VOICE.includes(modes.array) ? modes.array : "off");
      if (typeof v.notify === "boolean") vNote.set(v.notify);
    }
  }
  refresh(true);
  const timer = setInterval(() => !document.hidden && refresh(false), 5000);
  return { el: page, refresh: () => refresh(true), destroy: () => clearInterval(timer) };
}
