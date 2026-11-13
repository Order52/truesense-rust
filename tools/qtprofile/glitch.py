"""Glitch detector for truesense-qt on a real Wayland compositor (used by qtprofile.py glitch).

It starts the program as its own floating test window (see qtprofile.py for the safety rules),
then repeats random steps: resize the window (single and in quick bursts, like a drag in a tiling
layout), move it, switch the page through the single-instance socket, or several at once. After
each step it waits for the frames to settle, takes a screenshot of the window's rectangle from the
compositor (`grim -g`) and compares the regions of the picture that never change with the
picture the program itself renders offscreen at the same size and page (TRUESENSE_QT_SHOT): the
sidebar (logo, navigation), the header of a narrow window, the page title and the status text.
Stale damage (an old sidebar after a resize), black or transparent patches, a wrong-sized
buffer or half-drawn frames show up as large differences there; the font rendering of two
platforms plugins never differs by more than a few per cent of the pixels.

A step is a *transient* glitch when the picture is wrong shortly after the step (0.35 s) and
right a second later, *persistent* when it is still wrong then (what a person has to fix by
resizing again, "zooming"). Failing screenshots are kept in --out.

Limits: only the static regions are compared, and grim's capture may make the compositor repaint
the whole output first, which would hide a stale compositor frame. stale.py compares every pixel
of the window and asks for a capture without a repaint (copy_with_damage).
"""
import json
import os
import random
import shutil
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor

import numpy as np
from PIL import Image, ImageFilter

import qtprofile as q

PAGES = ["home", "monitor", "performance", "cooling", "lighting", "settings"]
# widths: below 520 the menu button layout, 520..720 the icon rail, from 720 the full sidebar
SIZES = [(760, 520), (600, 480), (480, 640), (900, 700), (700, 560), (560, 520), (420, 600), (840, 480)]
CORNER = 26  # the compositor rounds the window's corners (Hyprland `rounding`): not compared


def reference_env(home, rundir):
    return {
        "HOME": home, "XDG_RUNTIME_DIR": rundir, "PATH": "/nonexistent", "TRUESENSE_SIM": "1",
        "DBUS_SESSION_BUS_ADDRESS": "unix:path=/nonexistent", "TRUESENSE_APP_ID": "tstestshot",
        "QT_QPA_PLATFORM": "offscreen", "QT_QPA_PLATFORMTHEME": "qt6ct", "QT_QUICK_BACKEND": "software",
        "LANG": os.environ.get("LANG", "C.UTF-8"), "XDG_CONFIG_HOME": os.path.join(home, ".config"),
        "TRUESENSE_QT_TEST": "1", "TRUESENSE_QT_EXIT_AFTER_MS": "2500", "XDG_DATA_DIRS": "/usr/share",
    }


def render_reference(binary, page, w, h, out, base):
    """The program's own offscreen picture of `page` at w x h (the same fonts and palette)."""
    home = os.path.join(base, f"refhome-{page}-{w}x{h}")
    run = os.path.join(base, f"refrun-{page}-{w}x{h}")
    for d in (os.path.join(home, ".config", "truesense"), run):
        os.makedirs(d, exist_ok=True)
    os.chmod(run, 0o700)
    with open(os.path.join(home, ".config", "truesense", "ui.json"), "w") as f:
        json.dump({"onboarded": True, "width": w, "height": h, "theme": "dark"}, f)
    qt6ct = os.path.expanduser("~/.config/qt6ct")
    if os.path.isdir(qt6ct) and not os.path.isdir(os.path.join(home, ".config", "qt6ct")):
        shutil.copytree(qt6ct, os.path.join(home, ".config", "qt6ct"))
    env = reference_env(home, run)
    env["TRUESENSE_QT_SHOT"] = out
    subprocess.run([binary, "--page", page], env=env, capture_output=True, timeout=60)
    shutil.rmtree(home, ignore_errors=True)
    shutil.rmtree(run, ignore_errors=True)
    return os.path.isfile(out)


def static_regions(w, h):
    """(x, y, w, h, tolerated fraction) of the places that look the same in every frame of a page
    at a size. The sidebar is the strict one; text-only regions tolerate more because the GPU
    backends place glyphs a fraction of a pixel differently from the software reference."""
    regions = []
    if w >= 520:
        side = 256 if w >= 720 else 64
        regions.append((0, 0, side, h - 31, 0.03))                          # the whole sidebar
        regions.append((side + 24, 22, min(300, w - side - 60), 36, 0.2))   # the page title
        regions.append((side + 12, h - 28, min(220, w - side - 24), 26, 0.12))  # the service status text
    else:
        regions.append((0, 0, w, 49, 0.10))                                 # the header with the page title
        regions.append((12, h - 28, min(220, w - 24), 26, 0.12))
    return regions


