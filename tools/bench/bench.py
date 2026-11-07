#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""TrueSense benchmark: the numbers of docs/PERFORMANCE.md, measured the same way every time.

Everything runs in the firmware simulator (TRUESENSE_SIM=1) with a temporary HOME, short paths,
fake helper programs (pactl, amixer, systemctl, ... that only count their calls) and, for the
services, a private /tmp, /dev and /sys/class/hidraw that `run.sh` sets up with a user namespace.
Nothing here touches the real daemon, device, sound system or windows. Only processes this script
started are ever measured or stopped.

What is measured, and how (all from /proc, no tracing tools needed):
  * CPU: run time of every thread of the process (/proc/PID/task/*/schedstat, nanoseconds) over
    the window, as a percentage of one core. Children that were waited for are shown apart.
  * Wakeups per second: the change of voluntary_ctxt_switches summed over the threads. A thread
    that sleeps and is woken counts once; a thread that never sleeps does not.
  * Memory: Rss, Pss and the anonymous part (Pss_Anon) from smaps_rollup. Rss counts the shared
    program file; Pss divides shared pages among the processes that map them; Pss_Anon is what
    only this process owns.
  * Start-up: wall time of the whole command (fork, exec, run, exit), median and best of N.

Usage: run.sh [--seconds N] [--runs N] [--bin PATH] [--qt PATH] [--only a,b] [--json FILE]
"""

import argparse
import json
import os
import pty
import select
import shutil
import signal
import statistics
import struct
import subprocess
import sys
import threading
import time

CLK = os.sysconf("SC_CLK_TCK")
HELPERS = [
    "pactl", "amixer", "systemctl", "wpctl", "pw-cli", "nvidia-smi", "journalctl", "loginctl",
    "hyprctl", "notify-send", "dkms", "modinfo", "lsmod", "sensors", "sudo", "acerultrax-cli",
    "xrandr", "kscreen-doctor", "gdbus", "busctl", "curl", "nvidia-settings",
]
# The fake helper: counts every call in calls.log (shell built-ins only, so a PATH that holds
# nothing else works) and answers like the real tool just enough for the programs to carry on.
STUB = r"""#!/bin/sh
n=${0##*/}
echo "$n $*" >> "$TS_BENCH_CALLS"
# A real helper takes a few milliseconds to start and talk to its server; the stubs pause for an
# assumed 12 ms (pactl), 4 ms (amixer) and 3 ms (systemctl), so that overlapping them is visible.
# (The long-running `pactl subscribe` of the audio watcher does not pause.)
case "$n $1" in
 "pactl subscribe") ;;
 pactl*) /usr/bin/sleep 0.012;;
 amixer*) /usr/bin/sleep 0.004;;
 systemctl*) /usr/bin/sleep 0.003;;
esac
case "$n $*" in
 "pactl list short sources") printf '47\talsa_input.pci-0000_00_1f.3.analog-stereo\tPipeWire\ts32le 2ch 48000Hz\tSUSPENDED\n';;
 "pactl list short sinks") printf '48\talsa_output.pci-0000_00_1f.3.analog-stereo\tPipeWire\ts32le 2ch 48000Hz\tSUSPENDED\n';;
 "pactl list short cards") printf '49\talsa_card.pci-0000_00_1f.3\talsa\n';;
 "pactl subscribe") while read -r line; do echo "$line"; done < "$TS_BENCH_EVENTS";;
 "pactl get-default-sink") echo alsa_output.pci-0000_00_1f.3.analog-stereo;;
 "pactl get-default-source") echo alsa_input.pci-0000_00_1f.3.analog-stereo;;
 "amixer"*) printf "numid=1,iface=CARD,name='Headset Mic Jack'\n  ; type=BOOLEAN,access=r-------,values=1\n  : values=off\n";;
 "systemctl"*) echo inactive; exit 3;;
 *) exit 1;;
