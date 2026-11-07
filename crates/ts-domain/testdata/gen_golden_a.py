# SPDX-License-Identifier: GPL-3.0-only
"""Regenerates the golden files of the keyboard, keymap, designs and perkeyfx modules from the Python reference.

Run ONLY like this (the simulator variables keep it away from the real machine; the keyboard is a fake device):

    TRUESENSE_SIM=1 TRUESENSE_SOCKET=/nonexistent/x TRUESENSE_USERSTATE=/tmp/<unique>/user.json \
    PYTHONPATH=<python edition>/src python3 testdata/gen_golden_a.py testdata

The Rust tests (crates/ts-domain) compare their output with these files byte for byte.
"""
import json
import os
import sys
import tempfile

assert os.environ.get("TRUESENSE_SIM") and os.environ.get("TRUESENSE_USERSTATE") and os.environ.get("TRUESENSE_SOCKET"), "run with the simulator variables"

from truesense import designs, keyboard as k, keymap, keyprofiles, looks, perkeyfx, userstate  # noqa: E402
from truesense.device import HwError  # noqa: E402

OUT = sys.argv[1] if len(sys.argv) > 1 else "."


def dump(name, obj):
    with open(os.path.join(OUT, name), "w") as f:
        json.dump(obj, f, separators=(",", ":"))
        f.write("\n")


def hexc(c): return "%02x%02x%02x" % tuple(c)


# Floats are written as text (their repr) because the JSON reader of the Rust tests does not round them exactly.
# ---- keymap -------------------------------------------------------------------------------------------------------------------------------
def gen_keymap():
    probes = ["w", "W", "esc", "caps", "minus", "enter", "return", "win", "windows", "arrows", "ALL", "numpad", "cell0", "cell127", "CELL5", "cell128", "cell",
              "cell0000000000000000000000003", "nosuchkey", "", "wasd", "space", "predator", "=", "num.", "numenter", "f12", "f13"]
    cells_for = {}
    for p in probes:
        try: cells_for[p] = keymap.cells_for(p)
        except HwError as e: cells_for[p] = "ERR: " + str(e)
    colours = {}
    for c in ["ff0000", "#00ff00", "##0000FF", "12345", "1234567", "gg0000", "", "#", " ff0000", "ff0000\n", "AbCdEf"]:
        try: colours[c] = list(keymap.hexcolor(c))
        except HwError as e: colours[c] = "ERR: " + str(e)
    hs = [i / 97.0 - 1.2 for i in range(0, 300)] + [0.0, 1.0, 2.0, -1e-20, 0.5, 1 / 6, 1 / 3, 2 / 3, 0.9999999999]
    lerp = [[list(a), list(b), repr(t), list(keymap.lerp(a, b, t))] for a, b, t in
            [((0, 0, 0), (1, 1, 1), 0.5), ((0, 0, 0), (5, 5, 5), 0.5), ((10, 20, 30), (255, 0, 7), 0.25), ((255, 255, 255), (0, 0, 0), 1 / 21), ((1, 2, 3), (4, 5, 6), 0.0), ((9, 9, 9), (0, 0, 0), 0.5)]]
    dump("keymap_golden.json", {
        "names": [[n, c] for n, c in keymap.NAMES.items()],
        "groups": {g: c for g, c in keymap.GROUPS.items()},
        "cells_for": cells_for, "hexcolor": colours,
        "hsv": [[repr(h), list(keymap.hsv(h))] for h in hs], "lerp": lerp,
        "gradient": [list(c) for c in keymap.to_list(keymap.gradient((255, 51, 0), (102, 0, 255)))],
        "gradient2": [list(c) for c in keymap.to_list(keymap.gradient((1, 2, 3), (250, 251, 252)))],
        "rainbow": [list(c) for c in keymap.to_list(keymap.rainbow())],
        "canvas": [list(c) for c in keymap.to_list(keymap.canvas((7, 8, 9)))],
    })


