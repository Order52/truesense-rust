//! The arguments of a command: a JSON object whose members are named like the command line's long
//! options (`--long-ms` is `long_ms`). A command ignores members it does not know and fails with a
//! message a person can read for a missing or wrongly typed one. `null` counts as absent, because
//! the page sends `null` for a field the user left empty.
#![forbid(unsafe_code)]

use std::fmt::Display;
use std::str::FromStr;

use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use super::{CommandError, Outcome};

/// A command's arguments.
pub(crate) struct Args {
    members: Map<String, Value>,
}

/// Largest whole number a JSON double carries exactly; larger values are not "whole numbers" here.
const MAX_EXACT: f64 = 9_007_199_254_740_992.0;

impl Args {
    /// The arguments of a request: an object, or nothing at all (`null`).
    pub(crate) fn new(raw: &Value) -> Result<Self, CommandError> {
        match raw {
            Value::Object(m) => Ok(Self { members: m.clone() }),
            Value::Null => Ok(Self { members: Map::new() }),
            _ => Err(CommandError::failed("bad arguments: expected a JSON object")),
        }
    }

    /// The member, unless absent or `null`.
    fn get(&self, key: &str) -> Option<&Value> {
        self.members.get(key).filter(|v| !v.is_null())
    }

    fn wrong(key: &str) -> CommandError {
        CommandError::failed(format!("{key}: wrong kind of value"))
    }

    /// A text member; empty when absent.
    pub(crate) fn str(&self, key: &str) -> Result<String, CommandError> {
        match self.get(key) {
            None => Ok(String::new()),
            Some(Value::String(s)) => Ok(s.clone()),
            Some(_) => Err(Self::wrong(key)),
        }
    }

    /// A text member that has to be there.
    pub(crate) fn must_str(&self, key: &str) -> Result<String, CommandError> {
        let s = self.str(key)?;
        if s.is_empty() { Err(CommandError::failed(format!("{key} is needed"))) } else { Ok(s) }
    }

    /// A whole number, or `None` when absent.
    pub(crate) fn int_opt(&self, key: &str) -> Result<Option<i64>, CommandError> {
        match self.get(key) {
            None => Ok(None),
            Some(Value::Number(n)) => {
                if let Some(i) = n.as_i64() {
                    return Ok(Some(i));
                }
                match n.as_f64() {
                    Some(f) if f.fract() == 0.0 && f.abs() <= MAX_EXACT => Ok(Some(f as i64)),
                    Some(_) => Err(CommandError::failed(format!("{key} must be a whole number"))),
                    None => Err(Self::wrong(key)),
                }
            }
            Some(_) => Err(Self::wrong(key)),
        }
    }

    /// A whole number that has to be there.
    pub(crate) fn must_int(&self, key: &str) -> Result<i64, CommandError> {
        self.int_opt(key)?.ok_or_else(|| CommandError::failed(format!("{key} is needed")))
    }

    /// A whole number that fits `i32` (millivolts, megahertz), or `None` when absent.
    pub(crate) fn i32_opt(&self, key: &str) -> Result<Option<i32>, CommandError> {
        self.int_opt(key)?
            .map(|n| {
                i32::try_from(n).map_err(|_| CommandError::failed(format!("{key}: {n} is out of range")))
            })
            .transpose()
    }

    /// A number, or `None` when absent.
    pub(crate) fn float_opt(&self, key: &str) -> Result<Option<f64>, CommandError> {
        match self.get(key) {
            None => Ok(None),
            Some(Value::Number(n)) => n.as_f64().map(Some).ok_or_else(|| Self::wrong(key)),
            Some(_) => Err(Self::wrong(key)),
        }
    }

    /// A flag; false when absent.
    pub(crate) fn boolean(&self, key: &str) -> Result<bool, CommandError> {
        match self.get(key) {
            None => Ok(false),
            Some(Value::Bool(b)) => Ok(*b),
            Some(_) => Err(Self::wrong(key)),
        }
    }

    /// A list of texts, or `None` when absent.
    pub(crate) fn strings(&self, key: &str) -> Result<Option<Vec<String>>, CommandError> {
        self.decode(key)
    }

    /// Any member as a typed value, or `None` when absent.
    pub(crate) fn decode<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>, CommandError> {
        match self.get(key) {
            None => Ok(None),
            Some(v) => serde_json::from_value(v.clone()).map(Some).map_err(|_| Self::wrong(key)),
        }
    }

    /// A member as raw JSON, or `None` when absent.
    pub(crate) fn raw(&self, key: &str) -> Option<&Value> {
        self.get(key)
    }

    /// The command line's positional action, with its default.
    pub(crate) fn action(&self, default: &str) -> Result<String, CommandError> {
        let a = self.str("action")?;
        Ok(if a.is_empty() { default.to_owned() } else { a })
    }

    /// A text member parsed into the type an operation wants (a power mode, an output, ...). The
    /// application layer owns the words each type accepts, so the window never has its own list.
    pub(crate) fn parsed<T: FromStr>(&self, key: &str) -> Result<T, CommandError>
    where
        T::Err: Display,
    {
        parse_word(&self.must_str(key)?)
    }
}