def _blurred(img):
    # Heavy enough that a glyph moved by half a pixel does not count, light enough that a missing
    # sidebar, a shifted highlight or a black patch does.
    return np.asarray(img.convert("RGB").filter(ImageFilter.BoxBlur(4)), dtype=np.int16)


def compare(shot, ref, w, h):
    """How wrong the static regions are: (worst ratio of a region's changed pixels to what it may
    change, number of 32x32 blocks that are more than half changed, [(rect, fraction)]).
    A ratio above 1 is a glitch."""
    if shot.size != ref.size:
        return 99.0, 99, [("size %sx%s vs %sx%s" % (shot.size + ref.size), 1.0)]
    a, b = _blurred(shot), _blurred(ref)
    diff = (np.abs(a - b).max(axis=2) > 12)
    # the corners the compositor rounds off
    for (cx, cy) in ((0, 0), (w - CORNER, 0), (0, h - CORNER), (w - CORNER, h - CORNER)):
        diff[max(cy, 0):cy + CORNER, max(cx, 0):cx + CORNER] = False
    fractions = []
    ratio = 0.0
    bad_blocks = 0
    for (x, y, rw, rh, tolerated) in static_regions(w, h):
        x2, y2 = min(w, x + rw), min(h, y + rh)
        if x2 <= x or y2 <= y:
            continue
        region = diff[y:y2, x:x2]
        fraction = float(region.mean())
        fractions.append(((x, y, x2 - x, y2 - y), fraction))
        ratio = max(ratio, fraction / tolerated)
        for by in range(0, region.shape[0], 32):
            for bx in range(0, region.shape[1], 32):
                blk = region[by:by + 32, bx:bx + 32]
                if blk.size >= 256 and blk.mean() > 0.5:
                    bad_blocks += 1
    return ratio, bad_blocks, fractions


def grim(rect, path):
    x, y, w, h = rect
    r = subprocess.run(["grim", "-g", f"{x},{y} {w}x{h}", path], capture_output=True, timeout=20)
    return r.returncode == 0 and os.path.isfile(path)


class Stepper:
    def __init__(self, inst, rng):
        self.inst = inst
        self.rng = rng
        self.page = "home"
        self.addr = None

    def window(self):
        w = self.inst.find_window()
        if w is None:
            raise RuntimeError("the test window is gone")
        self.addr = w["address"]
        return w

    def resize(self, w, h):
        q.hypr("dispatch", 'hl.dsp.window.resize({ x = %d, y = %d, window = "address:%s" })' % (w, h, self.addr))

    def move(self, x, y):
        q.hypr("dispatch", 'hl.dsp.window.move({ x = %d, y = %d, window = "address:%s" })' % (x, y, self.addr))

    def monitor_size(self):
        try:
            m = json.loads(q.hypr("monitors", "-j"))[0]
            return m["width"], m["height"]
        except (ValueError, IndexError, KeyError):
            return 2560, 1600

    def keep_on_screen(self, dx=20, dy=40):
        """A window that grew must not hang over the edge of the screen (the screenshot would
        contain nothing there): put its bottom-right corner back near the corner."""
        w = self.window()
        mw, mh = self.monitor_size()
        self.move(max(0, mw - w["size"][0] - dx), max(60, mh - w["size"][1] - dy))

    def step(self):
        """One random step; returns its description."""
        r = self.rng
        kind = r.choice(["resize", "resize", "burst", "page", "page", "move", "resize+page"])
        desc = kind
        if kind in ("resize", "resize+page"):
            w, h = r.choice(SIZES)
            self.resize(w, h)
            desc += f" {w}x{h}"
        if kind == "burst":
            sizes = [r.choice(SIZES) for _ in range(4)]
            for (w, h) in sizes:
                self.resize(w, h)
                time.sleep(0.02)
            desc += " " + ",".join(f"{w}x{h}" for w, h in sizes)
        if kind in ("page", "resize+page"):
            self.page = r.choice([p for p in PAGES if p != self.page])
            self.inst.go(self.page)
            desc += f" page={self.page}"
        if kind == "move":
            dx, dy = r.choice([(20, 40), (300, 200), (150, 120), (40, 300)])
            self.keep_on_screen(dx, dy)
            desc += f" corner offset {dx},{dy}"
        else:
            time.sleep(0.12)
            self.keep_on_screen()
        return desc


