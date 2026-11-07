//! The translation catalogues and the QML agree: every `qsTrId` id in the QML (and every title in
//! the page registry) has an English entry with text, and no catalogue keeps an id the QML no longer
//! uses. Pure file scanning, no Qt.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn qml_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap().flatten() {
        let p = entry.path();
        if p.is_dir() {
            qml_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "qml") {
            out.push(p);
        }
    }
}

/// The ids in `qsTrId("...")` calls that are real code, not documentation comments.
fn qml_ids() -> BTreeSet<String> {
    let mut files = Vec::new();
    qml_files(&root().join("qml"), &mut files);
    let mut ids = BTreeSet::new();
    for f in files {
        for line in fs::read_to_string(&f).unwrap().lines() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            let mut rest = line;
            while let Some(i) = rest.find("qsTrId(\"") {
                let after = &rest[i + 8..];
                let end = after.find('"').unwrap();
                ids.insert(after[..end].to_owned());
                rest = &after[end..];
            }
        }
    }
    ids
}

fn registry_ids() -> BTreeSet<String> {
    let v: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root().join("qml/pages/registry.json")).unwrap()).unwrap();
    v["groups"]
        .as_array()
        .unwrap()
        .iter()
        .chain(v["pages"].as_array().unwrap())
        .map(|e| e["titleId"].as_str().unwrap().to_owned())
        .collect()
}

/// `(id, source, translation text, unfinished)` of every message of a `.ts` file.
fn messages(name: &str) -> Vec<(String, String, String, bool)> {
    let text = fs::read_to_string(root().join("i18n").join(name)).unwrap();
    let mut out = Vec::new();
    for chunk in text.split("<message id=\"").skip(1) {
        let id = chunk[..chunk.find('"').unwrap()].to_owned();
        let between = |open: &str, close: &str| {
            chunk.find(open).and_then(|s| {
                let start = chunk[s..].find('>').map(|g| s + g + 1)?;
                chunk[start..].find(close).map(|e| chunk[start..start + e].to_owned())
            })
        };
        let source = between("<source>", "</source>").unwrap_or_default();
        let translation = between("<translation", "</translation>").unwrap_or_default();
        out.push((id, source, translation, chunk.contains("type=\"unfinished\"")));
    }
    out
}

#[test]
fn every_id_used_in_qml_or_the_registry_has_english_text() {
    let en = messages("en.ts");
    let have: BTreeSet<&str> = en.iter().map(|m| m.0.as_str()).collect();
    for id in qml_ids().into_iter().chain(registry_ids()) {
        assert!(
            have.contains(id.as_str()),
            "qsTrId(\"{id}\") has no entry in i18n/en.ts: run crates/ts-qt/tools/i18n-update.sh"
        );
    }
    for (id, source, translation, unfinished) in &en {
        assert!(
            !source.is_empty(),
            "{id}: empty English source (is the //% comment on the line before the qsTrId call?)"
        );
        assert_eq!(source, translation, "{id}: English translation must equal its source");
        assert!(!unfinished, "{id}");
    }
}

/// Every `i18n/*.ts` file name; English is the only one today, and the checks below apply to any
/// language added later.
fn catalogues() -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(root().join("i18n"))
        .unwrap()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.ends_with(".ts"))
        .collect();
    names.sort();
    names
}

#[test]
fn english_is_present_and_no_catalogue_keeps_an_id_the_qml_no_longer_uses() {
    let files = catalogues();
    assert!(files.iter().any(|f| f == "en.ts"), "i18n/en.ts is the source of every language: {files:?}");
    let used: BTreeSet<String> = qml_ids().into_iter().chain(registry_ids()).collect();
    for file in &files {
        for (id, ..) in messages(file) {
            assert!(
                used.contains(&id),
                "{file} keeps the unused id {id}: run crates/ts-qt/tools/i18n-update.sh"
            );
        }
    }
}

#[test]
fn placeholders_survive_in_every_translation() {
    let en = messages("en.ts");
    for file in catalogues().iter().filter(|f| *f != "en.ts") {
        let other = messages(file);
        for (id, source, ..) in &en {
            let Some((_, _, tr, unfinished)) = other.iter().find(|m| &m.0 == id) else { continue };
            if *unfinished || tr.is_empty() {
                continue;
            }
            for n in 1..=3 {
                let p = format!("%{n}");
                assert_eq!(
                    source.contains(&p),
                    tr.contains(&p),
                    "{file} {id}: placeholder {p} must be kept in the translation"
                );
            }
        }
    }
}

#[test]
fn arabic_is_not_shipped() {
    assert!(
        !catalogues().iter().any(|f| f == "ar.ts"),
        "the Arabic catalogue was dropped on purpose; a language is added with its own native review"
    );
}
