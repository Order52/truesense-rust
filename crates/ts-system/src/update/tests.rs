//! Tests of the updater with a fake network, a fake machine and a temporary directory.

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::sync::Mutex;

use super::extract::testkit::{self, Entry, TempDir};
use super::*;

/// The files of a fake release archive.
type Files = Vec<(&'static str, Vec<u8>)>;

const API: &str = "https://api.github.com/repos/o/r";
const WEB: &str = "https://github.com/o/r";

/// What GitHub serves, by URL; anything unknown is a 404. Remembers every request.
#[derive(Default)]
struct FakeNet {
    routes: Mutex<HashMap<String, Result<Vec<u8>, FetchError>>>,
    asked: Mutex<Vec<String>>,
}

impl FakeNet {
    fn serve(&self, url: &str, body: &[u8]) {
        self.routes.lock().unwrap().insert(url.to_owned(), Ok(body.to_vec()));
    }
    fn fail(&self, url: &str, error: FetchError) {
        self.routes.lock().unwrap().insert(url.to_owned(), Err(error));
    }
    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
}

impl HttpFetcher for FakeNet {
    fn get(&self, request: &HttpRequest<'_>) -> Result<Vec<u8>, FetchError> {
        self.asked.lock().unwrap().push(request.url.to_owned());
        match self.routes.lock().unwrap().get(request.url) {
            Some(Ok(body)) if body.len() as u64 > request.max_bytes => {
                Err(FetchError::TooLarge { limit: request.max_bytes })
            }
            Some(answer) => answer.clone(),
            None => Err(FetchError::Status { code: 404, url: request.url.to_owned() }),
        }
    }
}

/// A machine that records what is run and answers from fields.
struct FakeHost {
    euid: u32,
    env: HashMap<String, String>,
    programs: HashSet<&'static str>,
    agent: bool,
    installed: Mutex<Option<String>>,
    exit_code: i32,
    spawn_error: bool,
    /// Each run: the command and, for an installer, whether it existed and was executable then.
    runs: Mutex<Vec<(Vec<String>, bool)>>,
}

impl FakeHost {
    fn user() -> Self {
        Self {
            euid: 1000,
            env: HashMap::new(),
            programs: ["sudo", "pkexec", "foot"].into_iter().collect(),
            agent: false,
            installed: Mutex::new(None),
            exit_code: 0,
            spawn_error: false,
            runs: Mutex::new(Vec::new()),
        }
    }
    fn runs(&self) -> Vec<Vec<String>> {
        self.runs.lock().unwrap().iter().map(|(a, _)| a.clone()).collect()
    }
}

impl Host for FakeHost {
    fn euid(&self) -> u32 {
        self.euid
    }
    fn env(&self, key: &str) -> Option<String> {
        self.env.get(key).cloned()
    }
    fn has_program(&self, name: &str) -> bool {
        self.programs.contains(name)
    }
    fn polkit_agent_running(&self) -> bool {
        self.agent
    }
    fn installed_version(&self) -> Option<String> {
        self.installed.lock().unwrap().clone()
    }
    fn run_attached(&self, argv: &[String]) -> io::Result<i32> {
        let script = argv.last().map(Path::new);
        let ready =
            script.and_then(|p| fs::metadata(p).ok()).is_some_and(|m| m.is_file() && m.mode() & 0o111 != 0);
        self.runs.lock().unwrap().push((argv.to_vec(), ready));
        if self.spawn_error { Err(io::Error::from(io::ErrorKind::NotFound)) } else { Ok(self.exit_code) }
    }
}

/// A release archive: files below `truesense-<version>/` (or `root`).
fn release_files(version: &str) -> Files {
    vec![
        ("tools/install.sh", b"#!/bin/sh\n".to_vec()),
        (
            "Cargo.toml",
            format!("[workspace.package]\nversion = \"{version}\"\nrust-version = \"1.85\"\n").into_bytes(),
        ),
        ("kernel/truesense-wmi/truesense_wmi.c", b"//".to_vec()),
    ]
}

fn tarball(files: &[(&str, Vec<u8>)], root: &str) -> Vec<u8> {
    let entries: Vec<Entry> = files
        .iter()
        .map(|(name, body)| {
            let name = if root.is_empty() { (*name).to_owned() } else { format!("{root}/{name}") };
            Entry { name, typeflag: b'0', link: String::new(), body: body.clone(), mode: 0o644 }
        })
        .collect();
    testkit::archive(&entries)
}

fn sha(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn sums_for(version: &str, blob: &[u8]) -> Vec<u8> {
    format!("{}  truesense-{version}.tar.gz\n", sha(blob)).into_bytes()
}

/// A fake world: a network serving release `v<version>` as `blob`, a host, a temp dir.
struct World {
    net: Arc<FakeNet>,
    host: Arc<FakeHost>,
    tmp: TempDir,
}

impl World {
    fn new(host: FakeHost) -> Self {
        Self { net: Arc::new(FakeNet::default()), host: Arc::new(host), tmp: TempDir::new() }
    }

    fn serve_release(&self, version: &str, blob: &[u8], sums: Option<Vec<u8>>) {
        self.net.serve(&format!("{WEB}/releases/download/v{version}/truesense-{version}.tar.gz"), blob);
        if let Some(sums) = sums {
            self.net.serve(&format!("{WEB}/releases/download/v{version}/SHA256SUMS"), &sums);
        }
    }

    fn serve_github(&self, release_tag: Option<&str>, tags: &[&str]) {
        if let Some(tag) = release_tag {
            let body = format!(
                "{{\"tag_name\":\"{tag}\",\"html_url\":\"https://github.com/o/r/releases/tag/{tag}\",\"body\":\"notes\"}}"
            );
            self.net.serve(&format!("{API}/releases/latest"), body.as_bytes());
        }
        let list: Vec<String> = tags.iter().map(|t| format!("{{\"name\":\"{t}\"}}")).collect();
        self.net.serve(&format!("{API}/tags?per_page=30"), format!("[{}]", list.join(",")).as_bytes());
    }

    fn updater(&self) -> Updater {
        Updater::new("o/r", self.net.clone(), self.host.clone())
            .unwrap()
            .with_current("0.4.0")
            .with_clock(Arc::new(|| 1_000_000.0))
            .with_cache_path(Some(self.tmp.0.join("cache/update.json")))
            .with_temp_dir(self.tmp.0.join("work"))
    }

    fn work_entries(&self) -> usize {
        fs::read_dir(self.tmp.0.join("work")).map_or(0, Iterator::count)
    }

    /// A world serving a good release 9.9.9 with a good SHA256SUMS.
    fn with_good_release() -> (Self, Vec<u8>) {
        let world = Self::new(FakeHost::user());
        fs::create_dir_all(world.tmp.0.join("work")).unwrap();
        let blob = tarball(&release_files("9.9.9"), "truesense-9.9.9");
        world.serve_release("9.9.9", &blob, Some(sums_for("9.9.9", &blob)));
        world.serve_github(Some("v9.9.9"), &["v9.9.9", "v0.4.0"]);
        (world, blob)
    }
}

// ---- checking ------------------------------------------------------------------------------

#[test]
fn a_check_finds_a_newer_release_and_uses_the_cache_until_it_is_old() {
    let world = World::new(FakeHost::user());
    world.serve_github(Some("v9.9.9"), &["v9.9.9"]);
    let now = Arc::new(Mutex::new(1_000_000.0));
    let clock = now.clone();
    let updater = world.updater().with_clock(Arc::new(move || *clock.lock().unwrap()));
    let status = updater.check(true, None);
    assert!(status.available);
    assert_eq!((status.current.as_str(), status.latest.as_deref()), ("0.4.0", Some("9.9.9")));
    assert_eq!(status.error, None);
    assert_eq!(status.checked_at, 1_000_000.0);
    let info = status.info.unwrap();
    assert_eq!((info.tag.as_str(), info.notes.as_str()), ("v9.9.9", "notes"));
    let requests = world.net.asked().len();
    assert_eq!(requests, 2, "the release and the tag list");
    // Two more checks within the day are answered from the cache.
    *now.lock().unwrap() += 3600.0;
    assert!(updater.check(false, None).available);
    assert!(updater.check(false, None).available);
    assert_eq!(world.net.asked().len(), requests, "a fresh cache is used instead of asking GitHub again");
    assert_eq!(updater.check(false, None).checked_at, 1_000_000.0, "a cached answer keeps its own time");
    // Older than the window: asked again. A shorter window makes it older sooner.
    assert_eq!(updater.check(false, Some(1)).checked_at, 1_003_600.0);
    assert_eq!(world.net.asked().len(), requests + 2);
    *now.lock().unwrap() += 24.0 * 3600.0 + 1.0;
    updater.check(false, None);
    assert_eq!(world.net.asked().len(), requests + 4);
    updater.check(false, Some(0));
    assert_eq!(world.net.asked().len(), requests + 6, "0 hours: always ask");
}

#[test]
fn offline_the_last_known_answer_survives_and_the_error_is_reported() {
    let world = World::new(FakeHost::user());
    world.serve_github(Some("v9.9.9"), &["v9.9.9"]);
    let updater = world.updater();
    updater.check(true, None);
    world.net.fail(&format!("{API}/releases/latest"), FetchError::Transport("no network".into()));
    let status = updater.check(true, None);
    assert_eq!(status.error.as_deref(), Some("could not reach GitHub: no network"));
    assert_eq!(status.latest.as_deref(), Some("9.9.9"), "the last known answer survives being offline");
    assert!(status.available);
    // The failure is cached as such: the next ordinary check asks again instead of trusting it.
    let before = world.net.asked().len();
    updater.check(false, None);
    assert!(world.net.asked().len() > before);
    // With nothing cached at all the answer is empty.
    let world = World::new(FakeHost::user());
    world.net.fail(&format!("{API}/releases/latest"), FetchError::Status { code: 403, url: "u".into() });
    let status = world.updater().check(false, None);
    assert_eq!((status.latest, status.info, status.available), (None, None, false));
    assert_eq!(status.error.as_deref(), Some("could not reach GitHub: HTTP 403 from u"));
}

#[test]
fn the_running_version_decides_whether_a_release_is_available() {
    let world = World::new(FakeHost::user());
    world.serve_github(Some("v0.5.0"), &["v0.5.0"]);
    for (current, available) in [
        ("0.4.9", true),
        ("0.5.0", false),
        ("0.5.1", false),
        ("0.10.0", false),
        ("v0.4.0", true),
        ("not-a-version", false),
    ] {
        let status = world.updater().with_current(current).check(true, None);
        assert_eq!(status.available, available, "current {current}");
        assert_eq!(status.current, current);
    }
}

#[test]
fn a_newer_tag_beats_an_older_release_page() {
    let world = World::new(FakeHost::user());
    world.serve_github(Some("v0.3.0"), &["v0.3.1", "v0.3.0", "junk", "v0.3.1-rc1"]);
    let info = world.updater().latest().unwrap();
    assert_eq!(
        (info.version.as_str(), info.tag.as_str()),
        ("0.3.1", "v0.3.1"),
        "a tag newer than the latest release page must win"
    );
    assert_eq!(info.url, "https://github.com/o/r/releases/tag/v0.3.1");
    assert_eq!(info.notes, "");
    // The same tag in both places: the release page's notes are kept.
    world.serve_github(Some("v0.3.2"), &["v0.3.2", "v0.3.1"]);
    let info = world.updater().latest().unwrap();
    assert_eq!((info.tag.as_str(), info.notes.as_str()), ("v0.3.2", "notes"));
}

#[test]
fn a_repository_with_tags_but_no_release_page_still_has_a_latest() {
    let world = World::new(FakeHost::user());
    world.serve_github(None, &["v0.2.0", "v0.10.0", "v0.9.9"]);
    assert_eq!(world.updater().latest().unwrap().tag, "v0.10.0");
    let world = World::new(FakeHost::user());
    world.serve_github(None, &["nightly", "junk"]);
    assert_eq!(
        world.updater().latest().unwrap_err().to_string(),
        "no release or version tag found in the repository"
    );
}

#[test]
fn github_answers_that_are_not_what_the_api_documents_are_errors_not_panics() {
    let world = World::new(FakeHost::user());
    world.net.serve(&format!("{API}/releases/latest"), b"not json");
    world.net.serve(&format!("{API}/tags?per_page=30"), b"[]");
    assert!(matches!(world.updater().latest(), Err(UpdateError::BadAnswer(_))));
    for release in ["null", "[]", "{}", "{\"tag_name\":5}", "{\"tag_name\":null,\"body\":7}", "\"x\""] {
        world.net.serve(&format!("{API}/releases/latest"), release.as_bytes());
        for tags in ["{}", "null", "[1,null,{\"name\":5},{}]", "\"x\""] {
            world.net.serve(&format!("{API}/tags?per_page=30"), tags.as_bytes());
            let _ = world.updater().latest();
        }
    }
    world.net.serve(&format!("{API}/releases/latest"), b"{\"tag_name\":\"v1.0.0\"}");
    world.net.serve(&format!("{API}/tags?per_page=30"), b"[1,null,{\"name\":5},{}]");
    assert_eq!(world.updater().latest().unwrap().tag, "v1.0.0", "garbage entries are skipped");
    world.net.serve(&format!("{API}/tags?per_page=30"), b"{}");
    assert!(matches!(world.updater().latest(), Err(UpdateError::BadAnswer(_))));
}

#[test]
fn notes_are_cut_to_1500_characters_not_bytes() {
    let world = World::new(FakeHost::user());
    let body = "é".repeat(2000);
    world.net.serve(
        &format!("{API}/releases/latest"),
        format!("{{\"tag_name\":\"v1.0.0\",\"body\":\"{body}\"}}").as_bytes(),
    );
    world.net.serve(&format!("{API}/tags?per_page=30"), b"[]");
    assert_eq!(world.updater().latest().unwrap().notes.chars().count(), 1500);
}

#[test]
fn only_the_system_config_names_the_repository() {
    let tmp = TempDir::new();
    let missing = tmp.0.join("none.toml");
    assert_eq!(system_repo(&missing).unwrap(), "Order52/truesense", "the built-in default");
    let system = tmp.0.join("system.toml");
    fs::write(&system, "[update]\nrepo = \"fork/truesense\"\n").unwrap();
    assert_eq!(system_repo(&system).unwrap(), "fork/truesense");
    fs::write(&system, "[update]\nrepo = \"../..\"\n").unwrap();
    assert_eq!(
        system_repo(&system).unwrap_err().to_string(),
        "[update] repo must look like owner/name, not \"../..\""
    );
    fs::write(&system, "[update\nrepo = ").unwrap();
    assert!(matches!(system_repo(&system), Err(UpdateError::Config(_))));
    // The user's config is not an input: there is no way to pass it.
    let world = World::new(FakeHost::user());
    assert!(matches!(
        Updater::new("evil", world.net.clone(), world.host.clone()),
        Err(UpdateError::InvalidRepo(_))
    ));
    assert_eq!(DEFAULT_REPO, "Order52/truesense");
}

#[test]
fn the_cache_file_is_compatible_with_the_python_edition_and_remembers_notifications() {
    let world = World::new(FakeHost::user());
    let path = world.tmp.0.join("cache/update.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    // Exactly what the Python edition wrote.
    fs::write(
        &path,
        r#"{"checked_at": 999999.0, "info": {"version": "9.9.9", "tag": "v9.9.9", "url": "u", "notes": ""}, "error": null}"#,
    )
    .unwrap();
    let updater = world.updater();
    let status = updater.check(false, None);
    assert!(status.available && world.net.asked().is_empty(), "the Python cache is read as it is");
    assert_eq!(updater.notified(), None);
    updater.mark_notified("9.9.9");
    assert_eq!(updater.notified().as_deref(), Some("9.9.9"));
    // A refresh keeps the notification (the daily timer must notify once per version).
    world.serve_github(Some("v9.9.9"), &["v9.9.9"]);
    updater.check(true, None);
    assert_eq!(updater.notified().as_deref(), Some("9.9.9"));
    let written: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(written["info"]["tag"], "v9.9.9");
    assert_eq!(written["error"], Value::Null);
    assert_eq!(written["checked_at"], 1_000_000.0);
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o644);
}

#[test]
fn a_corrupt_or_unwritable_cache_is_ignored() {
    let world = World::new(FakeHost::user());
    world.serve_github(Some("v9.9.9"), &["v9.9.9"]);
    let path = world.tmp.0.join("cache/update.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    for junk in ["", "not json", "[]", "{\"checked_at\": \"x\"}", "{\"info\": 5}", "\u{0}"] {
        fs::write(&path, junk).unwrap();
        assert!(world.updater().check(false, None).available, "{junk:?}");
    }
    // A cache path below a file cannot be written; the check still answers.
    let blocked = world.tmp.0.join("file");
    fs::write(&blocked, "x").unwrap();
    let updater = world.updater().with_cache_path(Some(blocked.join("update.json")));
    assert!(updater.check(true, None).available);
    assert!(world.updater().with_cache_path(None).check(true, None).available);
    updater.mark_notified("1.0.0");
    assert_eq!(updater.notified(), None);
}

#[test]
fn the_cache_path_follows_xdg_then_home() {
    let env = |pairs: &'static [(&'static str, &'static str)]| {
        move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| (*v).to_owned())
    };
    assert_eq!(
        default_cache_path(&env(&[("XDG_CACHE_HOME", "/x"), ("HOME", "/h")])),
        Some(PathBuf::from("/x/truesense/update.json"))
    );
    assert_eq!(
        default_cache_path(&env(&[("HOME", "/h")])),
        Some(PathBuf::from("/h/.cache/truesense/update.json"))
    );
    assert_eq!(
        default_cache_path(&env(&[("XDG_CACHE_HOME", "relative"), ("HOME", "/h")])),
        Some(PathBuf::from("/h/.cache/truesense/update.json"))
    );
    assert_eq!(default_cache_path(&env(&[])), None);
}

#[test]
fn status_and_apply_results_serialise_with_the_0_3_x_field_names() {
    let world = World::new(FakeHost::user());
    world.serve_github(Some("v9.9.9"), &["v9.9.9"]);
    let status = serde_json::to_value(world.updater().check(true, None)).unwrap();
    for key in ["current", "latest", "available", "checked_at", "error", "info"] {
        assert!(status.get(key).is_some(), "{key}");
    }
    assert_eq!(status["info"]["version"], "9.9.9");
    let result = ApplyResult {
        version: "9.9.9".into(),
        sha256: "ab".into(),
        verified: Some(true),
        returncode: 0,
        via: "installer",
        installed: None,
    };
    assert_eq!(
        serde_json::to_value(&result).unwrap(),
        serde_json::json!({"version": "9.9.9", "sha256": "ab", "verified": true, "returncode": 0, "via": "installer"})
    );
    let terminal = ApplyResult {
        version: "9.9.9".into(),
        sha256: String::new(),
        verified: None,
        returncode: 1,
        via: "terminal",
        installed: Some("0.4.0".into()),
    };
    assert_eq!(
        serde_json::to_value(&terminal).unwrap(),
        serde_json::json!({"version": "9.9.9", "sha256": "", "returncode": 1, "via": "terminal", "installed": "0.4.0"})
    );
}

// ---- downloading ---------------------------------------------------------------------------

#[test]
fn a_good_release_is_verified_unpacked_and_its_installer_made_executable() {
    let (world, blob) = World::with_good_release();
    let release = world.updater().download("v9.9.9").unwrap();
    assert_eq!(release.sha256(), sha(&blob));
    assert_eq!(release.sha256().len(), 64);
    let installer = release.installer();
    assert_eq!(fs::metadata(&installer).unwrap().mode() & 0o777, 0o755);
    assert_eq!(installer, release.root().join("tools/install.sh"));
    assert!(release.root().ends_with("truesense-9.9.9"));
    let work = release.root().parent().unwrap().parent().unwrap().to_path_buf();
    assert_eq!(fs::metadata(&work).unwrap().mode() & 0o777, 0o700, "the download folder is private");
    assert!(work.starts_with(world.tmp.0.join("work")));
    assert_eq!(
        world.net.asked(),
        [
            format!("{WEB}/releases/download/v9.9.9/SHA256SUMS"),
            format!("{WEB}/releases/download/v9.9.9/truesense-9.9.9.tar.gz")
        ],
        "the list is fetched first, then the archive it names"
    );
    drop(release);
    assert!(!work.exists(), "dropping the release deletes everything");
}

#[test]
fn the_tag_may_be_given_without_a_v_or_with_blanks_and_the_url_stays_clean() {
    let (world, _blob) = World::with_good_release();
    let release = world.updater().download(" v9.9.9\n").unwrap();
    assert!(
        world.net.asked().iter().all(|u| !u.contains(' ') && !u.contains('\n')),
        "{:?}",
        world.net.asked()
    );
    drop(release);
}

#[test]
fn a_release_that_is_not_what_the_tag_says_is_refused_and_leaves_nothing() {
    let cases: Vec<(&str, Files, &str, &str)> = vec![
        (
            "wrong version inside",
            release_files("9.9.8"),
            "truesense-9.9.9",
            "the download says version 9.9.8, not v9.9.9; refusing",
        ),
        (
            "no version at all",
            release_files("9.9.9")
                .into_iter()
                .map(
                    |(n, b)| {
                        if n == "Cargo.toml" { (n, b"[package]\nname = \"x\"\n".to_vec()) } else { (n, b) }
                    },
                )
                .collect(),
            "truesense-9.9.9",
            "the download says version ?, not v9.9.9; refusing",
        ),
        (
            "an unparsable version",
            release_files("9.9.9")
                .into_iter()
                .map(|(n, b)| if n == "Cargo.toml" { (n, b"version = \"main\"\n".to_vec()) } else { (n, b) })
                .collect(),
            "truesense-9.9.9",
            "the download says version main, not v9.9.9; refusing",
        ),
        (
            "not a release: no installer",
            release_files("9.9.9").into_iter().filter(|(n, _)| *n != "tools/install.sh").collect(),
            "truesense-9.9.9",
            "the download is not a TrueSense release (missing tools/install.sh)",
        ),
        (
            "not a release: no kernel module",
            release_files("9.9.9").into_iter().filter(|(n, _)| !n.starts_with("kernel")).collect(),
            "truesense-9.9.9",
            "the download is not a TrueSense release (missing kernel/truesense-wmi/truesense_wmi.c)",
        ),
        ("files without a top folder", release_files("9.9.9"), "", "unexpected layout in the download"),
    ];
    for (label, files, root, message) in cases {
        let world = World::new(FakeHost::user());
        fs::create_dir_all(world.tmp.0.join("work")).unwrap();
        let blob = if label == "two top folders" {
            let mut entries: Vec<Entry> = release_files("9.9.9")
                .into_iter()
                .map(|(n, b)| Entry {
                    name: format!("a/{n}"),
                    typeflag: b'0',
                    link: String::new(),
                    body: b,
                    mode: 0o644,
                })
                .collect();
            entries.push(Entry {
                name: "b/extra".into(),
                typeflag: b'0',
                link: String::new(),
                body: vec![],
                mode: 0o644,
            });
            testkit::archive(&entries)
        } else {
            tarball(&files, root)
        };
        world.serve_release("9.9.9", &blob, Some(sums_for("9.9.9", &blob)));
        let text = world.updater().download("v9.9.9").unwrap_err().to_string();
        if label == "two top folders" {
            assert_eq!(text, "unexpected layout in the download");
        } else {
            assert_eq!(text, message, "{label}");
        }
        assert_eq!(world.work_entries(), 0, "{label}: nothing is left behind");
    }
}

#[test]
fn a_symlink_a_path_escape_or_a_special_file_in_the_archive_is_refused() {
    let evil: Vec<(&str, Entry)> = vec![
        (
            "path escape",
            Entry {
                name: "../evil.sh".into(),
                typeflag: b'0',
                link: String::new(),
                body: b"x".to_vec(),
                mode: 0o644,
            },
        ),
        (
            "symlink",
            Entry {
                name: "truesense-9.9.9/l".into(),
                typeflag: b'2',
                link: "/etc/passwd".into(),
                body: vec![],
                mode: 0o644,
            },
        ),
        (
            "hard link",
            Entry {
                name: "truesense-9.9.9/h".into(),
                typeflag: b'1',
                link: "tools/install.sh".into(),
                body: vec![],
                mode: 0o644,
            },
        ),
        (
            "device",
            Entry {
                name: "truesense-9.9.9/dev".into(),
                typeflag: b'3',
                link: String::new(),
                body: vec![],
                mode: 0o644,
            },
        ),
    ];
    for (label, extra) in evil {
        let world = World::new(FakeHost::user());
        fs::create_dir_all(world.tmp.0.join("work")).unwrap();
        let mut entries = vec![extra];
        entries.extend(release_files("9.9.9").into_iter().map(|(n, b)| Entry {
            name: format!("truesense-9.9.9/{n}"),
            typeflag: b'0',
            link: String::new(),
            body: b,
            mode: 0o644,
        }));
        let blob = testkit::archive(&entries);
        world.serve_release("9.9.9", &blob, Some(sums_for("9.9.9", &blob)));
        let err = world.updater().download("v9.9.9").unwrap_err();
        assert!(matches!(err, UpdateError::Extract(ExtractError::Unsafe(_))), "{label}: {err}");
        assert_eq!(world.work_entries(), 0, "{label}");
        assert!(!world.tmp.0.join("evil.sh").exists() && !world.tmp.0.join("work/evil.sh").exists());
    }
}

#[test]
fn an_archive_that_is_not_gzip_is_refused_after_the_checksum_passed() {
    let world = World::new(FakeHost::user());
    fs::create_dir_all(world.tmp.0.join("work")).unwrap();
    world.serve_release("9.9.9", b"plain text", Some(sums_for("9.9.9", b"plain text")));
    assert_eq!(
        world.updater().download("v9.9.9").unwrap_err().to_string(),
        "the download is not a gzip archive"
    );
    assert_eq!(world.work_entries(), 0);
}

#[test]
fn only_version_tags_are_downloaded() {
    let (world, _blob) = World::with_good_release();
    for tag in ["main", "", "v9.9", "v9.9.9-rc1", "../../x", "v9.9.9/../../evil", "v9.9.9?x=1", "latest"] {
        let err = world.updater().download(tag).unwrap_err();
        assert!(matches!(err, UpdateError::NotAVersionTag(_)), "{tag:?}");
    }
    assert!(world.net.asked().is_empty(), "nothing is requested for a name that is not a version");
    assert_eq!(world.updater().download("main").unwrap_err().to_string(), "not a version tag: \"main\"");
}

#[test]
fn a_checksum_mismatch_refuses_the_download() {
    let (world, blob) = World::with_good_release();
    world.serve_release(
        "9.9.9",
        &blob,
        Some(format!("{}  truesense-9.9.9.tar.gz\n", "0".repeat(64)).into_bytes()),
    );
    let text = world.updater().download("v9.9.9").unwrap_err().to_string();
    assert!(
        text.starts_with("checksum mismatch for truesense-9.9.9.tar.gz (expected 000000000000…, got "),
        "{text}"
    );
    assert!(text.ends_with("…): refusing"));
    assert!(text.contains(&sha(&blob)[..12]));
    assert_eq!(world.work_entries(), 0);
    // A tampered archive with the original list.
    let (world, blob) = World::with_good_release();
    let mut tampered = blob.clone();
    let last = tampered.len() / 2;
    tampered[last] ^= 1;
    world.serve_release("9.9.9", &tampered, None);
    assert!(matches!(world.updater().download("v9.9.9"), Err(UpdateError::ChecksumMismatch { .. })));
}

#[test]
fn a_release_without_a_checksum_list_is_always_refused_whatever_its_version() {
    for version in ["9.9.9", "0.3.15", "0.1.0"] {
        let world = World::new(FakeHost::user());
        let blob = tarball(&release_files(version), &format!("truesense-{version}"));
        world.serve_release(version, &blob, None);
        let text = world.updater().download(&format!("v{version}")).unwrap_err().to_string();
        assert!(text.contains("no SHA256SUMS") && text.contains("refusing"), "{text}");
        assert!(!world.net.asked().iter().any(|u| u.ends_with(".tar.gz")), "the archive is not even fetched");
    }
}

#[test]
fn a_list_that_does_not_name_the_archive_is_refused() {
    let (world, blob) = World::with_good_release();
    world.serve_release(
        "9.9.9",
        &blob,
        Some(format!("{}  something-else.tar.gz\n", sha(&blob)).into_bytes()),
    );
    assert_eq!(
        world.updater().download("v9.9.9").unwrap_err().to_string(),
        "SHA256SUMS of v9.9.9 does not list truesense-9.9.9.tar.gz: refusing"
    );
    world.serve_release("9.9.9", &blob, Some(b"".to_vec()));
    assert!(matches!(world.updater().download("v9.9.9"), Err(UpdateError::NotListed { .. })));
    let clash =
        format!("{0}  truesense-9.9.9.tar.gz\n{1}  truesense-9.9.9.tar.gz\n", sha(&blob), "1".repeat(64));
    world.serve_release("9.9.9", &blob, Some(clash.into_bytes()));
    assert!(
        matches!(world.updater().download("v9.9.9"), Err(UpdateError::Checksums(_))),
        "an ambiguous list is no anchor"
    );
}

#[test]
fn offline_never_means_skip_the_check() {
    let (world, _blob) = World::with_good_release();
    world
        .net
        .fail(&format!("{WEB}/releases/download/v9.9.9/SHA256SUMS"), FetchError::Transport("offline".into()));
    let text = world.updater().download("v9.9.9").unwrap_err().to_string();
    assert_eq!(text, "could not fetch the release checksums (offline); try again later");
    world.net.fail(
        &format!("{WEB}/releases/download/v9.9.9/SHA256SUMS"),
        FetchError::Status { code: 503, url: "u".into() },
    );
    assert_eq!(
        world.updater().download("v9.9.9").unwrap_err().to_string(),
        "could not fetch the release checksums (HTTP 503); try again later"
    );
    world
        .net
        .fail(&format!("{WEB}/releases/download/v9.9.9/SHA256SUMS"), FetchError::TooLarge { limit: 1 << 20 });
    assert!(matches!(world.updater().download("v9.9.9"), Err(UpdateError::ChecksumsFetch(_))));
    assert!(!world.net.asked().iter().any(|u| u.ends_with(".tar.gz")));
}

#[test]
fn a_failing_archive_download_is_reported_and_an_oversized_one_refused() {
    let (world, blob) = World::with_good_release();
    let archive_url = format!("{WEB}/releases/download/v9.9.9/truesense-9.9.9.tar.gz");
    world.net.fail(&archive_url, FetchError::Transport("connection reset".into()));
    assert_eq!(
        world.updater().download("v9.9.9").unwrap_err().to_string(),
        "could not download the release (connection reset); try again later"
    );
    world.net.serve(&archive_url, &blob);
    let err = world.updater().with_max_download(10).download("v9.9.9").unwrap_err();
    assert_eq!(err.to_string(), "the download is unexpectedly large; refusing");
    assert_eq!(world.work_entries(), 0);
}

#[test]
fn the_python_layout_is_available_for_the_old_releases() {
    let world = World::new(FakeHost::user());
    fs::create_dir_all(world.tmp.0.join("work")).unwrap();
    let files = vec![
        ("tools/install.sh", b"#!/bin/sh\n".to_vec()),
        ("src/truesense/__init__.py", b"__version__ = \"0.3.17\"\n".to_vec()),
        ("kernel/truesense-wmi/truesense_wmi.c", b"//".to_vec()),
    ];
    let blob = tarball(&files, "truesense-0.3.17");
    world.serve_release("0.3.17", &blob, Some(sums_for("0.3.17", &blob)));
    assert!(world.updater().download("v0.3.17").is_err(), "the Rust layout does not accept it");
    assert!(world.updater().with_layout(Layout::python_edition()).download("v0.3.17").is_ok());
}

#[test]
fn the_version_is_read_from_the_first_standalone_assignment() {
    for (text, key, want) in [
        ("version = \"1.2.3\"\nrust-version = \"1.85\"", "version", Some("1.2.3")),
        ("rust-version = \"1.85\"\nversion = \"1.2.3\"", "version", Some("1.2.3")),
        ("rust-version = \"1.85\"", "version", None),
        ("my_version = \"1\"", "version", None),
        ("version=\"4.5.6\"", "version", Some("4.5.6")),
        ("version  =\n  \"4.5.6\"", "version", Some("4.5.6")),
        ("version = \"\"\nversion = \"7.8.9\"", "version", Some("7.8.9")),
        ("version = 5\nversion = \"7.8.9\"", "version", Some("7.8.9")),
        ("version = \"unterminated", "version", None),
        ("__version__ = \"0.3.17\"", "__version__", Some("0.3.17")),
        ("x.__version__ = \"0.3.17\"", "__version__", Some("0.3.17")),
        ("", "version", None),
        ("éversion = \"1\"", "version", None),
    ] {
        assert_eq!(quoted_assignment(text, key).as_deref(), want, "{text:?}");
    }
}

#[test]
fn a_work_folder_never_reuses_a_name_that_exists() {
    let tmp = TempDir::new();
    let first = WorkDir::create(&tmp.0).unwrap();
    let second = WorkDir::create(&tmp.0).unwrap();
    assert_ne!(first.path, second.path);
    let (a, b) = (first.path.clone(), second.path.clone());
    drop(first);
    assert!(!a.exists() && b.exists());
    drop(second);
    assert!(WorkDir::create(&tmp.0.join("missing")).is_err());
}

// ---- applying ------------------------------------------------------------------------------

#[test]
fn applying_runs_the_verified_installer_through_sudo_and_cleans_up() {
    let (world, blob) = World::with_good_release();
    let result = world.updater().apply(&ApplyOptions::default()).unwrap();
    assert_eq!((result.version.as_str(), result.returncode, result.via), ("9.9.9", 0, "installer"));
    assert_eq!(result.verified, Some(true));
    assert_eq!(result.sha256, sha(&blob));
    let runs = world.host.runs.lock().unwrap().clone();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].0[0], "sudo", "from a terminal: sudo asks for the password in place");
    assert!(runs[0].0[1].ends_with("truesense-9.9.9/tools/install.sh"));
    assert!(runs[0].1, "the installer existed and was executable when it was run");
    assert_eq!(world.work_entries(), 0, "the download is deleted afterwards");
}

