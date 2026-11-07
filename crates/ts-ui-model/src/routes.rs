//! The page registry: which pages the app has, how they are grouped in the sidebar, and what a
//! name given on the command line (`truesense-qt --page curves`) means.
//!
//! The data is one JSON document, `crates/ts-qt/qml/pages/registry.json`, so that adding or
//! renaming a page is a one-line edit of one file. The Qt app embeds the same text and hands it to
//! QML; this module parses and validates it (so a typo is a failing unit test, not a blank sidebar)
//! and resolves names, including the older names of the web window (`curves`, `predator`).
#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::fmt;

use serde::Deserialize;
use serde_json::{Value, json};

/// A sidebar heading.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Group {
    /// Stable id (`now`, `control`, ...).
    pub id: String,
    /// Translation id of the heading (`qsTrId`).
    #[serde(rename = "titleId")]
    pub title_id: String,
    /// The English source text, also written to `en.ts`.
    pub title: String,
}

/// One page of the app.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Page {
    /// Stable id, also the name used on the command line (`home`, `cooling`).
    pub id: String,
    /// The group it sits under in the sidebar.
    pub group: String,
    /// Name of an icon in `qml/icons`.
    pub icon: String,
    /// Translation id of the title (`qsTrId`).
    #[serde(rename = "titleId")]
    pub title_id: String,
    /// The English source text, also written to `en.ts`.
    pub title: String,
    /// The QML component of the page in `qml/pages` (`HomePage` is `HomePage.qml`).
    pub qml: String,
    /// Other names that lead here (older web-window page names, plain words).
    #[serde(default)]
    pub aliases: Vec<String>,
}

#[derive(Deserialize)]
struct Document {
    groups: Vec<Group>,
    pages: Vec<Page>,
    #[serde(rename = "notFound")]
    not_found: NotFound,
}

/// The page shown for a name that matches nothing.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct NotFound {
    /// Its QML component.
    pub qml: String,
}

/// Why the registry document is unusable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteError {
    /// Not valid JSON or not the expected shape.
    Parse(String),
    /// A rule of the registry is broken; the text names the page and the rule.
    Invalid(String),
}

impl fmt::Display for RouteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(e) => write!(f, "page registry: {e}"),
            Self::Invalid(e) => write!(f, "page registry: {e}"),
        }
    }
}

impl std::error::Error for RouteError {}

/// What a name resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolved<'a> {
    /// A page of the registry.
    Found(&'a Page),
    /// Nothing matches; show the not-found page.
    NotFound,
}

/// The validated registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Routes {
    groups: Vec<Group>,
    pages: Vec<Page>,
    not_found: NotFound,
}

/// The name the way it is compared: trimmed, lower case, no leading `#` or `/`, `_` and spaces as `-`.
pub fn normalise(name: &str) -> String {
    name.trim().trim_start_matches(['#', '/']).to_lowercase().replace(['_', ' '], "-")
}

fn is_component_name(s: &str) -> bool {
    s.ends_with("Page")
        && s.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && s.chars().all(|c| c.is_ascii_alphanumeric())
}

impl Routes {
    /// Parses and validates the registry document.
    ///
    /// # Errors
    /// [`RouteError`] for bad JSON, an unknown group, a duplicate id or alias, an empty field or a
    /// component name that is not `<Name>Page`.
    pub fn parse(text: &str) -> Result<Self, RouteError> {
        let doc: Document = serde_json::from_str(text).map_err(|e| RouteError::Parse(e.to_string()))?;
        let bad = |m: String| Err(RouteError::Invalid(m));
        let group_ids: BTreeSet<&str> = doc.groups.iter().map(|g| g.id.as_str()).collect();
        if group_ids.len() != doc.groups.len() {
            return bad("two groups have the same id".to_owned());
        }
        let mut names = BTreeSet::new();
        for page in &doc.pages {
            if page.id.is_empty() || page.icon.is_empty() || page.title_id.is_empty() || page.title.is_empty()
            {
                return bad(format!("page '{}' has an empty id, icon or title", page.id));
            }
            if !group_ids.contains(page.group.as_str()) {
                return bad(format!("page '{}' is in the unknown group '{}'", page.id, page.group));
            }
            if !is_component_name(&page.qml) {
                return bad(format!("page '{}': '{}' is not a <Name>Page component", page.id, page.qml));
            }
            for name in std::iter::once(&page.id).chain(&page.aliases) {
                if normalise(name) != *name || !names.insert(name.clone()) {
                    return bad(format!(
                        "page '{}': name '{name}' is duplicated or not in normal form",
                        page.id
                    ));
                }
            }
        }
        if !is_component_name(&doc.not_found.qml) {
            return bad(format!("not-found page '{}' is not a <Name>Page component", doc.not_found.qml));
        }
        Ok(Self { groups: doc.groups, pages: doc.pages, not_found: doc.not_found })
    }

