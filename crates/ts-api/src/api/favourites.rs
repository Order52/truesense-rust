//! Favourites: the star on looks, fan curves, Smart auto profiles and whole-setup profiles, and
//! the ordered list of the starred ones that the Home page and the PREDATOR key use.
//!
//! The list itself (`~/.config/truesense/favourites.json`, beside the user's configuration file)
//! is [`ts_domain::favourites`]. This module adds what only the application layer knows: whether
//! the named item still exists (a favourite whose look was deleted stays in the list, greyed, until
//! the user removes it), and how each kind is applied. Applying goes through the very operations
//! of the library pages (`apply_look`, `use_fan_curve`, `use_smart_profile`, `apply_profile`), so
//! the same safety rules and the same daemon routing apply as when the item is chosen by hand.
//!
//! Positions are 1-based everywhere in this layer, as the list is shown and as `favourite:N`
//! counts.
#![forbid(unsafe_code)]

use std::fmt;

use serde::Serialize;
use ts_domain::actions::{FavouritesFacade, HostError, HostResult};
use ts_domain::favourites::{
    Favourite, FavouriteStore, FavouritesError, Kind, MAX_FAVOURITES, next_position,
};

use super::{Api, ApiError, ApiResult, ThemeSpec};

pub use ts_domain::favourites::Kind as FavouriteKind;

/// Where `favourite-next` (the key and the command line alike) remembers the position it reached,
/// in the user state: the same key the key service's runner uses, so the two continue each other.
const NEXT_KEY: &str = "favourite";

/// One line of the favourites list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FavouriteInfo {
    /// The place in the list, counting from 1.
    pub position: usize,
    /// `look`, `fan_curve`, `smart_profile` or `profile`.
    pub kind: &'static str,
    /// The item's name in its library.
    pub name: String,
    /// Whether the item still exists; a missing one is shown greyed with a remove button, and
    /// applying it fails with the library's own "no such ..." message.
    pub exists: bool,
}

impl fmt::Display for FavouriteInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:>2}  {:<13} {}", self.position, self.kind, self.name)?;
        if !self.exists {
            f.write_str("   (missing)")?;
        }
        Ok(())
    }
}

/// How the library words "there is no such item", per kind, for a favourite that cannot be added.
fn missing(kind: Kind, name: &str) -> String {
    match kind {
        Kind::Look => format!("no saved look '{name}' (truesense look list)"),
        Kind::FanCurve => format!("no fan curve '{name}' (truesense fan curve list)"),
        Kind::SmartProfile => format!("no Smart auto profile '{name}' (truesense power smart list)"),
        Kind::Profile => format!("no profile '{name}' (truesense profile list)"),
        Kind::Theme => format!("no theme '{name}' (truesense theme list)"),
    }
}

fn in_libraries(libraries: &[(Kind, Vec<String>)], fav: &Favourite) -> bool {
    libraries.iter().any(|(kind, names)| *kind == fav.kind && names.contains(&fav.name))
}

fn convert(e: FavouritesError) -> ApiError {
    match e {
        FavouritesError::File(source) => ApiError::domain("favourites", source),
        other => ApiError::invalid(other.to_string()),
    }
}

impl Api {
    /// The favourites file's store; `None` when there is no user configuration file (no `HOME`).
    fn favourite_store(&self) -> Option<FavouriteStore> {
        self.s.config.user.as_deref().map(FavouriteStore::beside)
    }

    fn favourite_store_or_err(&self) -> ApiResult<FavouriteStore> {
        self.favourite_store().ok_or_else(|| convert(FavouritesError::NoLocation))
    }

    /// The names a library holds, for the `exists` flag. A library that cannot be read counts as
    /// empty: the list must still show.
    fn library_names(&self, kind: Kind) -> Vec<String> {
        match kind {
            Kind::Look => self.s.looks.names(),
            Kind::FanCurve => self.s.curves.names().unwrap_or_default(),
            Kind::SmartProfile => self.s.smart.names(),
            Kind::Profile => self.s.bundles.names(),
            Kind::Theme => self.theme_store().list().themes.into_iter().map(|t| t.id).collect(),
        }
    }

    /// Every library's names, read once for a whole list.
    fn libraries(&self) -> Vec<(Kind, Vec<String>)> {
        Kind::ALL.iter().map(|k| (*k, self.library_names(*k))).collect()
    }

    fn item_exists(&self, kind: Kind, name: &str) -> bool {
        self.library_names(kind).iter().any(|n| n == name)
    }

