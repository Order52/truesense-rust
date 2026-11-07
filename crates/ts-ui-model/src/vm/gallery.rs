//! What the theme gallery decides without Qt: which cards a search and a filter chip leave, the
//! chips with their counts, and the cards' order.
//!
//! The Themes tab (and the Templates section of the Per-key tab) show cards in a gallery. A card is
//! `{id, name, mood, builtin, top, suits}` (what `theme list` gives; a template card has the same
//! members, with its description as `mood`). The tags a chip filters on come from the `suits` the
//! catalogue gives each theme (power modes and activities), grouped into a few words people look
//! for; `top` is the catalogue's own list of twelve favourites. Everything is a pure function of its
//! arguments.
#![forbid(unsafe_code)]

use serde_json::{Value, json};

use crate::json::{bool_at, i64_at, list_at, str_at};

/// The chips in display order. `all` leaves everything; `top` the twelve the catalogue ranks;
/// `starred` the favourites; `yours` the user's own; the rest group `suits`.
pub const CHIPS: [&str; 9] = ["all", "top", "starred", "gaming", "work", "night", "music", "calm", "yours"];

/// The suits (lower case) that belong to a tag chip.
fn tag_suits(chip: &str) -> &'static [&'static str] {
    match chip {
        "gaming" => &["gaming", "driving games", "strategy and sim games", "retro games", "horror games"],
        "work" => &["typing", "coding", "terminal work", "work", "focus", "reading"],
        "night" => &["night", "late night", "evenings", "winding down", "chill"],
        "music" => &["music", "parties"],
        "calm" => &["quiet", "eco", "on battery", "winding down", "chill"],
        _ => &[],
    }
}

fn suits_of(card: &Value) -> Vec<String> {
    list_at(card, "suits")
        .map(|l| l.iter().filter_map(Value::as_str).map(str::to_lowercase).collect())
        .unwrap_or_default()
}

fn starred(args: &Value, id: &str) -> bool {
    list_at(args, "starred").is_some_and(|l| l.iter().filter_map(Value::as_str).any(|s| s == id))
}

/// Whether a card belongs under a chip.
fn in_chip(chip: &str, card: &Value, args: &Value) -> bool {
    let id = str_at(card, "id").unwrap_or("");
    match chip {
        "all" => true,
        "top" => i64_at(card, "top").unwrap_or(0) > 0,
        "starred" => starred(args, id),
        "yours" => !bool_at(card, "builtin").unwrap_or(true),
        tag => {
            let wanted = tag_suits(tag);
            !wanted.is_empty() && suits_of(card).iter().any(|s| wanted.contains(&s.as_str()))
        }
    }
}

/// Whether every word of the search is in the card's id, name, mood or suits.
fn matches(query: &str, card: &Value) -> bool {
    let hay = format!(
        "{} {} {} {}",
        str_at(card, "id").unwrap_or(""),
        str_at(card, "name").unwrap_or(""),
        str_at(card, "mood").unwrap_or(""),
        suits_of(card).join(" ")
    )
    .to_lowercase();
    query.to_lowercase().split_whitespace().all(|word| hay.contains(word))
}

/// The cards that pass the search and the chip, as ids in order. The running one comes first when
/// `pin_active` is set, so it is always in sight.
fn visible(args: &Value) -> Vec<String> {
    let chip = str_at(args, "chip").unwrap_or("all");
    let query = str_at(args, "query").unwrap_or("");
    let cards = list_at(args, "themes").unwrap_or_default();
    let mut ids: Vec<String> = cards
        .iter()
        .filter(|c| matches(query, c) && in_chip(chip, c, args))
        .filter_map(|c| str_at(c, "id").map(str::to_owned))
        .collect();
    if let Some(active) = str_at(args, "active").filter(|_| bool_at(args, "pin_active").unwrap_or(false))
        && let Some(at) = ids.iter().position(|i| i == active)
    {
        let id = ids.remove(at);
        ids.insert(0, id);
    }
    ids
}

/// Each chip with how many cards it would show for the current search.
fn chips(args: &Value) -> Value {
    let query = str_at(args, "query").unwrap_or("");
    let cards = list_at(args, "themes").unwrap_or_default();
    json!(CHIPS
        .iter()
        .map(|chip| {
            let n = cards.iter().filter(|c| matches(query, c) && in_chip(chip, c, args)).count();
            json!({"id": chip, "count": n})
        })
        .collect::<Vec<_>>())
}

