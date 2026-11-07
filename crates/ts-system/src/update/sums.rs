//! `SHA256SUMS`: the trust anchor of a download.
#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use super::UpdateError;

/// File name to lowercase hex SHA-256.
pub type Checksums = BTreeMap<String, String>;

/// Reads a `SHA256SUMS` file: the `sha256sum` output format, a 64-digit hash, blanks, an
/// optional `*` (binary mode) and the file name. Lines that are not in that format are ignored,
/// as `sha256sum -c --ignore-missing` would. Only the base name counts, so a list made from a
/// `dist/` directory still matches.
///
/// A file named twice with different hashes is an error: the list is the trust anchor of the
/// download, and an ambiguous anchor must not be settled by picking one.
pub fn parse_sums(data: &[u8]) -> Result<Checksums, UpdateError> {
    let mut sums = Checksums::new();
    for line in String::from_utf8_lossy(data).lines() {
        let Some((hash, name)) = split_line(line.trim()) else { continue };
        let name = name.rsplit('/').next().unwrap_or_default();
        if name.is_empty() || name == "." || name == ".." {
            continue;
        }
        if let Some(previous) = sums.insert(name.to_owned(), hash.clone()) {
            if previous != hash {
                return Err(UpdateError::Checksums(format!(
                    "SHA256SUMS lists {name} twice with different hashes"
                )));
            }
        }
    }
    Ok(sums)
}

/// `<64 hex digits><blanks>[*]<name without blanks>` into the lowercased hash and the name.
fn split_line(line: &str) -> Option<(String, &str)> {
    let hash = line.get(..64).filter(|h| h.bytes().all(|b| b.is_ascii_hexdigit()))?;
    let rest = &line[64..];
    let name = rest.trim_start();
    // At least one blank must separate the hash from the name.
    if name.len() == rest.len() {
        return None;
    }
    let name = name.strip_prefix('*').unwrap_or(name);
    (!name.is_empty() && !name.contains(char::is_whitespace)).then(|| (hash.to_ascii_lowercase(), name))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const A: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const B: &str = "FEDCBA9876543210FEDCBA9876543210FEDCBA9876543210FEDCBA9876543210";

    #[test]
    fn the_sha256sum_formats_are_read() {
        let text = format!("{A}  truesense-0.5.0.tar.gz\n{B} *dist/other.bin\r\n\n  {A}\ttab.txt  \n");
        let sums = parse_sums(text.as_bytes()).unwrap();
        assert_eq!(sums["truesense-0.5.0.tar.gz"], A);
        assert_eq!(sums["other.bin"], B.to_lowercase(), "the base name, the hash lowercased");
        assert_eq!(sums["tab.txt"], A);
        assert_eq!(sums.len(), 3);
    }

    #[test]
    fn lines_that_are_not_checksums_are_ignored() {
        let short = &A[..63];
        let text = format!(
            "# comment\n{short}  short.txt\n{A}nospace.txt\n{A}  \n{A}  two words.txt\n{}g  badhex.txt\n   \n\u{0}\u{ffff}\n{A}  ok.txt\n",
            &A[..63]
        );
        let sums = parse_sums(text.as_bytes()).unwrap();
        assert_eq!(sums.keys().collect::<Vec<_>>(), ["ok.txt"]);
        assert!(parse_sums(b"").unwrap().is_empty());
        assert!(parse_sums(&[0xff, 0xfe, 0x00]).unwrap().is_empty());
    }

    #[test]
    fn names_that_are_not_files_are_skipped() {
        let text = format!("{A}  dir/\n{A}  /\n{A}  .\n{A}  ..\n{A}  a/..\n{A}  a/b\n");
        let sums = parse_sums(text.as_bytes()).unwrap();
        assert_eq!(sums.keys().collect::<Vec<_>>(), ["b"]);
    }

    #[test]
    fn a_name_twice_is_fine_with_the_same_hash_and_an_error_with_another() {
        let same = format!("{A}  f\n{A}  f\n{}  f\n", A.to_uppercase());
        assert_eq!(parse_sums(same.as_bytes()).unwrap().len(), 1);
        let clash = format!("{A}  f\n{B}  f\n");
        let err = parse_sums(clash.as_bytes()).unwrap_err();
        assert_eq!(err.to_string(), "SHA256SUMS lists f twice with different hashes");
        let clash_by_dir = format!("{A}  x/f\n{B}  y/f\n");
        assert!(parse_sums(clash_by_dir.as_bytes()).is_err(), "base names collide");
    }

    #[test]
    fn multibyte_text_around_the_hash_never_panics() {
        for text in ["é".repeat(64), format!("{}é  x", &A[..63]), format!("{A}é x"), "ö".repeat(200)] {
            let _ = parse_sums(text.as_bytes());
        }
    }
}