def run(a):
    sizes_w, sizes_h = map(int, a.size.split("x"))
    base = a.out
    os.makedirs(base, exist_ok=True)
    q.install_rule(sizes_w, sizes_h)
    binary = os.path.realpath(a.bin)
    refs = {}

    def ref_path(page, w, h):
        return os.path.join(base, f"ref-{page}-{w}x{h}.png")

    # references first, in parallel and offscreen (no screen time)
    jobs = [(p, w, h) for p in PAGES for (w, h) in SIZES if not os.path.isfile(ref_path(p, w, h))]
    if jobs:
        print(f"rendering {len(jobs)} reference pictures offscreen ...", flush=True)
        with ThreadPoolExecutor(max_workers=4) as pool:
            list(pool.map(lambda j: render_reference(binary, j[0], j[1], j[2], ref_path(*j), base), jobs))
    summary = []
    for backend in a.backends.split(","):
        rng = random.Random(a.seed)
        stats = {"backend": backend, "steps": 0, "transient": 0, "persistent": 0, "worst": 0.0, "notes": [],
                 "baseline_worst": 0.0}
        with q.Instance(binary, backend, "wayland", a.rundir, (sizes_w, sizes_h)) as inst:
            inst.start("home")
            st = Stepper(inst, rng)
            st.window()
            # calibration: the freshly started window against its reference (what "clean" scores)
            time.sleep(0.8)
            win = st.window()
            shot = os.path.join(base, f"{backend}-calib.png")
            w, h = win["size"]
            if grim((win["at"][0], win["at"][1], w, h), shot):
                ref = ref_path("home", w, h)
                if not os.path.isfile(ref):
                    render_reference(binary, "home", w, h, ref, base)
                s, blocks, _ = compare(Image.open(shot), Image.open(ref), w, h)
                stats["baseline_worst"] = s
                # negative control: the same picture against another page's reference must be
                # flagged, or the detector is blind
                other = ref_path("settings", w, h)
                if not os.path.isfile(other):
                    render_reference(binary, "settings", w, h, other, base)
                cs, cb, _ = compare(Image.open(shot), Image.open(other), w, h)
                stats["control"] = cs
                print(f"[{backend}] clean-start score {s:.4f} blocks {blocks}; "
                      f"control (wrong page) {cs:.4f} blocks {cb}", flush=True)
                if not (cs > a.threshold or cb >= 2):
                    print("    WARNING: the control was not detected; lower --threshold", flush=True)
            for n in range(a.iterations):
                desc = st.step()
                time.sleep(0.35)
                result = []
                for phase, delay in (("early", 0.0), ("late", 1.0)):
                    if delay:
                        time.sleep(delay)
                    win = st.window()
                    w, h = win["size"]
                    x, y = win["at"]
                    path = os.path.join(base, f"{backend}-{n:03d}-{phase}.png")
                    if not grim((x, y, w, h), path):
                        result.append((None, 0, [], path))
                        continue
                    ref = ref_path(st.page, w, h)
                    if not os.path.isfile(ref):
                        render_reference(binary, st.page, w, h, ref, base)
                    score, blocks, fr = compare(Image.open(path), Image.open(ref), w, h)
                    result.append((score, blocks, fr, path))
                    if phase == "early" and not (score > a.threshold or blocks >= 2):
                        break  # fine at once: the late look is not needed
                stats["steps"] += 1
                early = result[0]
                bad_early = early[0] is not None and (early[0] > a.threshold or early[1] >= 2)
                bad_late = len(result) > 1 and result[1][0] is not None and (result[1][0] > a.threshold or result[1][1] >= 2)
                stats["worst"] = max(stats["worst"], early[0] or 0.0, (result[1][0] or 0.0) if len(result) > 1 else 0.0)
                keep = False
                if bad_late:
                    stats["persistent"] += 1
                    keep = True
                elif bad_early:
                    stats["transient"] += 1
                    keep = True
                if keep and len(stats["notes"]) < 8:
                    stats["notes"].append(f"#{n} {desc}: early {early[0]:.3f}/{early[1]}"
                                          + (f" late {result[1][0]:.3f}/{result[1][1]}" if len(result) > 1 else "")
                                          + f"  {os.path.basename(early[3])}")
                if not keep:
                    for r in result:
                        try:
                            os.unlink(r[3])
                        except OSError:
                            pass
        summary.append(stats)
        print(f"[{backend}] steps {stats['steps']}  transient {stats['transient']}  persistent {stats['persistent']}  "
              f"worst score {stats['worst']:.3f}  clean-start {stats['baseline_worst']:.4f}", flush=True)
        for note in stats["notes"]:
            print("    " + note)
    with open(os.path.join(base, "glitch.json"), "w") as f:
        json.dump(summary, f, indent=1)
    if any(c["class"] == q.APP_ID for c in q.hypr_clients()):
        sys.exit("a window of class %s is still open" % q.APP_ID)