esac
"""
HID_DESCRIPTOR = bytes([0x06, 0x88, 0x00, 0x09, 0x01, 0xA1, 0x01, 0xC0])


class Sandbox:
    """A temporary world for the programs under test."""

    def __init__(self, root):
        self.root = root
        self.bin = f"{root}/bin"
        self.calls = f"{root}/calls.log"
        os.makedirs(self.bin, exist_ok=True)
        for sub in ("home", "run", "xdg"):
            os.makedirs(f"{root}/{sub}", exist_ok=True)
        with open(f"{self.bin}/stub", "w") as f:
            f.write(STUB)
        os.chmod(f"{self.bin}/stub", 0o755)
        for name in HELPERS:
            link = f"{self.bin}/{name}"
            if not os.path.lexists(link):
                os.symlink("stub", link)
        open(self.calls, "w").close()
        # `pactl subscribe` of the fake sound system: the stub copies this pipe to its output, so
        # a line written here is a change event (nothing is written during an idle measurement).
        self.events = f"{root}/events.fifo"
        os.mkfifo(self.events)
        self.events_fd = os.open(self.events, os.O_RDWR)
        self.env = {
            "PATH": self.bin,
            "HOME": f"{root}/home",
            "XDG_RUNTIME_DIR": f"{root}/xdg",
            "TRUESENSE_SIM": "1",
            "TRUESENSE_SOCKET": f"{root}/d.sock",
            "TRUESENSE_RUN": f"{root}/run",
            "TRUESENSE_STATE": f"{root}/state.json",
            "TRUESENSE_USERSTATE": f"{root}/user.json",
            "TS_BENCH_CALLS": self.calls,
            "TS_BENCH_EVENTS": self.events,
            "LANG": "C",
        }

    def calls_count(self, prefix=None):
        with open(self.calls) as f:
            lines = f.read().splitlines()
        if prefix is None:
            return len(lines)
        return sum(1 for line in lines if line.startswith(prefix))

    def calls_by_name(self):
        out = {}
        with open(self.calls) as f:
            for line in f.read().splitlines():
                name = line.split(" ", 1)[0]
                out[name] = out.get(name, 0) + 1
        return out


# ---- /proc readers ---------------------------------------------------------------------------


def threads(pid):
    try:
        return sorted(os.listdir(f"/proc/{pid}/task"))
    except OSError:
        return []


def cpu_ns(pid):
    total = 0
    for tid in threads(pid):
        try:
            with open(f"/proc/{pid}/task/{tid}/schedstat") as f:
                total += int(f.read().split()[0])
        except (OSError, ValueError, IndexError):
            pass
    return total


def child_ticks(pid):
    try:
        with open(f"/proc/{pid}/stat") as f:
            rest = f.read().rsplit(")", 1)[1].split()
        return int(rest[13]) + int(rest[14])  # cutime + cstime (fields 16 and 17)
    except (OSError, ValueError, IndexError):
        return 0


def own_ticks(pid):
    try:
        with open(f"/proc/{pid}/stat") as f:
            rest = f.read().rsplit(")", 1)[1].split()
        return int(rest[11]) + int(rest[12])  # utime + stime
    except (OSError, ValueError, IndexError):
        return 0


def switches(pid):
    voluntary = 0
    for tid in threads(pid):
        try:
            with open(f"/proc/{pid}/task/{tid}/status") as f:
                for line in f:
                    if line.startswith("voluntary_ctxt_switches"):
                        voluntary += int(line.split()[1])
        except (OSError, ValueError):
            pass
    return voluntary


def per_thread(pid):
    """{tid: (name, voluntary switches, cpu ns)} for a process."""
    out = {}
    for tid in threads(pid):
        try:
            with open(f"/proc/{pid}/task/{tid}/comm") as f:
                name = f.read().strip()
            vol = 0
            with open(f"/proc/{pid}/task/{tid}/status") as f:
                for line in f:
                    if line.startswith("voluntary_ctxt_switches"):
                        vol = int(line.split()[1])
            with open(f"/proc/{pid}/task/{tid}/schedstat") as f:
                ns = int(f.read().split()[0])
            out[tid] = (name, vol, ns)
        except (OSError, ValueError, IndexError):
            pass
    return out


def memory(pid):
    out = {"rss": 0, "pss": 0, "anon": 0}
    try:
        with open(f"/proc/{pid}/smaps_rollup") as f:
            for line in f:
                key, _, value = line.partition(":")
                if key == "Rss":
                    out["rss"] = int(value.split()[0])
                elif key == "Pss":
                    out["pss"] = int(value.split()[0])
                elif key == "Pss_Anon":
                    out["anon"] = int(value.split()[0])
    except (OSError, ValueError):
        pass
    return out


def measure(pid, seconds):
    """CPU %, wakeups/s, children's CPU % over a window, plus memory and thread count at its end."""
    if cpu_ns(pid) == 0 and own_ticks(pid) == 0:
        use_ns = False
    else:
        use_ns = True
    c0 = cpu_ns(pid) if use_ns else own_ticks(pid)
    m0 = memory(pid)
    k0, s0, t0 = child_ticks(pid), switches(pid), time.monotonic()
    d0 = per_thread(pid) if os.environ.get("TS_BENCH_VERBOSE") else None
    time.sleep(seconds)
    if d0 is not None:
        d1 = per_thread(pid)
        for tid, (name, vol, ns) in sorted(d1.items()):
            v0, n0 = d0.get(tid, (name, 0, 0))[1:]
            print(f"  thread {name:<16} {(vol - v0) / seconds:6.2f} wakeups/s  {(ns - n0) / seconds / 1e7:7.4f} % cpu", file=sys.stderr)
    c1 = cpu_ns(pid) if use_ns else own_ticks(pid)
    k1, s1, t1 = child_ticks(pid), switches(pid), time.monotonic()
    dt = t1 - t0
    cpu = (c1 - c0) / 1e9 if use_ns else (c1 - c0) / CLK
    # Memory is read at both ends of the window and the larger value kept: the kernel may trim a
    # process's resident pages at any moment (a Qt app measured 50 MB once and 99 MB the next
    # second), and what matters is what the program holds when it is working.
    m1 = memory(pid)
    mem = {k: max(m0[k], m1[k]) for k in m1}
    return {
        "cpu_pct": 100.0 * cpu / dt,
        "children_cpu_pct": 100.0 * (k1 - k0) / CLK / dt,
        "wakeups": (s1 - s0) / dt,
        "threads": len(threads(pid)),
        **mem,
    }


