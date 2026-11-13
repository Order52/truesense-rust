#!/usr/bin/env python3
"""Looks for a stale or wrong picture of truesense-qt on the real Hyprland screen, over the WHOLE window.

    tools/qtprofile/stale.py run   [--steps 40] [--tiled] [--size 900x640] [--backend software] [--bin PATH]
    tools/qtprofile/stale.py watch [--minutes 20] [--every 20] [--class truesense]

`run` starts an own test window (class `tstests`, the same isolation as qtprofile.py: private HOME and
runtime directory, TRUESENSE_SIM=1, no D-Bus, empty PATH) whose QML is the real Main.qml wrapped by
stale-driver.qml, so the tool can act inside the window (switch pages, scroll, open drop-downs,
tooltips, dialogs, toasts, hide and show) and ask the app for its own picture (`grabToImage` of the
whole scene, popups included). After every step it pauses the live values, captures the window
from the compositor and compares every pixel with the app's own picture.

The capture is `scap ... damage` (wlr-screencopy copy_with_damage, built from scap/ on first use),
which asks for the frame as it is rather than a repainted one. grim asks for a plain copy, which a
compositor may answer by repainting the whole output first: a stale region would be "fixed" by the
very screenshot meant to find it (like zooming in and out). Whether Hyprland 0.57 repaints for a
damage capture too is not known (a new capture returns at once, marked fully damaged), so a clean
result proves the app's own picture right, not that no compositor frame was ever stale.

`watch` never touches any window: every --every seconds it captures the user's window (class
`truesense`) with copy_with_damage twice (skipping the look when the window is changing) and then
with grim, and keeps the pictures when they differ by more than a few blocks (the screen shows
something a full repaint does not: a stale compositor frame). Only that window's rectangle is captured, and only while it is on the active
workspace.

Pictures of failures are kept in --out. A Hyprland rule for the test class turns off its dimming
(decoration:dim_inactive darkens an unfocused window by 10 %, which would count as a difference).
"""
import argparse
import json
import os
import random
import shutil
import subprocess
import sys
import time

import numpy as np
from PIL import Image

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import qtprofile as q  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))
APP_ID = "tstests"


def build_scap(rundir):
    """Builds scap into `rundir` once (wayland-scanner and a C compiler are needed)."""
    out = os.path.join(rundir, "scap-bin")
    if os.path.isfile(out):
        return out
    os.makedirs(rundir, exist_ok=True)
    xml = os.path.join(HERE, "scap", "wlr-screencopy-unstable-v1.xml")
    hdr, code = os.path.join(rundir, "sc.h"), os.path.join(rundir, "sc.c")
    subprocess.run(["wayland-scanner", "client-header", xml, hdr], check=True)
    subprocess.run(["wayland-scanner", "private-code", xml, code], check=True)
    subprocess.run(["cc", "-O1", "-I", rundir, "-o", out, os.path.join(HERE, "scap", "scap.c"), code, "-lwayland-client"],
                   check=True)
    return out


def capture(scap, rect, path, damage=True):
    x, y, w, h = rect
    r = subprocess.run([scap, str(x), str(y), str(w), str(h), path] + (["damage"] if damage else []),
                       capture_output=True, timeout=15)
    if r.returncode != 0:
        raise RuntimeError("scap: " + r.stderr.decode(errors="replace"))


def diff(a_path, b_path, corner=26, threshold=24):
    """{'frac': changed pixels, 'blocks': 16x16 blocks more than a quarter changed, 'bbox': of those}.
    The rounded corners and a 3 px frame (the compositor's border) are not compared."""
    a = Image.open(a_path).convert("RGB")
    b = Image.open(b_path).convert("RGB")
    if a.size != b.size:
        return {"size": [list(a.size), list(b.size)], "blocks": 99}
    d = np.abs(np.asarray(a, dtype=np.int16) - np.asarray(b, dtype=np.int16)).max(axis=2) > threshold
    h, w = d.shape
    for (cx, cy) in ((0, 0), (w - corner, 0), (0, h - corner), (w - corner, h - corner)):
        d[max(cy, 0):cy + corner, max(cx, 0):cx + corner] = False
    d[:3, :] = d[-3:, :] = False
    d[:, :3] = d[:, -3:] = False
    bad = [(bx, by) for by in range(0, h, 16) for bx in range(0, w, 16) if d[by:by + 16, bx:bx + 16].mean() > 0.25]
    bbox = None
    if bad:
        bbox = (min(p[0] for p in bad), min(p[1] for p in bad), max(p[0] for p in bad) + 16, max(p[1] for p in bad) + 16)
    return {"frac": round(float(d.mean()), 5), "blocks": len(bad), "bbox": bbox}


