#!/usr/bin/env python3
"""Measures truesense-qt on a real Wayland session (Hyprland) without touching the user's own window.

    tools/qtprofile/qtprofile.py profile [--backends software,opengl,vulkan] [--pages home,cooling]
                                         [--settle 2] [--seconds 6] [--bin PATH] [--json out.json]
    tools/qtprofile/qtprofile.py glitch  [--backends ...] [--iterations 20] [--bin PATH]
    tools/qtprofile/qtprofile.py compare --old BIN [--new BIN] [--platform offscreen|wayland]

`profile` prints the per-page idle table: CPU % of one core, wakeups per second (voluntary context
switches of every thread), frames per second (the window's own `frameSwapped` count, printed by
`TRUESENSE_QT_FPS=1`), resident memory, per backend. `glitch` (see glitch.py) drives resizes and page
switches and compares screenshots of the window with a fresh offscreen render of the same size.
`stale.py` (its own program) compares the whole window on the screen with the app's own picture
after page switches, scrolling, popups, resizes, workspace moves, and can watch the real window.

Safety (the machine belongs to somebody who is working on it):
  * the program is started as its OWN instance: window class `tstest` (TRUESENSE_APP_ID), a private
    runtime directory (so the single-instance socket never reaches a real window), a temporary HOME,
    TRUESENSE_SIM=1 (simulated hardware), no D-Bus (no tray icon) and an EMPTY PATH (a program that
    looked for pactl, systemctl, ... would find nothing);
  * a Hyprland window rule is installed for class `tstest` only: floating, fixed size, bottom-right
    corner, never takes the focus;
  * it closes only the window whose pid it started, and ends only that pid after checking
    /proc/PID/exe.
`--platform offscreen` needs no display at all (frame counts and CPU of the paint path only).
"""
import argparse
import json
import os
import shutil
import signal
import subprocess
import sys
import time

sys.dont_write_bytecode = True  # a tool run must leave nothing behind in the tree

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import procsample  # noqa: E402

APP_ID = "tstest"
PAGES = ["home", "monitor", "performance", "cooling", "lighting", "keys", "battery", "audio",
         "profiles", "health", "settings"]
DEFAULT_BIN = os.environ.get("TRUESENSE_QT_BIN", "target/release/truesense-qt")

# How each backend is chosen. TRUESENSE_QT_RENDER is the app's own switch (software|opengl|vulkan);
# QT_QUICK_BACKEND / QSG_RHI_BACKEND are Qt's and work with every build of the program.
BACKENDS = {
    "software": {"TRUESENSE_QT_RENDER": "software", "QT_QUICK_BACKEND": "software"},
    "opengl": {"TRUESENSE_QT_RENDER": "opengl", "QSG_RHI_BACKEND": "opengl"},
    "vulkan": {"TRUESENSE_QT_RENDER": "vulkan", "QSG_RHI_BACKEND": "vulkan"},
    # whatever the program chooses when nothing is set
    "default": {},
}


def hypr(*args):
    """Runs hyprctl; returns stdout ('' on failure)."""
    try:
        return subprocess.run(["hyprctl", *args], capture_output=True, text=True, timeout=10).stdout
    except (OSError, subprocess.SubprocessError):
        return ""


def hypr_clients():
    try:
        return json.loads(hypr("clients", "-j") or "[]")
    except ValueError:
        return []


def install_rule(width, height, app_id=APP_ID, left=0):
    """The window rule for class `app_id` (only ever a `tstest...` class): floating, fixed size,
    bottom-right corner (`left` pixels further left), never takes the focus."""
    assert app_id.startswith("tstest")
    code = ('hl.window_rule({ name = "%s-rule", match = { class = "^%s$" }, float = true, '
            'size = { %d, %d }, move = { "monitor_w-%d", "monitor_h-%d" }, no_initial_focus = true, '
            'no_focus = true, no_anim = true })') % (app_id, app_id, width, height, width + 20 + left, height + 60)
    hypr("eval", code)
    errors = hypr("configerrors").strip()
    if errors:
        sys.exit("hyprland rejected the window rule: " + errors)


