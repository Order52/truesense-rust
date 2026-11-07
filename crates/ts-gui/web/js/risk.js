// Safety zones for undervolts and power limits: where it is usually fine, where it may be
// unstable, where freezes are likely. Same numbers as undervolt.py, gpuundervolt.py and cpu.py;
// the daemon enforces them again, the page uses them to colour the warning and ask first.

export const CPU_MAX_MV = 120;
const SAFE = { core: 60, cache: 50 };
const CAUTION = { core: 90, cache: 70 };

/** @returns {{level: "safe"|"caution"|"danger", why: string}} for core/cache offsets in mV (negative or positive). */
export function cpuRisk(core, cache) {
  const c = Math.abs(core), k = Math.abs(cache);
  if (c > CAUTION.core || k > CAUTION.cache)
    return { level: "danger", why: `Beyond the danger line (core ${CAUTION.core} mV / cache ${CAUTION.cache} mV). Freezes and reboots are likely; unsaved work can be lost.` };
  if (c > SAFE.core || k > SAFE.cache)
    return { level: "caution", why: `Past the safety line (core ${SAFE.core} mV / cache ${SAFE.cache} mV). It may be unstable: test under load before trusting it.` };
  return { level: "safe", why: c || k ? "Inside the safe range." : "Stock voltage." };
}

export const GPU_OFFSET_RANGE = [-300, 250];
export const GPU_CAP_RANGE = [600, 3300];
const GPU_SAFE_OFFSET = 150;
const GPU_DANGER_OFFSET = 200;

export function gpuRisk(offset, cap) {
  if (offset > GPU_DANGER_OFFSET) return { level: "danger", why: `Beyond the danger line (+${GPU_DANGER_OFFSET} MHz). Games and the driver can crash; the screen can glitch.` };
  if (offset > GPU_SAFE_OFFSET) return { level: "caution", why: `Past the safety line (+${GPU_SAFE_OFFSET} MHz). It may crash in some games: test under load.` };
  if (offset > 0 && !cap)
    return { level: "caution", why: "A positive offset without a clock cap can push the clock above stock: set a cap (2400 MHz is typical) or keep the offset small." };
  if (offset < -100) return { level: "caution", why: "A large negative offset only slows the GPU; it does not save voltage the way a positive offset with a cap does." };
  if (cap && cap < 1500) return { level: "caution", why: "A cap this low will cost a lot of speed in games." };
  return { level: "safe", why: offset || cap ? "Inside the safe range." : "Stock." };
}

const PL_SAFE = { pl1: [35, 80], pl2: [35, 100] };
const PL_DANGER = { pl1: 100, pl2: 115 };

export function plRisk(pl1, pl2) {
  if (pl2 < pl1) return { level: "danger", why: "PL2 (burst) must not be below PL1 (sustained).", invalid: true };
  if (pl1 > PL_DANGER.pl1 || pl2 > PL_DANGER.pl2)
    return { level: "danger", why: `Beyond the danger line (${PL_DANGER.pl1} W sustained / ${PL_DANGER.pl2} W burst): the cooling cannot keep up and the CPU will throttle hard or overheat.` };
  if (pl1 > PL_SAFE.pl1[1] || pl2 > PL_SAFE.pl2[1])
    return { level: "caution", why: `Above stock (${PL_SAFE.pl1[1]} W sustained / ${PL_SAFE.pl2[1]} W burst): hotter and louder, and it may throttle.` };
  if (pl1 < PL_SAFE.pl1[0] || pl2 < PL_SAFE.pl2[0]) return { level: "caution", why: "Very low limits make the laptop feel slow." };
  return { level: "safe", why: "Inside the safe range." };
}