/// Parses a word into the type an operation wants, with the type's own error text.
pub(crate) fn parse_word<T: FromStr>(word: &str) -> Result<T, CommandError>
where
    T::Err: Display,
{
    word.parse().map_err(|e: T::Err| CommandError::failed(e.to_string()))
}

/// The `on | off | toggle | status` pattern of every boolean setting: the answer is the new state.
/// `words` lists the actions the setting accepts (`toggle` is not always one of them).
pub(crate) fn on_off(
    args: &Args,
    words: &[&str],
    get: impl FnOnce() -> Result<bool, CommandError>,
    set: impl FnOnce(bool) -> Result<bool, CommandError>,
) -> Outcome {
    let action = args.action("status")?;
    if !words.contains(&action.as_str()) {
        return Err(CommandError::failed(format!("action: {}", words.join(" | "))));
    }
    let current = get()?;
    let state = match action.as_str() {
        "status" => current,
        "toggle" => set(!current)?,
        other => set(other == "on")?,
    };
    Ok(Value::Bool(state))
}

/// The usual four words.
pub(crate) const SWITCH: &[&str] = &["on", "off", "toggle", "status"];
/// The three words of a setting that has no toggle.
pub(crate) const SWITCH_NO_TOGGLE: &[&str] = &["on", "off", "status"];

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn args(v: Value) -> Args {
        Args::new(&v).unwrap_or_else(|_| Args { members: Map::new() })
    }

    #[test]
    fn only_objects_and_null_are_arguments() {
        assert!(Args::new(&json!(null)).is_ok());
        assert!(Args::new(&json!({})).is_ok());
        for bad in [json!([]), json!("x"), json!(3), json!(true)] {
            assert!(Args::new(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn null_counts_as_absent() {
        let a = args(json!({"name": null, "level": null, "on": null, "xs": null}));
        assert!(a.raw("name").is_none() && a.raw("missing").is_none());
        assert_eq!(a.str("name").ok(), Some(String::new()));
        assert_eq!(a.int_opt("level").ok(), Some(None));
        assert_eq!(a.boolean("on").ok(), Some(false));
        assert_eq!(a.strings("xs").ok(), Some(None));
    }

    #[test]
    fn whole_numbers_are_checked() {
        let a = args(json!({"a": 7, "b": 7.0, "c": 7.5, "d": "7", "e": 1e300, "f": -3, "g": 3000000000i64}));
        assert_eq!(a.int_opt("a").ok(), Some(Some(7)));
        assert_eq!(a.int_opt("b").ok(), Some(Some(7)));
        assert_eq!(a.int_opt("f").ok(), Some(Some(-3)));
        for bad in ["c", "d", "e"] {
            assert!(a.int_opt(bad).is_err(), "{bad}");
        }
        assert!(a.i32_opt("g").is_err(), "does not fit i32");
        assert_eq!(a.i32_opt("a").ok(), Some(Some(7)));
        assert_eq!(a.must_int("zzz").map_err(|e| e.to_string()), Err("zzz is needed".to_owned()));
    }

    #[test]
    fn wrong_kinds_say_which_member() {
        let a = args(json!({"name": 5, "flag": "yes", "cells": {"a": 1}}));
        assert_eq!(a.str("name").map_err(|e| e.to_string()), Err("name: wrong kind of value".to_owned()));
        assert!(a.boolean("flag").is_err());
        assert!(a.strings("cells").is_err());
        assert_eq!(a.must_str("none").map_err(|e| e.to_string()), Err("none is needed".to_owned()));
    }

    #[test]
    fn the_action_has_a_default() {
        assert_eq!(args(json!({})).action("list").ok().as_deref(), Some("list"));
        assert_eq!(args(json!({"action": "show"})).action("list").ok().as_deref(), Some("show"));
        assert_eq!(args(json!({"action": ""})).action("list").ok().as_deref(), Some("list"));
    }

    #[test]
    fn a_switch_answers_the_new_state_and_refuses_other_words() {
        use std::cell::Cell;
        let set_to = Cell::new(None);
        let run = |action: &str, words: &[&str]| {
            set_to.set(None);
            let a = args(json!({"action": action}));
            on_off(
                &a,
                words,
                || Ok(true),
                |on| {
                    set_to.set(Some(on));
                    Ok(on)
                },
            )
        };
        assert_eq!(run("status", SWITCH).ok(), Some(json!(true)));
        assert_eq!(set_to.get(), None);
        assert_eq!(run("off", SWITCH).ok(), Some(json!(false)));
        assert_eq!(run("toggle", SWITCH).ok(), Some(json!(false)));
        assert_eq!(run("on", SWITCH).ok(), Some(json!(true)));
        let refused = run("toggle", SWITCH_NO_TOGGLE);
        assert_eq!(refused.map_err(|e| e.to_string()), Err("action: on | off | status".to_owned()));
        assert!(run("maybe", SWITCH).is_err());
    }
}
