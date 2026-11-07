//! The store of named undervolt profiles, shared by the CPU and the GPU undervolt: one JSON file
//! per profile (`NAME.json`) in a directory, built-in profiles that are always there and
//! read-only, names that cannot escape the directory, atomic saves.
//!
//! The two kinds of profile differ only in their fields, their limits and the words in their
//! messages, so that is what the [`Kind`] trait carries; the file handling is written once.

use std::fmt;
use std::marker::PhantomData;
use std::path::PathBuf;

use serde::Serialize;
use serde_json::Value;
use ts_foundation::fsx::{self, FsError};

/// The longest profile name.
const MAX_NAME: usize = 40;

/// A failed profile operation. The wording is the Python edition's, which users see.
#[derive(Debug)]
pub enum ProfileError {
    /// The profile is not valid (a value out of range, or not the right shape); the text says
    /// what is allowed.
    Invalid(String),
    /// The name uses characters other than letters, digits, `.`, `_`, `-`, or is too long.
    BadName,
    /// There is no such profile.
    NotFound {
        /// `undervolt` or `GPU undervolt`.
        what: &'static str,
        /// The name asked for.
        name: String,
        /// The command that lists profiles, when the message should point at it.
        hint: Option<&'static str>,
    },
    /// The file is not JSON.
    NotJson {
        /// `undervolt` or `GPU undervolt`.
        what: &'static str,
        /// The profile's name.
        name: String,
    },
    /// A built-in profile cannot be overwritten.
    BuiltInReadOnly {
        /// The name that was refused.
        name: String,
        /// The command that copies a profile, for the message.
        copy_command: &'static str,
    },
    /// A built-in profile cannot be deleted.
    BuiltInUndeletable {
        /// The name that was refused.
        name: String,
    },
    /// The file system failed.
    Io(FsError),
}

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => f.write_str(message),
            Self::BadName => write!(f, "profile names use letters, digits, . _ - (max {MAX_NAME})"),
            Self::NotFound { what, name, hint: Some(hint) } => {
                write!(f, "no {what} profile '{name}' ({hint})")
            }
            Self::NotFound { what, name, hint: None } => write!(f, "no {what} profile '{name}'"),
            Self::NotJson { what, name } => write!(f, "{what} profile '{name}' is not valid JSON"),
            Self::BuiltInReadOnly { name, copy_command } => write!(
                f,
                "'{name}' is built in and read-only; copy it ({copy_command} copy {name}) and edit the copy"
            ),
            Self::BuiltInUndeletable { name } => write!(f, "'{name}' is built in and cannot be deleted"),
            Self::Io(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for ProfileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<FsError> for ProfileError {
    fn from(e: FsError) -> Self {
        Self::Io(e)
    }
}

/// What differs between the profile files of the CPU and of the GPU.
pub(crate) trait Kind {
    /// The profile.
    type Profile: Clone + Serialize;
    /// `undervolt` or `GPU undervolt`, as it appears in messages.
    const WHAT: &'static str;
    /// The command that lists the profiles, for the "no such profile" message.
    const LIST_COMMAND: &'static str;
    /// The command prefix that copies a profile, for the "built in" message.
    const COPY_COMMAND: &'static str;
    /// The built-in profile of that name.
    fn builtin(name: &str) -> Option<Self::Profile>;
    /// The built-in names in display order.
    fn builtin_names() -> &'static [&'static str];
    /// Validates a decoded JSON value into a profile.
    fn parse(value: &Value) -> Result<Self::Profile, ProfileError>;
    /// Validates a profile that is already typed.
    fn check(profile: &Self::Profile) -> Result<(), ProfileError>;
}

/// The profiles of one kind in one directory. Saving applies nothing.
pub(crate) struct NamedStore<K: Kind> {
    dir: PathBuf,
    kind: PhantomData<fn() -> K>,
}