    /// The favourites in order, each with whether its item still exists. A missing or damaged
    /// file is an empty list.
    pub fn favourites(&self) -> Vec<FavouriteInfo> {
        let list = self.favourite_store().map(|s| s.list()).unwrap_or_default();
        let libraries = self.libraries();
        list.into_iter()
            .enumerate()
            .map(|(i, f)| {
                let exists = in_libraries(&libraries, &f);
                FavouriteInfo { position: i + 1, kind: f.kind.name(), name: f.name, exists }
            })
            .collect()
    }

    /// Stars an item. It must exist (the star button only appears on existing items, and a typo on
    /// the command line should say so). Returns `false` when it was already starred.
    pub fn favourite_add(&self, kind: Kind, name: &str) -> ApiResult<bool> {
        let store = self.favourite_store_or_err()?;
        if !self.item_exists(kind, name) {
            return Err(ApiError::invalid(missing(kind, name)));
        }
        store.add(kind, name).map_err(convert)
    }

    /// Flips the star of an item and answers whether it is starred now. Un-starring needs no check
    /// (a dangling favourite can always be removed); starring does.
    pub fn favourite_toggle(&self, kind: Kind, name: &str) -> ApiResult<bool> {
        let store = self.favourite_store_or_err()?;
        if !store.contains(kind, name) && !self.item_exists(kind, name) {
            return Err(ApiError::invalid(missing(kind, name)));
        }
        store.toggle(kind, name).map_err(convert)
    }

    /// Un-stars an item; `false` when it was not starred.
    pub fn favourite_remove(&self, kind: Kind, name: &str) -> ApiResult<bool> {
        self.favourite_store_or_err()?.remove(kind, name).map_err(convert)
    }

    /// Moves the favourite at position `from` to position `to` (both from 1; the others shift).
    pub fn favourite_move(&self, from: usize, to: usize) -> ApiResult<()> {
        let out_of_range = || ApiError::invalid("positions count from 1");
        let from = from.checked_sub(1).ok_or_else(out_of_range)?;
        let to = to.checked_sub(1).ok_or_else(out_of_range)?;
        self.favourite_store_or_err()?.move_to(from, to).map_err(convert)
    }

    /// Removes every favourite whose item no longer exists and answers what was removed (the
    /// positions are those the entries had before).
    pub fn favourites_prune(&self) -> ApiResult<Vec<FavouriteInfo>> {
        let store = self.favourite_store_or_err()?;
        let before = store.list();
        let libraries = self.libraries();
        let removed = store.prune(|f| in_libraries(&libraries, f)).map_err(convert)?;
        Ok(removed
            .into_iter()
            .filter_map(|f| {
                let position = before.iter().position(|b| *b == f)? + 1;
                Some(FavouriteInfo { position, kind: f.kind.name(), name: f.name, exists: false })
            })
            .collect())
    }

    /// Applies the favourite at this position (from 1) and says what was applied.
    pub fn favourite_apply_at(&self, position: usize) -> ApiResult<String> {
        let list = self.favourite_store().map(|s| s.list()).unwrap_or_default();
        let fav = position
            .checked_sub(1)
            .and_then(|i| list.get(i))
            .ok_or_else(|| ApiError::invalid(no_such_position(position, list.len())))?;
        self.apply_favourite_item(fav.kind, &fav.name)?;
        Ok(format!("Favourite {position}/{}: {} {}", list.len(), fav.kind.label(), fav.name))
    }

    /// Applies the favourite after the one reached last (by the key or by this command), wrapping
    /// at the end, and says what was applied. The position is remembered first, so a favourite
    /// whose item was deleted fails once and the next call moves on.
    pub fn favourite_next(&self) -> ApiResult<String> {
        let list = self.favourite_store().map(|s| s.list()).unwrap_or_default();
        let previous = self.s.user.get(NEXT_KEY).and_then(|v| v.as_i64());
        let at =
            next_position(previous, list.len()).ok_or_else(|| ApiError::invalid(no_such_position(1, 0)))?;
        // A convenience: a read-only state file must not stop the apply.
        let _ = self.s.user.set(NEXT_KEY, serde_json::json!(at));
        self.favourite_apply_at(at + 1)
    }

    /// Applies one item of a library by kind and name, starred or not: what a chip does. It
    /// answers `Look: calm` and the like. A name that does not exist fails with the library's own
    /// message.
    pub fn favourite_apply(&self, kind: Kind, name: &str) -> ApiResult<String> {
        self.apply_favourite_item(kind, name)?;
        Ok(format!("{}: {name}", kind.label()))
    }