class Driven:
    """An own test window whose QML is stale-driver.qml (see there)."""

    def __init__(self, a):
        self.dir = os.path.join(a.rundir, "driver")
        os.makedirs(self.dir, exist_ok=True)
        shutil.copy(os.path.join(HERE, "stale-driver.qml"), os.path.join(self.dir, "Driver.qml"))
        self.cmd_path = os.path.join(self.dir, "cmd.js")
        with open(self.cmd_path, "w") as f:
            f.write("0\n")
        self.seq = 0
        self.net_dx = 0
        w, h = map(int, a.size.split("x"))
        placing = "" if a.tiled else (
            'float = true, size = { %d, %d }, move = { "monitor_w-%d", "monitor_h-%d" }, ' % (w, h, w + 20, h + 60))
        q.hypr("eval", 'hl.window_rule({ name = "%s-rule", match = { class = "^%s$" }, %sno_initial_focus = true, '
                       'no_dim = true })' % (APP_ID, APP_ID, placing))
        errors = q.hypr("configerrors").strip()
        if errors:
            sys.exit("hyprland rejected the window rule: " + errors)
        env = {"TRUESENSE_QT_QML_URL": "file://" + os.path.join(self.dir, "Driver.qml")}
        self.inst = q.Instance(a.bin, a.backend, "wayland", a.rundir, (w, h), extra_env=env, app_id=APP_ID)
        self.inst.start("home")

    def window(self):
        c = self.inst.find_window()
        if c is None:
            raise RuntimeError("the test window is gone")
        return c

    def rect(self):
        c = self.window()
        return (c["at"][0], c["at"][1], c["size"][0], c["size"][1])

    def js(self, code, timeout=8):
        """Runs `code` in the window (it ends by calling done(result)); returns the result."""
        self.seq += 1
        before = len(self.inst.log_text())
        with open(self.cmd_path + ".tmp", "w") as f:
            f.write("%d\n%s\n" % (self.seq, code))
        os.replace(self.cmd_path + ".tmp", self.cmd_path)
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            for line in self.inst.log_text()[before:].splitlines():
                if line.startswith("REPORT {"):
                    d = json.loads(line[7:])
                    if d.get("seq") == self.seq:
                        if "error" in d:
                            raise RuntimeError(d["error"])
                        return d.get("result")
            time.sleep(0.05)
        raise RuntimeError("no answer to: " + code[:80])

    def grab(self, path):
        """The app's own picture of its whole scene (popups included)."""
        return self.js("w.contentItem.parent.parent.grabToImage(function (g) { done(g.saveToFile(%s)) })" % json.dumps(path))

    def stop(self):
        # A tiled resize moves the split for the neighbours too, and the split outlives the window:
        # put it back where it was before closing.
        c = self.inst.find_window()
        if c is not None and self.net_dx:
            q.hypr("dispatch", 'hl.dsp.window.resize({ x = %d, y = 0, relative = true, window = "address:%s" })'
                   % (-self.net_dx, c["address"]))
            time.sleep(0.3)
        self.inst.stop()


FIND = r'''
function find(pred) { var out = []; function walk(it) { if (!it) return; try { if (pred(it)) out.push(it) } catch (e) {}
  var c = it.children; for (var i = 0; i < c.length; i++) walk(c[i]) } walk(w.contentItem.parent); return out }
function flick() { var f = find(function (i) { return i.flickableDirection !== undefined && i.visible && i.height > 200 }); return f.length ? f[f.length - 1] : null }
function popups() { var res = []; find(function (i) { var d = i.data; for (var k = 0; k < d.length; k++) { var o = d[k];
  if (o && o.open !== undefined && o.close !== undefined && o.opened !== undefined) res.push(o) } return false }); return res }
'''
PAGES = ["home", "monitor", "performance", "cooling", "lighting", "keys", "battery", "audio", "profiles", "health", "settings"]


