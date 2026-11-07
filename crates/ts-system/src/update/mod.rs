//! update: find a newer TrueSense release on GitHub and install it.
//!
//! Checking only reads public release information (one HTTPS request, no identifiers, at most
//! once a day when automatic checks are on). Installing downloads the tagged release from the
//! project's repository, verifies it against the release's `SHA256SUMS`, checks that it really
//! is that version, unpacks it with an extractor that cannot write outside its folder, and
//! hands over to its `tools/install.sh` with administrator rights (a graphical password prompt,
//! or `sudo` in a terminal): the same trust as `curl ... | sudo bash`, but only when you ask.
//!
//! The repository comes from the SYSTEM config (root-owned) or the built-in default, never from
//! the user's config, so a file the user can write cannot point the updater somewhere else
//! ([`system_repo`]).
//!
//! The checksum list is **required**: a release without a `SHA256SUMS`, a list that does not
//! name the archive, or a list that cannot be fetched (offline, a server error) refuses the
//! install. Nothing downgrades verification to "skipped", because anyone able to delete a file
//! or answer 404 for it could otherwise turn it off.
//!
//! Every effect is injected. The network is the [`HttpFetcher`] trait (the real one,
//! [`CurlFetcher`], spawns `curl` through the platform's `CommandRunner`; this crate has no HTTP
//! library), the machine is the [`Host`] trait, and the clock, the cache file and the temporary
//! directory are fields of the [`Updater`].
#![forbid(unsafe_code)]

pub mod extract;
pub mod fetch;
pub mod host;
pub mod sums;
pub mod version;

use std::fmt;
use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use ts_foundation::config::{Config, ConfigError};
use ts_foundation::fsx;

pub use extract::{ExtractError, ExtractLimits, extract_tar_gz};
pub use fetch::{CurlFetcher, FetchError, HttpFetcher, HttpRequest};
pub use host::{Elevation, Host, SystemHost, elevate_command, polkit_agent_running, terminal_command};
pub use sums::{Checksums, parse_sums};
pub use version::{Version, valid_repo};

/// The repository releases come from unless the system config says otherwise.
pub const DEFAULT_REPO: &str = "Order52/truesense-rust";

/// The most bytes a release archive may have; the real one is a few megabytes.
pub(crate) const MAX_DOWNLOAD: u64 = 40 * 1024 * 1024;

/// The seconds since the Unix epoch, as a float (the Python edition's `time.time()`).
pub type Clock = Arc<dyn Fn() -> f64 + Send + Sync>;

/// Everything that can go wrong. The texts keep the wording of the 0.3.x releases where there
/// was one.
#[derive(Debug)]
pub enum UpdateError {
    /// The repository is not `owner/name`.
    InvalidRepo(String),
    /// The system config could not be read.
    Config(ConfigError),
    /// What was asked for is not a version tag (`v1.2.3`).
    NotAVersionTag(String),
    /// The repository has neither a release nor a version tag.
    NoRelease,
    /// There is nothing known to install: nothing was asked for and no release is known.
    NothingToInstall,
    /// No tag was asked for and the newest release is not newer than the running version:
    /// installing it would be a reinstall or a downgrade nobody asked for.
    AlreadyNewest {
        /// The running version.
        current: String,
        /// The newest release known.
        newest: String,
    },
    /// GitHub's API could not be asked.
    Github(FetchError),
    /// GitHub answered with something that is not what the API documents.
    BadAnswer(String),
    /// The check failed and no earlier answer is cached; the text is the status error.
    Unreachable(String),
    /// The release has no `SHA256SUMS`: it cannot be verified and is refused.
    NoChecksums {
        /// The release tag.
        tag: String,
    },
    /// The `SHA256SUMS` could not be fetched. Never means "skip the check".
    ChecksumsFetch(FetchError),
    /// The `SHA256SUMS` is unusable (the text says how).
    Checksums(String),
    /// The `SHA256SUMS` does not name the release archive.
    NotListed {
        /// The release tag.
        tag: String,
        /// The archive that was expected in the list.
        asset: String,
    },
    /// The archive does not match its listed checksum.
    ChecksumMismatch {
        /// The archive name.
        asset: String,
        /// The hash the list gives.
        expected: String,
        /// The hash of what was downloaded.
        actual: String,
    },
    /// The archive could not be fetched.
    Download(FetchError),
    /// The archive is not safe or not valid.
    Extract(ExtractError),
    /// The unpacked archive is not a release of the version the tag says (the text says how).
    Layout(String),
    /// Neither `sudo` nor `pkexec` with a polkit agent is available.
    NoElevation,
    /// From the window with no password prompt and no terminal to ask in.
    NoTerminal,
    /// The installer could not be started.
    Installer(io::Error),
    /// The terminal window could not be opened.
    Terminal(io::Error),
    /// A file operation on the download failed.
    Io {
        /// What was being done.
        op: &'static str,
        /// The path.
        path: PathBuf,
        /// What the operating system said.
        source: io::Error,
    },
}

impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRepo(r) => write!(f, "[update] repo must look like owner/name, not {r:?}"),
            Self::Config(e) => write!(f, "read the system config: {e}"),
            Self::NotAVersionTag(t) => write!(f, "not a version tag: {t:?}"),
            Self::NoRelease => f.write_str("no release or version tag found in the repository"),
            Self::NothingToInstall => f.write_str("no release known to install"),
            Self::AlreadyNewest { current, newest } => {
                write!(f, "TrueSense {current} is already the newest version (the newest release is {newest})")
            }
            Self::Github(e) => e.fmt(f),
            Self::BadAnswer(why) => write!(f, "unexpected answer from GitHub: {why}"),
            Self::Unreachable(text) => f.write_str(text),
            Self::NoChecksums { tag } => write!(
                f,
                "release {tag} has no SHA256SUMS: refusing to install something that cannot be verified"
            ),
            Self::ChecksumsFetch(e) => match e {
                FetchError::Status { code, .. } => {
                    write!(f, "could not fetch the release checksums (HTTP {code}); try again later")
                }
                other => write!(f, "could not fetch the release checksums ({other}); try again later"),
            },
            Self::Checksums(why) => f.write_str(why),
            Self::NotListed { tag, asset } => write!(f, "SHA256SUMS of {tag} does not list {asset}: refusing"),
            Self::ChecksumMismatch { asset, expected, actual } => write!(
                f,
                "checksum mismatch for {asset} (expected {}…, got {}…): refusing",
                short(expected),
                short(actual)
            ),
            Self::Download(FetchError::TooLarge { .. }) => f.write_str("the download is unexpectedly large; refusing"),
            Self::Download(e) => write!(f, "could not download the release ({e}); try again later"),
            Self::Extract(e) => e.fmt(f),
            Self::Layout(why) => f.write_str(why),
            Self::NoElevation => f.write_str("need sudo, or pkexec with a polkit agent, to install the update"),
            Self::NoTerminal => f.write_str(
                "this desktop has no password prompt for the window (no polkit agent) and no terminal was found. Run this in a terminal instead: truesense update apply",
            ),
            Self::Installer(e) => write!(f, "run the installer: {e}"),
            Self::Terminal(e) => write!(f, "open a terminal: {e}"),
            Self::Io { op, path, source } => write!(f, "{op} {}: {source}", path.display()),
        }
    }
}

impl std::error::Error for UpdateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Config(e) => Some(e),
            Self::Github(e) | Self::ChecksumsFetch(e) | Self::Download(e) => Some(e),
            Self::Extract(e) => Some(e),
            Self::Installer(e) | Self::Terminal(e) => Some(e),
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// The first twelve characters of a hash, for messages.
fn short(hash: &str) -> &str {
    hash.get(..12).unwrap_or(hash)
}

