//! Game rules: a program name mapped to the power mode to use while it runs
//! (`"hl2_linux": "performance"`, `"eldenring.exe": "turbo"`).
//!
//! The service looks at the running programs every few seconds. A matching program raises the
//! automatic power mode (native and smart auto both honour it); when the program ends the
//! automatic profile takes over again by itself. A name matches the process name (as `ps -e`
//! shows it, which the kernel cuts to 15 characters), the program file's name, or, for
//! Wine and Proton games, any `.exe` on the command line, because such a game runs as
//! `wine64-preloader` with the game's `.exe` as an argument. Case does not matter. Turbo still
//! backs off to Performance if the laptop overheats, and Eco is forced when the battery is low
//! (that is the engine's business, see [`crate::autoprofile`]).
//!
//! Matching is pure: [`running`] works on a list of [`Process`]es passed in. Reading that list
//! from `/proc` is the separate [`list_processes`], which takes the directory to read so tests
//! build a fake `/proc`. [`Watcher`] adds the "look every third tick" schedule.
#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use serde_json::{Map, Value};

use crate::smartprofile::Mode;

/// At most this many rules.
pub const MAX_RULES: usize = 64;

/// The kernel keeps this many characters of a process name (`TASK_COMM_LEN` minus the NUL).
const COMM_LEN: usize = 15;
/// Only this many leading command-line arguments are looked at for a Wine/Proton `.exe`.
const MAX_ARGS: usize = 12;
/// Only this much of a command line is read.
const MAX_CMDLINE: usize = 4096;
/// The longest program name a rule may have.
pub const PROGRAM_NAME_MAX: usize = 80;
/// The process list is read every this many ticks (games do not start every second).
pub(crate) const SCAN_EVERY_TICKS: u64 = 3;

/// Why a rule or a rule set was refused. The messages are the Python edition's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RulesError {
    /// The rule set is not a name to mode map.
    NotAMap,
    /// A program name with characters outside letters, digits, `. _ + -` and space, or empty,
    /// or longer than 80 characters (a path separator above all is refused).
    BadName,
    /// A mode that is not one of the five, in a rule set (the message names the rule).
    BadMode(String),
    /// A mode that is not one of the five, in a single `add`.
    BadAddMode,
    /// More than [`MAX_RULES`] rules.
    TooMany,
    /// `remove` of a name that has no rule (shown as typed, so a typo is noticed).
    NoRule(String),
}

impl fmt::Display for RulesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RulesError::NotAMap => f.write_str("game rules are a name -> mode map"),
            RulesError::BadName => f.write_str("program name: letters, digits, . _ + - and spaces (max 80)"),
            RulesError::BadMode(key) => write!(f, "{key}: mode must be one of {}", Mode::names_joined()),
            RulesError::BadAddMode => write!(f, "mode: {}", Mode::names_joined()),
            RulesError::TooMany => write!(f, "at most {MAX_RULES} rules"),
            RulesError::NoRule(name) => write!(f, "no rule for '{name}'"),
        }
    }
}

impl std::error::Error for RulesError {}

/// Python's `str.strip()`: whitespace as `str.isspace` sees it, which also counts the four
/// ASCII separators 0x1c to 0x1f that Rust's `trim` leaves alone.
fn py_strip(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
}

/// Normalises a program name (trimmed, lower case) and checks it: letters, digits, `. _ + -`
/// and spaces, 1 to 80 characters. Anything else is refused, a path separator above all,
/// because the name is matched against file names and must never look like a path.
pub fn clean_name(name: &str) -> Result<String, RulesError> {
    let n = py_strip(name).to_lowercase();
    let ok = (1..=PROGRAM_NAME_MAX).contains(&n.len())
        && n.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'+' | b' ' | b'-')
        });
    if ok { Ok(n) } else { Err(RulesError::BadName) }
}

/// The rules: lower-case program name to mode. Serialises as the plain JSON object the daemon
/// keeps in its state and the clients show.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Rules(BTreeMap<String, Mode>);

impl Rules {
    /// No rules.
    pub fn new() -> Rules {
        Rules::default()
    }

    /// Number of rules.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there are no rules.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The mode for an exact (already lower-case) name.
    pub fn get(&self, name: &str) -> Option<Mode> {
        self.0.get(name).copied()
    }