    /// One item through the operation its library page uses.
    fn apply_favourite_item(&self, kind: Kind, name: &str) -> ApiResult<()> {
        match kind {
            Kind::Look => self.apply_look(name).map(drop),
            Kind::FanCurve => self.use_fan_curve(name).map(drop),
            Kind::SmartProfile => self.use_smart_profile(name).map(drop),
            Kind::Profile => self.apply_profile(name).map(drop),
            Kind::Theme => self.apply_theme(&ThemeSpec::new(name)).map(drop),
        }
    }
}

fn no_such_position(position: usize, total: usize) -> String {
    match total {
        0 => "no favourites yet (star one: truesense favourite add look NAME)".to_owned(),
        _ => format!("no favourite {position}: you have {total} (the most is {MAX_FAVOURITES})"),
    }
}

/// The key service's runner reads the favourites and starts a Smart profile through the same
/// application layer a command would.
impl FavouritesFacade for Api {
    fn favourites(&self) -> HostResult<Vec<Favourite>> {
        Ok(self.favourite_store().map(|s| s.list()).unwrap_or_default())
    }

    fn use_smart_profile(&self, name: &str) -> HostResult<()> {
        Api::use_smart_profile(self, name).map(drop).map_err(|e| HostError::new(e.to_string()))
    }

