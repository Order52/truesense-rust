//! Favourites: the starred looks, fan curves, Smart auto profiles, themes and whole-setup profiles.
//!
//! The list, the star toggle and the apply of one favourite are application-layer operations
//! (`Api::favourites`, `favourite_toggle`, `favourite_apply`, ...), so the window, the command
//! line and the PREDATOR key apply a favourite by the same code. Positions count from 1, as the
//! list shows them and as the key action `favourite:N` counts.
#![forbid(unsafe_code)]

use serde_json::{Value, json};
use ts_api::api::{Api, FavouriteKind};

use super::args::Args;
use super::{CommandError, Ctx, Outcome, lift, val};

/// The handler of one command of this area; `None` when it is not one of ours.
pub(super) fn dispatch(c: &Ctx<'_>, key: &str, a: &Args) -> Option<Outcome> {
    lift(handle(c.api, key, a))
}

fn handle(api: &Api, key: &str, a: &Args) -> Result<Option<Value>, CommandError> {
    Ok(Some(match key {
        "favourite/list" => val(api.favourites())?,
        "favourite/toggle" => {
            let (kind, name) = item(a)?;
            json!({ "starred": api.favourite_toggle(kind, &name)? })
        }
        "favourite/remove" => {
            let (kind, name) = item(a)?;
            json!({ "removed": api.favourite_remove(kind, &name)? })
        }
        "favourite/move" => {
            api.favourite_move(position(a, "from")?, position(a, "to")?)?;
            // The new order, so a list that was dragged shows what is stored.
            val(api.favourites())?
        }
        "favourite/apply" => {
            // A chip of the list knows its position; the star of a library page knows the item.
            if a.int_opt("position")?.is_some() {
                json!(api.favourite_apply_at(position(a, "position")?)?)
            } else {
                let (kind, name) = item(a)?;
                json!(api.favourite_apply(kind, &name)?)
            }
        }
        _ => return Ok(None),
    }))
}

/// The kind and name of a library item.
fn item(a: &Args) -> Result<(FavouriteKind, String), CommandError> {
    Ok((a.parsed("kind")?, a.must_str("name")?))
}

/// A position from 1: a number that is not negative or absurd, so a wrong member is refused with
/// a message instead of wrapping into a different one.
fn position(a: &Args, key: &str) -> Result<usize, CommandError> {
    let n = a.must_int(key)?;
    usize::try_from(n).map_err(|_| CommandError::failed(format!("{key}: positions count from 1")))
}