# ---- perkeyfx -----------------------------------------------------------------------------------------------------------------------------
def gen_perkeyfx():
    cyan = ["00aec7"] * 126 + ["000000"] * 2
    mixed = []
    for i in range(128): mixed.append("%02x%02x%02x" % ((i * 37) % 256, (i * 91 + 13) % 256, (i * 53 + 200) % 256) if i % 5 else "000000")
    bases = {"cyan": cyan, "mixed": mixed}
    frames = []

    def add(mode, b, t, speed, keys):
        cells = perkeyfx.render(mode, t, bases[b], keys, speed)
        frames.append({"mode": mode, "base": b, "t": repr(t), "speed": speed, "keys": keys, "cells": "".join(hexc(c) for c in cells)})
    for mode in perkeyfx.MODES:
        for b in bases:
            for t in (-2.3, 0.37, 123.456):
                for speed in (1, 5, 10):
                    if speed == 5 and t != 0.37: continue
                    add(mode, b, t, speed, None)
        add(mode, "mixed", 7.77, 5, [21, 20, 26, 32, 0, 125])
    add("off", "cyan", 1.0, 5, None)
    add("disco", "mixed", 1.0, 5, [3, 4])
    add("pulse", "cyan", 1.0, 0, None)          # speed clamps to 1
    add("pulse", "cyan", 1.0, 99, None)         # and to 10
    add("wave", "cyan", 1.0, -4, [1])
    dump("perkeyfx_golden.json", {"modes": list(perkeyfx.MODES.items()), "bases": bases, "frames": frames,
                                  "hash": [[i, kk, z, repr(perkeyfx._h(i, kk, z))] for i in (0, 1, 7, 125, 127) for kk in (-3, -1, 0, 1, 5, 1234567) for z in (0, 7, 12, 13)]})


# ---- designs ------------------------------------------------------------------------------------------------------------------------------
def gen_designs():
    out = {}
    saved = os.environ.get("XDG_CONFIG_HOME")
    with tempfile.TemporaryDirectory() as d:
        os.environ["XDG_CONFIG_HOME"] = d
        for n, des in designs.load_all().items():
            out[n] = {"design": des, "cells": [hexc(c) for c in designs.cells_of(des)]}
        # a user file with overlapping keys (order decides), a numeric cell, odd info types, dropped and kept bad files
        os.makedirs(designs.user_dir())
        files = {"overlap": {"info": "x" * 100, "base": "#112233", "keys": {"arrows": "ff0000", "up": "00ff00", "wasd": "0000ff", "w": "ffffff"}, "cells": {"3": "010203", "03": "040506", "125": "070809"}},
                 "minimal": {}, "infonum": {"info": 5, "base": "000001"}, "bad1": {"cells": {"128": "ffffff"}}, "bad2": {"cells": {"x": "ffffff"}}, "bad3": {"base": "12"}, "bad4": {"keys": {"w": "zz"}}}
        for n, v in files.items():
            with open(os.path.join(designs.user_dir(), n + ".json"), "w") as f: json.dump(v, f)
        for n, des in designs.load_all().items():
            if n in files: out["user:" + n] = {"design": des, "cells": [hexc(c) for c in designs.cells_of(des)]}
        out["user:_names"] = sorted(n for n in designs.load_all() if n in files)
        p = designs.save("mine", [(i, 2, 3) for i in range(128)])
        with open(p) as f: out["saved:mine"] = f.read()
        p = designs.save("short", [(9, 9, 9)] * 3)
        with open(p) as f: out["saved:short"] = f.read()
        errs = {}
        for bad in ("", "../x", "a/b", "a.b", "a\\b", "x" * 41):
            try: designs.save(bad, [])
            except HwError as e: errs[bad] = str(e)
        out["save_errors"] = errs
        try: designs.get("nosuch")
        except HwError as e: out["get_error"] = str(e)
    if saved is None: os.environ.pop("XDG_CONFIG_HOME", None)
    else: os.environ["XDG_CONFIG_HOME"] = saved
    dump("designs_golden.json", out)


def pixel(x, y, c): return (x * 37 + y * 91 + c * 53 + (x * y) % 17 * 11) % 256