    /// Every page in sidebar order.
    pub fn pages(&self) -> &[Page] {
        &self.pages
    }

    /// The page for `name` (an id or an alias, in any case), or [`Resolved::NotFound`].
    pub fn resolve(&self, name: &str) -> Resolved<'_> {
        let wanted = normalise(name);
        self.pages
            .iter()
            .find(|p| p.id == wanted || p.aliases.contains(&wanted))
            .map_or(Resolved::NotFound, Resolved::Found)
    }

    /// The id to navigate to for `name`, or `None` when it matches nothing.
    pub fn resolve_id(&self, name: &str) -> Option<&str> {
        match self.resolve(name) {
            Resolved::Found(p) => Some(&p.id),
            Resolved::NotFound => None,
        }
    }

    /// The first page: where the app starts.
    pub fn first(&self) -> Option<&Page> {
        self.pages.first()
    }

    /// The pages under each heading, headings in order, empty headings left out.
    pub fn grouped(&self) -> Vec<(&Group, Vec<&Page>)> {
        self.groups
            .iter()
            .map(|g| (g, self.pages.iter().filter(|p| p.group == g.id).collect::<Vec<_>>()))
            .filter(|(_, pages)| !pages.is_empty())
            .collect()
    }

    /// The JSON QML reads: `{"groups": [...], "pages": [{..., "shortcut": 1}], "notFound": {...}}`.
    /// `shortcut` is the digit of Ctrl+digit for the first nine pages, `0` for the rest.
    pub fn to_json(&self) -> Value {
        let pages: Vec<Value> = self
            .pages
            .iter()
            .enumerate()
            .map(|(i, p)| {
                json!({
                    "id": p.id, "group": p.group, "icon": p.icon, "titleId": p.title_id,
                    "title": p.title, "qml": p.qml, "aliases": p.aliases,
                    "shortcut": if i < 9 { i + 1 } else { 0 },
                })
            })
            .collect();
        let groups: Vec<Value> = self
            .groups
            .iter()
            .map(|g| json!({"id": g.id, "titleId": g.title_id, "title": g.title}))
            .collect();
        json!({ "groups": groups, "pages": pages, "notFound": { "qml": self.not_found.qml } })
    }

    /// The QML component of the not-found page.
    pub fn not_found_qml(&self) -> &str {
        &self.not_found.qml
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// The real registry the app ships; validating it here makes a typo a failing test.
    const REAL: &str = include_str!("../../ts-qt/qml/pages/registry.json");

    fn real() -> Routes {
        Routes::parse(REAL).expect("the shipped registry is valid")
    }

    #[test]
    fn the_shipped_registry_has_the_twelve_entries_in_five_groups() {
        let r = real();
        let ids: Vec<&str> = r.pages().iter().map(|p| p.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "home",
                "monitor",
                "performance",
                "cooling",
                "lighting",
                "keys",
                "battery",
                "audio",
                "profiles",
                "health",
                "settings"
            ]
        );
        let grouped: Vec<(&str, usize)> = r.grouped().iter().map(|(g, p)| (g.id.as_str(), p.len())).collect();
        assert_eq!(grouped, [("now", 2), ("control", 4), ("hardware", 2), ("library", 1), ("system", 2)]);
        assert_eq!(r.first().map(|p| p.id.as_str()), Some("home"));
        assert_eq!(r.not_found_qml(), "NotFoundPage");
    }

    #[test]
    fn every_shipped_page_has_a_qml_file_next_to_the_registry() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../ts-qt/qml/pages");
        let r = real();
        for p in r.pages() {
            assert!(dir.join(format!("{}.qml", p.qml)).is_file(), "missing {}.qml", p.qml);
        }
        assert!(dir.join(format!("{}.qml", r.not_found_qml())).is_file());
    }

    #[test]
    fn names_resolve_case_insensitively_and_through_aliases() {
        let r = real();
        assert_eq!(r.resolve_id("Home"), Some("home"));
        assert_eq!(r.resolve_id("  #/cooling "), Some("cooling"));
        assert_eq!(r.resolve_id("curves"), Some("cooling"));
        assert_eq!(r.resolve_id("fan_curves"), Some("cooling"));
        assert_eq!(r.resolve_id("PREDATOR"), Some("keys"));
        assert_eq!(r.resolve_id("undervolt"), Some("performance"));
        assert_eq!(r.resolve_id("perkey"), Some("lighting"));
        assert_eq!(r.resolve_id("nonsense"), None);
        assert_eq!(r.resolve_id(""), None);
        assert_eq!(r.resolve("x"), Resolved::NotFound);
    }

    #[test]
    fn json_for_qml_numbers_the_first_nine_shortcuts() {
        let j = real().to_json();
        let pages = j["pages"].as_array().unwrap();
        assert_eq!(pages[0]["shortcut"], json!(1));
        assert_eq!(pages[8]["shortcut"], json!(9));
        assert_eq!(pages[9]["shortcut"], json!(0));
        assert_eq!(j["notFound"]["qml"], json!("NotFoundPage"));
    }

    fn doc(pages: &str) -> String {
        format!(
            r#"{{"groups":[{{"id":"g","titleId":"t","title":"G"}}],"pages":[{pages}],"notFound":{{"qml":"NotFoundPage"}}}}"#
        )
    }

    fn page(id: &str, extra: &str) -> String {
        format!(
            r#"{{"id":"{id}","group":"g","icon":"i","titleId":"t_{id}","title":"T","qml":"{}Page"{extra}}}"#,
            id.to_uppercase()
        )
    }

    #[test]
    fn validation_rejects_broken_registries() {
        assert!(matches!(Routes::parse("{"), Err(RouteError::Parse(_))));
        assert!(Routes::parse(&doc(&page("a", ""))).is_ok());
        let dup = format!("{},{}", page("a", ""), page("a", ""));
        assert!(matches!(Routes::parse(&doc(&dup)), Err(RouteError::Invalid(_))));
        let alias_clash = format!("{},{}", page("a", ""), page("b", r#","aliases":["a"]"#));
        assert!(matches!(Routes::parse(&doc(&alias_clash)), Err(RouteError::Invalid(_))));
        let unnormal = page("a", r#","aliases":["Not Normal"]"#);
        assert!(matches!(Routes::parse(&doc(&unnormal)), Err(RouteError::Invalid(_))));
        let bad_group = page("a", "").replace(r#""group":"g""#, r#""group":"zzz""#);
        assert!(matches!(Routes::parse(&doc(&bad_group)), Err(RouteError::Invalid(_))));
        let bad_qml = page("a", "").replace("APage", "a_page");
        assert!(matches!(Routes::parse(&doc(&bad_qml)), Err(RouteError::Invalid(_))));
        let empty_title = page("a", "").replace(r#""title":"T""#, r#""title":"""#);
        assert!(matches!(Routes::parse(&doc(&empty_title)), Err(RouteError::Invalid(_))));
    }

    #[test]
    fn empty_groups_are_not_listed() {
        let text = r#"{"groups":[{"id":"g","titleId":"t","title":"G"},{"id":"e","titleId":"t2","title":"E"}],
            "pages":[],"notFound":{"qml":"NotFoundPage"}}"#;
        assert!(Routes::parse(text).unwrap().grouped().is_empty());
    }
}