/// Calls a verb: `filter` (ids to show), `chips` (the chips with counts).
pub fn call(verb: &str, args: &Value) -> Result<Value, String> {
    match verb {
        "filter" => Ok(json!(visible(args))),
        "chips" => Ok(chips(args)),
        other => Err(format!("unknown gallery verb: {other}")),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn card(id: &str, name: &str, top: i64, builtin: bool, suits: &[&str]) -> Value {
        json!({"id": id, "name": name, "mood": format!("{name} mood"), "top": top, "builtin": builtin, "suits": suits})
    }

    fn sample() -> Value {
        json!({"themes": [
            card("predator", "Predator", 1, true, &["Balanced", "Performance", "default theme"]),
            card("hearth", "Hearth", 2, true, &["Balanced", "Quiet", "evenings"]),
            card("grid", "Grid", 0, true, &["gaming", "Performance"]),
            card("matrix", "Matrix", 8, true, &["Balanced", "coding", "terminal work"]),
            card("stage", "Stage", 5, true, &["music", "parties", "any mode"]),
            card("mine", "Mine", 0, false, &[]),
        ], "starred": ["grid", "mine"], "active": "matrix"})
    }

    fn ids(v: Value) -> Vec<String> {
        v.as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_owned()).collect()
    }

    fn with(mut args: Value, key: &str, value: Value) -> Value {
        args[key] = value;
        args
    }

    #[test]
    fn without_a_search_or_chip_every_card_shows_in_the_catalogues_order() {
        assert_eq!(ids(call("filter", &sample()).unwrap()), ["predator", "hearth", "grid", "matrix", "stage", "mine"]);
        assert_eq!(ids(call("filter", &json!({})).unwrap()), Vec::<String>::new());
    }

    #[test]
    fn each_chip_keeps_what_it_says() {
        let s = sample();
        for (chip, want) in [
            ("top", vec!["predator", "hearth", "matrix", "stage"]),
            ("starred", vec!["grid", "mine"]),
            ("yours", vec!["mine"]),
            ("gaming", vec!["grid"]),
            ("work", vec!["matrix"]),
            ("night", vec!["hearth"]),
            ("music", vec!["stage"]),
            ("calm", vec!["hearth"]),
            ("all", vec!["predator", "hearth", "grid", "matrix", "stage", "mine"]),
        ] {
            assert_eq!(ids(call("filter", &with(s.clone(), "chip", json!(chip))).unwrap()), want, "{chip}");
        }
        assert_eq!(ids(call("filter", &with(s, "chip", json!("nonsense"))).unwrap()), Vec::<String>::new());
    }

    #[test]
    fn a_search_needs_every_word_in_the_name_mood_id_or_suits_in_any_case() {
        let s = sample();
        let q = |text: &str| ids(call("filter", &with(s.clone(), "query", json!(text))).unwrap());
        assert_eq!(q("HEARTH"), ["hearth"]);
        assert_eq!(q("performance"), ["predator", "grid"]);
        assert_eq!(q("balanced coding"), ["matrix"]);
        assert_eq!(q("hearth mood"), ["hearth"]);
        assert_eq!(q("  "), ["predator", "hearth", "grid", "matrix", "stage", "mine"]);
        assert_eq!(q("zzz"), Vec::<String>::new());
        let both = with(with(s, "query", json!("performance")), "chip", json!("gaming"));
        assert_eq!(ids(call("filter", &both).unwrap()), ["grid"], "search and chip both apply");
    }

    #[test]
    fn the_running_card_can_be_pinned_to_the_front_when_it_is_shown() {
        let s = with(sample(), "pin_active", json!(true));
        assert_eq!(ids(call("filter", &s).unwrap())[0], "matrix");
        let hidden = with(s, "chip", json!("music"));
        assert_eq!(ids(call("filter", &hidden).unwrap()), ["stage"], "never added to a list it does not belong in");
    }

    #[test]
    fn the_chips_carry_counts_for_the_current_search() {
        let counts = |args: &Value| -> Vec<(String, i64)> {
            call("chips", args).unwrap().as_array().unwrap().iter()
                .map(|c| (c["id"].as_str().unwrap().to_owned(), c["count"].as_i64().unwrap()))
                .collect()
        };
        let all = counts(&sample());
        assert_eq!(all.iter().map(|(c, _)| c.as_str()).collect::<Vec<_>>(), CHIPS);
        assert_eq!(all[0], ("all".to_owned(), 6));
        assert_eq!(all[1], ("top".to_owned(), 4));
        let narrowed = counts(&with(sample(), "query", json!("performance")));
        assert_eq!(narrowed[0].1, 2);
        assert_eq!(narrowed[1].1, 1, "predator is the only top theme that mentions performance");
    }

    #[test]
    fn junk_arguments_never_panic() {
        for args in [json!(null), json!([]), json!({"themes": "x"}), json!({"themes": [null, 3, {"suits": 4}], "chip": 7, "query": []})] {
            assert!(call("filter", &args).is_ok());
            assert!(call("chips", &args).is_ok());
        }
        assert!(call("nope", &json!({})).is_err());
    }
}