    /// The rules in name order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, Mode)> {
        self.0.iter().map(|(k, m)| (k.as_str(), *m))
    }

    /// The rules as the JSON object of the state file and the `game_rules` reply.
    pub fn to_json(&self) -> Value {
        Value::Object(self.0.iter().map(|(k, m)| (k.clone(), Value::from(m.name()))).collect::<Map<_, _>>())
    }

    /// Validates rules that arrived as decoded JSON (the stored state, a request) and returns
    /// them with the names cleaned. Names are checked in sorted order, so the first error is
    /// deterministic; each rule's mode is checked before its name. When two names clean to the
    /// same one the later in sorted order wins.
    pub fn from_json(v: &Value) -> Result<Rules, RulesError> {
        let obj = v.as_object().ok_or(RulesError::NotAMap)?;
        let mut out = BTreeMap::new();
        for (k, m) in obj {
            let mode = m.as_str().and_then(Mode::parse).ok_or_else(|| RulesError::BadMode(k.clone()))?;
            out.insert(clean_name(k)?, mode);
        }
        if out.len() > MAX_RULES {
            return Err(RulesError::TooMany);
        }
        Ok(Rules(out))
    }

    /// The rules with `name` (cleaned) set to `mode`; the receiver is unchanged.
    pub fn add(&self, name: &str, mode: &str) -> Result<Rules, RulesError> {
        let key = clean_name(name)?;
        let mode = Mode::parse(mode).ok_or(RulesError::BadAddMode)?;
        let mut out = self.0.clone();
        out.insert(key, mode);
        if out.len() > MAX_RULES {
            return Err(RulesError::TooMany);
        }
        Ok(Rules(out))
    }

    /// The rules without `name` (cleaned). A name with no rule is an error, so a typo is
    /// noticed.
    pub fn remove(&self, name: &str) -> Result<Rules, RulesError> {
        let key = clean_name(name)?;
        let mut out = self.0.clone();
        if out.remove(&key).is_none() {
            return Err(RulesError::NoRule(name.to_owned()));
        }
        Ok(Rules(out))
    }
}

/// What the matcher needs of one running program: the kernel's name for it and the start of
/// its command line (NUL-separated arguments).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Process {
    /// The contents of `/proc/PID/comm`.
    pub comm: Vec<u8>,
    /// The first bytes of `/proc/PID/cmdline`.
    pub cmdline: Vec<u8>,
}

/// The first `n` characters (not bytes) of `s`.
fn first_chars(s: &str, n: usize) -> &str {
    s.char_indices().nth(n).map_or(s, |(i, _)| &s[..i])
}

impl Process {
    /// The lower-case names this process answers to: its comm, the file name of its first
    /// argument, and the file name of any of the first twelve arguments ending in `.exe`.
    /// File names are also given cut to 15 characters, because the kernel truncates comm to
    /// that and a rule written from `ps` output has the short form.
    ///
    /// Bad UTF-8 is replaced, never an error: process names are arbitrary bytes.
    pub fn names(&self) -> BTreeSet<String> {
        let mut res = BTreeSet::new();
        let comm = py_strip(&String::from_utf8_lossy(&self.comm)).to_lowercase();
        if !comm.is_empty() {
            res.insert(comm);
        }
        let cmd = &self.cmdline[..self.cmdline.len().min(MAX_CMDLINE)];
        let args = cmd.split(|b| *b == 0).filter(|a| !a.is_empty()).take(MAX_ARGS);
        for (i, arg) in args.enumerate() {
            let text = String::from_utf8_lossy(arg).replace('\\', "/");
            let base = text.rsplit('/').next().unwrap_or("").to_lowercase();
            if i == 0 || base.ends_with(".exe") {
                res.insert(first_chars(&base, COMM_LEN).to_owned());
                res.insert(base);
            }
        }
        res
    }
}

/// The mode of the highest-ranked rule whose program is among `procs`, or `None`.
pub fn running(rules: &Rules, procs: &[Process]) -> Option<Mode> {
    if rules.is_empty() {
        return None;
    }
    let mut best: Option<Mode> = None;
    for p in procs {
        for name in p.names() {
            let mode = rules.get(&name).or_else(|| rules.get(first_chars(&name, COMM_LEN)));
            if let Some(m) = mode {
                if best.is_none_or(|b| m > b) {
                    best = Some(m);
                }
            }
        }
    }
    best
}