# ---- starting and stopping what we measure ------------------------------------------------------


class Proc:
    def __init__(self, argv, env, log):
        self.out = open(log, "wb")
        self.p = subprocess.Popen(argv, env=env, stdout=self.out, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL)
        self.pid = self.p.pid
        self.exe = os.path.realpath(f"/proc/{self.pid}/exe")

    def alive(self):
        return self.p.poll() is None

    def log_tail(self):
        self.out.flush()
        try:
            with open(self.out.name, errors="replace") as f:
                return f.read()[-600:]
        except OSError:
            return ""

    def stop(self):
        # Only ever our own child, checked by pid (we hold the Popen handle).
        if self.p.poll() is None:
            self.p.send_signal(signal.SIGTERM)
            try:
                self.p.wait(10)
            except subprocess.TimeoutExpired:
                self.p.kill()
                self.p.wait()
        self.out.close()


def run_cli(binary, sb, args, env_extra=None, timeout=60):
    env = dict(sb.env)
    env.update(env_extra or {})
    return subprocess.run([binary, *args], env=env, capture_output=True, timeout=timeout)


def startup(argv, env, runs, warm=3):
    """Wall milliseconds of whole commands: (median, best)."""
    times = []
    for i in range(runs + warm):
        t0 = time.perf_counter()
        pid = os.posix_spawn(
            argv[0], argv, env,
            file_actions=[(os.POSIX_SPAWN_OPEN, 0, os.devnull, os.O_RDONLY, 0),
                          (os.POSIX_SPAWN_OPEN, 1, os.devnull, os.O_WRONLY, 0),
                          (os.POSIX_SPAWN_OPEN, 2, os.devnull, os.O_WRONLY, 0)],
        )
        os.waitpid(pid, 0)
        dt = (time.perf_counter() - t0) * 1000.0
        if i >= warm:
            times.append(dt)
    return statistics.median(times), min(times)