class Instance:
    """One own, isolated truesense-qt process."""

    def __init__(self, binary, backend, platform="wayland", rundir="/tmp/tsqt", size=(760, 520),
                 extra_env=None, app_id=APP_ID):
        self.binary = os.path.realpath(binary)
        self.backend = backend
        self.platform = platform
        self.size = size
        self.app_id = app_id
        self.rundir = os.path.join(rundir, "run-" + platform)
        self.home = os.path.join(rundir, "home")
        self.extra_env = dict(extra_env or {})
        self.proc = None
        self.log_path = os.path.join(rundir, f"log-{platform}-{backend}.txt")
        self.address = None

    def _prepare(self):
        os.makedirs(self.rundir, mode=0o700, exist_ok=True)
        os.chmod(self.rundir, 0o700)
        stub = os.path.join(os.path.dirname(self.rundir), "stubbin")
        os.makedirs(stub, exist_ok=True)
        cfg = os.path.join(self.home, ".config", "truesense")
        os.makedirs(cfg, exist_ok=True)
        with open(os.path.join(cfg, "ui.json"), "w") as f:
            json.dump({"onboarded": True, "width": self.size[0], "height": self.size[1]}, f)
        # the user's Qt platform theme settings, so fonts and palette are what the real window gets
        real = os.path.expanduser("~/.config/qt6ct")
        qt6ct = os.path.join(self.home, ".config", "qt6ct")
        if os.path.isdir(real) and not os.path.isdir(qt6ct):
            shutil.copytree(real, qt6ct)
            conf = os.path.join(qt6ct, "qt6ct.conf")
            with open(conf) as f:
                text = f.read().replace(real, qt6ct)
            with open(conf, "w") as f:
                f.write(text)
        if self.platform == "wayland":
            display = os.environ.get("WAYLAND_DISPLAY", "wayland-1")
            src = os.path.join(os.environ.get("XDG_RUNTIME_DIR", "/run/user/%d" % os.getuid()), display)
            dst = os.path.join(self.rundir, display)
            if not os.path.lexists(dst):
                os.symlink(src, dst)

    def env(self):
        e = {
            "HOME": self.home, "XDG_RUNTIME_DIR": self.rundir, "PATH": os.path.join(os.path.dirname(self.rundir), "stubbin"),
            "DBUS_SESSION_BUS_ADDRESS": "unix:path=/nonexistent", "TRUESENSE_SIM": "1",
            "TRUESENSE_APP_ID": self.app_id, "TRUESENSE_QT_FPS": "1", "LANG": os.environ.get("LANG", "C.UTF-8"),
            "XDG_DATA_DIRS": "/usr/share", "XDG_CONFIG_HOME": os.path.join(self.home, ".config"),
            "XDG_CONFIG_DIRS": "/etc/xdg", "QT_QPA_PLATFORMTHEME": "qt6ct",
            "QT_WAYLAND_DISABLE_WINDOWDECORATION": "1",
        }
        if self.platform == "wayland":
            e["WAYLAND_DISPLAY"] = os.environ.get("WAYLAND_DISPLAY", "wayland-1")
            e["QT_QPA_PLATFORM"] = "wayland;xcb"
        else:
            e["QT_QPA_PLATFORM"] = "offscreen"
        e.update(BACKENDS[self.backend])
        e.update(self.extra_env)
        return e

    def start(self, page="home", timeout=20):
        self._prepare()
        log = open(self.log_path, "w")
        self.proc = subprocess.Popen([self.binary, "--page", page], env=self.env(), stdout=log, stderr=subprocess.STDOUT,
                                     cwd=os.path.dirname(self.rundir))
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.proc.poll() is not None:
                raise RuntimeError(f"truesense-qt ended at start: see {self.log_path}")
            if self.platform == "offscreen" or self.find_window():
                time.sleep(1.0)
                return
            time.sleep(0.2)
        raise RuntimeError("the window never appeared")

    def find_window(self):
        for c in hypr_clients():
            if c["pid"] == self.proc.pid and c["class"] == self.app_id:
                self.address = c["address"]
                return c
        return None

    def go(self, page):
        """Shows `page` through the single-instance socket of THIS instance."""
        r = subprocess.run([self.binary, "--page", page], env=self.env(), capture_output=True, timeout=10)
        return r.returncode == 0

    def log_text(self):
        try:
            with open(self.log_path, errors="replace") as f:
                return f.read()
        except OSError:
            return ""

    def stop(self):
        if self.proc is None:
            return
        pid = self.proc.pid
        if self.platform == "wayland":
            w = self.find_window()
            if w is not None:
                hypr("dispatch", 'hl.dsp.window.close({ window = "address:%s" })' % w["address"])
        try:
            self.proc.wait(timeout=6)
        except subprocess.TimeoutExpired:
            # only our own child, and only after checking what the pid really is
            try:
                if os.path.realpath(f"/proc/{pid}/exe") == self.binary:
                    self.proc.send_signal(signal.SIGTERM)
                    self.proc.wait(timeout=5)
            except (OSError, subprocess.SubprocessError):
                pass
        self.proc = None

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.stop()


