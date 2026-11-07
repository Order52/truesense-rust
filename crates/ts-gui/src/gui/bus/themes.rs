//! Themes: the built-in catalogue, the user's own, and the one that runs on the keyboard.
//!
//! Every verb is an application-layer operation (`Api::themes`, `theme`, `apply_theme`,
//! `stop_theme`, `save_theme`, `delete_theme`), so the window, the command line and a look that
//! carries a theme part start a theme by the same code and the same checks. The key service draws
//! it; starting one only records the choice.
#![forbid(unsafe_code)]

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, PoisonError};

use serde_json::{Value, json};
use ts_api::api::{Api, ColourwayPick, ThemeSpec};

use super::args::Args;
use super::{CommandError, Ctx, Outcome, lift, val};

/// The thumbnails of built-in themes already drawn, by theme, choices and time. A built-in theme
/// never changes while the program runs, so its picture is drawn once; a theme of the user's can be
/// edited under the program, so it is always drawn again. The cache is bounded: when it is full it
/// starts over, which only costs a redraw.
#[derive(Default)]
pub(crate) struct PreviewCache(Mutex<HashMap<String, Value>>);

/// The most pictures kept: 32 themes with a few colourways each, with room to spare.
const PREVIEW_CACHE_MAX: usize = 512;

impl PreviewCache {
    fn get(&self, key: &str) -> Option<Value> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).get(key).cloned()
    }

    fn put(&self, key: String, value: Value) {
        let mut map = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if map.len() >= PREVIEW_CACHE_MAX {
            map.clear();
        }
        map.insert(key, value);
    }
}

/// `theme preview`: the 128 cells of one still frame of a theme with the choices given
/// (`colourway`, `custom`, `params` as in `theme apply`, and `t`, the moment in seconds).
fn preview(c: &Ctx<'_>, a: &Args) -> Result<Value, CommandError> {
    let spec = spec(a)?;
    let t = a.float_opt("t")?;
    let builtin = Api::theme_is_builtin(&spec.name);
    let key = format!(
        "{}|{}|{:?}|{}|{t:?}",
        spec.name,
        serde_json::to_string(&spec.colourway).unwrap_or_default(),
        spec.custom,
        serde_json::to_string(&spec.params).unwrap_or_default(),
    );
    if builtin && let Some(hit) = c.previews.get(&key) {
        return Ok(hit);
    }
    let cells = c.api.theme_preview(&spec, t)?;
    let answer = json!({"name": spec.name, "colourway": spec.colourway, "t": t, "cells": cells});
    if builtin {
        c.previews.put(key, answer.clone());
    }
    Ok(answer)
}

/// The handler of one command of this area; `None` when it is not one of ours.
pub(super) fn dispatch(c: &Ctx<'_>, key: &str, a: &Args) -> Option<Outcome> {
    lift(handle(c, key, a))
}

fn handle(c: &Ctx<'_>, key: &str, a: &Args) -> Result<Option<Value>, CommandError> {
    let api = c.api;
    Ok(Some(match key {
        "theme/preview" => preview(c, a)?,
        "theme/list" => val(api.themes())?,
        "theme/show" => val(api.theme(&a.must_str("name")?)?)?,
        "theme/apply" => val(api.apply_theme(&spec(a)?)?)?,
        "theme/stop" => json!({ "stopped": api.stop_theme()? }),
        "theme/save" => val(api.save_theme(&theme_json(a)?)?)?,
        "theme/delete" => {
            let name = a.must_str("name")?;
            api.delete_theme(&name)?;
            json!(name)
        }
        _ => return Ok(None),
    }))
}

/// The theme to start from the arguments: its name and any of the user's choices. Anything the
/// application layer checks (a colourway that exists, a setting the theme has) is not repeated
/// here; the shape of each member is.
fn spec(a: &Args) -> Result<ThemeSpec, CommandError> {
    let params: Option<BTreeMap<String, Value>> = a.decode("params")?;
    Ok(ThemeSpec {
        name: a.must_str("name")?,
        colourway: a.decode::<ColourwayPick>("colourway")?,
        custom: a.strings("custom")?.unwrap_or_default(),
        params: params.unwrap_or_default(),
        speed: a.int_opt("speed")?,
        brightness: a.int_opt("brightness")?,
    })
}

/// The theme to save, as the text of the catalogue's schema: given as an object (`theme`) or as
/// text (`json`).
fn theme_json(a: &Args) -> Result<String, CommandError> {
    match (a.raw("theme"), a.raw("json")) {
        (Some(object @ Value::Object(_)), _) => Ok(object.to_string()),
        (Some(_), _) => Err(CommandError::failed("theme: expected a theme object")),
        (None, Some(Value::String(text))) => Ok(text.clone()),
        (None, Some(_)) => Err(CommandError::failed("json: expected text")),
        (None, None) => Err(CommandError::failed("theme is needed")),
    }
}