/// The repository to update from: the built-in default, overridable only by the root-owned
/// system config at `system_config`. The user's config is deliberately not an input.
pub fn system_repo(system_config: &Path) -> Result<String, UpdateError> {
    let config = Config::load(Some(system_config), None, &|_| None).map_err(UpdateError::Config)?;
    let repo = config.update.repo;
    if valid_repo(&repo) { Ok(repo) } else { Err(UpdateError::InvalidRepo(repo)) }
}

/// The newest release found. The JSON field names are those of the 0.3.x releases (this is also
/// what the cache file holds).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Info {
    /// The version without a `v` (`0.5.1`).
    pub version: String,
    /// The tag as published (`v0.5.1`).
    pub tag: String,
    /// The release page.
    pub url: String,
    /// The release notes, cut to 1500 characters.
    pub notes: String,
}

/// The answer of [`Updater::check`]. The JSON field names are those of the 0.3.x releases.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Status {
    /// The running version.
    pub current: String,
    /// The newest version known, if any.
    pub latest: Option<String>,
    /// The newest version is newer than the running one.
    pub available: bool,
    /// When the answer was obtained (seconds since the epoch).
    pub checked_at: f64,
    /// Why the lookup failed; `latest` and `info` then hold the last known answer, if any.
    pub error: Option<String>,
    /// The newest release known, if any.
    pub info: Option<Info>,
}

/// The cache file `~/.cache/truesense/update.json`: `{"checked_at", "info", "error"}` as the
/// Python edition wrote it, plus `notified` (the version the user was last told about, so the
/// daily timer notifies once per version) and `repo` (whose releases `info` describes, so an
/// answer from another repository, such as the one an older edition asked, is not trusted).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct CacheFile {
    checked_at: f64,
    info: Option<Info>,
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    notified: Option<String>,
}

/// Where the cache lives: `$XDG_CACHE_HOME/truesense/update.json`, else
/// `~/.cache/truesense/update.json`; `None` without either variable.
pub fn default_cache_path(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let base = env("XDG_CACHE_HOME")
        .filter(|d| Path::new(d).is_absolute())
        .map(PathBuf::from)
        .or_else(|| env("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("truesense").join("update.json"))
}

/// What a release archive must contain to count as a TrueSense release, and where the version it
/// claims to be is written, so a tag cannot be paired with another version's code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    /// Files that must exist (as regular files) under the archive's single top folder.
    pub required: Vec<String>,
    /// The file that states the version, relative to the top folder.
    pub version_file: String,
    /// The name whose `name = "x.y.z"` assignment in that file is the version.
    pub version_key: String,
}

impl Layout {
    /// The Rust edition's source release: the workspace `Cargo.toml` names the version.
    pub fn rust_edition() -> Self {
        Self {
            required: ["tools/install.sh", "kernel/truesense-wmi/truesense_wmi.c", "Cargo.toml"]
                .map(String::from)
                .to_vec(),
            version_file: "Cargo.toml".to_owned(),
            version_key: "version".to_owned(),
        }
    }

    /// The 0.3.x (Python edition) release layout.
    pub fn python_edition() -> Self {
        Self {
            required: [
                "tools/install.sh",
                "src/truesense/__init__.py",
                "kernel/truesense-wmi/truesense_wmi.c",
            ]
            .map(String::from)
            .to_vec(),
            version_file: "src/truesense/__init__.py".to_owned(),
            version_key: "__version__".to_owned(),
        }
    }
}

/// The text of the first `key = "value"` in `text`, where `key` stands alone (`rust-version` is
/// not `version`) and the value is not empty.
fn quoted_assignment(text: &str, key: &str) -> Option<String> {
    let mut from = 0;
    while let Some(i) = text[from..].find(key) {
        let start = from + i;
        let end = start + key.len();
        from = end;
        let standalone =
            text[..start].chars().next_back().is_none_or(|c| !(c.is_alphanumeric() || c == '_' || c == '-'));
        if !standalone {
            continue;
        }
        let Some(rest) = text[end..].trim_start().strip_prefix('=') else { continue };
        let Some(rest) = rest.trim_start().strip_prefix('"') else { continue };
        let Some(close) = rest.find('"') else { continue };
        if close > 0 {
            return Some(rest[..close].to_owned());
        }
    }
    None
}

/// A verified, unpacked release in a private temporary folder. The folder and everything in it
/// is deleted when this is dropped, also when the installer failed.
#[derive(Debug)]
pub struct Release {
    root: PathBuf,
    sha256: String,
    work_dir: PathBuf,
}

impl Release {
    /// The unpacked top folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The SHA-256 of the downloaded archive, in lowercase hex.
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// The release's installer script, `tools/install.sh` in the top folder.
    pub fn installer(&self) -> PathBuf {
        self.root.join("tools").join("install.sh")
    }
}

impl Drop for Release {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.work_dir);
    }
}

