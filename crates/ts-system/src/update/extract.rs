//! A strict `.tar.gz` extractor for downloaded releases.
//!
//! The archive comes from the network and is unpacked by a process that is about to run its
//! contents as root, so the extractor is safe by construction instead of by checking what could
//! go wrong afterwards:
//!
//! * only regular files and directories are ever created: links (symbolic and hard), device
//!   nodes, FIFOs and every other entry type refuse the whole archive, so no later entry can be
//!   redirected through a link and nothing needs resolving;
//! * every name is made relative and cleaned, and an absolute name, a `..` component, a NUL or
//!   an over-long name refuses the whole archive;
//! * files are created with `O_EXCL` (never overwriting, never following a link), directories
//!   are walked one component at a time and a component that is not a real directory is
//!   refused, and modes are reduced to `0644` or `0755` (no setuid, no sticky, no ownership);
//! * the amount of work is bounded: the number of entries, the total size of the files, the
//!   size of the metadata a header may carry and the padding after the end of the archive.
//!
//! The tar format is parsed by hand (ustar, GNU long names and PAX path/size records, which is
//! what GitHub, `git archive`, GNU tar and bsdtar produce) on top of `flate2`'s decompressor, so
//! the code that decides what may be written is this file and nothing else. It never panics:
//! every malformed input is an [`ExtractError`].
#![forbid(unsafe_code)]

use std::fmt;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, BufRead, BufReader, Read};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;

/// A tar block: headers and data are made of these.
const BLOCK: usize = 512;

/// The longest name accepted, in bytes (Linux allows 4096 in a path, no release needs half of
/// this).
const MAX_NAME: usize = 1024;

/// The most bytes a PAX extended header or a GNU long-name record may carry.
const MAX_META: u64 = 16 * 1024;

/// The most bytes of metadata (extended headers, global headers, data of entries that carry
/// none) that may be read and thrown away in one archive. Real releases use a few hundred.
const META_BUDGET: u64 = 4 * 1024 * 1024;

/// The most zero padding after the end-of-archive marker (tar pads to its blocking factor, 10 KiB
/// by default).
const TAIL_LIMIT: u64 = 8 * 1024 * 1024;

/// What an archive may unpack to, so a hostile or corrupt download cannot fill the disk (a
/// "zip bomb": a few kilobytes that expand to gigabytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtractLimits {
    /// Entries of any kind.
    pub max_files: usize,
    /// Total uncompressed size of the regular files.
    pub max_bytes: u64,
}

impl Default for ExtractLimits {
    /// Generous for a release (a few hundred files, a few MB) and far below what could hurt.
    fn default() -> Self {
        Self { max_files: 20_000, max_bytes: 400 * 1024 * 1024 }
    }
}

/// Why an archive was refused.
#[derive(Debug)]
pub enum ExtractError {
    /// The data does not start with the gzip magic number.
    NotGzip,
    /// The archive asks for something this extractor never does (unsafe path, link, device,
    /// too large). The text keeps the wording of the 0.3.x releases.
    Unsafe(String),
    /// The archive is damaged or truncated.
    Invalid(String),
    /// Writing below the destination failed.
    Io {
        /// The path being created.
        path: PathBuf,
        /// What the operating system said.
        source: io::Error,
    },
}

impl fmt::Display for ExtractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotGzip => f.write_str("the download is not a gzip archive"),
            Self::Unsafe(reason) => f.write_str(reason),
            Self::Invalid(why) => write!(f, "the download is not a valid archive: {why}"),
            Self::Io { path, source } => write!(f, "unpack {}: {source}", path.display()),
        }
    }
}

impl std::error::Error for ExtractError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

fn unsafe_path(name: &str) -> ExtractError {
    ExtractError::Unsafe(format!("the download contains an unsafe path ({}); refusing", printable(name)))
}

fn invalid(why: impl Into<String>) -> ExtractError {
    ExtractError::Invalid(why.into())
}

/// A name made safe to put in a message: control characters and an over-long tail are cut.
fn printable(s: &str) -> String {
    let short: String = s.chars().take(80).collect();
    let mut out: String = short.chars().map(|c| if c.is_control() { '?' } else { c }).collect();
    if s.chars().count() > 80 {
        out.push_str("...");
    }
    out
}

/// Turns an archive member name into a relative path whose components are all plain names, or
/// refuses it. The empty string means the archive's root directory itself (`./`).
pub fn clean_name(name: &str) -> Result<String, ExtractError> {
    if name.is_empty() || name.len() > MAX_NAME || name.contains('\0') || name.starts_with('/') {
        return Err(unsafe_path(name));
    }
    let mut parts = Vec::new();
    for part in name.split('/') {
        match part {
            "" | "." => {}
            ".." => return Err(unsafe_path(name)),
            p => parts.push(p),
        }
    }
    Ok(parts.join("/"))
}

/// Unpacks a `.tar.gz` stream into `dest`, which must already exist and should be empty.
///
/// On any error the caller deletes `dest`: entries before the offending one may have been
/// written, nothing outside `dest` ever is. See the module documentation for what is refused.
pub fn extract_tar_gz(reader: impl Read, dest: &Path, limits: &ExtractLimits) -> Result<(), ExtractError> {
    let mut buffered = BufReader::new(reader);
    // Looked at before the decompressor so that plain text is "not a gzip archive" and not a
    // confusing error from deep inside the decoder.
    let head = buffered.fill_buf().map_err(|e| invalid(e.to_string()))?;
    if head.len() < 2 || head[..2] != [0x1f, 0x8b] {
        return Err(ExtractError::NotGzip);
    }
    match fs::metadata(dest) {
        Ok(m) if m.is_dir() => {}
        Ok(_) => {
            return Err(ExtractError::Io {
                path: dest.to_path_buf(),
                source: io::Error::new(io::ErrorKind::NotADirectory, "not a directory"),
            });
        }
        Err(source) => return Err(ExtractError::Io { path: dest.to_path_buf(), source }),
    }
    let mut tar = Tar { input: GzDecoder::new(buffered), meta_left: META_BUDGET };
    extract_entries(&mut tar, dest, limits)?;
    // The tar part ended; the gzip trailer (length and checksum) comes after it. Reading on makes
    // a download cut short or corrupted there an error instead of half-trusted, and the rest must
    // be padding.
    tar.finish()
}

/// The decompressed stream with the helpers the tar format needs.
struct Tar<R: Read> {
    input: GzDecoder<R>,
    /// What may still be read and discarded.
    meta_left: u64,
}