#[test]
fn applying_a_given_tag_does_not_need_it_to_be_the_newest() {
    let (world, _blob) = World::with_good_release();
    let old = tarball(&release_files("0.4.1"), "truesense-0.4.1");
    world.serve_release("0.4.1", &old, Some(sums_for("0.4.1", &old)));
    let result = world
        .updater()
        .apply(&ApplyOptions { tag: Some("v0.4.1".into()), ..ApplyOptions::default() })
        .unwrap();
    assert_eq!(result.version, "0.4.1");
    assert!(matches!(
        world.updater().apply(&ApplyOptions { tag: Some("main".into()), ..ApplyOptions::default() }),
        Err(UpdateError::NotAVersionTag(_))
    ));
}

#[test]
fn the_installer_exit_code_is_reported_and_the_download_still_deleted() {
    let (world, _blob) = World::with_good_release();
    let host = FakeHost { exit_code: 3, ..FakeHost::user() };
    let world = World { host: Arc::new(host), ..world };
    let result = world.updater().apply(&ApplyOptions::default()).unwrap();
    assert_eq!(result.returncode, 3);
    assert_eq!(world.work_entries(), 0);
}

#[test]
fn root_runs_the_installer_itself_and_never_looks_for_an_agent() {
    let (world, _blob) = World::with_good_release();
    let host = FakeHost { euid: 0, agent: true, ..FakeHost::user() };
    let world = World { host: Arc::new(host), ..world };
    world.updater().apply(&ApplyOptions::default()).unwrap();
    let runs = world.host.runs();
    assert_eq!(runs[0].len(), 1);
    assert!(runs[0][0].ends_with("tools/install.sh"));
}

