#!/usr/bin/env python3
"""Read-only /proc sampler: CPU %, wakeups/s (voluntary context switches) and RSS of one process,
split by thread name. Used by qtprofile.py; runnable alone: procsample.py PID [SECONDS]."""
import os
import sys
import time

HZ = os.sysconf("SC_CLK_TCK")


def snapshot(pid):
    """{tid: (name, cpu_jiffies, voluntary, nonvoluntary)} for every thread of pid."""
    out = {}
    base = f"/proc/{pid}/task"
    try:
        tids = os.listdir(base)
    except OSError:
        return out
    for tid in tids:
        try:
            with open(f"{base}/{tid}/stat") as f:
                stat = f.read()
            with open(f"{base}/{tid}/status") as f:
                status = f.read()
        except OSError:
            continue
        rp = stat.rfind(")")
        name = stat[stat.find("(") + 1:rp]
        fields = stat[rp + 2:].split()
        jiffies = int(fields[11]) + int(fields[12])
        vol = nonvol = 0
        for line in status.splitlines():
            if line.startswith("voluntary_ctxt_switches"):
                vol = int(line.split()[1])
            elif line.startswith("nonvoluntary_ctxt_switches"):
                nonvol = int(line.split()[1])
        out[tid] = (name, jiffies, vol, nonvol)
    return out


def memory(pid):
    """RSS, RssAnon, RssFile, RssShmem in MB."""
    res = {}
    try:
        with open(f"/proc/{pid}/status") as f:
            for line in f:
                for key in ("VmRSS", "RssAnon", "RssFile", "RssShmem"):
                    if line.startswith(key + ":"):
                        res[key] = int(line.split()[1]) / 1024.0
    except OSError:
        pass
    return res


def pss(pid):
    """Proportional set size in MB (shared library pages are divided among their users), 0 when
    /proc/PID/smaps_rollup is not readable. Steadier than RSS on a machine that reclaims page cache."""
    try:
        with open(f"/proc/{pid}/smaps_rollup") as f:
            for line in f:
                if line.startswith("Pss:"):
                    return int(line.split()[1]) / 1024.0
    except OSError:
        pass
    return 0.0


def sample(pid, seconds):
    """Measures `seconds` of wall time; returns a dict with totals and per-thread-name rows."""
    a = snapshot(pid)
    t0 = time.monotonic()
    time.sleep(seconds)
    b = snapshot(pid)
    dt = time.monotonic() - t0
    groups = {}
    for tid, (name, j1, v1, n1) in b.items():
        if tid not in a:
            continue
        _, j0, v0, n0 = a[tid]
        g = groups.setdefault(name, [0, 0, 0, 0])
        g[0] += 1
        g[1] += (j1 - j0) / HZ
        g[2] += v1 - v0
        g[3] += n1 - n0
    cpu = sum(g[1] for g in groups.values()) / dt * 100.0
    wake = sum(g[2] for g in groups.values()) / dt
    mem = memory(pid)
    return {
        "cpu_pct": cpu,
        "wakeups_s": wake,
        "rss_mb": mem.get("VmRSS", 0.0),
        "rss_anon_mb": mem.get("RssAnon", 0.0),
        "rss_file_mb": mem.get("RssFile", 0.0),
        "pss_mb": pss(pid),
        "threads": {n: {"count": g[0], "cpu_pct": g[1] / dt * 100.0, "wakeups_s": g[2] / dt}
                    for n, g in groups.items()},
        "seconds": dt,
    }


if __name__ == "__main__":
    pid = int(sys.argv[1])
    secs = float(sys.argv[2]) if len(sys.argv) > 2 else 5.0
    r = sample(pid, secs)
    print(f"cpu {r['cpu_pct']:.1f}%  wakeups {r['wakeups_s']:.0f}/s  rss {r['rss_mb']:.0f} MB "
          f"(anon {r['rss_anon_mb']:.0f}, file {r['rss_file_mb']:.0f}, pss {r['pss_mb']:.0f})")
    for n, g in sorted(r["threads"].items(), key=lambda kv: -kv[1]["wakeups_s"]):
        print(f"  {n:<20} x{g['count']:<2} cpu {g['cpu_pct']:5.1f}%  wakeups {g['wakeups_s']:6.0f}/s")
    print(open("/proc/loadavg").read().strip())