def pty_first_frame(argv, env, rows=40, cols=120, timeout=15.0, settle=0.05):
    """Milliseconds from exec until a terminal program has drawn something and then been quiet for
    `settle` seconds (its first frame), and the bytes it wrote."""
    master, slave = pty.openpty()
    fcntl_winsz = struct.pack("HHHH", rows, cols, 0, 0)
    import fcntl
    import termios
    fcntl.ioctl(slave, termios.TIOCSWINSZ, fcntl_winsz)
    t0 = time.perf_counter()
    p = subprocess.Popen(argv, env=env, stdin=slave, stdout=slave, stderr=slave, close_fds=True,
                         start_new_session=True)
    os.close(slave)
    got = b""
    first = None
    last = None
    try:
        while time.perf_counter() - t0 < timeout:
            ready, _, _ = select.select([master], [], [], 0.02)
            now = time.perf_counter()
            if ready:
                try:
                    chunk = os.read(master, 65536)
                except OSError:
                    break
                if not chunk:
                    break
                got += chunk
                last = now
                if first is None and len(got) > 200:
                    first = now
            elif first is not None and last is not None and now - last >= settle:
                break
        return ((last or time.perf_counter()) - t0) * 1000.0 if first else None, len(got)
    finally:
        if p.poll() is None:
            p.kill()
        p.wait()
        os.close(master)


# ---- the measurements ------------------------------------------------------------------------------


def wander():
    """Moves the fake package temperature every second (52 to 60 C, a fixed pseudo-random walk), so
    live values really change: an unchanging machine would hide most of what a live window costs."""
    path = "/sys/class/hwmon/hwmon0/temp1_input"
    i = 0
    while os.path.exists(path):
        try:
            with open(path, "w") as f:
                f.write(f"{52000 + (i * 7919) % 9000}\n")
        except OSError:
            return
        i += 1
        time.sleep(1)


def fmt_kb(kb):
    return f"{kb / 1024:.1f} MB"


def bench_services(binary, qt, sb, seconds, only, results):
    procs = []
    try:
        if only("daemon"):
            # The GPU sampler's helper must not exist: nvidia-smi is a stub that fails at once.
            d = Proc([binary, "daemon"], sb.env, f"{sb.root}/daemon.log")
            procs.append(d)
            wait_for(lambda: os.path.exists(sb.env["TRUESENSE_SOCKET"]), 10)
            run_cli(binary, sb, ["fan", "smart", "on"])
            run_cli(binary, sb, ["power", "auto", "smart"])
            time.sleep(5)
            results["daemon"] = measure(d.pid, seconds)
            results["daemon"]["pid"] = d.pid
        if only("keys"):
            k = Proc([binary, "keys", "run"], sb.env, f"{sb.root}/keys.log")
            procs.append(k)
            fifo_fd = os.open("/dev/hidraw9", os.O_RDWR) if os.path.exists("/dev/hidraw9") else None
            time.sleep(5)
            results["keys"] = measure(k.pid, seconds)
            if not k.alive() or os.environ.get("TS_BENCH_VERBOSE"):
                print("key service log:", k.log_tail(), file=sys.stderr)
            if fifo_fd is not None:
                os.close(fifo_fd)
        if only("audio"):
            open(sb.calls, "w").close()
            a = Proc([binary, "audio", "voice", "watch"], sb.env, f"{sb.root}/audio.log")
            procs.append(a)
            time.sleep(100)  # the login repair looks at 20 s and 90 s after start; measure after both
            before = sb.calls_by_name()
            results["audio"] = measure(a.pid, seconds)
            after = sb.calls_by_name()
            spawns = sum(after.values()) - sum(before.values())
            results["audio"]["spawns_per_s"] = spawns / seconds
            if os.environ.get("TS_BENCH_VERBOSE"):
                print("  audio helper calls:", {k: after.get(k, 0) - before.get(k, 0) for k in after}, file=sys.stderr)
                print("  audio log:", a.log_tail(), file=sys.stderr)
        if only("qt") and qt:
            bench_qt(qt, sb, seconds, results)
    finally:
        for p in reversed(procs):
            p.stop()