#[test]
fn from_the_window_with_an_agent_the_graphical_prompt_is_used() {
    let (world, _blob) = World::with_good_release();
    let mut host = FakeHost { agent: true, ..FakeHost::user() };
    host.env.insert("WAYLAND_DISPLAY".into(), "wayland-1".into());
    let world = World { host: Arc::new(host), ..world };
    world.updater().apply(&ApplyOptions { gui: true, ..ApplyOptions::default() }).unwrap();
    assert_eq!(world.host.runs()[0][0], "pkexec");
    // Forced the other way by the override.
    let (world, _blob) = World::with_good_release();
    let mut host = FakeHost::user();
    host.env.insert("DISPLAY".into(), ":0".into());
    let world = World { host: Arc::new(host), ..world };
    world.updater().apply(&ApplyOptions { gui: true, agent: Some(true), ..ApplyOptions::default() }).unwrap();
    assert_eq!(world.host.runs()[0][0], "pkexec");
}

#[test]
fn from_the_window_without_an_agent_a_terminal_runs_the_command_line_update() {
    let (world, _blob) = World::with_good_release();
    *world.host.installed.lock().unwrap() = Some("9.9.9".into());
    let result = world.updater().apply(&ApplyOptions { gui: true, ..ApplyOptions::default() }).unwrap();
    assert_eq!(
        (result.via, result.returncode, result.sha256.as_str(), result.verified),
        ("terminal", 0, "", None)
    );
    assert_eq!(result.installed.as_deref(), Some("9.9.9"));
    let run = &world.host.runs()[0];
    assert_eq!(&run[..3], ["foot", "sh", "-c"]);
    assert_eq!(
        run[3],
        "truesense update apply --yes --version 9.9.9; echo; printf \"Press Enter to close this window... \"; read _"
    );
    assert!(!world.net.asked().iter().any(|u| u.ends_with(".tar.gz")), "the window itself downloads nothing");
}