    fn apply_theme(&self, name: &str) -> HostResult<()> {
        Api::apply_theme(self, &ThemeSpec::new(name)).map(drop).map_err(|e| HostError::new(e.to_string()))
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use serde_json::json;

    use super::super::tests::Fx;
    use super::*;

    fn names(list: &[FavouriteInfo]) -> Vec<String> {
        list.iter().map(|f| format!("{}:{}:{}", f.position, f.kind, f.name)).collect()
    }

    #[test]
    fn an_empty_list_when_nothing_is_starred_and_the_file_is_beside_the_config() {
        let fx = Fx::new();
        assert!(fx.api.favourites().is_empty());
        fx.api.favourite_add(Kind::Look, "native").unwrap();
        assert!(fx.dir.0.join("home/favourites.json").exists());
    }

    #[test]
    fn adding_needs_an_existing_item_of_that_kind() {
        let fx = Fx::new();
        fx.rig.bundles.add("gaming", "");
        assert!(fx.api.favourite_add(Kind::Look, "native").unwrap());
        assert!(!fx.api.favourite_add(Kind::Look, "native").unwrap(), "no duplicate");
        assert!(fx.api.favourite_add(Kind::Profile, "gaming").unwrap());
        assert!(fx.api.favourite_add(Kind::FanCurve, "truesense").unwrap());
        assert!(fx.api.favourite_add(Kind::SmartProfile, "truesense").unwrap());
        for (kind, name, want) in [
            (Kind::Look, "ghost", "no saved look 'ghost' (truesense look list)"),
            (Kind::FanCurve, "ghost", "no fan curve 'ghost' (truesense fan curve list)"),
            (Kind::SmartProfile, "ghost", "no Smart auto profile 'ghost' (truesense power smart list)"),
            (Kind::Profile, "ghost", "no profile 'ghost' (truesense profile list)"),
            (Kind::Profile, "native", "no profile 'native' (truesense profile list)"),
        ] {
            assert_eq!(fx.api.favourite_add(kind, name).unwrap_err().to_string(), want);
            assert_eq!(fx.api.favourite_toggle(kind, name).unwrap_err().to_string(), want);
        }
        assert_eq!(
            names(&fx.api.favourites()),
            ["1:look:native", "2:profile:gaming", "3:fan_curve:truesense", "4:smart_profile:truesense"]
        );
    }

    #[test]
    fn a_theme_can_be_starred_listed_and_applied_like_any_other_item() {
        let fx = Fx::new();
        assert!(fx.api.favourite_toggle(Kind::Theme, "hearth").unwrap());
        assert_eq!(
            fx.api.favourite_add(Kind::Theme, "ghost").unwrap_err().to_string(),
            "no theme 'ghost' (truesense theme list)"
        );
        assert_eq!(names(&fx.api.favourites()), ["1:theme:hearth"]);
        assert!(fx.api.favourites()[0].exists);
        assert_eq!(fx.api.favourite_apply(Kind::Theme, "hearth").unwrap(), "Theme: hearth");
        assert_eq!(fx.api.theme_status().active.unwrap().spec.name, "hearth");
        // The same call the PREDATOR key's runner makes.
        let facade: &dyn FavouritesFacade = &fx.api;
        fx.api.stop_theme().unwrap();
        facade.apply_theme("sunset").unwrap();
        assert_eq!(fx.api.theme_status().active.unwrap().spec.name, "sunset");
        assert!(facade.apply_theme("ghost").is_err());
        assert_eq!(fx.api.favourite_apply_at(1).unwrap(), "Favourite 1/1: Theme hearth");
        assert_eq!(fx.api.theme_status().active.unwrap().spec.name, "hearth");
        assert!(!fx.api.favourite_toggle(Kind::Theme, "hearth").unwrap());
    }

    #[test]
    fn toggle_stars_and_unstars_and_a_dangling_star_can_always_be_removed() {
        let fx = Fx::new();
        fx.rig.bundles.add("gaming", "");
        assert!(fx.api.favourite_toggle(Kind::Profile, "gaming").unwrap());
        assert!(fx.api.favourites()[0].exists);
        fx.api.delete_profile("gaming").unwrap();
        let list = fx.api.favourites();
        assert_eq!((list[0].name.as_str(), list[0].exists), ("gaming", false));
        assert_eq!(list[0].to_string(), " 1  profile       gaming   (missing)");
        assert!(!fx.api.favourite_toggle(Kind::Profile, "gaming").unwrap(), "the toggle removes it");
        assert!(fx.api.favourites().is_empty());
        assert!(!fx.api.favourite_remove(Kind::Profile, "gaming").unwrap());
    }

    #[test]
    fn move_uses_positions_from_one() {
        let fx = Fx::new();
        for n in ["a", "b", "c"] {
            fx.rig.bundles.add(n, "");
            fx.api.favourite_add(Kind::Profile, n).unwrap();
        }
        fx.api.favourite_move(3, 1).unwrap();
        assert_eq!(names(&fx.api.favourites()), ["1:profile:c", "2:profile:a", "3:profile:b"]);
        assert_eq!(fx.api.favourite_move(0, 1).unwrap_err().to_string(), "positions count from 1");
        assert_eq!(fx.api.favourite_move(1, 0).unwrap_err().to_string(), "positions count from 1");
        assert_eq!(
            fx.api.favourite_move(1, 4).unwrap_err().to_string(),
            "position out of range: favourites are 1..3"
        );
        assert_eq!(names(&fx.api.favourites()), ["1:profile:c", "2:profile:a", "3:profile:b"]);
    }

    #[test]
    fn apply_by_kind_and_name_uses_each_librarys_own_operation() {
        let fx = Fx::new();
        fx.rig.bundles.add("gaming", "");
        fx.rig.daemon.answer_ok("fan_curve", json!({"name": "truesense"}));
        fx.rig.daemon.answer_ok("smart_profile", json!({"name": "truesense"}));
        assert_eq!(fx.api.favourite_apply(Kind::Look, "native").unwrap(), "Look: native");
        assert_eq!(fx.api.favourite_apply(Kind::Profile, "gaming").unwrap(), "Profile: gaming");
        assert_eq!(fx.rig.bundles.applied(), ["gaming"]);
        assert_eq!(fx.api.favourite_apply(Kind::FanCurve, "truesense").unwrap(), "Fan curve: truesense");
        let args = fx.rig.daemon.last_args("fan_curve").unwrap();
        assert_eq!(args.get("action"), Some(&json!("use")));
        assert_eq!(args.get("name"), Some(&json!("truesense")));
        assert_eq!(
            fx.api.favourite_apply(Kind::SmartProfile, "truesense").unwrap(),
            "Smart profile: truesense"
        );
        assert_eq!(fx.rig.daemon.last_args("smart_profile").unwrap().get("name"), Some(&json!("truesense")));
    }

    #[test]
    fn applying_something_that_is_gone_or_with_no_daemon_is_a_normal_error() {
        let fx = Fx::new();
        assert!(fx.api.favourite_apply(Kind::Profile, "ghost").is_err());
        assert!(fx.api.favourite_apply(Kind::Look, "ghost").is_err());
        assert!(fx.api.favourite_apply(Kind::FanCurve, "ghost").is_err());
        assert!(fx.api.favourite_apply(Kind::SmartProfile, "ghost").is_err());
        let off = Fx::offline();
        assert!(off.api.favourite_apply(Kind::FanCurve, "truesense").is_err());
        assert!(off.api.favourite_apply(Kind::SmartProfile, "truesense").is_err());
    }

    #[test]
    fn apply_at_and_next_say_what_they_applied_and_share_the_keys_position() {
        let fx = Fx::new();
        for n in ["a", "b", "c"] {
            fx.rig.bundles.add(n, "");
            fx.api.favourite_add(Kind::Profile, n).unwrap();
        }
        assert_eq!(fx.api.favourite_apply_at(2).unwrap(), "Favourite 2/3: Profile b");
        assert_eq!(fx.api.favourite_next().unwrap(), "Favourite 1/3: Profile a");
        assert_eq!(fx.api.favourite_next().unwrap(), "Favourite 2/3: Profile b");
        // The key's runner stores the same key; whoever acted last decides where the cycle goes on.
        assert_eq!(fx.api.services().user.get("favourite"), Some(json!(1)));
        fx.api.services().user.set("favourite", json!(2)).unwrap();
        assert_eq!(fx.api.favourite_next().unwrap(), "Favourite 1/3: Profile a");
        assert_eq!(fx.rig.bundles.applied(), ["b", "a", "b", "a"]);
        assert_eq!(
            fx.api.favourite_apply_at(0).unwrap_err().to_string(),
            "no favourite 0: you have 3 (the most is 24)"
        );
        assert_eq!(
            fx.api.favourite_apply_at(4).unwrap_err().to_string(),
            "no favourite 4: you have 3 (the most is 24)"
        );
    }

    #[test]
    fn an_empty_list_cannot_be_applied_and_says_how_to_start() {
        let fx = Fx::new();
        let want = "no favourites yet (star one: truesense favourite add look NAME)";
        assert_eq!(fx.api.favourite_apply_at(1).unwrap_err().to_string(), want);
        assert_eq!(fx.api.favourite_next().unwrap_err().to_string(), want);
    }

    #[test]
    fn a_dangling_favourite_fails_once_in_the_cycle_and_the_next_call_moves_on() {
        let fx = Fx::new();
        for n in ["a", "b"] {
            fx.rig.bundles.add(n, "");
            fx.api.favourite_add(Kind::Profile, n).unwrap();
        }
        fx.api.delete_profile("a").unwrap();
        assert!(fx.api.favourite_next().is_err());
        assert_eq!(fx.api.favourite_next().unwrap(), "Favourite 2/2: Profile b");
    }

    #[test]
    fn prune_drops_the_dangling_ones_and_says_which() {
        let fx = Fx::new();
        for n in ["a", "b", "c"] {
            fx.rig.bundles.add(n, "");
            fx.api.favourite_add(Kind::Profile, n).unwrap();
        }
        fx.api.favourite_add(Kind::Look, "native").unwrap();
        fx.api.delete_profile("a").unwrap();
        fx.api.delete_profile("c").unwrap();
        let removed = fx.api.favourites_prune().unwrap();
        assert_eq!(names(&removed), ["1:profile:a", "3:profile:c"]);
        assert_eq!(names(&fx.api.favourites()), ["1:profile:b", "2:look:native"]);
        assert!(fx.api.favourites_prune().unwrap().is_empty());
    }

    #[test]
    fn the_list_serialises_with_the_documented_fields() {
        let fx = Fx::new();
        fx.api.favourite_add(Kind::Look, "native").unwrap();
        assert_eq!(
            serde_json::to_value(fx.api.favourites()).unwrap(),
            json!([{"position": 1, "kind": "look", "name": "native", "exists": true}])
        );
    }

    #[test]
    fn without_a_home_the_list_is_empty_and_changes_say_why() {
        let fx = Fx::build(|mut s| {
            s.config.user = None;
            s
        });
        assert!(fx.api.favourites().is_empty());
        assert_eq!(
            fx.api.favourite_add(Kind::Look, "native").unwrap_err().to_string(),
            "there is no place to keep favourites (HOME is not set)"
        );
        assert!(fx.api.favourite_remove(Kind::Look, "native").is_err());
    }

    #[test]
    fn a_damaged_file_is_an_empty_list_and_the_next_star_repairs_it() {
        let fx = Fx::new();
        std::fs::create_dir_all(fx.dir.0.join("home")).unwrap();
        std::fs::write(fx.dir.0.join("home/favourites.json"), b"][").unwrap();
        assert!(fx.api.favourites().is_empty());
        assert!(fx.api.favourite_add(Kind::Look, "native").unwrap());
        assert_eq!(fx.api.favourites().len(), 1);
    }

    #[test]
    fn the_facade_for_the_key_service_lists_and_starts_smart_profiles() {
        let fx = Fx::new();
        fx.api.favourite_add(Kind::Look, "native").unwrap();
        let facade: &dyn FavouritesFacade = &fx.api;
        assert_eq!(facade.favourites().unwrap(), vec![Favourite::new(Kind::Look, "native")]);
        fx.rig.daemon.answer_ok("smart_profile", json!({"name": "truesense"}));
        assert!(facade.use_smart_profile("truesense").is_ok());
        assert!(facade.use_smart_profile("ghost").is_err());
    }
}