/// Where the kernel shows running processes.
pub const PROC_DIR: &str = "/proc";

/// Reads the running programs from a `/proc` tree. A process whose name cannot be read is
/// skipped, one whose command line cannot be read is kept with its name only: processes come
/// and go while the directory is read, so a miss is normal and never an error. An unreadable
/// root gives an empty list. Only the first 4096 bytes of a command line are read.
pub fn list_processes(proc_dir: &Path) -> Vec<Process> {
    let Ok(rd) = std::fs::read_dir(proc_dir) else { return Vec::new() };
    let mut procs = Vec::new();
    for entry in rd.filter_map(Result::ok) {
        let name = entry.file_name();
        if !name.to_str().is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())) {
            continue;
        }
        let dir = entry.path();
        let Some(comm) = read_prefix(&dir.join("comm"), 256) else { continue };
        let cmdline = read_prefix(&dir.join("cmdline"), MAX_CMDLINE).unwrap_or_default();
        procs.push(Process { comm, cmdline });
    }
    procs
}

fn read_prefix(path: &Path, limit: usize) -> Option<Vec<u8>> {
    let file = File::open(path).ok()?;
    let mut buf = Vec::new();
    file.take(limit as u64).read_to_end(&mut buf).ok()?;
    Some(buf)
}

/// Remembers the game mode between ticks: the process list is only read every
/// `SCAN_EVERY_TICKS` ticks.
#[derive(Debug, Clone, Default)]
pub struct Watcher {
    /// `None` until the first look; then the last answer.
    cached: Option<Option<Mode>>,
}

impl Watcher {
    /// A watcher that has not looked yet.
    pub fn new() -> Watcher {
        Watcher::default()
    }