/// A profile name: 1 to 40 of letters, digits, `.`, `_`, `-`. No separator can be in it, so the
/// file is always directly inside the store's directory.
fn valid_name(name: &str) -> bool {
    (1..=MAX_NAME).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

impl<K: Kind> NamedStore<K> {
    pub(crate) fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into(), kind: PhantomData }
    }

    pub(crate) fn dir(&self) -> &std::path::Path {
        &self.dir
    }

    fn path(&self, name: &str) -> Result<PathBuf, ProfileError> {
        if !valid_name(name) {
            return Err(ProfileError::BadName);
        }
        Ok(self.dir.join(format!("{name}.json")))
    }

    pub(crate) fn is_builtin(name: &str) -> bool {
        K::builtin(name).is_some()
    }

    /// The built-in names, then the user's, sorted. A file whose name could never be loaded is
    /// not listed.
    pub(crate) fn names(&self) -> Result<Vec<String>, ProfileError> {
        let mut out: Vec<String> = K::builtin_names().iter().map(|n| (*n).to_owned()).collect();
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(FsError::Io { op: "list", path: self.dir.clone(), source: e }.into()),
        };
        let mut own: Vec<String> = entries
            .filter_map(Result::ok)
            .filter_map(|e| e.file_name().into_string().ok())
            .filter_map(|file| file.strip_suffix(".json").map(str::to_owned))
            .filter(|name| valid_name(name) && !Self::is_builtin(name))
            .collect();
        own.sort();
        out.extend(own);
        Ok(out)
    }

    /// A built-in or saved profile, validated. `stock` is the old name of `native`.
    pub(crate) fn load(&self, name: &str) -> Result<K::Profile, ProfileError> {
        let name = if name == "stock" { "native" } else { name };
        if let Some(profile) = K::builtin(name) {
            return Ok(profile);
        }
        let path = self.path(name)?;
        let bytes = match fsx::read_limited(&path, fsx::MAX_SETTINGS_FILE) {
            Ok(bytes) => bytes,
            Err(e) if e.is_not_found() => {
                return Err(ProfileError::NotFound {
                    what: K::WHAT,
                    name: name.to_owned(),
                    hint: Some(K::LIST_COMMAND),
                });
            }
            Err(e) => return Err(e.into()),
        };
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| ProfileError::NotJson { what: K::WHAT, name: name.to_owned() })?;
        K::parse(&value)
    }

    /// Validates and writes a profile atomically. Built-in names are refused.
    pub(crate) fn save(&self, name: &str, profile: &K::Profile) -> Result<K::Profile, ProfileError> {
        if Self::is_builtin(name) {
            return Err(ProfileError::BuiltInReadOnly {
                name: name.to_owned(),
                copy_command: K::COPY_COMMAND,
            });
        }
        K::check(profile)?;
        let path = self.path(name)?;
        fsx::write_atomic_mkdir(&path, &to_json(profile)?, 0o644)?;
        Ok(profile.clone())
    }

    /// Removes a saved profile. Built-in profiles cannot be deleted.
    pub(crate) fn delete(&self, name: &str) -> Result<(), ProfileError> {
        if Self::is_builtin(name) {
            return Err(ProfileError::BuiltInUndeletable { name: name.to_owned() });
        }
        let path = self.path(name)?;
        std::fs::remove_file(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ProfileError::NotFound { what: K::WHAT, name: name.to_owned(), hint: None }
            } else {
                FsError::Io { op: "remove", path, source: e }.into()
            }
        })
    }

    /// Saves a copy of `name` as `new`, or as `NAME-copy` (then `-2`, `-3`, ...) when `new` is
    /// missing, empty, or taken; returns the name used.
    pub(crate) fn copy(&self, name: &str, new: Option<&str>) -> Result<String, ProfileError> {
        let base = match new {
            Some(n) if !n.is_empty() => n.to_owned(),
            _ => format!("{name}-copy"),
        };
        let taken = self.names()?;
        let mut target = base.clone();
        let mut n = 2;
        while taken.contains(&target) {
            target = format!("{base}-{n}");
            n += 1;
        }
        let profile = self.load(name)?;
        self.save(&target, &profile)?;
        Ok(target)
    }
}

/// The file's text: the Python edition's `json.dump(..., indent=1)` (one space per level, no
/// trailing newline), which both older editions read.
fn to_json<P: Serialize>(profile: &P) -> Result<Vec<u8>, ProfileError> {
    let mut out = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b" ");
    let mut serializer = serde_json::Serializer::with_formatter(&mut out, formatter);
    profile
        .serialize(&mut serializer)
        .map_err(|e| ProfileError::Invalid(format!("cannot encode the profile: {e}")))?;
    Ok(out)
}
