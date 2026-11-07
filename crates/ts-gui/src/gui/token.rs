//! Secrets of the window: random tokens and the comparison that checks them.
//!
//! Whoever holds the session token may drive the laptop's firmware, so it is 256 random bits, it
//! never appears in a URL, and it is compared without leaking where the first difference is.
//! No crate is needed: the operating system's random source is read through an injected function
//! (a test passes a counter), and the encoding is the 40 lines of base64url below.
#![forbid(unsafe_code)]

use std::fs::File;
use std::io::{self, Read};
use std::sync::Arc;

use super::GuiError;

/// Fills a buffer with random bytes. Injected so a test is deterministic and so nothing in this
/// module needs the real random device.
pub type Entropy = Arc<dyn Fn(&mut [u8]) -> io::Result<()> + Send + Sync>;

/// The kernel's random source (`/dev/urandom`, read-only: it never blocks once the pool is seeded).
pub fn system_entropy() -> Entropy {
    Arc::new(|buf: &mut [u8]| File::open("/dev/urandom")?.read_exact(buf))
}

/// Random bytes in a token: 256 bits.
const TOKEN_BYTES: usize = 32;

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// URL-safe base64 without padding, so a token can sit in a URL fragment or a header unescaped.
pub(crate) fn base64url(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
        let symbols = chunk.len() + 1;
        for i in 0..symbols {
            out.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 0x3f) as usize]));
        }
    }
    out
}

/// A fresh token: 256 random bits, URL-safe.
pub(crate) fn new_token(entropy: &Entropy) -> Result<String, GuiError> {
    let mut raw = [0u8; TOKEN_BYTES];
    entropy(&mut raw).map_err(|e| GuiError::Entropy(e.to_string()))?;
    Ok(base64url(&raw))
}

/// Compares two secrets in time that depends only on their lengths: every byte is looked at, so
/// how many leading bytes matched cannot be measured from outside. (The length of a token is not a
/// secret: they all have the same one.)
pub(crate) fn same_secret(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        diff |= usize::from(a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0));
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64url_matches_the_rfc_vectors_without_padding() {
        for (raw, text) in [
            (&b""[..], ""),
            (b"f", "Zg"),
            (b"fo", "Zm8"),
            (b"foo", "Zm9v"),
            (b"foob", "Zm9vYg"),
            (b"fooba", "Zm9vYmE"),
            (b"foobar", "Zm9vYmFy"),
            (&[0xfb, 0xff, 0xfe], "-__-"),
        ] {
            assert_eq!(base64url(raw), text, "{raw:?}");
        }
    }

    #[test]
    fn a_token_is_43_url_safe_characters_from_32_random_bytes() {
        let counter = std::sync::atomic::AtomicU8::new(0);
        let entropy: Entropy = Arc::new(move |buf: &mut [u8]| {
            for b in buf {
                *b = counter.fetch_add(7, std::sync::atomic::Ordering::SeqCst);
            }
            Ok(())
        });
        let token = new_token(&entropy).unwrap_or_default();
        assert_eq!(token.len(), 43);
        assert!(token.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
        assert_ne!(token, new_token(&entropy).unwrap_or_default(), "two tokens must differ");
    }

    #[test]
    fn the_system_source_gives_different_tokens_each_time() {
        let entropy = system_entropy();
        let (a, b) = (new_token(&entropy), new_token(&entropy));
        assert!(matches!((&a, &b), (Ok(a), Ok(b)) if a != b && a.len() == 43));
    }

    #[test]
    fn a_failing_source_is_an_error_not_a_weak_token() {
        let entropy: Entropy = Arc::new(|_| Err(io::Error::other("no entropy")));
        assert!(matches!(new_token(&entropy), Err(GuiError::Entropy(m)) if m.contains("no entropy")));
    }

    #[test]
    fn secrets_compare_equal_only_when_identical() {
        assert!(same_secret("abc", "abc"));
        assert!(same_secret("", ""));
        for (a, b) in [("abc", "abd"), ("abc", "ab"), ("ab", "abc"), ("", "a"), ("abc", "ABC")] {
            assert!(!same_secret(a, b), "{a} {b}");
        }
    }
}