def step(d, kind, rng):
    if kind == "page":
        d.js("Nav.go(%s); done(1)" % json.dumps(rng.choice(PAGES)))
    elif kind == "fling":
        d.js(FIND + "var f = flick(); if (f) f.flick(0, %d); done(1)" % rng.choice([-4000, 4000, -1500, 2500]))
        for _ in range(60):  # a fling coasts for a while: compare the picture it comes to rest at
            if not d.js(FIND + "var f = flick(); done(f ? f.moving : false)"):
                break
            time.sleep(0.1)
    elif kind == "scroll":
        d.js(FIND + "var f = flick(); if (f) f.contentY = Math.max(0, (f.contentHeight - f.height) * %f); done(1)" % rng.random())
    elif kind == "dropdown":
        d.js(FIND + "var c = find(function (i) { return i.popup !== undefined && i.currentIndex !== undefined && i.visible });"
             " if (c.length) c[%d %% c.length].popup.open(); done(c.length)" % rng.randrange(10))
    elif kind == "popup":
        d.js(FIND + "var p = popups(); if (p.length) p[%d %% p.length].open(); done(p.length)" % rng.randrange(10))
    elif kind == "close":
        d.js(FIND + "var p = popups(); for (var k = 0; k < p.length; k++) p[k].close();"
             " var c = find(function (i) { return i.popup !== undefined && i.popup && i.popup.opened });"
             " for (var j = 0; j < c.length; j++) c[j].popup.close(); done(1)")
    elif kind == "tooltip":
        d.js(FIND + "var b = find(function (i) { return i.hovered !== undefined && i.visible && i.width > 20 });"
             " if (b.length) b[%d %% b.length].ToolTip.show('A tooltip', 1500); done(b.length)" % rng.randrange(50))
    elif kind == "toast":
        d.js("Toasts.show('A toast that says something', 'ok'); done(1)")
    elif kind == "hide":
        d.js("w.hide(); done(1)")
        time.sleep(rng.choice([0.3, 1.5]))
        d.js("w.showPage(''); done(1)")
    elif kind == "live":
        d.js("Bus.setActive(true); done(1)")
        time.sleep(rng.choice([1.2, 3.0]))
    elif kind == "resize":
        c = d.window()
        if d.tiled:
            dx = rng.choice([-200, 200, -100, 120])
            q.hypr("dispatch", 'hl.dsp.window.resize({ x = %d, y = 0, relative = true, window = "address:%s" })'
                   % (dx, c["address"]))
            d.net_dx += dx
        else:
            for _ in range(rng.choice([1, 4])):
                w, h = rng.choice([(900, 640), (760, 520), (600, 480), (480, 640), (1000, 700), (700, 560)])
                q.hypr("dispatch", 'hl.dsp.window.resize({ x = %d, y = %d, window = "address:%s" })' % (w, h, c["address"]))
                time.sleep(0.03)
            time.sleep(0.1)
            c = d.window()
            mw, mh = 2560, 1600
            try:
                m = json.loads(q.hypr("monitors", "-j"))[0]
                mw, mh = m["width"], m["height"]
            except (ValueError, IndexError, KeyError):
                pass
            q.hypr("dispatch", 'hl.dsp.window.move({ x = %d, y = %d, window = "address:%s" })'
                   % (mw - c["size"][0] - 20, mh - c["size"][1] - 60, c["address"]))
    elif kind == "away":
        c = d.window()
        home = str(c["workspace"]["id"])
        q.hypr("dispatch", 'hl.dsp.window.move({ workspace = "9", follow = false, window = "address:%s" })' % c["address"])
        time.sleep(rng.choice([0.3, 2.5]))
        q.hypr("dispatch", 'hl.dsp.window.move({ workspace = "%s", follow = false, window = "address:%s" })' % (home, c["address"]))
        time.sleep(0.8)


