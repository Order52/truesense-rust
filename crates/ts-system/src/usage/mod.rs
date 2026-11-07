//! usage: what the machine is doing right now, for the Monitor page: CPU load per core and in
//! total, CPU clocks, load average, package power, memory and swap, and the busiest processes.
//!
//! Every function takes a root directory (`/` on a real machine, a temporary tree in tests) and
//! reads no clock itself: samplers that need the time are given it, so a test steps it by hand.
//! Nothing here starts a process, takes a lock shared with another module or keeps a global; the
//! samplers are plain structs for the application layer to keep behind a mutex.
//!
//! Adapted from predator-sense by cleyton1986 (GPL-3.0), used with the author's permission. The
//! idea of sampling `/proc/stat` and `/proc/<pid>/stat` and the process table layout come from
//! his `hardware/procs.rs`; the code here was rewritten (no globals, no unwraps, bounded reads,
//! one file per process instead of three, guest time not counted twice, counter-reset handling).
#![forbid(unsafe_code)]

pub mod cpu;
pub(crate) mod files;
pub mod memory;
pub mod procs;

pub use cpu::{
    CoreFreq, CoreLoad, CpuLoad, CpuSampler, CpuTicks, LoadAverage, RaplMeter, StatSnapshot, busy_pct,
    parse_loadavg, parse_stat, read_freqs, read_governor, read_loadavg,
};
pub use memory::{MemInfo, MemoryUsage, parse_meminfo, read_meminfo};
pub use procs::{MAX_TOP, PidStat, ProcSampler, ProcSnapshot, ProcessInfo, parse_passwd, parse_pid_stat};