def gen_images():
    from PIL import Image
    out = {}
    for name, (w, h) in {"exact22x6": (22, 6), "grad64x40": (64, 40), "small10x3": (10, 3), "wide22x40": (22, 40), "noise100x37": (100, 37), "gray30x30": (30, 30),
                         "tall60x6": (60, 6), "solid40x12": (40, 12), "one1x1": (1, 1), "huge1000x500": (1000, 500),
                         # Pillow 13 chooses the order of the two passes by the shape of the picture: these sit on both sides of the line
                         **{"order%dx%d" % wh: wh for wh in [(23, 7), (23, 9), (24, 9), (30, 12), (30, 30), (34, 30), (44, 60), (50, 60), (64, 100), (80, 100), (100, 200), (150, 200), (10, 3), (21, 5), (21, 7), (5, 200), (200, 5), (22, 7), (23, 6)]}}.items():
        data = bytes(pixel(x, y, c) for y in range(h) for x in range(w) for c in range(3))
        im = Image.frombytes("RGB", (w, h), data).resize((22, 6), Image.LANCZOS)
        cv = [(0, 0, 0)] * 128
        for col in range(22):
            for row in range(6):
                if col * 6 + row < 126: cv[col * 6 + row] = im.getpixel((col, row))
        out[name] = {"w": w, "h": h, "cells": [hexc(c) for c in cv]}
    dump("image_golden.json", out)