def qt_env(sb):
    env = dict(sb.env)
    # The software scene-graph renderer is what the app uses by default; offscreen has no GPU anyway.
    env.update({"QT_QPA_PLATFORM": "offscreen", "QT_QUICK_BACKEND": "software", "DBUS_SESSION_BUS_ADDRESS": "disabled:",
                "XDG_CONFIG_HOME": f"{sb.root}/home/.config", "XDG_CACHE_HOME": f"{sb.root}/home/.cache"})
    return env


def bench_qt(qt, sb, seconds, results):
    """The window shown and idle, and the window started hidden (`--background`), each against a
    simulated daemon. Always offscreen, always with this sandbox's own runtime directory."""
    d = Proc([sb.binary, "daemon"], sb.env, f"{sb.root}/daemon-qt.log")
    try:
        wait_for(lambda: os.path.exists(sb.env["TRUESENSE_SOCKET"]), 10)
        for key, extra in (("qt_visible", []), ("qt_hidden", ["--background"])):
            q = Proc([qt, *extra], qt_env(sb), f"{sb.root}/qt.log")
            try:
                time.sleep(6)
                if q.alive():
                    results[key] = measure(q.pid, seconds)
                else:
                    print(f"{key}: the app exited:", q.log_tail(), file=sys.stderr)
            finally:
                q.stop()
    finally:
        d.stop()


def wait_for(cond, seconds):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        if cond():
            return True
        time.sleep(0.05)
    return False