#[test]
fn the_terminal_path_judges_by_what_got_installed() {
    let (world, _blob) = World::with_good_release();
    // The terminal closed but nothing was installed: not a success, even with exit code 0.
    *world.host.installed.lock().unwrap() = Some("0.3.5".into());
    let result = world.updater().apply(&ApplyOptions { gui: true, ..ApplyOptions::default() }).unwrap();
    assert_eq!(result.returncode, 1);
    assert_eq!(result.installed.as_deref(), Some("0.3.5"));
    // With a real exit code of the terminal, that code is kept.
    let host = FakeHost { exit_code: 7, ..FakeHost::user() };
    let world = World { host: Arc::new(host), ..World::with_good_release().0 };
    assert_eq!(
        world.updater().apply(&ApplyOptions { gui: true, ..ApplyOptions::default() }).unwrap().returncode,
        7
    );
    // Installed after all: success whatever the terminal said.
    *world.host.installed.lock().unwrap() = Some("9.9.9".into());
    assert_eq!(
        world.updater().apply(&ApplyOptions { gui: true, ..ApplyOptions::default() }).unwrap().returncode,
        0
    );
}

#[test]
fn from_the_window_with_no_agent_and_no_terminal_it_says_what_to_do() {
    let (world, _blob) = World::with_good_release();
    let host = FakeHost { programs: HashSet::new(), ..FakeHost::user() };
    let world = World { host: Arc::new(host), ..world };
    let err = world.updater().apply(&ApplyOptions { gui: true, ..ApplyOptions::default() }).unwrap_err();
    assert!(matches!(err, UpdateError::NoTerminal));
    assert!(err.to_string().contains("truesense update apply"));
}