impl<R: Read> Tar<R> {
    /// Reads exactly `buf.len()` bytes; `Ok(false)` only for a clean end before the first byte.
    fn fill(&mut self, buf: &mut [u8]) -> Result<bool, ExtractError> {
        let mut done = 0;
        while done < buf.len() {
            match self.input.read(&mut buf[done..]) {
                Ok(0) if done == 0 => return Ok(false),
                Ok(0) => return Err(invalid("unexpected end of the archive")),
                Ok(n) => done += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(invalid(e.to_string())),
            }
        }
        Ok(true)
    }

    fn next_block(&mut self) -> Result<Option<[u8; BLOCK]>, ExtractError> {
        let mut block = [0u8; BLOCK];
        Ok(self.fill(&mut block)?.then_some(block))
    }

    /// Reads and throws away `size` bytes of data plus its padding, within the metadata budget.
    fn discard(&mut self, size: u64) -> Result<(), ExtractError> {
        let padded = padded(size);
        if padded > self.meta_left {
            return Err(ExtractError::Unsafe("the download is unexpectedly large; refusing".into()));
        }
        self.meta_left -= padded;
        let mut left = padded;
        let mut sink = [0u8; BLOCK * 8];
        while left > 0 {
            let n = left.min(sink.len() as u64) as usize;
            if !self.fill(&mut sink[..n])? {
                return Err(invalid("unexpected end of the archive"));
            }
            left -= n as u64;
        }
        Ok(())
    }

    /// Reads a metadata record (extended header, long name) of at most [`MAX_META`] bytes.
    fn read_meta(&mut self, size: u64) -> Result<Vec<u8>, ExtractError> {
        if size > MAX_META {
            return Err(ExtractError::Unsafe("the download is unexpectedly large; refusing".into()));
        }
        let padded = padded(size);
        if padded > self.meta_left {
            return Err(ExtractError::Unsafe("the download is unexpectedly large; refusing".into()));
        }
        self.meta_left -= padded;
        let mut data = vec![0u8; padded as usize];
        if !data.is_empty() && !self.fill(&mut data)? {
            return Err(invalid("unexpected end of the archive"));
        }
        data.truncate(size as usize);
        Ok(data)
    }