# ---- keyboard: the transcript of every HID transfer, against a fake device ----------------------------------------------------------------
STATE0 = {"keyboard": {"mode": "effect", "name": "ripple", "color": "ff8800", "speed": 7, "brightness": 20, "direction": 1}}
CELLS_A = ["%02x%02x%02x" % (i, 255 - i, (i * 3) % 256) for i in range(128)]
SCENARIOS = [
    {"name": "effects", "steps": [["set_effect", n, "00aec7", 5, 30, 0] for n in k.EFFECTS] + [["set_effect", "snake", "AABBCC", 12, 50, 3], ["set_effect", "static", "ffffff", 1, 0, 0], ["set_effect", "wave", "zz", 3, 10, 2]]},
    {"name": "effect_errors", "steps": [["set_effect", "nonsense", "ffffff", 5, 30, 0], ["set_effect", "wave", "ffffff", 0, 30, 0], ["set_effect", "wave", "ffffff", 13, 30, 0], ["set_effect", "wave", "ffffff", 5, 51, 0],
                                        ["set_effect", "wave", "ffffff", 5, -1, 0], ["set_effect", "operatingmode", "00ff00", 5, 30, 0], ["set_effect", "operatingmode", "zz", 5, 30, 0]]},
    {"name": "speed_scale", "steps": [["set_effect", "wave", "00aec7", s, 25, 0] for s in range(1, 13)]},
    {"name": "operating", "steps": [["set_operating_mode", "00aec7"], ["set_operating_mode", "#FF0000", 20], ["set_operating_mode", "zz"], ["follow_mode", "eco"], ["follow_mode", "turbo"], ["follow_mode", "bogus"],
                                    ["follow_mode", "quiet", {"quiet": "123456"}], ["follow_mode", "eco", {"eco": "ab"}]]},
    {"name": "brightness_read", "bright": 33, "steps": [["get_brightness"], ["get_brightness"]]},
    {"name": "per_key_explicit", "steps": [["per_key", {"fill": "010203", "count": 128}, 20], ["per_key", {"list": CELLS_A}, 0], ["per_key", {"fill": "ffffff", "count": 3}, 50], ["per_key", {"fill": "ffffff", "count": 200}, 7],
                                           ["per_key", {"fill": "ffffff", "count": 1}, 51], ["per_key", {"fill": "ffffff", "count": 1}, -1]]},
    {"name": "per_key_keeps", "bright": 31, "steps": [["per_key", {"list": CELLS_A}, None]]},
    {"name": "per_key_keeps_dark", "bright": 0, "steps": [["per_key", {"list": CELLS_A}, None]]},
    {"name": "restore_default", "steps": [["restore_default"]]},
    {"name": "brightness_effect", "init": STATE0, "steps": [["set_brightness", 40], ["set_brightness", 0], ["set_brightness", 51], ["set_brightness", -1]]},
    {"name": "brightness_operating", "init": {"keyboard": {"mode": "operating", "color": "336699", "brightness": 50}}, "steps": [["set_brightness", 10]]},
    {"name": "brightness_perkey", "init": {"keyboard": {"mode": "perkey", "brightness": 12, "cells": CELLS_A}}, "steps": [["set_brightness", 30]]},
    {"name": "brightness_unknown", "steps": [["set_brightness", 17]]},
    {"name": "brightness_off_effect", "init": {"keyboard": {"mode": "effect", "name": "off"}}, "steps": [["set_brightness", 9]]},
    {"name": "off_on", "bright": 22, "init": STATE0, "steps": [["off"], ["on"]]},
    {"name": "off_on_perkey", "bright": 12, "init": {"keyboard": {"mode": "perkey", "brightness": 12, "cells": CELLS_A}}, "steps": [["off"], ["on"]]},
    {"name": "off_when_dark", "bright": 0, "init": STATE0, "steps": [["off"], ["on"]]},
    {"name": "on_nothing_saved", "steps": [["on"]]},
    {"name": "replay", "steps": [["replay", {"mode": "effect", "name": "wave", "color": "00ff00", "speed": 4, "brightness": 20, "direction": 0}], ["replay", {"mode": "effect", "name": "star"}],
                                 ["replay", {"mode": "operating", "color": "112233", "brightness": 40}], ["replay", {"mode": "operating"}],
                                 ["replay", {"mode": "perkey", "brightness": 18, "cells": CELLS_A}], ["replay", {"mode": "perkey", "cells": CELLS_A}], ["replay", {"mode": "perkey"}],
                                 ["replay", {"mode": "off"}], ["replay", {"mode": "effect", "name": "nosuch"}], ["replay", {}], ["replay", {"mode": "effect", "name": "off"}]]},
    {"name": "replay_last", "init": STATE0, "steps": [["replay_last"]]},
    {"name": "speed", "init": STATE0, "bright": 15, "steps": [["set_speed", 12], ["set_speed", 1], ["set_speed", 0], ["set_speed", 13]]},
    {"name": "speed_no_animation", "init": {"keyboard": {"mode": "effect", "name": "static", "color": "ffffff", "speed": 5, "brightness": 20, "direction": 0}}, "steps": [["set_speed", 5]]},
    {"name": "speed_unknown", "steps": [["set_speed", 5]]},
    {"name": "color_effect", "init": STATE0, "bright": 27, "steps": [["set_color", "00ff00"], ["set_color", "#ABCDEF"], ["set_color", "zz"]]},
    {"name": "color_nocolor", "init": {"keyboard": {"mode": "effect", "name": "wave", "speed": 5, "brightness": 20, "direction": 0}}, "bright": 27, "steps": [["set_color", "00ff00"]]},
    {"name": "color_operating", "init": {"keyboard": {"mode": "operating", "color": "336699", "brightness": 50}}, "bright": 27, "steps": [["set_color", "00ff00"]]},
    {"name": "color_perkey", "init": {"keyboard": {"mode": "perkey", "brightness": 12, "cells": CELLS_A}}, "bright": 27, "steps": [["set_color", "00ff00"]]},
    {"name": "color_unknown_dark", "bright": 0, "steps": [["set_color", "123456"]]},
    {"name": "paint", "bright": 20, "steps": [["paint", [["w", "ff0000"], ["arrows", "00ff00"]], None], ["paint", [["w", "0000ff"], ["wasd", "ffffff"]], 10], ["paint", [["nokey", "ff0000"]], None], ["paint", [["w", "zz"]], None],
                                              ["paint", [], 5]]},
    {"name": "paint_remembered", "bright": 20, "init": {"keyboard": {"mode": "perkey", "brightness": 12, "cells": CELLS_A}}, "steps": [["paint", [["esc", "010101"]], None]]},
    {"name": "paint_cells", "bright": 20, "steps": [["paint_cells", [[0, "ff0000"], [127, "00ff00"], [64, "#0000ff"]], None], ["paint_cells", [[1, "zz"]], None]]},
    {"name": "timeout_flag", "timeout": 1, "steps": [["hid_timeout_get"], ["hid_timeout_set", False], ["hid_timeout_set", True]]},
    {"name": "timeout_flag_off", "timeout": 0, "steps": [["hid_timeout_get"]]},
    {"name": "timeout_flag_unreadable", "timeout": None, "steps": [["hid_timeout_get"]]},
]