#[test]
fn the_terminal_named_by_the_environment_is_preferred() {
    let (world, _blob) = World::with_good_release();
    let mut host = FakeHost::user();
    host.programs.insert("xterm");
    host.env.insert("TERMINAL".into(), "/usr/bin/xterm".into());
    *host.installed.lock().unwrap() = Some("9.9.9".into());
    let world = World { host: Arc::new(host), ..world };
    world.updater().apply(&ApplyOptions { gui: true, ..ApplyOptions::default() }).unwrap();
    assert_eq!(world.host.runs()[0][..4], ["xterm", "-e", "sh", "-c"]);
}

#[test]
fn applying_needs_a_way_to_become_root_and_cleans_up_when_there_is_none() {
    let (world, _blob) = World::with_good_release();
    let host = FakeHost { programs: HashSet::new(), ..FakeHost::user() };
    let world = World { host: Arc::new(host), ..world };
    let err = world.updater().apply(&ApplyOptions::default()).unwrap_err();
    assert!(matches!(err, UpdateError::NoElevation));
    assert!(world.host.runs().is_empty());
    assert_eq!(world.work_entries(), 0, "the verified download is deleted although nothing ran");
}

#[test]
fn an_installer_that_cannot_be_started_is_an_error_and_cleans_up() {
    let (world, _blob) = World::with_good_release();
    let host = FakeHost { spawn_error: true, ..FakeHost::user() };
    let world = World { host: Arc::new(host), ..world };
    let err = world.updater().apply(&ApplyOptions::default()).unwrap_err();
    assert!(err.to_string().starts_with("run the installer: "), "{err}");
    assert_eq!(world.work_entries(), 0);
    // And the same for a terminal that cannot be opened.
    let (world, _blob) = World::with_good_release();
    let host = FakeHost { spawn_error: true, ..FakeHost::user() };
    let world = World { host: Arc::new(host), ..world };
    let err = world.updater().apply(&ApplyOptions { gui: true, ..ApplyOptions::default() }).unwrap_err();
    assert!(err.to_string().starts_with("open a terminal: "), "{err}");
}