def fps_between(text_before, text_after):
    """Mean of the `REPORT {"fps": n}` lines printed between two snapshots of the log (None: no line)."""
    vals = []
    for line in text_after[len(text_before):].splitlines():
        if line.startswith("REPORT {"):
            try:
                vals.append(json.loads(line[7:])["fps"])
            except (ValueError, KeyError):
                pass
    return sum(vals) / len(vals) if vals else None


def renderer_of(text):
    """The backend the program says it chose (from its REPORT lines), '' when it does not say."""
    for line in text.splitlines():
        if line.startswith("REPORT {"):
            try:
                return json.loads(line[7:]).get("renderer", "")
            except ValueError:
                pass
    return ""


def measure(inst, page, settle, seconds):
    inst.go(page)
    time.sleep(settle)
    before = inst.log_text()
    r = procsample.sample(inst.proc.pid, seconds)
    r["fps"] = fps_between(before, inst.log_text())
    r["load1"] = os.getloadavg()[0]
    r["renderer"] = renderer_of(inst.log_text())
    return r


def cmd_profile(a):
    backends = a.backends.split(",")
    pages = PAGES if a.pages == "all" else a.pages.split(",")
    if a.platform == "wayland":
        install_rule(*map(int, a.size.split("x")))
    results = []
    size = tuple(map(int, a.size.split("x")))
    print(f"load average at start: {open('/proc/loadavg').read().split()[0]}  binary: {a.bin}")
    print(f"{'backend':<9} {'page':<12} {'cpu %':>6} {'wake/s':>7} {'fps':>6} {'rss MB':>7} {'anon':>6} {'file':>6} {'pss':>6}  renderer")
    for backend in backends:
        with Instance(a.bin, backend, a.platform, a.rundir, size, extra_env=dict(kv.split('=', 1) for kv in a.env)) as inst:
            inst.start("home")
            for page in pages:
                r = measure(inst, page, a.settle, a.seconds)
                r.update(backend=backend, page=page)
                results.append(r)
                fps = "-" if r["fps"] is None else f"{r['fps']:.1f}"
                print(f"{backend:<9} {page:<12} {r['cpu_pct']:6.1f} {r['wakeups_s']:7.0f} {fps:>6} {r['rss_mb']:7.0f} "
                      f"{r['rss_anon_mb']:6.0f} {r['rss_file_mb']:6.0f} {r['pss_mb']:6.0f}  {r['renderer']}", flush=True)
            head = inst.log_text().splitlines()[:6]
            print("  (log head: " + " | ".join(l[:100] for l in head if not l.startswith("REPORT")) + ")")
    print(f"load average at end: {open('/proc/loadavg').read().split()[0]}")
    if a.json:
        with open(a.json, "w") as f:
            json.dump(results, f, indent=1)
    if a.platform == "wayland" and any(c["class"] == APP_ID for c in hypr_clients()):
        sys.exit("a window of class %s is still open" % APP_ID)