/// A private working folder that deletes itself unless it is handed on.
struct WorkDir {
    path: PathBuf,
    keep: bool,
}

impl WorkDir {
    /// A new folder below `base`, mode 0700, with a name nobody can have prepared: creation
    /// fails if the name exists (also as a link), and the next attempt takes another.
    fn create(base: &Path) -> Result<Self, UpdateError> {
        let pid = std::process::id();
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
        for attempt in 0..100u32 {
            let path = base.join(format!("truesense-update-{pid}-{nanos:08x}-{attempt}"));
            match DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => {
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
                        .map_err(|source| UpdateError::Io { op: "protect", path: path.clone(), source })?;
                    return Ok(Self { path, keep: false });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(source) => {
                    return Err(UpdateError::Io {
                        op: "make a download folder in",
                        path: base.to_path_buf(),
                        source,
                    });
                }
            }
        }
        Err(UpdateError::Io {
            op: "make a download folder in",
            path: base.to_path_buf(),
            source: io::Error::new(io::ErrorKind::AlreadyExists, "no unused name"),
        })
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

/// What to install and from where it is asked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApplyOptions {
    /// The tag to install; `None` means the newest release.
    pub tag: Option<String>,
    /// Asked from the window: a missing password prompt falls back to a terminal window.
    pub gui: bool,
    /// Overrides the polkit agent detection (tests); `None` looks at the running processes.
    pub agent: Option<bool>,
}

/// What [`Updater::apply`] reports. The JSON field names are those of the 0.3.x releases. The
/// terminal path has no checksum of its own: the terminal runs `truesense update apply`, which
/// verifies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApplyResult {
    /// The version installed (or asked to be) without a `v`.
    pub version: String,
    /// SHA-256 of the downloaded archive; empty on the terminal path.
    pub sha256: String,
    /// The archive matched its `SHA256SUMS` (always true when present; absent on the terminal path).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verified: Option<bool>,
    /// The installer's exit code (0 is success).
    pub returncode: i32,
    /// `installer` or `terminal`.
    pub via: &'static str,
    /// On the terminal path: the version found installed afterwards.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed: Option<String>,
}

/// Finds and installs releases.
pub struct Updater {
    repo: String,
    current: String,
    check_hours: u64,
    fetcher: Arc<dyn HttpFetcher>,
    host: Arc<dyn Host>,
    clock: Clock,
    cache_path: Option<PathBuf>,
    temp_dir: PathBuf,
    layout: Layout,
    api_base: String,
    web_base: String,
    max_download: u64,
}

impl Updater {
    /// An updater for `repo` (`owner/name`, see [`system_repo`]). Defaults: the running version
    /// from the build, a check at most every 24 hours, the wall clock, the standard cache file,
    /// the system temporary directory, the Rust edition's [`Layout`] and GitHub's endpoints.
    pub fn new(repo: &str, fetcher: Arc<dyn HttpFetcher>, host: Arc<dyn Host>) -> Result<Self, UpdateError> {
        if !valid_repo(repo) {
            return Err(UpdateError::InvalidRepo(repo.to_owned()));
        }
        Ok(Self {
            repo: repo.to_owned(),
            current: ts_foundation::buildinfo::VERSION.to_owned(),
            check_hours: 24,
            fetcher,
            host,
            clock: Arc::new(wall_clock),
            cache_path: default_cache_path(&fsx::env_nonempty),
            temp_dir: std::env::temp_dir(),
            layout: Layout::rust_edition(),
            api_base: "https://api.github.com".to_owned(),
            web_base: "https://github.com".to_owned(),
            max_download: MAX_DOWNLOAD,
        })
    }

