#!/usr/bin/env python3
"""Deterministic before/after of the live-data widgets, offscreen (no window on any screen).

Feeds the same widgets a page shows (two Gauges, a Sparkline, a LineChart) a new reading every
second, exactly what the live tick does, and measures the CPU, wake-ups and frames per second of a
`qml` process, once with the components of git revision REV (default: the commit before the
Canvas widgets were replaced) and once with the working tree's. Real pages cannot be compared
fairly because the simulated hardware reads the real sensors, whose noise comes and goes.

    tools/qtprofile/bench.py [--rev REV] [--runs 2] [--module DIR]

DIR is the QML module directory made by crates/ts-qt/tools/qml-tests.sh (default
$CARGO_TARGET_DIR/qml-test-modules, run that script first).
"""
import argparse
import os
import shutil
import subprocess
import sys
import tempfile
import time

sys.dont_write_bytecode = True
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import procsample  # noqa: E402

REPO = os.path.abspath(os.path.join(HERE, "..", ".."))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rev", default="b83616b^")
    ap.add_argument("--runs", type=int, default=2)
    ap.add_argument("--module", default=os.path.join(os.environ.get("CARGO_TARGET_DIR", os.path.join(REPO, "target")), "qml-test-modules"))
    a = ap.parse_args()
    qml = "/usr/lib/qt6/bin/qml"
    work = tempfile.mkdtemp(prefix="tsbench-", dir="/tmp")
    try:
        os.makedirs(os.path.join(work, "old"))
        for name in ("Gauge", "Sparkline", "LineChart"):
            text = subprocess.run(["git", "-C", REPO, "show", f"{a.rev}:crates/ts-qt/qml/components/{name}.qml"],
                                  capture_output=True, text=True, check=True).stdout
            with open(os.path.join(work, "old", name + ".qml"), "w") as f:
                f.write(text)
        shutil.copy(os.path.join(HERE, "bench.qml"), os.path.join(work, "Bench.qml"))
        home, run = os.path.join(work, "home"), os.path.join(work, "run")
        os.makedirs(home)
        os.makedirs(run, mode=0o700)
        env = {"QT_QPA_PLATFORM": "offscreen", "QT_QUICK_BACKEND": "software", "HOME": home, "XDG_RUNTIME_DIR": run,
               "PATH": "/nonexistent", "QML_XHR_ALLOW_FILE_READ": "1", "QT_FORCE_STDERR_LOGGING": "1",
               "DBUS_SESSION_BUS_ADDRESS": "unix:path=/nonexistent", "LANG": "C.UTF-8"}
        print(f"old = components of {a.rev}, new = working tree; load average {open('/proc/loadavg').read().split()[0]}")
        for _ in range(a.runs):
            for variant in ("old", "new"):
                args = [qml, "-I", a.module, "Bench.qml"] + (["old"] if variant == "old" else [])
                p = subprocess.Popen(args, env=env, cwd=work, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
                time.sleep(2.5)
                r = procsample.sample(p.pid, 5.0)
                out = p.communicate(timeout=30)[0]
                frames = [int(l.split()[-1]) for l in out.splitlines() if "FRAMES" in l]
                mean = sum(frames[2:7]) / max(1, len(frames[2:7]))
                print(f"{variant}: cpu {r['cpu_pct']:4.1f} %   wakeups {r['wakeups_s']:5.0f} /s   frames {mean:5.1f} /s")
    finally:
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