def cmd_compare(a):
    """Measures two builds side by side, at the same time, so both see the same machine (the
    sensors the simulated hardware reads are real, and so is the load): old vs new, per page."""
    import threading
    backends = a.backends.split(",")
    pages = PAGES if a.pages == "all" else a.pages.split(",")
    size = tuple(map(int, a.size.split("x")))
    if a.platform == "wayland":
        install_rule(*size, app_id="tstest")
        install_rule(*size, app_id="tstestb", left=size[0] + 20)
    print(f"old {a.old}\nnew {a.new}\nload average at start: {open('/proc/loadavg').read().split()[0]}")
    print(f"{'backend':<9} {'page':<12} | {'old cpu':>7} {'wake/s':>7} {'fps':>6} {'pss':>5} | {'new cpu':>7} {'wake/s':>7} {'fps':>6} {'pss':>5}")
    for backend in backends:
        env = dict(kv.split("=", 1) for kv in a.env)
        with Instance(a.old, backend, a.platform, a.rundir + "/a", size, env, "tstest") as old, \
             Instance(a.new, backend, a.platform, a.rundir + "/b", size, env, "tstestb") as new:
            old.start("home")
            new.start("home")
            for page in pages:
                out = {}
                old.go(page)
                new.go(page)
                time.sleep(a.settle)
                marks = {i.proc.pid: i.log_text() for i in (old, new)}

                def run(inst, key):
                    out[key] = procsample.sample(inst.proc.pid, a.seconds)
                threads = [threading.Thread(target=run, args=(old, "old")), threading.Thread(target=run, args=(new, "new"))]
                for t in threads:
                    t.start()
                for t in threads:
                    t.join()
                for key, inst in (("old", old), ("new", new)):
                    out[key]["fps"] = fps_between(marks[inst.proc.pid], inst.log_text())
                f = lambda r: "-" if r["fps"] is None else f"{r['fps']:.1f}"
                o, n = out["old"], out["new"]
                print(f"{backend:<9} {page:<12} | {o['cpu_pct']:7.1f} {o['wakeups_s']:7.0f} {f(o):>6} {o['pss_mb']:5.0f} | "
                      f"{n['cpu_pct']:7.1f} {n['wakeups_s']:7.0f} {f(n):>6} {n['pss_mb']:5.0f}", flush=True)
    print(f"load average at end: {open('/proc/loadavg').read().split()[0]}")
    if a.platform == "wayland" and any(c["class"].startswith("tstest") for c in hypr_clients()):
        sys.exit("a test window is still open")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    for name in ("profile", "glitch"):
        p = sub.add_parser(name)
        p.add_argument("--bin", default=DEFAULT_BIN)
        p.add_argument("--backends", default="software,opengl,vulkan")
        p.add_argument("--platform", choices=["wayland", "offscreen"], default="wayland")
        p.add_argument("--size", default="760x520")
        p.add_argument("--rundir", default="/tmp/tsqt", help="short private directory for sockets and the fake HOME")
        if name == "profile":
            p.add_argument("--pages", default="all")
            p.add_argument("--settle", type=float, default=2.0)
            p.add_argument("--seconds", type=float, default=6.0)
            p.add_argument("--json")
            p.add_argument("--env", action="append", default=[], metavar="KEY=VALUE",
                           help="extra environment for the program (repeatable), e.g. QSG_RENDER_LOOP=basic")
        else:
            p.add_argument("--iterations", type=int, default=10)
            p.add_argument("--out", default="/tmp/tsqt/glitch")
            p.add_argument("--threshold", type=float, default=1.5, help="ratio of changed pixels to the tolerated amount")
            p.add_argument("--seed", type=int, default=1)
    p = sub.add_parser("compare", help="two builds side by side at the same time")
    p.add_argument("--old", required=True)
    p.add_argument("--new", default=DEFAULT_BIN)
    p.add_argument("--backends", default="software")
    p.add_argument("--platform", choices=["wayland", "offscreen"], default="offscreen")
    p.add_argument("--size", default="760x520")
    p.add_argument("--rundir", default="/tmp/tsqt")
    p.add_argument("--pages", default="all")
    p.add_argument("--settle", type=float, default=2.0)
    p.add_argument("--seconds", type=float, default=6.0)
    p.add_argument("--env", action="append", default=[], metavar="KEY=VALUE")
    a = ap.parse_args()
    if a.cmd == "profile":
        cmd_profile(a)
    elif a.cmd == "compare":
        cmd_compare(a)
    else:
        import glitch
        glitch.run(a)


if __name__ == "__main__":
    main()
