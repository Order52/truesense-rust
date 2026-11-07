// "Newest value wins": for sliders and colour pickers that apply live. One write runs at a
// time; while it runs, only the newest request is kept, and it runs right after.

/**
 * @param {(err: Error) => void} [onError]
 * @returns {{submit: (fn: () => Promise<unknown>) => void, busy: () => boolean, idle: () => Promise<void>}}
 */
export function createLatest(onError = () => {}) {
  let running = false;
  let pending = null;
  let waiters = [];
  const finish = () => {
    const w = waiters;
    waiters = [];
    w.forEach((r) => r());
  };
  async function drain(fn) {
    running = true;
    try {
      let next = fn;
      while (next) {
        pending = null;
        try {
          await next();
        } catch (e) {
          onError(e instanceof Error ? e : new Error(String(e)));
        }
        next = pending;
      }
    } finally {
      running = false;
      finish();
    }
  }
  return {
    submit(fn) {
      if (running) pending = fn;
      else drain(fn);
    },
    busy: () => running,
    idle: () => (running ? new Promise((r) => waiters.push(r)) : Promise.resolve()),
  };
}

/** Run `fn` after `ms` of quiet: a burst of calls becomes one. */
export function debounce(fn, ms, timers = { set: setTimeout, clear: clearTimeout }) {
  let t = null;
  const wrapped = (...a) => {
    if (t !== null) timers.clear(t);
    t = timers.set(() => {
      t = null;
      fn(...a);
    }, ms);
  };
  wrapped.cancel = () => {
    if (t !== null) timers.clear(t);
    t = null;
  };
  return wrapped;
}