def run_scenario(sc):
    log = []
    tmp = tempfile.mkdtemp(prefix="ts-golden-")
    userstate.PATH = os.path.join(tmp, "user.json")
    for key, val in (sc.get("init") or {}).items(): userstate.set(key, val)
    bright = sc.get("bright", 25); timeout = sc.get("timeout", 1)
    pending = [0]

    def ioctl(fd, req, buf):
        nr = req & 0xFF
        if nr == 6:
            assert len(buf) == 9 and buf[0] == 0 and (req >> 16) & 0x3FFF == 9, "feature report must be 9 bytes with report id 0"
            log.append("set " + bytes(buf[1:]).hex()); pending[0] = buf[1]
        else:
            assert nr == 7 and len(buf) == 9
            log.append("get %d" % (len(buf) - 1))
            if pending[0] == 0x88: buf[1:9] = bytes([0x88, 0, 3, 9, bright, 1, 1, 0x50])
            elif pending[0] == 0xB0:
                if timeout is None: buf[1:9] = bytes(8)
                else: buf[1:9] = bytes([0xB0, 0, timeout, 0, 0, 0, 0, 0x4F])
            else: buf[1:9] = bytes(8)

    def write(fd, b):
        assert len(b) == 65 and b[0] == 0
        log.append("out " + bytes(b[1:]).hex()); return len(b)

    class T:
        @staticmethod
        def sleep(s): log.append("sleep %d" % round(s * 1000))
    saved = (k._sim, k._hidraw, k._dev_open, k._dev_close, k._dev_ioctl, k._dev_write, k.time, dict(k._HID))
    k._sim = lambda: False; k._hidraw = lambda: "/dev/fake"; k._HID["fd"] = None
    k._dev_open = lambda node, flags: 7; k._dev_close = lambda fd: None; k._dev_ioctl = ioctl; k._dev_write = write; k.time = T
    results = []
    try:
        for step in sc["steps"]:
            fn, args = step[0], step[1:]
            try:
                if fn == "per_key":
                    spec = args[0]
                    cells = [tuple(bytes.fromhex(c)) for c in spec["list"]] if "list" in spec else [tuple(bytes.fromhex(spec["fill"]))] * spec["count"]
                    r = k.per_key(cells, args[1])
                elif fn == "replay": r = k.replay(args[0])
                elif fn == "replay_last": r = k.replay()
                elif fn == "paint": r = k.paint(dict(args[0]), args[1])
                elif fn == "paint_cells": r = k.paint_cells({c: v for c, v in args[0]}, args[1])
                elif fn == "hid_timeout_set": r = k.hid_timeout_set(args[0])
                elif fn == "set_operating_mode": r = k.set_operating_mode(args[0], brightness=args[1]) if len(args) > 1 else k.set_operating_mode(args[0])     # the 2nd positional parameter is the unused `speed`
                else: r = getattr(k, fn)(*args)
                results.append(r if isinstance(r, (int, str, bool, type(None))) else None)
            except HwError as e:
                results.append("ERR: " + str(e))
            log.append("--")
    finally:
        k._sim, k._hidraw, k._dev_open, k._dev_close, k._dev_ioctl, k._dev_write, k.time = saved[:7]; k._HID.update(saved[7])
    st = userstate.load()
    return {"name": sc["name"], "init": sc.get("init") or {}, "bright": bright, "timeout": timeout, "steps": sc["steps"], "log": log, "results": results,
            "state": {key: st.get(key) for key in ("keyboard", "kb_restore") if key in st}}


def gen_keyboard():
    dump("keyboard_golden.json", {"scenarios": [run_scenario(sc) for sc in SCENARIOS],
                                  "operating_colors": k.OPERATING_COLORS, "effects": list(k.EFFECTS.items()), "no_color": sorted(k.NO_COLOR)})