#[test]
fn applying_with_nothing_known_and_no_network_reports_the_check_error() {
    let world = World::new(FakeHost::user());
    world.net.fail(&format!("{API}/releases/latest"), FetchError::Transport("offline".into()));
    let err = world.updater().apply(&ApplyOptions::default()).unwrap_err();
    assert_eq!(err.to_string(), "could not reach GitHub: offline");
    assert!(world.host.runs().is_empty());
}

#[test]
fn an_unverifiable_release_never_reaches_the_installer() {
    let (world, blob) = World::with_good_release();
    world.serve_release(
        "9.9.9",
        &blob,
        Some(format!("{}  truesense-9.9.9.tar.gz\n", "f".repeat(64)).into_bytes()),
    );
    assert!(matches!(
        world.updater().apply(&ApplyOptions::default()),
        Err(UpdateError::ChecksumMismatch { .. })
    ));
    assert!(world.host.runs().is_empty(), "nothing ran as root");
    // Same when the list is missing.
    let (world, blob) = World::with_good_release();
    world.serve_release("9.9.9", &blob, None);
    world.net.routes.lock().unwrap().remove(&format!("{WEB}/releases/download/v9.9.9/SHA256SUMS"));
    assert!(matches!(world.updater().apply(&ApplyOptions::default()), Err(UpdateError::NoChecksums { .. })));
    assert!(world.host.runs().is_empty());
}

