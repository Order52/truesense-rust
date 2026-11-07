//! `favourite`: the short list of starred looks, fan curves, Smart auto profiles and whole-setup
//! profiles that the window's Home page and the PREDATOR key (`favourite:N`, `favourite-next`) use.
//!
//! New in 0.5; the Python and Go editions have no such command. It lives in one file: the command
//! tree entry ([`command`]) and the handler ([`run`]) of this area, so that the area is added to
//! the program with one line in the tree and one in the dispatch table.
#![forbid(unsafe_code)]

use ts_api::api::{Api, FavouriteInfo, FavouriteKind};

use super::argparse::{Arg, Command, Matches};
use super::handlers::{Cx, Outcome, fail};

/// The kinds `add` and `remove` accept (the words of the favourites file and of the bus).
const KINDS: [&str; 4] = ["look", "fan_curve", "smart_profile", "profile"];

/// The `favourite` area of the command tree.
pub(super) fn command() -> Command {
    let kind = || Arg::pos("kind").choices(&KINDS);
    Command::new("favourite")
        .help("starred looks, fan curves, Smart profiles and profiles (the Home page and the PREDATOR key use them): list | add KIND NAME | remove KIND NAME | move FROM TO | apply N | apply KIND NAME | next")
        .subs(
            "verb",
            vec![
                Command::new("list"),
                Command::new("add")
                    .help("star something that exists (at most 24)")
                    .arg(kind())
                    .arg(Arg::pos("name")),
                Command::new("remove").arg(kind()).arg(Arg::pos("name")),
                Command::new("move")
                    .help("move the favourite at position FROM to position TO (positions count from 1)")
                    .arg(Arg::pos("from").int())
                    .arg(Arg::pos("to").int()),
                Command::new("apply")
                    .help("apply the Nth favourite, or any look, curve or profile by KIND NAME")
                    .arg(Arg::pos("what"))
                    .arg(Arg::pos("name").optional()),
                Command::new("next").help("apply the favourite after the last one applied (the key does the same)"),
            ],
        )
}

pub(super) fn run(cx: &mut Cx<'_, '_>, verb: Option<&str>, m: &Matches) -> Outcome {
    let api = cx.api;
    match verb.unwrap_or("list") {
        "list" => list(cx, api),
        "add" => {
            let (kind, name) = item(m)?;
            let added = api.favourite_add(kind, name)?;
            let text = if added {
                format!("starred {kind} {name}")
            } else {
                format!("{kind} {name} is already a favourite")
            };
            cx.emit(&added, Some(&text));
        }
        "remove" => {
            let (kind, name) = item(m)?;
            if !api.favourite_remove(kind, name)? {
                return Err(fail(format!("{kind} {name} is not a favourite (truesense favourite list)")));
            }
            cx.emit(&true, Some(&format!("removed {kind} {name} from the favourites")));
        }
        "move" => {
            let (from, to) = (position(m, "from")?, position(m, "to")?);
            api.favourite_move(from, to)?;
            list(cx, api);
        }
        "apply" => {
            let said = apply(api, m)?;
            cx.emit_text(&said);
        }
        "next" => {
            let said = api.favourite_next()?;
            cx.emit_text(&said);
        }
        other => return Err(fail(format!("unknown favourite command {other}"))),
    }
    Ok(0)
}

fn list(cx: &mut Cx<'_, '_>, api: &Api) {
    let all = api.favourites();
    let text = if all.is_empty() {
        "(no favourites yet; `truesense favourite add look NAME`)".to_owned()
    } else {
        all.iter().map(FavouriteInfo::to_string).collect::<Vec<_>>().join("\n")
    };
    cx.emit(&all, Some(&text));
}

/// `apply 2` (a position) or `apply look calm` (an item).
fn apply(api: &Api, m: &Matches) -> Result<String, super::handlers::Fail> {
    let what = m.str("what").unwrap_or_default();
    let name = m.str("name");
    if what.bytes().all(|b| b.is_ascii_digit()) && !what.is_empty() {
        if name.is_some() {
            return Err(fail("apply N takes no name (or apply KIND NAME)"));
        }
        let n: usize = what.parse().map_err(|_| fail(format!("'{what}' is not a usable position")))?;
        return Ok(api.favourite_apply_at(n)?);
    }
    let kind: FavouriteKind = what.parse().map_err(fail)?;
    let name = name.ok_or_else(|| fail(format!("apply {kind} needs a NAME")))?;
    Ok(api.favourite_apply(kind, name)?)
}

fn item(m: &Matches) -> Result<(FavouriteKind, &str), super::handlers::Fail> {
    let kind: FavouriteKind = m.str("kind").unwrap_or_default().parse().map_err(fail)?;
    Ok((kind, m.str("name").unwrap_or_default()))
}

/// A position argument: argparse already made it a whole number; a negative one is refused here.
fn position(m: &Matches, key: &str) -> Result<usize, super::handlers::Fail> {
    let n = m.int(key).unwrap_or_default();
    usize::try_from(n).map_err(|_| fail("positions count from 1"))
}
