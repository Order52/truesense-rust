// The state every page shares: the latest live message from the stream, the connection
// state, and a short history of the readings for the graphs.

import { createStore } from "./store.js";
import { createHistory } from "./spark.js";

/** @type {ReturnType<typeof createStore>} live: the last stream message; conn: "connecting" | "online" | "offline" | "stale" */
export const store = createStore({ live: null, conn: "connecting" });

export const history = {
  cpuTemp: createHistory(),
  gpuTemp: createHistory(),
  fanRpm: createHistory(),
  cpuLoad: createHistory(),
  gpuLoad: createHistory(),
  power: createHistory(),
};

const num = (v) => (typeof v === "number" && Number.isFinite(v) ? v : null);

/** Folds one stream message into the store and the histories. */
export function ingest(msg) {
  const s = msg.sensors || {};
  history.cpuTemp.push(s.cpu_temp);
  history.gpuTemp.push(s.gpu_temp);
  history.fanRpm.push(Math.max(num(s.cpu_fan_rpm) ?? 0, num(s.gpu_fan_rpm) ?? 0));
  history.cpuLoad.push(msg.load && msg.load.cpu_pct);
  history.gpuLoad.push(msg.load && msg.load.gpu_pct);
  const p = msg.power || {};
  history.power.push((num(p.cpu_w) ?? 0) + (num(p.gpu_w) ?? 0) || null);
  store.set({ live: msg });
}