    /// The mode asked for by a running game rule, or `None`. With no rules the answer is
    /// `None` without scanning. Otherwise `scan` (read the process list and call [`running`])
    /// is called on the first look and on every third tick, and the last answer is reused in
    /// between.
    pub fn current(
        &mut self,
        tick: u64,
        rules: &Rules,
        scan: impl FnOnce(&Rules) -> Option<Mode>,
    ) -> Option<Mode> {
        if rules.is_empty() {
            self.cached = Some(None);
            return None;
        }
        if tick % SCAN_EVERY_TICKS == 0 || self.cached.is_none() {
            self.cached = Some(scan(rules));
        }
        self.cached.flatten()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::smartprofile::testdir::TempDir;
    use serde_json::json;

    fn proc(comm: &str, args: &[&str]) -> Process {
        let mut cmdline = Vec::new();
        for a in args {
            cmdline.extend_from_slice(a.as_bytes());
            cmdline.push(0);
        }
        Process { comm: format!("{comm}\n").into_bytes(), cmdline }
    }

    fn rules(pairs: &[(&str, &str)]) -> Rules {
        Rules::from_json(&Value::Object(pairs.iter().map(|(k, v)| ((*k).to_owned(), json!(v))).collect()))
            .expect("valid rules")
    }

    #[test]
    fn clean_name_table() {
        let ok = [
            ("Game.EXE", "game.exe"),
            ("  hl2_linux ", "hl2_linux"),
            ("a", "a"),
            ("My Game+1-x", "my game+1-x"),
            ("\u{1c}x\u{1f}", "x"),
            ("\u{a0}x\u{2003}", "x"),
            ("\u{212a}", "k"),
        ];
        for (input, want) in ok {
            assert_eq!(clean_name(input).as_deref(), Ok(want), "{input:?}");
        }
        let long = "a".repeat(81);
        let bad = [
            "",
            "   ",
            "../x",
            "a/b",
            "a\\b",
            "x\0",
            "é",
            "a\nb",
            "a;b",
            "a*",
            "tab\tname",
            long.as_str(),
            "日本",
        ];
        for input in bad {
            assert_eq!(clean_name(input), Err(RulesError::BadName), "{input:?}");
        }
        assert!(clean_name(&"a".repeat(80)).is_ok(), "80 characters is the longest");
        assert_eq!(
            RulesError::BadName.to_string(),
            "program name: letters, digits, . _ + - and spaces (max 80)"
        );
    }

    #[test]
    fn rules_validate_clean_the_names_and_check_the_modes() {
        let r = Rules::from_json(&json!({"Game.EXE": "performance", " HL2_linux": "turbo"})).expect("valid");
        assert_eq!(r.get("game.exe"), Some(Mode::Performance));
        assert_eq!(r.get("hl2_linux"), Some(Mode::Turbo));
        assert_eq!(r.get("Game.EXE"), None, "stored names are cleaned");
        assert_eq!(r.len(), 2);
        assert_eq!(r.to_json(), json!({"game.exe": "performance", "hl2_linux": "turbo"}));
        assert_eq!(Rules::from_json(&r.to_json()).expect("round trip"), r);
        assert!(Rules::from_json(&json!({})).expect("empty is fine").is_empty());
    }

    #[test]
    fn invalid_rule_sets_are_refused_with_the_python_messages() {
        for bad in [json!(null), json!([]), json!("x"), json!(3), json!([["a", "eco"]])] {
            assert_eq!(Rules::from_json(&bad), Err(RulesError::NotAMap), "{bad}");
        }
        for mode in
            [json!("warp"), json!("Turbo"), json!(""), json!(5), json!(null), json!(["eco"]), json!({})]
        {
            let e = Rules::from_json(&json!({"x": mode})).expect_err("bad mode");
            assert_eq!(e.to_string(), "x: mode must be one of eco, quiet, balanced, performance, turbo");
        }
        assert_eq!(Rules::from_json(&json!({"../x": "turbo"})), Err(RulesError::BadName));
        // the mode of a rule is checked before its name
        assert_eq!(Rules::from_json(&json!({"../x": "warp"})), Err(RulesError::BadMode("../x".into())));
        let many: Map<String, Value> = (0..=MAX_RULES).map(|i| (format!("p{i}"), json!("eco"))).collect();
        assert_eq!(Rules::from_json(&Value::Object(many)), Err(RulesError::TooMany));
        let exact: Map<String, Value> = (0..MAX_RULES).map(|i| (format!("p{i}"), json!("eco"))).collect();
        assert_eq!(Rules::from_json(&Value::Object(exact)).expect("64 are allowed").len(), MAX_RULES);
        assert_eq!(RulesError::TooMany.to_string(), "at most 64 rules");
    }

    #[test]
    fn names_that_clean_to_the_same_one_collapse() {
        let r = Rules::from_json(&json!({"A": "eco", "a ": "turbo"})).expect("valid");
        assert_eq!(r.len(), 1);
        assert!(r.get("a").is_some());
    }

    #[test]
    fn add_and_remove_follow_the_daemon_handler() {
        let base = Rules::new();
        let r = base.add("Game.EXE", "performance").expect("add");
        assert_eq!(r.get("game.exe"), Some(Mode::Performance));
        assert!(base.is_empty(), "the receiver is unchanged");
        assert_eq!(r.add("game.exe", "turbo").expect("replace").get("game.exe"), Some(Mode::Turbo));
        assert_eq!(r.add("x", "warp"), Err(RulesError::BadAddMode));
        assert_eq!(RulesError::BadAddMode.to_string(), "mode: eco, quiet, balanced, performance, turbo");
        assert_eq!(r.add("../x", "turbo"), Err(RulesError::BadName));
        assert_eq!(r.add("../x", "warp"), Err(RulesError::BadName), "the name is checked first");
        assert_eq!(r.remove("nothing"), Err(RulesError::NoRule("nothing".into())));
        assert_eq!(r.remove("Nothing").expect_err("typo").to_string(), "no rule for 'Nothing'");
        assert!(r.remove(" GAME.exe ").expect("cleaned").is_empty());
        let full =
            (0..MAX_RULES).fold(Rules::new(), |acc, i| acc.add(&format!("p{i}"), "eco").expect("room"));
        assert_eq!(full.add("one-more", "eco"), Err(RulesError::TooMany));
        assert!(full.add("p3", "turbo").is_ok(), "changing an existing rule is not adding one");
    }

    #[test]
    fn a_process_answers_to_its_comm_its_program_and_its_exe_arguments() {
        let p = proc("hl2_linux", &["/home/u/hl2_linux", "-game", "hl2"]);
        let n = p.names();
        assert!(n.contains("hl2_linux") && !n.contains("-game") && !n.contains("hl2"), "{n:?}");
        let wine = proc("wine64-preloader", &["Z:\\games\\EldenRing.exe"]);
        let n = wine.names();
        assert!(n.contains("eldenring.exe") && n.contains("wine64-preloader"), "{n:?}");
        let proton = proc("steam", &["/usr/bin/steam", "-silent", "C:\\x\\Game.EXE", "other.exe", "--flag"]);
        let n = proton.names();
        assert!(
            n.contains("steam")
                && n.contains("game.exe")
                && n.contains("other.exe")
                && !n.contains("-silent"),
            "{n:?}"
        );
    }

    #[test]
    fn long_program_names_also_answer_to_their_first_fifteen_characters() {
        let p = proc("verylongprogram", &["/opt/verylongprogramname-bin"]);
        let n = p.names();
        assert!(n.contains("verylongprogramname-bin") && n.contains("verylongprogram"), "{n:?}");
        let r = rules(&[("verylongprogramname-bin", "turbo")]);
        assert_eq!(running(&r, std::slice::from_ref(&p)), Some(Mode::Turbo));
        let r = rules(&[("verylongprogram", "quiet")]);
        assert_eq!(
            running(&r, &[p]),
            Some(Mode::Quiet),
            "a rule written from ps output (15 characters) matches"
        );
        // the cut counts characters, never bytes, and never splits one
        assert_eq!(first_chars("ééééééééééééééééé", 15).chars().count(), 15);
        assert_eq!(first_chars("abc", 15), "abc");
        assert_eq!(first_chars("", 3), "");
    }

    #[test]
    fn only_the_first_twelve_arguments_and_4096_bytes_count() {
        let mut args: Vec<String> = vec!["/bin/sh".into()];
        args.extend((1..12).map(|i| format!("arg{i}")));
        args.push("late.exe".into()); // the 13th argument
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        assert!(!proc("sh", &refs).names().contains("late.exe"));
        args.remove(5); // now it is the 12th
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        assert!(proc("sh", &refs).names().contains("late.exe"));
        let mut p = proc("sh", &["/bin/sh"]);
        p.cmdline.extend(std::iter::repeat_n(b'x', 5000));
        p.cmdline.push(0);
        p.cmdline.extend_from_slice(b"hidden.exe\0");
        assert!(!p.names().contains("hidden.exe"), "beyond 4096 bytes nothing is read");
    }

    #[test]
    fn empty_arguments_are_skipped_before_counting() {
        let p = Process { comm: b"x\n".to_vec(), cmdline: b"\0\0/bin/real\0\0a.exe\0".to_vec() };
        let n = p.names();
        assert!(n.contains("real") && n.contains("a.exe"), "{n:?}");
    }

    #[test]
    fn hostile_process_data_never_panics() {
        let weird: [&[u8]; 6] = [b"", b"\xff\xfe\xfd", b"\0\0\0", b"\xc3", b"\\\\\\", b"/"];
        for comm in weird {
            for cmd in weird {
                let p = Process { comm: comm.to_vec(), cmdline: cmd.to_vec() };
                let _ = p.names();
                let _ = running(&rules(&[("x", "eco")]), &[p]);
            }
        }
        let p = Process { comm: vec![0xff; 100], cmdline: vec![0xc3; 10_000] };
        let _ = p.names();
        let _ = running(&Rules::new(), &[]);
    }

    #[test]
    fn the_highest_mode_wins_and_no_match_is_none() {
        let procs = [
            proc("bash", &["/bin/bash"]),
            proc("hl2_linux", &["/home/u/hl2_linux", "-game"]),
            proc("wine64-preloader", &["Z:\\games\\EldenRing.exe"]),
        ];
        assert_eq!(running(&rules(&[("hl2_linux", "performance")]), &procs), Some(Mode::Performance));
        assert_eq!(
            running(&rules(&[("eldenring.exe", "turbo"), ("hl2_linux", "performance")]), &procs),
            Some(Mode::Turbo),
            "the highest mode wins"
        );
        assert_eq!(
            running(&rules(&[("eldenring.exe", "eco"), ("hl2_linux", "quiet")]), &procs),
            Some(Mode::Quiet),
            "higher rank, not later or alphabetical"
        );
        assert_eq!(running(&rules(&[("notepad.exe", "turbo")]), &procs), None);
        assert_eq!(running(&Rules::new(), &procs), None);
        assert_eq!(running(&rules(&[("bash", "turbo")]), &[]), None);
        assert_eq!(running(&rules(&[("BASH", "turbo")]), &procs), Some(Mode::Turbo), "case does not matter");
    }

    #[test]
    fn list_processes_reads_a_fake_proc_tree() {
        let dir = TempDir::new("gr-proc");
        let mk = |pid: &str, comm: &str, cmdline: Option<&[u8]>| {
            let d = dir.path().join(pid);
            std::fs::create_dir_all(&d).expect("mkdir");
            std::fs::write(d.join("comm"), format!("{comm}\n")).expect("comm");
            if let Some(c) = cmdline {
                std::fs::write(d.join("cmdline"), c).expect("cmdline");
            }
        };
        mk("10", "bash", Some(b"/bin/bash\0"));
        mk("11", "hl2_linux", Some(b"/home/u/hl2_linux\0-game\0hl2\0"));
        mk("12", "wine64-preloader", Some(b"Z:\\games\\EldenRing.exe\0"));
        mk("13", "kthreadd", None); // no cmdline: kept with its name only
        std::fs::create_dir_all(dir.path().join("14")).expect("mkdir"); // no comm: skipped
        std::fs::create_dir_all(dir.path().join("self")).expect("mkdir"); // not a pid
        std::fs::write(dir.path().join("cpuinfo"), "x").expect("file");
        std::fs::write(dir.path().join("15"), "a file, not a process").expect("file");
        let procs = list_processes(dir.path());
        assert_eq!(procs.len(), 4, "{procs:?}");
        assert_eq!(running(&rules(&[("hl2_linux", "performance")]), &procs), Some(Mode::Performance));
        assert_eq!(
            running(&rules(&[("eldenring.exe", "turbo"), ("hl2_linux", "performance")]), &procs),
            Some(Mode::Turbo)
        );
        assert_eq!(running(&rules(&[("kthreadd", "quiet")]), &procs), Some(Mode::Quiet));
        assert_eq!(running(&rules(&[("notepad.exe", "turbo")]), &procs), None);
        assert!(
            list_processes(Path::new("/nonexistent/ts-decision-proc")).is_empty(),
            "an unreadable root is an empty list"
        );
    }

    #[test]
    fn a_huge_cmdline_file_is_read_only_in_part() {
        let dir = TempDir::new("gr-big");
        let d = dir.path().join("7");
        std::fs::create_dir_all(&d).expect("mkdir");
        std::fs::write(d.join("comm"), "big\n").expect("comm");
        std::fs::write(d.join("cmdline"), vec![b'a'; 1 << 20]).expect("cmdline");
        let procs = list_processes(dir.path());
        assert_eq!(procs[0].cmdline.len(), MAX_CMDLINE);
    }

    #[test]
    fn the_watcher_scans_on_the_first_look_and_every_third_tick() {
        let r = rules(&[("game", "turbo")]);
        let mut w = Watcher::new();
        let mut scans = 0;
        let mut answer = Some(Mode::Performance);
        let mut ask = |w: &mut Watcher, tick: u64, answer: Option<Mode>| {
            w.current(tick, &r, |_| {
                scans += 1;
                answer
            })
        };
        assert_eq!(
            ask(&mut w, 1, answer),
            Some(Mode::Performance),
            "never looked: scan now, whatever the tick"
        );
        answer = None;
        assert_eq!(ask(&mut w, 2, answer), Some(Mode::Performance), "tick 2 reuses the last answer");
        assert_eq!(ask(&mut w, 3, answer), None, "tick 3 looks again: the game ended");
        assert_eq!(ask(&mut w, 4, Some(Mode::Turbo)), None, "the new game is seen only at the next look");
        assert_eq!(ask(&mut w, 6, Some(Mode::Turbo)), Some(Mode::Turbo));
        assert_eq!(scans, 3);
    }

    #[test]
    fn the_watcher_with_no_rules_never_scans_and_waits_for_the_next_third_tick_afterwards() {
        let mut w = Watcher::new();
        assert_eq!(w.current(1, &Rules::new(), |_| panic!("no scan without rules")), None);
        let r = rules(&[("game", "turbo")]);
        // Python marks the cache as filled when there were no rules: a rule added at tick 4 is
        // looked for only from tick 6 on
        assert_eq!(w.current(4, &r, |_| Some(Mode::Turbo)), None);
        assert_eq!(w.current(6, &r, |_| Some(Mode::Turbo)), Some(Mode::Turbo));
    }
}