LOOKS = [
    {"keyboard": {"mode": "effect", "name": "wave", "color": "00ff00", "speed": 4, "brightness": 20, "direction": 0},
     "rear": {"mode": "breathing", "color": "ff0000", "speed": 2, "brightness": 80, "direction": 1, "zones": None}},
    {"keyboard": {"mode": "effect", "name": "static", "color": "ffffff", "speed": 0, "brightness": 5, "direction": 0}, "rear": None},
    {"keyboard": None, "rear": {"mode": "static", "color": "ff0000", "speed": 3, "brightness": 100, "direction": 1, "zones": ["ff0000", "00ff00", "0000ff"]}},
    {"keyboard": {"mode": "perkey", "brightness": 12, "cells": ["00aec7"] * 3 + ["000000"]}, "rear": {}},
    {"keyboard": {"mode": "off"}, "rear": None},
    {"keyboard": {"mode": "operating", "color": "00aec7", "brightness": 50}, "rear": {"mode": "wave", "color": "000000", "speed": 1, "brightness": 0}},
    {"keyboard": {}, "rear": None}, {"keyboard": None, "rear": None}, {},
    {"keyboard": {"brightness": 5}, "rear": {"brightness": 7}},
    {"keyboard": {"mode": "effect", "name": "ripple"}, "rear": {"mode": "neon"}},
]


def gen_looks():
    out = {"describe": [[lk, looks.describe(lk)] for lk in LOOKS], "files": [], "builtin": looks.load("native")}
    with tempfile.TemporaryDirectory() as d:
        looks.DIR = os.path.join(d, "looks")
        for i, lk in enumerate(LOOKS[:6]):
            looks.save("look%d" % i, lk)
            with open(os.path.join(looks.DIR, "look%d.json" % i)) as f: out["files"].append([lk, f.read()])
        out["names"] = looks.names()
        errs = {}
        for label, fn in (("save native", lambda: looks.save("native", {})), ("save bad", lambda: looks.save("bad/name", {"keyboard": None, "rear": None})), ("save long", lambda: looks.save("x" * 41, {"a": 1})),
                          ("load none", lambda: looks.load("nosuch")), ("load bad", lambda: looks.load("a b")), ("delete native", lambda: looks.delete("native")), ("delete none", lambda: looks.delete("nosuch")),
                          ("delete bad", lambda: looks.delete("a/b"))):
            try: fn()
            except HwError as e: errs[label] = str(e)
        out["errors"] = errs
        out["copies"] = [looks.copy("look0"), looks.copy("look0"), looks.copy("look0", "mine"), looks.copy("look0", "mine"), looks.copy("native")]
        out["names_after"] = looks.names()
    dump("looks_golden.json", out)


def gen_keyprofiles():
    out = {}
    with tempfile.TemporaryDirectory() as d:
        keyprofiles.DIR = os.path.join(d, "lighting")
        cells = [(i, 255 - i, (i * 7) % 256) for i in range(128)]
        keyprofiles.save("full", cells)
        keyprofiles.save("short", [(1, 2, 3), (4, 5, 6)])
        keyprofiles.save("a-b_c9", [])
        out["files"] = {n: open(os.path.join(keyprofiles.DIR, n + ".json")).read() for n in ("full", "short", "a-b_c9")}
        msgs = []
        for args in (("list",), ("save", "fill1", ["fill", "ff0000"]), ("save", "grad", ["gradient", "ff0000", "0000ff"]), ("save", "rain", ["rainbow"]), ("save", "dflt"), ("list",), ("load", "fill1"),
                     ("delete", "fill1"), ("load", "fill1"), ("delete", "fill1"), ("load",), ("save",), ("delete",), ("bogus",), ("bogus", "x"), ("save", "x", ["fill"]), ("save", "x", ["gradient", "ff0000"]),
                     ("save", "x", ["fill", "zz"]), ("save", "x", ["nonsense"]), ("save", "bad/name"), ("save", "---"), ("save", ""), ("load", "a b"), ("save", "ünï", ["fill", "00ff00"]), ("list",)):
            try:
                r = keyprofiles.command(args[0], args[1] if len(args) > 1 else None, args[2] if len(args) > 2 else ())
                msgs.append([list(args), r])
            except HwError as e: msgs.append([list(args), "ERR: " + str(e)])
        out["commands"] = msgs
        out["saved"] = {n: open(os.path.join(keyprofiles.DIR, n + ".json")).read() for n in ("grad", "rain", "dflt")}
        out["exists"] = [[n, keyprofiles.exists(n)] for n in ("full", "nosuch", "bad/name", "")]
    dump("keyprofiles_golden.json", out)


if __name__ == "__main__":
    gen_keymap(); gen_perkeyfx(); gen_designs(); gen_images(); gen_keyboard(); gen_looks(); gen_keyprofiles()