def bench_startup(binary, qt, sb, runs, only, results):
    out = {}
    env = sb.env
    fast = f"{sb.root}/truesense-fast"
    if not os.path.lexists(fast):
        os.symlink(binary, fast)
    d = Proc([binary, "daemon"], sb.env, f"{sb.root}/daemon-start.log")
    try:
        wait_for(lambda: os.path.exists(sb.env["TRUESENSE_SOCKET"]), 10)
        time.sleep(1)
        out["version"] = startup([binary, "--version"], env, runs * 5)
        out["fan status"] = startup([binary, "fan", "status"], env, runs)
        before = sb.calls_by_name()
        out["status"] = startup([binary, "status"], env, runs)
        out["status_helpers"] = (sum(sb.calls_by_name().values()) - sum(before.values())) / (runs + 3)
        before = sb.calls_by_name()
        out["doctor"] = startup([binary, "doctor"], env, runs)
        out["doctor_helpers"] = (sum(sb.calls_by_name().values()) - sum(before.values())) / (runs + 3)
        out["fast"] = startup([fast, "fan", "smart"], env, runs * 3)
        for name, argv in (("tui", [binary, "tui"]),):
            frames = []
            for _ in range(max(3, runs // 3)):
                ms, nbytes = pty_first_frame(argv, {**env, "TERM": "xterm-256color"})
                if ms is not None:
                    frames.append(ms)
            if frames:
                out["tui"] = (statistics.median(frames), min(frames))
        if qt and only("qt"):
            qenv = qt_env(sb)
            qenv.update({"TRUESENSE_QT_TEST": "1", "TRUESENSE_QT_EXIT_AFTER_MS": "1"})
            out["qt"] = startup([qt], qenv, max(3, runs // 3))
    finally:
        d.stop()
    results["startup"] = out


def bench_sizes(binary, qt, results):
    sizes = {"truesense": os.path.getsize(binary)}
    if qt and os.path.exists(qt):
        sizes["truesense-qt"] = os.path.getsize(qt)
    results["sizes"] = sizes


def table(results):
    lines = []
    lines.append("| Process | CPU % of one core | Wakeups/s | Threads | RSS | PSS | Own memory (anon) |")
    lines.append("|---|---|---|---|---|---|---|")
    names = [("daemon", "Daemon (smart fan on, smart auto profile)"),
             ("keys", "Key service (theme renderer idle, config poll)"),
             ("audio", "Audio watcher"),
             ("qt_visible", "Qt app, window visible, idle"),
             ("qt_hidden", "Qt app, window hidden")]
    for key, label in names:
        r = results.get(key)
        if not r:
            continue
        lines.append(f"| {label} | {r['cpu_pct']:.3f} | {r['wakeups']:.1f} | {r['threads']} | "
                     f"{fmt_kb(r['rss'])} | {fmt_kb(r['pss'])} | {fmt_kb(r['anon'])} |")
    a = results.get("audio")
    if a:
        lines.append("")
        lines.append(f"The audio watcher started {a['spawns_per_s']:.2f} helper programs per second "
                     f"(pactl, amixer) while idle.")
    s = results.get("startup")
    if s:
        lines.append("")
        lines.append("| Start-up (whole command, wall time) | Median | Best | Helper programs per run |")
        lines.append("|---|---|---|---|")
        rows = [("version", "`truesense --version`", None), ("fan status", "`truesense fan status`", None),
                ("status", "`truesense status`", "status_helpers"), ("doctor", "`truesense doctor`", "doctor_helpers"),
                ("fast", "`truesense-fast fan smart`", None), ("tui", "terminal UI, first frame", None),
                ("qt", "Qt app, first frame and exit (offscreen)", None)]
        for key, label, helpers in rows:
            if key in s:
                med, best = s[key]
                extra = f"{s[helpers]:.0f}" if helpers else "0"
                lines.append(f"| {label} | {med:.1f} ms | {best:.1f} ms | {extra} |")
    z = results.get("sizes")
    if z:
        lines.append("")
        lines.append("| Binary | Size |")
        lines.append("|---|---|")
        for k, v in z.items():
            lines.append(f"| {k} | {v / 1e6:.2f} MB |")
    return "\n".join(lines)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--seconds", type=float, default=60.0, help="length of each idle window")
    ap.add_argument("--runs", type=int, default=20, help="runs per start-up measurement")
    ap.add_argument("--bin", default=os.environ.get("TRUESENSE_BIN"))
    ap.add_argument("--qt", default=os.environ.get("TRUESENSE_QT_BIN"))
    ap.add_argument("--only", default="", help="comma list of: daemon,keys,audio,qt,startup,sizes")
    ap.add_argument("--json", help="also write the raw numbers here")
    args = ap.parse_args()
    if not os.environ.get("TS_BENCH_PRIVATE"):
        sys.exit("run this through tools/bench/run.sh: it gives the programs a private /tmp (the daemon writes "
                 "/tmp/truesense-state, which belongs to the real daemon on a real machine)")
    if not args.bin or not os.access(args.bin, os.X_OK):
        sys.exit("--bin PATH (or TRUESENSE_BIN) must be the truesense program")
    wanted = set(filter(None, args.only.split(",")))

    def only(name):
        return not wanted or name in wanted

    threading.Thread(target=wander, daemon=True).start()
    root = f"/tmp/b{os.getpid()}"
    sb = Sandbox(root)
    sb.binary = args.bin
    results = {}
    try:
        if only("sizes"):
            bench_sizes(args.bin, args.qt, results)
        if only("startup"):
            bench_startup(args.bin, args.qt, sb, args.runs, only, results)
        if wanted & {"daemon", "keys", "audio", "qt"} or not wanted:
            bench_services(args.bin, args.qt, sb, args.seconds, only, results)
    finally:
        shutil.rmtree(root, ignore_errors=True)
    print(table(results))
    if args.json:
        with open(args.json, "w") as f:
            json.dump(results, f, indent=1)


if __name__ == "__main__":
    main()