    /// After the end-of-archive marker: the rest of the stream must be zero padding, and reading
    /// it to the end verifies the gzip trailer.
    fn finish(&mut self) -> Result<(), ExtractError> {
        let mut seen = 0u64;
        let mut buf = [0u8; 8192];
        loop {
            match self.input.read(&mut buf) {
                Ok(0) => return Ok(()),
                Ok(n) => {
                    seen += n as u64;
                    if seen > TAIL_LIMIT || buf[..n].iter().any(|b| *b != 0) {
                        return Err(invalid("unexpected data after the end of the archive"));
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(invalid(e.to_string())),
            }
        }
    }
}

/// `size` rounded up to whole blocks.
fn padded(size: u64) -> u64 {
    size.div_ceil(BLOCK as u64).saturating_mul(BLOCK as u64)
}

/// The fields of a header that the extractor uses.
struct Header {
    name: String,
    typeflag: u8,
    size: u64,
    mode: u32,
}

/// A tar number field: octal digits padded with spaces and NULs. The base-256 form (high bit of
/// the first byte) is for files over 8 GiB and is refused.
fn parse_octal(field: &[u8]) -> Result<u64, ExtractError> {
    if field.first().is_some_and(|b| b & 0x80 != 0) {
        return Err(invalid("a number field in the binary form is not supported"));
    }
    let text: &[u8] = {
        let start = field.iter().position(|b| *b != b' ' && *b != 0).unwrap_or(field.len());
        let end =
            field[start..].iter().position(|b| *b == 0 || *b == b' ').map_or(field.len(), |i| start + i);
        &field[start..end]
    };
    let mut value = 0u64;
    for &d in text {
        if !(b'0'..=b'7').contains(&d) {
            return Err(invalid("a number field in a header is not octal"));
        }
        value = value
            .checked_mul(8)
            .and_then(|v| v.checked_add(u64::from(d - b'0')))
            .ok_or_else(|| invalid("a number in a header is too large"))?;
    }
    Ok(value)
}

/// The bytes of a NUL-terminated field.
fn c_field(field: &[u8]) -> &[u8] {
    let end = field.iter().position(|b| *b == 0).unwrap_or(field.len());
    &field[..end]
}

fn parse_header(block: &[u8; BLOCK]) -> Result<Header, ExtractError> {
    // The checksum is the sum of all header bytes with the checksum field read as spaces. Old
    // tars summed signed bytes, so either is accepted.
    let (mut unsigned, mut signed) = (0u64, 0i64);
    for (i, &b) in block.iter().enumerate() {
        let b = if (148..156).contains(&i) { b' ' } else { b };
        unsigned += u64::from(b);
        signed += i64::from(b as i8);
    }
    let stored = parse_octal(&block[148..156])?;
    if stored != unsigned && i64::try_from(stored).ok() != Some(signed) {
        return Err(invalid("a header has a wrong checksum"));
    }
    let name_bytes = c_field(&block[0..100]);
    // POSIX ustar ("ustar\0" + "00") keeps a path prefix at 345; GNU tar ("ustar  \0") uses
    // those bytes for times, so the prefix only counts for the POSIX form.
    let prefix = if &block[257..265] == b"ustar\x0000" { c_field(&block[345..500]) } else { b"" };
    let raw = if prefix.is_empty() { name_bytes.to_vec() } else { [prefix, b"/", name_bytes].concat() };
    // A name that is not UTF-8 cannot be told safe or unsafe by the rules above: refuse it.
    let name = String::from_utf8(raw).map_err(|e| unsafe_path(&String::from_utf8_lossy(e.as_bytes())))?;
    Ok(Header {
        name,
        typeflag: block[156],
        size: parse_octal(&block[124..136])?,
        mode: u32::try_from(parse_octal(&block[100..108])?)
            .map_err(|_| invalid("a mode in a header is too large"))?,
    })
}

/// What a PAX extended header changes for the next entry.
#[derive(Default)]
struct PaxOverrides {
    path: Option<String>,
    size: Option<u64>,
}

/// Parses PAX records (`<len> <key>=<value>\n`, `len` counting the whole record).
fn parse_pax(data: &[u8]) -> Result<PaxOverrides, ExtractError> {
    let mut out = PaxOverrides::default();
    let mut rest = data;
    while !rest.is_empty() {
        let space =
            rest.iter().position(|b| *b == b' ').ok_or_else(|| invalid("a PAX record has no length"))?;
        let len: usize = std::str::from_utf8(&rest[..space])
            .ok()
            .filter(|t| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|t| t.parse().ok())
            .ok_or_else(|| invalid("a PAX record has a bad length"))?;
        if len <= space + 1 || len > rest.len() || rest[len - 1] != b'\n' {
            return Err(invalid("a PAX record has a wrong length"));
        }
        let body = &rest[space + 1..len - 1];
        rest = &rest[len..];
        let eq = body.iter().position(|b| *b == b'=').ok_or_else(|| invalid("a PAX record has no value"))?;
        let (key, value) = (&body[..eq], &body[eq + 1..]);
        match key {
            b"path" => {
                out.path = Some(
                    String::from_utf8(value.to_vec())
                        .map_err(|e| unsafe_path(&String::from_utf8_lossy(e.as_bytes())))?,
                );
            }
            b"size" => {
                out.size = Some(
                    std::str::from_utf8(value)
                        .ok()
                        .filter(|t| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()))
                        .and_then(|t| t.parse().ok())
                        .ok_or_else(|| invalid("a PAX size is not a number"))?,
                );
            }
            // Sparse files describe holes this extractor has no use for and a release has none.
            k if k.starts_with(b"GNU.sparse") => {
                return Err(ExtractError::Unsafe(
                    "the download contains an unsupported entry type 'S'; refusing".into(),
                ));
            }
            // Times, owners, comments, link targets (links are refused anyway): not needed.
            _ => {}
        }
    }
    Ok(out)
}

fn extract_entries<R: Read>(
    tar: &mut Tar<R>,
    dest: &Path,
    limits: &ExtractLimits,
) -> Result<(), ExtractError> {
    let mut entries = 0usize;
    let mut meta_headers = 0usize;
    let mut total = 0u64;
    let mut long_name: Option<String> = None;
    let mut pax = PaxOverrides::default();
    loop {
        let Some(block) = tar.next_block()? else { return Ok(()) };
        if block.iter().all(|b| *b == 0) {
            // The end of the archive is two zero blocks; one followed by the end of the data is
            // the same thing cut a little short, and the gzip checksum vouches for the rest.
            return match tar.next_block()? {
                None => Ok(()),
                Some(b) if b.iter().all(|x| *x == 0) => Ok(()),
                Some(_) => Err(invalid("data after the end-of-archive marker")),
            };
        }
        let header = parse_header(&block)?;
        match header.typeflag {
            b'g' => {
                // GitHub's archives start with one (the commit id): metadata, not a file.
                bump(&mut meta_headers, limits.max_files)?;
                tar.discard(header.size)?;
                continue;
            }
            b'x' => {
                bump(&mut meta_headers, limits.max_files)?;
                pax = parse_pax(&tar.read_meta(header.size)?)?;
                continue;
            }
            b'L' => {
                bump(&mut meta_headers, limits.max_files)?;
                let data = tar.read_meta(header.size)?;
                let name = String::from_utf8(c_field(&data).to_vec())
                    .map_err(|e| unsafe_path(&String::from_utf8_lossy(e.as_bytes())))?;
                long_name = Some(name);
                continue;
            }
            _ => {}
        }
        bump(&mut entries, limits.max_files)?;
        let name = pax.path.take().or_else(|| long_name.take()).unwrap_or(header.name);
        long_name = None;
        let size = pax.size.take().unwrap_or(header.size);
        let rel = clean_name(&name)?;
        // A plain file whose name ends in a slash is how old tars wrote a directory.
        let typeflag = if header.typeflag == 0 && name.ends_with('/') { b'5' } else { header.typeflag };
        match typeflag {
            b'0' | 0 => {
                if rel.is_empty() {
                    return Err(unsafe_path(&name));
                }
                total = total.saturating_add(size);
                if total > limits.max_bytes {
                    return Err(ExtractError::Unsafe("the download is unexpectedly large; refusing".into()));
                }
                write_file(tar, dest, &rel, size, header.mode)?;
            }
            b'5' => {
                if !rel.is_empty() {
                    ensure_dirs(dest, &rel, &name)?;
                }
                tar.discard(size)?;
            }
            b'1' | b'2' => {
                return Err(ExtractError::Unsafe(format!(
                    "the download contains an unsafe link ({}); refusing",
                    printable(&name)
                )));
            }
            b'3' | b'4' | b'6' => {
                return Err(ExtractError::Unsafe("the download contains a device file; refusing".into()));
            }
            other => {
                return Err(ExtractError::Unsafe(format!(
                    "the download contains an unsupported entry type {:?} ({}); refusing",
                    char::from(other),
                    printable(&name)
                )));
            }
        }
    }
}

/// Counts one more header and refuses an archive that has too many.
fn bump(count: &mut usize, max: usize) -> Result<(), ExtractError> {
    *count += 1;
    if *count > max {
        return Err(ExtractError::Unsafe("the download has too many entries; refusing".into()));
    }
    Ok(())
}

/// Makes every directory of `rel` below `dest`, one component at a time. A component that
/// exists as anything but a real directory (a file, or a link: none can have been made by this
/// extractor, but `dest` may not have been empty) refuses the archive.
fn ensure_dirs(dest: &Path, rel: &str, name: &str) -> Result<PathBuf, ExtractError> {
    let mut path = dest.to_path_buf();
    for part in rel.split('/') {
        path.push(part);
        match fs::symlink_metadata(&path) {
            Ok(m) if m.is_dir() => {}
            Ok(_) => {
                return Err(ExtractError::Unsafe(format!(
                    "the download uses a file or link as a directory ({}); refusing",
                    printable(name)
                )));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                DirBuilder::new()
                    .mode(0o755)
                    .create(&path)
                    .map_err(|source| ExtractError::Io { path: path.clone(), source })?;
                // The umask may have narrowed the mode.
                fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
                    .map_err(|source| ExtractError::Io { path: path.clone(), source })?;
            }
            Err(source) => return Err(ExtractError::Io { path: path.clone(), source }),
        }
    }
    Ok(path)
}

fn write_file<R: Read>(
    tar: &mut Tar<R>,
    dest: &Path,
    rel: &str,
    size: u64,
    mode: u32,
) -> Result<(), ExtractError> {
    let (dir, file_name) = match rel.rsplit_once('/') {
        Some((dir, file)) => (ensure_dirs(dest, dir, rel)?, file),
        None => (dest.to_path_buf(), rel),
    };
    let path = dir.join(file_name);
    let io_err = |source| ExtractError::Io { path: path.clone(), source };
    // Private while it is being written; `create_new` is `O_EXCL`: an existing file or link of
    // that name is an error, never overwritten and never followed.
    let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path).map_err(io_err)?;
    let copied = io::copy(&mut (&mut tar.input).take(size), &mut file).map_err(|e| invalid(e.to_string()))?;
    if copied != size {
        return Err(invalid("unexpected end of the archive"));
    }
    let perm = if mode & 0o100 != 0 { 0o755 } else { 0o644 };
    file.set_permissions(fs::Permissions::from_mode(perm)).map_err(io_err)?;
    drop(file);
    // The data is followed by padding up to the next block.
    let pad = padded(size) - size;
    if pad > 0 {
        let mut skip = vec![0u8; pad as usize];
        if !tar.fill(&mut skip)? {
            return Err(invalid("unexpected end of the archive"));
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(crate) mod testkit {
    //! Builders for tar archives, byte by byte, so that hostile headers the usual tar writers
    //! refuse to produce can be made too.
    use super::BLOCK;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::fs;
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    pub(crate) struct TempDir(pub(crate) PathBuf);
    impl TempDir {
        pub(crate) fn new() -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let p = std::env::temp_dir().join(format!(
                "ts-system-extract-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// One tar member, written byte by byte so that hostile headers the usual tar writers refuse
    /// to produce can be made too.
    #[derive(Clone)]
    pub(crate) struct Entry {
        pub(crate) name: String,
        pub(crate) typeflag: u8,
        pub(crate) link: String,
        pub(crate) body: Vec<u8>,
        pub(crate) mode: u32,
    }

    pub(crate) fn file(name: &str, body: &str) -> Entry {
        Entry { name: name.into(), typeflag: b'0', link: String::new(), body: body.into(), mode: 0o644 }
    }
    pub(crate) fn dir(name: &str) -> Entry {
        Entry { name: name.into(), typeflag: b'5', link: String::new(), body: vec![], mode: 0o755 }
    }
    pub(crate) fn other(name: &str, typeflag: u8, link: &str) -> Entry {
        Entry { name: name.into(), typeflag, link: link.into(), body: vec![], mode: 0o644 }
    }

    pub(crate) fn put(field: &mut [u8], text: &[u8]) {
        field[..text.len()].copy_from_slice(text);
    }

    pub(crate) fn header(name: &[u8], typeflag: u8, link: &[u8], size: usize, mode: u32) -> [u8; BLOCK] {
        let mut h = [0u8; BLOCK];
        put(&mut h[0..100], name);
        put(&mut h[100..108], format!("{mode:07o}\0").as_bytes());
        put(&mut h[108..116], b"0000000\0");
        put(&mut h[116..124], b"0000000\0");
        put(&mut h[124..136], format!("{size:011o}\0").as_bytes());
        put(&mut h[136..148], b"00000000000\0");
        h[148..156].fill(b' ');
        h[156] = typeflag;
        put(&mut h[157..257], link);
        put(&mut h[257..263], b"ustar\0");
        put(&mut h[263..265], b"00");
        seal(&mut h);
        h
    }

    pub(crate) fn seal(h: &mut [u8; BLOCK]) {
        h[148..156].fill(b' ');
        let sum: u32 = h.iter().map(|b| u32::from(*b)).sum();
        put(&mut h[148..156], format!("{sum:06o}\0 ").as_bytes());
    }

    pub(crate) fn data_blocks(body: &[u8]) -> Vec<u8> {
        let mut out = body.to_vec();
        out.resize(body.len().div_ceil(BLOCK) * BLOCK, 0);
        out
    }

    pub(crate) fn tar(entries: &[Entry]) -> Vec<u8> {
        let mut out = Vec::new();
        for e in entries {
            let mut name = e.name.as_bytes();
            if name.len() > 100 {
                // A name that does not fit the header goes in a GNU long-name record first.
                let mut long = name.to_vec();
                long.push(0);
                out.extend_from_slice(&header(b"././@LongLink", b'L', b"", long.len(), 0));
                out.extend(data_blocks(&long));
                name = &name[..100];
            }
            out.extend_from_slice(&header(name, e.typeflag, e.link.as_bytes(), e.body.len(), e.mode));
            out.extend(data_blocks(&e.body));
        }
        out.extend_from_slice(&[0u8; 1024]);
        out
    }

    pub(crate) fn gz(raw: &[u8]) -> Vec<u8> {
        let mut enc = GzEncoder::new(Vec::new(), Compression::fast());
        enc.write_all(raw).unwrap();
        enc.finish().unwrap()
    }

    pub(crate) fn archive(entries: &[Entry]) -> Vec<u8> {
        gz(&tar(entries))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    use super::testkit::*;

    /// Extracts into `<parent>/dest`; the parent lets a test prove nothing escaped.
    fn extract(blob: &[u8], limits: ExtractLimits) -> (TempDir, PathBuf, Result<(), ExtractError>) {
        let parent = TempDir::new();
        let dest = parent.0.join("dest");
        fs::create_dir(&dest).unwrap();
        let r = extract_tar_gz(blob, &dest, &limits);
        (parent, dest, r)
    }

    /// Fails unless nothing exists beside `dest` and everything inside it is a plain file or
    /// directory with a harmless mode. Returns the relative paths.
    fn only_plain_files(parent: &Path) -> Vec<String> {
        let names: Vec<_> = fs::read_dir(parent).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, ["dest"], "something was written outside the destination");
        let mut found = Vec::new();
        let mut stack = vec![parent.join("dest")];
        while let Some(d) = stack.pop() {
            for e in fs::read_dir(&d).unwrap() {
                let p = e.unwrap().path();
                let m = fs::symlink_metadata(&p).unwrap();
                let t = m.file_type();
                assert!(
                    !t.is_symlink()
                        && !t.is_fifo()
                        && !t.is_socket()
                        && !t.is_block_device()
                        && !t.is_char_device(),
                    "{p:?} is special"
                );
                assert_eq!(m.mode() & 0o7022, 0, "{p:?} has mode {:o}", m.mode());
                found.push(p.strip_prefix(parent.join("dest")).unwrap().display().to_string());
                if t.is_dir() {
                    stack.push(p);
                }
            }
        }
        found.sort();
        found
    }

    fn msg(r: Result<(), ExtractError>) -> String {
        r.unwrap_err().to_string()
    }

    #[test]
    fn an_ordinary_archive_is_unpacked_with_safe_modes() {
        let mut run = file("./pkg/bin/run.sh", "echo");
        run.mode = 0o4755; // setuid must not survive
        let mut data = file("pkg/data.txt", "hello");
        data.mode = 0o600;
        let blob = archive(&[dir("pkg/"), run, data, file("pkg/sub/deep/file", ""), dir("./")]);
        let (parent, dest, r) = extract(&blob, ExtractLimits::default());
        r.unwrap();
        assert_eq!(
            only_plain_files(&parent.0),
            [
                "pkg",
                "pkg/bin",
                "pkg/bin/run.sh",
                "pkg/data.txt",
                "pkg/sub",
                "pkg/sub/deep",
                "pkg/sub/deep/file"
            ]
        );
        let mode = |rel: &str| fs::symlink_metadata(dest.join(rel)).unwrap().mode() & 0o7777;
        assert_eq!(mode("pkg/bin/run.sh"), 0o755);
        assert_eq!(mode("pkg/data.txt"), 0o644);
        assert_eq!(mode("pkg/sub/deep/file"), 0o644);
        assert_eq!(mode("pkg"), 0o755);
        assert_eq!(fs::read_to_string(dest.join("pkg/bin/run.sh")).unwrap(), "echo");
        assert_eq!(fs::read_to_string(dest.join("pkg/data.txt")).unwrap(), "hello");
    }

    #[test]
    fn data_that_spans_several_blocks_is_copied_exactly() {
        let body: String = (0..5000).map(|i| char::from(b'a' + (i % 26) as u8)).collect();
        let blob = archive(&[file("big", &body), file("after", "x")]);
        let (_p, dest, r) = extract(&blob, ExtractLimits::default());
        r.unwrap();
        assert_eq!(fs::read_to_string(dest.join("big")).unwrap(), body);
        assert_eq!(fs::read_to_string(dest.join("after")).unwrap(), "x");
    }

    #[test]
    fn a_git_hub_archive_with_a_global_pax_header_is_accepted() {
        let comment = b"52 comment=0123456789abcdef0123456789abcdef01234567\n";
        let mut raw = Vec::new();
        raw.extend_from_slice(&header(b"pax_global_header", b'g', b"", comment.len(), 0o666));
        raw.extend(data_blocks(comment));
        raw.extend_from_slice(&header(b"r/f", b'0', b"", 2, 0o644));
        raw.extend(data_blocks(b"ok"));
        raw.extend_from_slice(&[0u8; 1024]);
        let (parent, _d, r) = extract(&gz(&raw), ExtractLimits::default());
        r.unwrap();
        assert_eq!(only_plain_files(&parent.0), ["r", "r/f"], "no file called pax_global_header");
    }

    #[test]
    fn pax_path_and_gnu_long_names_replace_the_short_name() {
        let long = format!("top/{}/file.txt", "d".repeat(120));
        let record = {
            let body = format!(" path={long}\n");
            let len = body.len() + 3; // three digits
            format!("{len}{body}")
        };
        assert_eq!(record.len().to_string().len(), 3);
        let mut raw = Vec::new();
        raw.extend_from_slice(&header(b"PaxHeader", b'x', b"", record.len(), 0o644));
        raw.extend(data_blocks(record.as_bytes()));
        raw.extend_from_slice(&header(b"truncated", b'0', b"", 1, 0o644));
        raw.extend(data_blocks(b"p"));
        // GNU long name: the name is the data of an 'L' entry, NUL-terminated.
        let gnu = format!("gnu/{}/leaf", "e".repeat(130));
        let mut gnu_data = gnu.clone().into_bytes();
        gnu_data.push(0);
        raw.extend_from_slice(&header(b"././@LongLink", b'L', b"", gnu_data.len(), 0));
        raw.extend(data_blocks(&gnu_data));
        raw.extend_from_slice(&header(b"short", b'0', b"", 1, 0o644));
        raw.extend(data_blocks(b"g"));
        raw.extend_from_slice(&[0u8; 1024]);
        let (_p, dest, r) = extract(&gz(&raw), ExtractLimits::default());
        r.unwrap();
        assert_eq!(fs::read_to_string(dest.join(&long)).unwrap(), "p");
        assert_eq!(fs::read_to_string(dest.join(&gnu)).unwrap(), "g");
        assert!(!dest.join("truncated").exists() && !dest.join("short").exists());
    }

    #[test]
    fn a_pax_path_cannot_smuggle_a_traversal() {
        for path in ["../evil", "/abs/evil", "a/../../evil"] {
            let body = format!(" path={path}\n");
            let record = format!("{}{body}", body.len() + 2);
            let mut raw = Vec::new();
            raw.extend_from_slice(&header(b"PaxHeader", b'x', b"", record.len(), 0o644));
            raw.extend(data_blocks(record.as_bytes()));
            raw.extend_from_slice(&header(b"innocent", b'0', b"", 1, 0o644));
            raw.extend(data_blocks(b"x"));
            raw.extend_from_slice(&[0u8; 1024]);
            let (parent, _d, r) = extract(&gz(&raw), ExtractLimits::default());
            assert!(msg(r).contains("unsafe path"), "{path}");
            only_plain_files(&parent.0);
        }
    }

    #[test]
    fn the_ustar_prefix_is_joined_to_the_name() {
        let mut h = header(b"leaf", b'0', b"", 1, 0o644);
        put(&mut h[345..500], b"some/prefix");
        seal(&mut h);
        let mut raw = h.to_vec();
        raw.extend(data_blocks(b"x"));
        raw.extend_from_slice(&[0u8; 1024]);
        let (_p, dest, r) = extract(&gz(&raw), ExtractLimits::default());
        r.unwrap();
        assert!(dest.join("some/prefix/leaf").is_file());
        // A prefix may not be used to climb either.
        let mut h = header(b"leaf", b'0', b"", 1, 0o644);
        put(&mut h[345..500], b"..");
        seal(&mut h);
        let mut raw = h.to_vec();
        raw.extend(data_blocks(b"x"));
        raw.extend_from_slice(&[0u8; 1024]);
        let (parent, _d, r) = extract(&gz(&raw), ExtractLimits::default());
        assert!(msg(r).contains("unsafe path"));
        only_plain_files(&parent.0);
    }

    #[test]
    fn malicious_archives_are_refused_and_write_nothing_outside() {
        let cases: Vec<(&str, Vec<Entry>, &str)> = vec![
            ("parent traversal", vec![file("../evil", "x")], "unsafe path"),
            ("nested traversal", vec![file("a/b/../../../evil", "x")], "unsafe path"),
            ("traversal behind a dot", vec![file("./../evil", "x")], "unsafe path"),
            ("traversal as a directory", vec![dir("../evildir/")], "unsafe path"),
            ("traversal in the middle", vec![dir("ok/"), file("ok/../../evil", "x")], "unsafe path"),
            ("absolute path", vec![file("/tmp/evil", "x")], "unsafe path"),
            ("absolute directory", vec![dir("/etc/")], "unsafe path"),
            ("symlink out", vec![other("link", b'2', "../../etc")], "unsafe link"),
            ("symlink to an absolute path", vec![other("link", b'2', "/etc/passwd")], "unsafe link"),
            (
                "a harmless-looking symlink inside the tree",
                vec![file("a", "x"), other("b", b'2', "a")],
                "unsafe link",
            ),
            (
                "symlink then write through it",
                vec![other("d", b'2', "/tmp"), file("d/evil", "x")],
                "unsafe link",
            ),
            ("hard link", vec![file("a", "x"), other("h", b'1', "a")], "unsafe link"),
            ("hard link to a system file", vec![other("h", b'1', "/etc/passwd")], "unsafe link"),
            ("char device", vec![other("null", b'3', "")], "device file"),
            ("block device", vec![other("sda", b'4', "")], "device file"),
            ("fifo", vec![other("pipe", b'6', "")], "device file"),
            ("empty name", vec![file("", "x")], "unsafe path"),
            ("contiguous file", vec![other("x", b'7', "")], "unsupported entry type"),
            ("GNU sparse", vec![other("s", b'S', "")], "unsupported entry type"),
            ("GNU incremental directory", vec![other("d", b'D', "")], "unsupported entry type"),
            ("vendor extension", vec![other("v", b'V', "")], "unsupported entry type"),
            ("a name that is the root as a file", vec![file("./", "x")], "unsafe path"),
            ("a name that is only dots and slashes", vec![file("././.", "x")], "unsafe path"),
            ("a very long name", vec![file(&format!("{}f", "a/".repeat(600)), "x")], "unsafe path"),
        ];
        for (name, entries, reason) in cases {
            let (parent, _d, r) = extract(&archive(&entries), ExtractLimits::default());
            let text = msg(r);
            assert!(text.contains(reason), "{name}: {text}");
            assert!(text.ends_with("refusing") || text.contains("unpack"), "{name}: {text}");
            only_plain_files(&parent.0);
        }
    }

    #[test]
    fn the_wording_of_the_refusals_is_the_0_3_x_one() {
        let (_p, _d, r) = extract(&archive(&[file("../evil", "x")]), ExtractLimits::default());
        assert_eq!(msg(r), "the download contains an unsafe path (../evil); refusing");
        let (_p, _d, r) = extract(&archive(&[other("l", b'2', "x")]), ExtractLimits::default());
        assert_eq!(msg(r), "the download contains an unsafe link (l); refusing");
        let (_p, _d, r) = extract(&archive(&[other("n", b'3', "")]), ExtractLimits::default());
        assert_eq!(msg(r), "the download contains a device file; refusing");
        let (_p, _d, r) = extract(&archive(&[other("x", b'7', "")]), ExtractLimits::default());
        assert_eq!(msg(r), "the download contains an unsupported entry type '7' (x); refusing");
    }

    #[test]
    fn control_characters_and_long_names_are_made_printable_in_messages() {
        let (_p, _d, r) = extract(&archive(&[file("../evil\u{1b}[31m", "x")]), ExtractLimits::default());
        assert_eq!(msg(r), "the download contains an unsafe path (../evil?[31m); refusing");
        let long = format!("../{}", "x".repeat(200));
        let text = msg(extract(&archive(&[file(&long, "x")]), ExtractLimits::default()).2);
        assert!(text.contains(&format!("{}...", "x".repeat(77))), "{text}");
    }

    #[test]
    fn clean_name_table() {
        for (input, want) in
            [("a", "a"), ("./a/./b", "a/b"), ("a//b", "a/b"), ("./", ""), (".", ""), ("a/b/", "a/b")]
        {
            assert_eq!(clean_name(input).unwrap(), want, "{input:?}");
        }
        let long = "a".repeat(1025);
        for input in ["", "/", "/a", "..", "a/..", "a/../b", "../a", "a\0b", long.as_str()] {
            assert!(clean_name(input).is_err(), "{input:?}");
        }
        assert_eq!(clean_name(&"a".repeat(1024)).unwrap().len(), 1024);
    }

    #[test]
    fn hostile_headers_that_tar_writers_refuse_to_make() {
        let raw = |name: &[u8], typeflag: u8, body: &[u8]| {
            let mut v = header(name, typeflag, b"", body.len(), 0o644).to_vec();
            v.extend(data_blocks(body));
            v.extend_from_slice(&[0u8; 1024]);
            gz(&v)
        };
        for (label, blob) in [
            ("a regular file called ./", raw(b"./", b'0', b"x")),
            ("traversal as a raw name", raw(b"../../evil", b'0', b"x")),
            ("a raw absolute name", raw(b"/etc/cron.d/evil", b'0', b"x")),
            ("a link with an empty target", raw(b"l", b'2', b"")),
            ("a non-UTF-8 name", raw(b"caf\xe9", b'0', b"x")),
        ] {
            let (parent, _d, r) = extract(&blob, ExtractLimits::default());
            assert!(r.is_err(), "{label} was accepted");
            only_plain_files(&parent.0);
        }
        // Control: the raw builder makes a valid archive.
        let (_p, dest, r) = extract(&raw(b"ok/file", b'0', b"hi"), ExtractLimits::default());
        r.unwrap();
        assert_eq!(fs::read_to_string(dest.join("ok/file")).unwrap(), "hi");
    }

    #[test]
    fn a_header_with_a_wrong_checksum_or_a_bad_number_is_invalid() {
        let mut bad = header(b"f", b'0', b"", 1, 0o644);
        bad[0] = b'g'; // changes the sum without resealing
        let mut raw = bad.to_vec();
        raw.extend(data_blocks(b"x"));
        raw.extend_from_slice(&[0u8; 1024]);
        assert!(msg(extract(&gz(&raw), ExtractLimits::default()).2).contains("wrong checksum"));

        let mut h = header(b"f", b'0', b"", 1, 0o644);
        put(&mut h[124..136], b"9999999999\0\0");
        seal(&mut h);
        let mut raw = h.to_vec();
        raw.extend_from_slice(&[0u8; 1024]);
        assert!(msg(extract(&gz(&raw), ExtractLimits::default()).2).contains("not octal"));

        let mut h = header(b"f", b'0', b"", 1, 0o644);
        h[124] = 0x80; // base-256 size
        seal(&mut h);
        let mut raw = h.to_vec();
        raw.extend_from_slice(&[0u8; 1024]);
        assert!(msg(extract(&gz(&raw), ExtractLimits::default()).2).contains("binary form"));

        let mut h = header(b"f", b'0', b"", 1, 0o644);
        put(&mut h[100..108], b"77777777"); // a mode that is fine as a number is fine
        seal(&mut h);
        let mut raw = h.to_vec();
        raw.extend(data_blocks(b"x"));
        raw.extend_from_slice(&[0u8; 1024]);
        let (parent, _d, r) = extract(&gz(&raw), ExtractLimits::default());
        r.unwrap();
        only_plain_files(&parent.0);
    }

    #[test]
    fn an_entry_before_the_malicious_one_may_exist_but_nothing_outside_does() {
        let (parent, dest, r) =
            extract(&archive(&[file("good", "x"), file("../evil", "x")]), ExtractLimits::default());
        assert!(r.is_err());
        only_plain_files(&parent.0);
        // Whether `good` was written before the refusal is not specified; it is inside dest.
        let _ = dest;
    }

    #[test]
    fn a_duplicate_never_overwrites_and_a_file_never_becomes_a_directory() {
        let (_p, dest, r) =
            extract(&archive(&[file("f", "first"), file("f", "second")]), ExtractLimits::default());
        assert!(matches!(r, Err(ExtractError::Io { .. })));
        assert_eq!(fs::read_to_string(dest.join("f")).unwrap(), "first");
        assert!(
            extract(&archive(&[file("f", "x"), file("f/child", "y")]), ExtractLimits::default()).2.is_err()
        );
        assert!(extract(&archive(&[dir("d/"), file("d", "y")]), ExtractLimits::default()).2.is_err());
        assert!(extract(&archive(&[file("f", "x"), dir("f/")]), ExtractLimits::default()).2.is_err());
        // A repeated directory entry is harmless.
        assert!(
            extract(&archive(&[dir("d/"), dir("d/"), file("d/x", "y")]), ExtractLimits::default()).2.is_ok()
        );
    }

    #[test]
    fn existing_links_in_the_destination_are_not_followed() {
        let parent = TempDir::new();
        let dest = parent.0.join("dest");
        let outside = parent.0.join("outside");
        fs::create_dir_all(&dest).unwrap();
        fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, dest.join("d")).unwrap();
        std::os::unix::fs::symlink(outside.join("target"), dest.join("f")).unwrap();
        let r = extract_tar_gz(&archive(&[file("d/evil", "x")])[..], &dest, &ExtractLimits::default());
        assert!(msg(r).contains("file or link as a directory"));
        let r = extract_tar_gz(&archive(&[file("f", "x")])[..], &dest, &ExtractLimits::default());
        assert!(r.is_err(), "O_EXCL refuses to write through a dangling link");
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0, "nothing was written through a link");
    }

    #[test]
    fn extraction_is_bounded() {
        let big = |n: &str| file(n, &"A".repeat(1 << 16));
        let small = ExtractLimits { max_files: 10, max_bytes: 100_000 };
        assert!(msg(extract(&archive(&[big("big"), big("big2")]), small).2).contains("unexpectedly large"));
        let many: Vec<Entry> = (0..20).map(|i| file(&format!("f{i}"), "x")).collect();
        let few = ExtractLimits { max_files: 5, max_bytes: 1 << 20 };
        assert!(msg(extract(&archive(&many), few).2).contains("too many entries"));
        // Exactly at the limit is fine.
        let exact = ExtractLimits { max_files: 20, max_bytes: 20 };
        assert!(extract(&archive(&many), exact).2.is_ok());
        let exact = ExtractLimits { max_files: 19, max_bytes: 20 };
        assert!(extract(&archive(&many), exact).2.is_err());
    }

    #[test]
    fn metadata_headers_count_and_are_bounded_too() {
        // Thousands of global headers must not be an unbounded loop.
        let mut raw = Vec::new();
        for _ in 0..50 {
            raw.extend_from_slice(&header(b"g", b'g', b"", 0, 0o644));
        }
        raw.extend_from_slice(&[0u8; 1024]);
        let limits = ExtractLimits { max_files: 10, max_bytes: 1 << 20 };
        assert!(msg(extract(&gz(&raw), limits).2).contains("too many entries"));
        // An extended header bigger than the cap is refused before it is read.
        let mut raw = header(b"x", b'x', b"", (MAX_META + 1) as usize, 0o644).to_vec();
        raw.extend(vec![0u8; (MAX_META as usize + 1).div_ceil(BLOCK) * BLOCK]);
        raw.extend_from_slice(&[0u8; 1024]);
        assert!(msg(extract(&gz(&raw), ExtractLimits::default()).2).contains("unexpectedly large"));
        // A directory that claims a lot of data is not a way to make us read forever.
        let mut d = dir("d/");
        d.body = vec![0u8; 8 * 1024];
        let mut raw = Vec::new();
        for _ in 0..600 {
            raw.extend_from_slice(&header(b"d/", b'5', b"", d.body.len(), 0o755));
            raw.extend(data_blocks(&d.body));
        }
        raw.extend_from_slice(&[0u8; 1024]);
        let r = extract(&gz(&raw), ExtractLimits::default()).2;
        assert!(msg(r).contains("unexpectedly large"));
    }

    #[test]
    fn a_pax_size_overrides_the_header_size() {
        let body = " size=3\n";
        let record = format!("{}{body}", body.len() + 2);
        let mut raw = Vec::new();
        raw.extend_from_slice(&header(b"PaxHeader", b'x', b"", record.len(), 0o644));
        raw.extend(data_blocks(record.as_bytes()));
        raw.extend_from_slice(&header(b"f", b'0', b"", 0, 0o644));
        raw.extend(data_blocks(b"abc"));
        raw.extend_from_slice(&[0u8; 1024]);
        let (_p, dest, r) = extract(&gz(&raw), ExtractLimits::default());
        r.unwrap();
        assert_eq!(fs::read_to_string(dest.join("f")).unwrap(), "abc");
    }

    #[test]
    fn pax_garbage_is_invalid_or_ignored_never_a_panic() {
        for record in [
            "",
            "5\n",
            "x path=a\n",
            "99 path=a\n",
            "10 path\n",
            "0 =\n",
            " \n",
            "4 a=\n",
            "11 sparse=1\n",
            "16 GNU.sparse.x=1\n",
        ] {
            let mut raw = Vec::new();
            raw.extend_from_slice(&header(b"PaxHeader", b'x', b"", record.len(), 0o644));
            raw.extend(data_blocks(record.as_bytes()));
            raw.extend_from_slice(&header(b"f", b'0', b"", 1, 0o644));
            raw.extend(data_blocks(b"x"));
            raw.extend_from_slice(&[0u8; 1024]);
            let _ = extract(&gz(&raw), ExtractLimits::default()).2;
        }
        let (_p, _d, r) = extract(
            &{
                let record = "18 GNU.sparse.x=1\n";
                let mut raw = header(b"P", b'x', b"", record.len(), 0o644).to_vec();
                raw.extend(data_blocks(record.as_bytes()));
                raw.extend_from_slice(&[0u8; 1024]);
                gz(&raw)
            },
            ExtractLimits::default(),
        );
        assert!(msg(r).contains("unsupported entry type"));
    }

    #[test]
    fn truncated_and_corrupt_downloads_are_refused() {
        let blob = archive(&[file("f", "0123456789"), file("g", "more")]);
        for n in [blob.len() - 1, blob.len() - 5, blob.len() / 2, 11, 3, 2] {
            let (parent, _d, r) = extract(&blob[..n], ExtractLimits::default());
            assert!(r.is_err(), "a download cut to {n} bytes was accepted");
            only_plain_files(&parent.0);
        }
        // A flipped bit in the middle breaks the deflate stream or the gzip checksum.
        let mut flipped = blob.clone();
        let mid = flipped.len() / 2;
        flipped[mid] ^= 0x10;
        assert!(extract(&flipped, ExtractLimits::default()).2.is_err());
        // The gzip checksum itself (the last eight bytes) is verified after the tar ended.
        let mut bad_crc = blob.clone();
        let at = bad_crc.len() - 8;
        bad_crc[at] ^= 0xff;
        assert!(msg(extract(&bad_crc, ExtractLimits::default()).2).contains("not a valid archive"));
    }

    #[test]
    fn plain_text_and_empty_input_are_not_gzip() {
        for input in [&b"this is not gzip"[..], b"", b"\x1f", b"PK\x03\x04", &tar(&[file("f", "x")])] {
            let (_p, _d, r) = extract(input, ExtractLimits::default());
            assert!(matches!(r, Err(ExtractError::NotGzip)), "{input:?}");
        }
        assert_eq!(msg(extract(b"junk", ExtractLimits::default()).2), "the download is not a gzip archive");
    }

    #[test]
    fn a_valid_gzip_of_something_that_is_not_tar_is_invalid() {
        for raw in
            [&b"hello world, this is plain text and not a tar archive at all"[..], &[7u8; 512], &[b'A'; 1500]]
        {
            let (_p, _d, r) = extract(&gz(raw), ExtractLimits::default());
            assert!(r.is_err());
        }
    }

    #[test]
    fn data_after_the_end_marker_and_a_missing_marker() {
        // Non-zero bytes after the marker: refused.
        let mut raw = tar(&[file("f", "x")]);
        raw.extend_from_slice(b"trailing garbage that is not padding");
        assert!(msg(extract(&gz(&raw), ExtractLimits::default()).2).contains("after the end"));
        // Zero padding up to a big blocking factor is fine.
        let mut raw = tar(&[file("f", "x")]);
        raw.resize(10240, 0);
        assert!(extract(&gz(&raw), ExtractLimits::default()).2.is_ok());
        // One zero block and the end of the data, or no marker at all, are tolerated.
        let mut raw = tar(&[file("f", "x")]);
        raw.truncate(raw.len() - 512);
        assert!(extract(&gz(&raw), ExtractLimits::default()).2.is_ok());
        raw.truncate(raw.len() - 512);
        assert!(extract(&gz(&raw), ExtractLimits::default()).2.is_ok());
        // A non-zero block right after the first zero block is not a marker.
        let mut raw = tar(&[file("f", "x")]);
        raw.truncate(raw.len() - 1024);
        raw.extend_from_slice(&[0u8; 512]);
        raw.extend_from_slice(&header(b"late", b'0', b"", 1, 0o644));
        raw.extend(data_blocks(b"x"));
        raw.extend_from_slice(&[0u8; 1024]);
        let (_p, dest, r) = extract(&gz(&raw), ExtractLimits::default());
        assert!(msg(r).contains("after the end-of-archive marker"));
        assert!(!dest.join("late").exists());
        // Endless zero padding is cut off.
        let mut raw = tar(&[file("f", "x")]);
        raw.resize(TAIL_LIMIT as usize + 4096, 0);
        assert!(extract(&gz(&raw), ExtractLimits::default()).2.is_err());
    }

    #[test]
    fn a_missing_or_wrong_destination_is_an_io_error() {
        let blob = archive(&[file("f", "x")]);
        let r =
            extract_tar_gz(&blob[..], Path::new("/nonexistent/ts-system/dest"), &ExtractLimits::default());
        assert!(matches!(r, Err(ExtractError::Io { .. })));
        assert!(std::error::Error::source(&r.unwrap_err()).is_some());
        let parent = TempDir::new();
        fs::write(parent.0.join("file"), "x").unwrap();
        let r = extract_tar_gz(&blob[..], &parent.0.join("file"), &ExtractLimits::default());
        assert!(matches!(r, Err(ExtractError::Io { .. })));
    }

    #[test]
    fn a_header_claiming_more_data_than_there_is_is_refused() {
        let mut raw = header(b"f", b'0', b"", 5000, 0o644).to_vec();
        raw.extend_from_slice(b"short");
        let (_p, _d, r) = extract(&gz(&raw), ExtractLimits::default());
        assert!(msg(r).contains("not a valid archive"));
    }

    /// A deterministic stand-in for a fuzzer: mutate a valid archive in thousands of ways and
    /// check the invariants that matter, whatever the verdict: no panic, nothing outside the
    /// destination, no special files, no mode wider than 0755.
    #[test]
    fn mutated_archives_never_escape_or_panic() {
        let mut long = Vec::new();
        let body = " path=pax/dir/file.txt\n";
        let record = format!("{}{body}", body.len() + 2);
        long.extend_from_slice(&header(b"PaxHeader", b'x', b"", record.len(), 0o644));
        long.extend(data_blocks(record.as_bytes()));
        long.extend_from_slice(&header(b"x", b'0', b"", 4, 0o644));
        long.extend(data_blocks(b"data"));
        let seeds = [
            tar(&[dir("r/"), file("r/f", "hello"), file("r/run.sh", "echo")]),
            tar(&[file("../evil", "x")]),
            tar(&[other("l", b'2', "/etc"), file("l/x", "y")]),
            tar(&[file("a/../../b", "x")]),
            long,
        ];
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let limits = ExtractLimits { max_files: 100, max_bytes: 1 << 20 };
        for seed in &seeds {
            for _ in 0..400 {
                let mut raw = seed.clone();
                for _ in 0..=(next() % 4) {
                    let i = (next() % raw.len() as u64) as usize;
                    match next() % 4 {
                        0 => raw[i] ^= 1 << (next() % 8),
                        1 => raw[i] = next() as u8,
                        2 => raw.truncate(i.max(1)),
                        _ => raw.insert(i, next() as u8),
                    }
                }
                // Mutations of the tar stream are wrapped in gzip so the compression checksum does
                // not reject them before the extractor sees them.
                let (parent, _d, _r) = extract(&gz(&raw), limits);
                only_plain_files(&parent.0);
            }
        }
        // Mutations of the compressed bytes themselves.
        let blob = gz(&seeds[0]);
        for _ in 0..400 {
            let mut b = blob.clone();
            let i = (next() % b.len() as u64) as usize;
            b[i] ^= 1 << (next() % 8);
            let (parent, _d, _r) = extract(&b, limits);
            only_plain_files(&parent.0);
        }
    }

    #[test]
    fn error_texts_and_sources() {
        assert_eq!(ExtractError::NotGzip.to_string(), "the download is not a gzip archive");
        assert_eq!(ExtractError::Invalid("x".into()).to_string(), "the download is not a valid archive: x");
        let e = ExtractError::Io { path: "/p".into(), source: io::Error::from(io::ErrorKind::NotFound) };
        assert!(e.to_string().starts_with("unpack /p: "));
        assert!(std::error::Error::source(&ExtractError::NotGzip).is_none());
        assert_eq!(ExtractLimits::default(), ExtractLimits { max_files: 20_000, max_bytes: 400 << 20 });
    }
}