#[test]
fn error_messages_have_their_sources() {
    use std::error::Error as _;
    let e = UpdateError::Download(FetchError::Transport("x".into()));
    assert!(e.source().is_some());
    assert!(UpdateError::NoRelease.source().is_none());
    assert_eq!(UpdateError::NothingToInstall.to_string(), "no release known to install");
    assert_eq!(UpdateError::BadAnswer("x".into()).to_string(), "unexpected answer from GitHub: x");
    let io =
        UpdateError::Io { op: "read", path: "/p".into(), source: io::Error::from(io::ErrorKind::NotFound) };
    assert!(io.to_string().starts_with("read /p: ") && io.source().is_some());
}

#[test]
fn the_default_updater_uses_the_build_version_and_github() {
    let world = World::new(FakeHost::user());
    let updater = Updater::new("o/r", world.net.clone(), world.host.clone()).unwrap();
    assert_eq!(updater.repo(), "o/r");
    assert_eq!(updater.status(None, 0.0, None).current, ts_foundation::buildinfo::VERSION);
    let mirrored = Updater::new("o/r", world.net.clone(), world.host.clone())
        .unwrap()
        .with_endpoints("https://api.example/", "https://web.example/");
    assert_eq!(mirrored.api_base, "https://api.example");
    assert_eq!(mirrored.web_base, "https://web.example");
    assert!(wall_clock() > 1.7e9);
}

// ---- resilience: truncated and interrupted downloads, no-op updates, the real hand-off ----------

/// The cache file the Go 0.4.0 edition left on a laptop (field names are the Python edition's).
const GO_CACHE: &str = r#"{"checked_at":1794050946.3348439,"info":{"version":"0.4.0","tag":"v0.4.0","url":"https://github.com/o/r/releases/tag/v0.4.0","notes":"notes"},"error":null}"#;

#[test]
fn a_download_cut_short_anywhere_never_reaches_extraction_and_leaves_nothing() {
    let (world, blob) = World::with_good_release();
    let sums = sums_for("9.9.9", &blob); // the list is of the whole file
    let asset = format!("{WEB}/releases/download/v9.9.9/truesense-9.9.9.tar.gz");
    for cut in (0..blob.len()).step_by(37).chain([blob.len() - 1]) {
        world.net.serve(&asset, &blob[..cut]);
        world.serve_release("9.9.9", &blob[..cut], Some(sums.clone()));
        let Err(err) = world.updater().download("v9.9.9") else {
            panic!("cut {cut}: a truncated download must be refused")
        };
        assert!(matches!(err, UpdateError::ChecksumMismatch { .. }), "cut {cut}: {err}");
        assert_eq!(world.work_entries(), 0, "cut {cut}: nothing left behind");
    }
}

#[test]
fn an_archive_that_is_truncated_but_published_with_its_own_checksum_fails_extraction_and_leaves_nothing() {
    let (world, blob) = World::with_good_release();
    let mut refused = 0;
    for cut in (1..blob.len()).step_by(29) {
        let part = &blob[..cut];
        world.serve_release("9.9.9", part, Some(sums_for("9.9.9", part)));
        match world.updater().download("v9.9.9") {
            Err(e) => {
                refused += 1;
                assert!(matches!(e, UpdateError::Extract(_) | UpdateError::Layout(_)), "cut {cut}: {e}");
            }
            Ok(release) => drop(release), // a cut inside the gzip trailer may still inflate completely
        }
        assert_eq!(world.work_entries(), 0, "cut {cut}: the half-extracted tree is deleted");
    }
    assert!(refused > blob.len() / 29 / 2, "most cuts are refused ({refused})");
    // and an apply on such an archive never runs an installer
    let part = &blob[..blob.len() / 2];
    world.serve_release("9.9.9", part, Some(sums_for("9.9.9", part)));
    assert!(world.updater().apply(&ApplyOptions::default()).is_err());
    assert!(world.host.runs().is_empty(), "no installer ran");
}