    /// Sets the running version (a test, or a build that knows better).
    #[must_use]
    pub fn with_current(mut self, version: &str) -> Self {
        self.current = version.to_owned();
        self
    }

    /// Sets how long a cached answer stays fresh, in hours (config `[update] check_hours`).
    #[must_use]
    pub fn with_check_hours(mut self, hours: u64) -> Self {
        self.check_hours = hours;
        self
    }

    /// Sets the clock.
    #[must_use]
    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    /// Sets the cache file; `None` means no cache.
    #[must_use]
    pub fn with_cache_path(mut self, path: Option<PathBuf>) -> Self {
        self.cache_path = path;
        self
    }

    /// Sets where downloads are unpacked.
    #[must_use]
    pub fn with_temp_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.temp_dir = dir.into();
        self
    }

    /// Sets what a release must look like.
    #[must_use]
    pub fn with_layout(mut self, layout: Layout) -> Self {
        self.layout = layout;
        self
    }

    /// Sets the API and web endpoints (GitHub's by default), for a mirror or a test.
    #[must_use]
    pub fn with_endpoints(mut self, api_base: &str, web_base: &str) -> Self {
        self.api_base = api_base.trim_end_matches('/').to_owned();
        self.web_base = web_base.trim_end_matches('/').to_owned();
        self
    }

    /// Sets the largest archive accepted, in bytes.
    #[must_use]
    pub fn with_max_download(mut self, bytes: u64) -> Self {
        self.max_download = bytes;
        self
    }

    /// The repository this updater reads.
    pub fn repo(&self) -> &str {
        &self.repo
    }

    fn get_json(&self, url: &str) -> Result<Value, UpdateError> {
        let body = self
            .fetcher
            .get(&HttpRequest {
                url,
                accept: Some("application/vnd.github+json"),
                timeout: Duration::from_secs(8),
                max_bytes: 4 * 1024 * 1024,
            })
            .map_err(UpdateError::Github)?;
        serde_json::from_slice(&body).map_err(|e| UpdateError::BadAnswer(e.to_string()))
    }

    /// The newest version of the repository: the highest of the newest release and the newest
    /// version tag (a tag may not have a release page yet). When two names parse to the same
    /// version the release page wins over a bare tag, and the earlier of equal tags over the
    /// later.
    pub fn latest(&self) -> Result<Info, UpdateError> {
        let mut found: Vec<(Version, Info)> = Vec::new();
        let mut add = |tag: &str, url: String, notes: String| {
            let tag = tag.trim();
            let Some(version) = Version::parse(tag) else { return };
            if found.iter().all(|(_, i)| i.tag != tag) {
                found.push((version, Info { version: version.to_string(), tag: tag.to_owned(), url, notes }));
            }
        };
        match self.get_json(&format!("{}/repos/{}/releases/latest", self.api_base, self.repo)) {
            Ok(release) => {
                if let Some(tag) = release.get("tag_name").and_then(Value::as_str) {
                    let url = release.get("html_url").and_then(Value::as_str).unwrap_or_default();
                    let notes = release.get("body").and_then(Value::as_str).unwrap_or_default();
                    add(tag, url.to_owned(), notes.chars().take(1500).collect());
                }
            }
            // A repository with tags but no release page yet answers 404 here.
            Err(UpdateError::Github(FetchError::Status { code: 404, .. })) => {}
            Err(e) => return Err(e),
        }
        let tags = self.get_json(&format!("{}/repos/{}/tags?per_page=30", self.api_base, self.repo))?;
        let tags =
            tags.as_array().ok_or_else(|| UpdateError::BadAnswer("the tag list is not a list".into()))?;
        for tag in tags.iter().filter_map(|t| t.get("name").and_then(Value::as_str)) {
            add(tag, format!("{}/{}/releases/tag/{tag}", self.web_base, self.repo), String::new());
        }
        let mut best: Option<(Version, Info)> = None;
        for (version, info) in found {
            if best.as_ref().is_none_or(|(v, _)| version > *v) {
                best = Some((version, info));
            }
        }
        best.map(|(_, info)| info).ok_or(UpdateError::NoRelease)
    }

    fn load_cache(&self) -> CacheFile {
        let mut cache: CacheFile = self
            .cache_path
            .as_deref()
            .and_then(|p| fsx::read_limited(p, fsx::MAX_SETTINGS_FILE).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        if !self.cache_is_for_this_repo(&cache) {
            // What was asked of another repository says nothing about this one: ask again (the
            // notification memory is about versions, so it stays).
            cache = CacheFile { notified: cache.notified, ..CacheFile::default() };
        }
        cache.repo = Some(self.repo.clone());
        cache
    }

    /// Whether the cached answer came from this updater's repository: by the `repo` it recorded,
    /// or, for a cache an older edition wrote, by the GitHub address of the release it holds.
    fn cache_is_for_this_repo(&self, cache: &CacheFile) -> bool {
        match (&cache.repo, &cache.info) {
            (Some(repo), _) => *repo == self.repo,
            (None, Some(info)) => {
                !info.url.contains("github.com/") || info.url.contains(&format!("github.com/{}/", self.repo))
            }
            (None, None) => true,
        }
    }

    /// The cache is a convenience: a read-only home or a full disk is not an error.
    fn save_cache(&self, cache: &CacheFile) {
        if let (Some(path), Ok(bytes)) = (&self.cache_path, serde_json::to_vec(cache)) {
            let _ = fsx::write_atomic_mkdir(path, &bytes, 0o644);
        }
    }

    fn status(&self, info: Option<&Info>, checked_at: f64, error: Option<String>) -> Status {
        let available = match (Version::parse(&self.current), info.and_then(|i| Version::parse(&i.tag))) {
            (Some(current), Some(newest)) => newest > current,
            _ => false,
        };
        Status {
            current: self.current.clone(),
            latest: info.map(|i| i.version.clone()),
            available,
            checked_at,
            error,
            info: info.cloned(),
        }
    }

    /// Whether a newer release exists. The cached answer is used unless `force` is set or it is
    /// older than `hours` (`None` means the updater's setting; 0 always asks again). A failed
    /// lookup is reported in `error` together with the last known answer, never as an error.
    pub fn check(&self, force: bool, hours: Option<u64>) -> Status {
        let window = hours.unwrap_or(self.check_hours) as f64 * 3600.0;
        let mut cache = self.load_cache();
        let now = (self.clock)();
        // An age below zero (the clock was set back) is not fresh: ask again.
        if !force && cache.error.is_none() && (0.0..window).contains(&(now - cache.checked_at)) {
            if let Some(info) = &cache.info {
                return self.status(Some(info), cache.checked_at, None);
            }
        }
        match self.latest() {
            Ok(info) => {
                cache.checked_at = now;
                cache.info = Some(info);
                cache.error = None;
                self.save_cache(&cache);
                self.status(cache.info.as_ref(), now, None)
            }
            Err(e) => {
                let message = format!("could not reach GitHub: {e}");
                cache.checked_at = now;
                cache.error = Some(message.clone());
                self.save_cache(&cache);
                self.status(cache.info.as_ref(), now, Some(message))
            }
        }
    }

    /// The version the user was last notified about (the daily timer notifies once per version).
    pub fn notified(&self) -> Option<String> {
        self.load_cache().notified
    }

    /// Remembers that the user was told about `version`.
    pub fn mark_notified(&self, version: &str) {
        let mut cache = self.load_cache();
        cache.notified = Some(version.to_owned());
        self.save_cache(&cache);
    }

    /// The release's `SHA256SUMS`. A release without one is an error ([`UpdateError::NoChecksums`]),
    /// and so is any failure to fetch it: a missing list must never silently turn verification off.
    pub fn checksums(&self, tag: &str) -> Result<Checksums, UpdateError> {
        let url = format!("{}/{}/releases/download/{tag}/SHA256SUMS", self.web_base, self.repo);
        let request =
            HttpRequest { url: &url, accept: None, timeout: Duration::from_secs(20), max_bytes: 1 << 20 };
        match self.fetcher.get(&request) {
            Ok(data) => parse_sums(&data),
            Err(FetchError::Status { code: 404, .. }) => {
                Err(UpdateError::NoChecksums { tag: tag.to_owned() })
            }
            Err(e) => Err(UpdateError::ChecksumsFetch(e)),
        }
    }

    /// Fetches and unpacks a tagged release into a fresh private folder.
    ///
    /// The release is downloaded as its published asset `truesense-<version>.tar.gz` and must
    /// match its `SHA256SUMS`; the unpacked folder must be a TrueSense release (the
    /// [`Layout`]) of exactly the version the tag says. On any failure nothing is left behind.
    pub fn download(&self, tag: &str) -> Result<Release, UpdateError> {
        let version = Version::parse(tag).ok_or_else(|| UpdateError::NotAVersionTag(tag.to_owned()))?;
        let tag = tag.trim();
        let asset = format!("truesense-{version}.tar.gz");
        let sums = self.checksums(tag)?;
        let expected = sums
            .get(&asset)
            .ok_or_else(|| UpdateError::NotListed { tag: tag.to_owned(), asset: asset.clone() })?;
        let url = format!("{}/{}/releases/download/{tag}/{asset}", self.web_base, self.repo);
        let archive = self
            .fetcher
            .get(&HttpRequest {
                url: &url,
                accept: None,
                timeout: Duration::from_secs(600),
                max_bytes: self.max_download,
            })
            .map_err(UpdateError::Download)?;
        let actual = hex(&Sha256::digest(&archive));
        if &actual != expected {
            return Err(UpdateError::ChecksumMismatch { asset, expected: expected.clone(), actual });
        }
        let mut work = WorkDir::create(&self.temp_dir)?;
        let src = work.path.join("src");
        DirBuilder::new().mode(0o700).create(&src).map_err(|source| UpdateError::Io {
            op: "make",
            path: src.clone(),
            source,
        })?;
        extract_tar_gz(&archive[..], &src, &ExtractLimits::default()).map_err(UpdateError::Extract)?;
        let root = self.validate(&src, version, tag)?;
        let installer = root.join("tools").join("install.sh");
        fs::set_permissions(&installer, fs::Permissions::from_mode(0o755))
            .map_err(|source| UpdateError::Io { op: "make executable", path: installer.clone(), source })?;
        work.keep = true;
        Ok(Release { root, sha256: actual, work_dir: work.path.clone() })
    }

    /// Checks that `src` holds exactly one top folder that is a TrueSense release of the
    /// version the tag says, and returns that folder.
    fn validate(&self, src: &Path, want: Version, tag: &str) -> Result<PathBuf, UpdateError> {
        let io_err = |path: &Path| {
            let path = path.to_path_buf();
            move |source| UpdateError::Io { op: "read", path, source }
        };
        let mut entries =
            fs::read_dir(src).map_err(io_err(src))?.collect::<Result<Vec<_>, _>>().map_err(io_err(src))?;
        let layout_error = || UpdateError::Layout("unexpected layout in the download".into());
        let entry = (entries.len() == 1).then(|| entries.remove(0)).ok_or_else(layout_error)?;
        if !entry.file_type().map_err(io_err(src))?.is_dir() {
            return Err(layout_error());
        }
        let root = entry.path();
        for need in &self.layout.required {
            if !fs::symlink_metadata(root.join(need)).is_ok_and(|m| m.is_file()) {
                return Err(UpdateError::Layout(format!(
                    "the download is not a TrueSense release (missing {need})"
                )));
            }
        }
        let version_path = root.join(&self.layout.version_file);
        let text = fsx::read_limited(&version_path, fsx::MAX_SETTINGS_FILE).map_err(|_| {
            UpdateError::Layout(format!(
                "the download is not a TrueSense release (missing {})",
                self.layout.version_file
            ))
        })?;
        let inside = quoted_assignment(&String::from_utf8_lossy(&text), &self.layout.version_key);
        if inside.as_deref().and_then(Version::parse) != Some(want) {
            return Err(UpdateError::Layout(format!(
                "the download says version {}, not {tag}; refusing",
                inside.as_deref().unwrap_or("?")
            )));
        }
        Ok(root)
    }

    /// Downloads the release and runs its installer with administrator rights.
    ///
    /// From the window (`gui`) with no polkit agent there is no way to ask for a password, so a
    /// terminal window runs `truesense update apply` instead, and the result is judged by what
    /// got installed. The downloaded files are deleted afterwards, whatever the outcome.
    pub fn apply(&self, options: &ApplyOptions) -> Result<ApplyResult, UpdateError> {
        let status = self.check(true, None);
        if let (Some(error), None) = (&status.error, &status.info) {
            return Err(UpdateError::Unreachable(error.clone()));
        }
        let tag = match (&options.tag, &status.info) {
            (Some(tag), _) => tag.clone(),
            (None, Some(info)) if status.available => info.tag.clone(),
            (None, Some(info)) => {
                return Err(UpdateError::AlreadyNewest {
                    current: status.current.clone(),
                    newest: info.version.clone(),
                });
            }
            (None, None) => return Err(UpdateError::NothingToInstall),
        };
        let version = Version::parse(&tag).ok_or_else(|| UpdateError::NotAVersionTag(tag.clone()))?;
        let euid = self.host.euid();
        // Only looked for when it can matter: when we are root and not in the window, nobody asks.
        let agent =
            options.agent.unwrap_or_else(|| (options.gui || euid != 0) && self.host.polkit_agent_running());
        if options.gui && euid != 0 && !agent {
            return self.apply_in_terminal(version);
        }
        let release = self.download(&tag)?;
        let argv = elevate_command(
            &release.installer().to_string_lossy(),
            &Elevation {
                euid,
                gui: options.gui,
                agent,
                desktop: self.host.env("WAYLAND_DISPLAY").is_some() || self.host.env("DISPLAY").is_some(),
                has_pkexec: self.host.has_program("pkexec"),
                has_sudo: self.host.has_program("sudo"),
            },
        )?;
        let returncode = self.host.run_attached(&argv).map_err(UpdateError::Installer)?;
        Ok(ApplyResult {
            version: version.to_string(),
            sha256: release.sha256().to_owned(),
            verified: Some(true),
            returncode,
            via: "installer",
            installed: None,
        })
    }

    fn apply_in_terminal(&self, version: Version) -> Result<ApplyResult, UpdateError> {
        let inner = format!(
            "truesense update apply --yes --version {version}; echo; printf \"Press Enter to close this window... \"; read _"
        );
        let terminal = self.host.env("TERMINAL");
        let argv = terminal_command(&inner, terminal.as_deref(), &|name| self.host.has_program(name))
            .ok_or(UpdateError::NoTerminal)?;
        let code = self.host.run_attached(&argv).map_err(UpdateError::Terminal)?;
        let installed = self.host.installed_version();
        let ok = installed.as_deref() == Some(version.to_string().as_str());
        Ok(ApplyResult {
            version: version.to_string(),
            sha256: String::new(),
            verified: None,
            // The terminal closed but nothing was installed: not a success, even with exit code 0.
            returncode: if ok {
                0
            } else if code != 0 {
                code
            } else {
                1
            },
            via: "terminal",
            installed,
        })
    }
}

/// Lowercase hex.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn wall_clock() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests;
