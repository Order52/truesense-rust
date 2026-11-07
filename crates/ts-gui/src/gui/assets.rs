//! The front end, embedded in the binary.
//!
//! The window is part of the executable: no file is read from disk at run time, so the page can
//! never be swapped by someone who can write to a directory, and a new binary always shows its
//! own page. The table is written out by hand (no build script, no extra crate) and a test
//! compares it with the directory, so a file added to `web/` and forgotten here fails the tests.
#![forbid(unsafe_code)]

use std::collections::HashMap;

/// Every served file: its path below `/` and its bytes.
pub(crate) const FILES: &[(&str, &[u8])] = &[
    ("app.css", include_bytes!("../../web/app.css")),
    ("index.html", include_bytes!("../../web/index.html")),
    ("js/actions.js", include_bytes!("../../web/js/actions.js")),
    ("js/api.js", include_bytes!("../../web/js/api.js")),
    ("js/app.js", include_bytes!("../../web/js/app.js")),
    ("js/board.js", include_bytes!("../../web/js/board.js")),
    ("js/color.js", include_bytes!("../../web/js/color.js")),
    ("js/curve.js", include_bytes!("../../web/js/curve.js")),
    ("js/format.js", include_bytes!("../../web/js/format.js")),
    ("js/icons.js", include_bytes!("../../web/js/icons.js")),
    ("js/kbd-layout.js", include_bytes!("../../web/js/kbd-layout.js")),
    ("js/latest.js", include_bytes!("../../web/js/latest.js")),
    ("js/pages/audio.js", include_bytes!("../../web/js/pages/audio.js")),
    ("js/pages/battery.js", include_bytes!("../../web/js/pages/battery.js")),
    ("js/pages/curves.js", include_bytes!("../../web/js/pages/curves.js")),
    ("js/pages/health.js", include_bytes!("../../web/js/pages/health.js")),
    ("js/pages/home.js", include_bytes!("../../web/js/pages/home.js")),
    ("js/pages/index.js", include_bytes!("../../web/js/pages/index.js")),
    ("js/pages/lighting.js", include_bytes!("../../web/js/pages/lighting.js")),
    ("js/pages/looks-tab.js", include_bytes!("../../web/js/pages/looks-tab.js")),
    ("js/pages/perkey.js", include_bytes!("../../web/js/pages/perkey.js")),
    ("js/pages/predator.js", include_bytes!("../../web/js/pages/predator.js")),
    ("js/pages/profiles.js", include_bytes!("../../web/js/pages/profiles.js")),
    ("js/pages/settings.js", include_bytes!("../../web/js/pages/settings.js")),
    ("js/pages/smart.js", include_bytes!("../../web/js/pages/smart.js")),
    ("js/pages/undervolt.js", include_bytes!("../../web/js/pages/undervolt.js")),
    ("js/risk.js", include_bytes!("../../web/js/risk.js")),
    ("js/spark.js", include_bytes!("../../web/js/spark.js")),
    ("js/sse.js", include_bytes!("../../web/js/sse.js")),
    ("js/state.js", include_bytes!("../../web/js/state.js")),
    ("js/store.js", include_bytes!("../../web/js/store.js")),
    ("js/ui.js", include_bytes!("../../web/js/ui.js")),
    ("logo.svg", include_bytes!("../../web/logo.svg")),
];

/// One served file, prepared once.
#[derive(Debug, Clone)]
pub(crate) struct Asset {
    /// The file.
    pub body: &'static [u8],
    /// Its media type.
    pub content_type: &'static str,
    /// A validator for `If-None-Match`: the page is revalidated on every load, so a new binary
    /// shows its new page without anybody clearing a cache.
    pub etag: String,
}

/// The media type of a file name; only these are ever served.
pub(crate) fn content_type(path: &str) -> Option<&'static str> {
    let ext = path.rsplit_once('.')?.1;
    Some(match ext {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "svg" => "image/svg+xml",
        "json" => "application/json; charset=utf-8",
        "png" => "image/png",
        "txt" => "text/plain; charset=utf-8",
        _ => return None,
    })
}

/// FNV-1a over the bytes: an ETag only has to change when the content does.
fn fingerprint(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
}

/// The embedded files by path.
#[derive(Debug)]
pub(crate) struct Assets {
    files: HashMap<String, Asset>,
}

impl Assets {
    /// Prepares the embedded table.
    pub(crate) fn embedded() -> Self {
        Self::from_table(FILES)
    }

    /// Prepares any table (tests use their own).
    pub(crate) fn from_table(table: &'static [(&'static str, &'static [u8])]) -> Self {
        let files = table
            .iter()
            .filter_map(|(path, body)| {
                let content_type = content_type(path)?;
                let etag = format!("\"{:016x}\"", fingerprint(body));
                Some((format!("/{path}"), Asset { body, content_type, etag }))
            })
            .collect();
        Self { files }
    }

    /// The file a request path names (`/` is the page); `None` for anything else, including every
    /// path that tries to leave the directory.
    pub(crate) fn find(&self, path: &str) -> Option<&Asset> {
        if path == "/" {
            return self.files.get("/index.html");
        }
        // Only the exact keys are served, so `..` and `//` can never reach another file.
        self.files.get(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static TABLE: &[(&str, &[u8])] = &[
        ("index.html", b"<p>home</p>"),
        ("js/app.js", b"export {};"),
        ("app.css", b"a{}"),
        ("logo.svg", b"<svg/>"),
        ("notes", b"no extension"),
        ("tool.exe", b"MZ"),
    ];

    #[test]
    fn only_files_of_a_known_type_are_served_with_that_type() {
        let a = Assets::from_table(TABLE);
        for (path, ty) in [
            ("/index.html", "text/html; charset=utf-8"),
            ("/js/app.js", "text/javascript; charset=utf-8"),
            ("/app.css", "text/css; charset=utf-8"),
            ("/logo.svg", "image/svg+xml"),
        ] {
            assert_eq!(a.find(path).map(|f| f.content_type), Some(ty), "{path}");
        }
        assert!(
            a.find("/notes").is_none() && a.find("/tool.exe").is_none(),
            "an unknown type is never served"
        );
    }

    #[test]
    fn the_root_is_the_page_and_nothing_can_leave_the_table() {
        let a = Assets::from_table(TABLE);
        assert_eq!(a.find("/").map(|f| f.body), Some(&b"<p>home</p>"[..]));
        for path in [
            "",
            "index.html",
            "/js",
            "/js/",
            "//index.html",
            "/../index.html",
            "/js/../index.html",
            "/%2e%2e/index.html",
            "/INDEX.HTML",
        ] {
            assert!(a.find(path).is_none(), "{path:?}");
        }
    }

    #[test]
    fn the_validator_follows_the_content() {
        let a = Assets::from_table(TABLE);
        let b = Assets::from_table(&[("index.html", b"<p>other</p>")]);
        let (one, same, other) = (a.find("/index.html"), Assets::from_table(TABLE), b.find("/index.html"));
        let etag = one.map(|f| f.etag.clone()).unwrap_or_default();
        assert!(etag.starts_with('"') && etag.ends_with('"') && etag.len() == 18, "{etag}");
        assert_eq!(same.find("/index.html").map(|f| f.etag.clone()), Some(etag.clone()));
        assert_ne!(other.map(|f| f.etag.clone()), Some(etag));
    }
}