#[test]
fn a_release_that_is_not_newer_is_not_installed_unless_a_tag_is_asked_for() {
    let (world, _blob) = World::with_good_release();
    // Running 9.9.9, newest is 9.9.9: the same version.
    let same = world.updater().with_current("9.9.9");
    let e = same.apply(&ApplyOptions::default()).unwrap_err();
    assert!(matches!(e, UpdateError::AlreadyNewest { .. }), "{e}");
    assert_eq!(e.to_string(), "TrueSense 9.9.9 is already the newest version (the newest release is 9.9.9)");
    // Running newer than anything published (a development build): the published release is a downgrade.
    let ahead = world.updater().with_current("10.0.0");
    assert!(matches!(ahead.apply(&ApplyOptions::default()).unwrap_err(), UpdateError::AlreadyNewest { .. }));
    assert!(!ahead.check(true, None).available);
    assert!(world.host.runs().is_empty(), "no installer ran for either");
    assert!(!world.net.asked().iter().any(|u| u.ends_with(".tar.gz")), "and nothing was downloaded");
    // Asking for a tag by name is a decision, and is honoured (reinstall or an older release).
    assert!(same.apply(&ApplyOptions { tag: Some("v9.9.9".into()), ..ApplyOptions::default() }).is_ok());
}

#[test]
fn a_release_of_another_edition_is_refused_by_the_layout_check_not_installed() {
    let world = World::new(FakeHost::user());
    fs::create_dir_all(world.tmp.0.join("work")).unwrap();
    // The Go edition's source release: no Cargo.toml, a go.mod.
    let go = tarball(
        &[
            ("tools/install.sh", b"#!/bin/sh\n".to_vec()),
            ("go.mod", b"module x\n".to_vec()),
            ("kernel/truesense-wmi/truesense_wmi.c", b"//".to_vec()),
        ],
        "truesense-9.9.9",
    );
    world.serve_release("9.9.9", &go, Some(sums_for("9.9.9", &go)));
    world.serve_github(Some("v9.9.9"), &["v9.9.9"]);
    let e = world.updater().apply(&ApplyOptions::default()).unwrap_err();
    assert!(e.to_string().contains("missing Cargo.toml"), "{e}");
    assert!(world.host.runs().is_empty());
    assert_eq!(world.work_entries(), 0);
}

#[test]
fn the_update_cache_the_go_edition_left_is_understood_and_gives_no_update() {
    let world = World::new(FakeHost::user());
    let cache = world.tmp.0.join("cache/update.json");
    fs::create_dir_all(cache.parent().unwrap()).unwrap();
    fs::write(&cache, GO_CACHE).unwrap();
    // Offline, with the cache still fresh: its answer is used without any request.
    let updater = world.updater().with_current("0.5.0").with_clock(Arc::new(|| 1_794_050_946.0 + 60.0));
    let status = updater.check(false, None);
    assert_eq!(status.latest.as_deref(), Some("0.4.0"));
    assert!(!status.available, "0.4.0 is older than the running 0.5.0");
    assert!(world.net.asked().is_empty());
    // Offline with a stale cache: the last answer survives and the error says why.
    let stale =
        world.updater().with_current("0.5.0").with_clock(Arc::new(|| 1_794_050_946.0 + 10.0 * 86_400.0));
    let status = stale.check(false, None);
    assert_eq!(status.latest.as_deref(), Some("0.4.0"));
    assert!(status.error.is_some());
}

/// A host that really runs what it is asked to (the installer of the bundle, a shell script).
struct ExecHost;

impl Host for ExecHost {
    fn euid(&self) -> u32 {
        0
    }
    fn env(&self, _key: &str) -> Option<String> {
        None
    }
    fn has_program(&self, _name: &str) -> bool {
        false
    }
    fn polkit_agent_running(&self) -> bool {
        false
    }
    fn installed_version(&self) -> Option<String> {
        None
    }
    fn run_attached(&self, argv: &[String]) -> io::Result<i32> {
        let status = std::process::Command::new(&argv[0]).args(&argv[1..]).status()?;
        Ok(status.code().unwrap_or(-1))
    }
}

#[test]
fn the_real_installer_of_a_hand_built_bundle_is_run_from_the_verified_tree() {
    let tmp = TempDir::new();
    let work = tmp.0.join("work");
    fs::create_dir_all(&work).unwrap();
    let marker = tmp.0.join("marker");
    // The bundle, built by hand: the layout of a real release and an installer that proves it
    // runs inside the verified tree (Cargo.toml and the kernel source next to it).
    let script = format!(
        "#!/bin/sh\nset -e\nroot=$(cd \"$(dirname \"$0\")/..\" && pwd)\ngrep -q '^version = \"9.9.9\"' \"$root/Cargo.toml\"\ntest -f \"$root/kernel/truesense-wmi/truesense_wmi.c\"\nprintf '%s' \"$root\" > '{}'\nexit 3\n",
        marker.display()
    );
    let mut files = release_files("9.9.9");
    files[0].1 = script.into_bytes();
    let blob = tarball(&files, "truesense-9.9.9");
    let net = Arc::new(FakeNet::default());
    net.serve(&format!("{WEB}/releases/download/v9.9.9/truesense-9.9.9.tar.gz"), &blob);
    net.serve(&format!("{WEB}/releases/download/v9.9.9/SHA256SUMS"), &sums_for("9.9.9", &blob));
    net.serve(
        &format!("{API}/releases/latest"),
        br#"{"tag_name":"v9.9.9","html_url":"https://github.com/o/r/releases/tag/v9.9.9","body":"n"}"#,
    );
    net.serve(&format!("{API}/tags?per_page=30"), br#"[{"name":"v9.9.9"}]"#);
    let updater = Updater::new("o/r", net, Arc::new(ExecHost))
        .unwrap()
        .with_current("0.5.0")
        .with_cache_path(None)
        .with_temp_dir(&work);
    let result = updater.apply(&ApplyOptions::default()).unwrap();
    assert_eq!(
        (result.returncode, result.via),
        (3, "installer"),
        "the installer's own exit code is reported"
    );
    let ran_in = fs::read_to_string(&marker).unwrap();
    assert!(ran_in.ends_with("truesense-9.9.9"), "{ran_in}");
    assert!(!Path::new(&ran_in).exists(), "and the tree is deleted afterwards");
    assert_eq!(fs::read_dir(&work).unwrap().count(), 0);
}