def cmd_run(a):
    scap = build_scap(a.rundir)
    os.makedirs(a.out, exist_ok=True)
    rng = random.Random(a.seed)
    kinds = (["page"] * 4 + ["fling", "fling", "scroll", "dropdown", "popup", "close", "close", "tooltip", "toast",
                             "hide", "live", "live", "resize", "resize", "away"])
    d = Driven(a)
    d.tiled = a.tiled
    bad = 0
    try:
        for n in range(a.steps):
            kind = rng.choice(kinds)
            try:
                step(d, kind, rng)
            except RuntimeError as e:
                print(f"{n:3d} {kind:<9} skipped: {e}")
                continue
            time.sleep(0.4)
            d.js("Bus.setActive(false); done(1)")  # the live values stop, the picture stands still
            time.sleep(0.5)
            screen = os.path.join(a.out, f"{n:03d}-screen.ppm")
            own = os.path.join(a.out, f"{n:03d}-app.png")
            capture(scap, d.rect(), screen)
            d.grab(own)
            r = diff(own, screen)
            wrong = r["blocks"] >= 2
            if wrong:
                time.sleep(1.0)
                later = os.path.join(a.out, f"{n:03d}-screen-later.ppm")
                capture(scap, d.rect(), later)
                r["later"] = diff(own, later)
                bad += 1
            else:
                os.unlink(screen)
                os.unlink(own)
            print(f"{n:3d} {kind:<9} {'WRONG' if wrong else 'ok':<5} {r}", flush=True)
    finally:
        d.stop()
    print(f"{a.steps} steps, {bad} with a picture on screen that is not the app's")
    return 1 if bad else 0


def cmd_watch(a):
    scap = build_scap(a.rundir)
    os.makedirs(a.out, exist_ok=True)
    end = time.time() + a.minutes * 60
    n = found = 0
    while time.time() < end:
        mine = [c for c in q.hypr_clients() if c["class"] == a.window_class]
        try:
            active = json.loads(q.hypr("activeworkspace", "-j"))["id"]
        except (ValueError, KeyError):
            active = None
        if mine and mine[0]["workspace"]["id"] == active and not mine[0].get("hidden"):
            c = mine[0]
            rect = (c["at"][0], c["at"][1], c["size"][0], c["size"][1])
            shown = os.path.join(a.out, f"watch-{n:04d}-screen.ppm")
            fresh = os.path.join(a.out, f"watch-{n:04d}-repainted.png")
            # Two looks at the screen first: when they differ, the window is changing right now
            # (scrolling, a page switch) and a third picture would not tell stale from new.
            capture(scap, rect, shown)
            time.sleep(0.3)
            capture(scap, rect, shown + ".again.ppm")
            moving = diff(shown, shown + ".again.ppm")["blocks"] >= 4
            os.replace(shown + ".again.ppm", shown)
            if moving:
                os.unlink(shown)
                time.sleep(a.every)
                continue
            x, y, w, h = rect
            subprocess.run(["grim", "-g", f"{x},{y} {w}x{h}", fresh], capture_output=True, timeout=20)
            r = diff(fresh, shown)
            print(time.strftime("%H:%M:%S"), n, r, flush=True)
            if r["blocks"] >= 4:
                found += 1
            else:
                os.unlink(shown)
                os.unlink(fresh)
            n += 1
        time.sleep(a.every)
    print(f"{n} looks, {found} with a stale frame on screen")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("run")
    p.add_argument("--bin", default=q.DEFAULT_BIN)
    p.add_argument("--backend", default="software", choices=sorted(q.BACKENDS))
    p.add_argument("--size", default="900x640", help="floating size (a tiled window gets its tile)")
    p.add_argument("--tiled", action="store_true", help="tiled on the active workspace, next to the windows there")
    p.add_argument("--steps", type=int, default=40)
    p.add_argument("--seed", type=int, default=7)
    p.add_argument("--rundir", default="/tmp/tsqt-stale")
    p.add_argument("--out", default="/tmp/tsqt-stale/out")
    p = sub.add_parser("watch")
    p.add_argument("--minutes", type=float, default=20)
    p.add_argument("--every", type=float, default=20)
    p.add_argument("--class", dest="window_class", default="truesense")
    p.add_argument("--rundir", default="/tmp/tsqt-stale")
    p.add_argument("--out", default="/tmp/tsqt-stale/out")
    a = ap.parse_args()
    if a.cmd == "run":
        sys.exit(cmd_run(a))
    cmd_watch(a)


if __name__ == "__main__":
    main()
