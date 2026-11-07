//! The `favourite` command: transcripts over the simulated services. The Python edition has no
//! such command, so these are written by hand (the help pages are in `testdata/help-spec.txt`,
//! the completions in the golden transcripts).

use serde_json::{Value, json};

use super::Fx;

fn json_of(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("not JSON ({e}): {text}"))
}

fn with_profiles(names: &[&str]) -> Fx {
    let fx = Fx::new();
    for n in names {
        fx.rig.bundles.add(n, "");
    }
    fx
}

#[test]
fn an_empty_list_says_how_to_start_and_is_an_empty_json_array() {
    let fx = Fx::new();
    let ran = fx.run("favourite list");
    assert_eq!(
        (ran.code, ran.out.as_str()),
        (0, "(no favourites yet; `truesense favourite add look NAME`)\n")
    );
    assert_eq!(fx.run("favourite").out, ran.out, "list is the default verb");
    assert_eq!(fx.run("--json favourite list").out, "[]\n");
}

#[test]
fn add_stars_once_and_the_list_shows_position_kind_and_name() {
    let fx = with_profiles(&["gaming"]);
    assert_eq!(fx.run("favourite add look native").out, "starred look native\n");
    assert_eq!(fx.run("favourite add look native").out, "look native is already a favourite\n");
    assert_eq!(fx.run("--json favourite add profile gaming").out, "true\n");
    assert_eq!(fx.run("--json favourite add profile gaming").out, "false\n");
    let ran = fx.run("favourite list");
    assert_eq!(ran.out, " 1  look          native\n 2  profile       gaming\n");
    assert_eq!(
        json_of(&fx.run("--json favourite list").out),
        json!([
            {"position": 1, "kind": "look", "name": "native", "exists": true},
            {"position": 2, "kind": "profile", "name": "gaming", "exists": true}
        ])
    );
}

#[test]
fn add_refuses_what_does_not_exist_and_a_wrong_kind_is_a_usage_error() {
    let fx = Fx::new();
    let ran = fx.run("favourite add look ghost");
    assert_eq!((ran.code, ran.out.as_str()), (1, ""));
    assert_eq!(ran.err, "error: no saved look 'ghost' (truesense look list)\n");
    assert_eq!(
        fx.run("favourite add profile ghost").err,
        "error: no profile 'ghost' (truesense profile list)\n"
    );
    let ran = fx.run("favourite add undervolt cpu");
    assert_eq!(ran.code, 2);
    assert!(
        ran.err.contains("argument kind: invalid choice: 'undervolt' (choose from 'look', 'fan_curve', 'smart_profile', 'profile', 'theme')"),
        "{}",
        ran.err
    );
    assert_eq!(fx.run("favourite add look").code, 2, "a name is needed");
    assert_eq!(fx.run("favourite list").out, "(no favourites yet; `truesense favourite add look NAME`)\n");
}

#[test]
fn remove_unstars_and_says_so_when_it_was_not_starred() {
    let fx = with_profiles(&["a", "b"]);
    fx.run("favourite add profile a");
    fx.run("favourite add profile b");
    assert_eq!(fx.run("favourite remove profile a").out, "removed profile a from the favourites\n");
    let ran = fx.run("favourite remove profile a");
    assert_eq!(ran.code, 1);
    assert_eq!(ran.err, "error: profile a is not a favourite (truesense favourite list)\n");
    assert_eq!(fx.run("--json favourite remove profile b").out, "true\n");
    assert_eq!(fx.run("--json favourite list").out, "[]\n");
}

#[test]
fn a_deleted_item_is_listed_as_missing_and_can_still_be_removed() {
    let fx = with_profiles(&["a"]);
    fx.run("favourite add profile a");
    fx.run("profile delete a");
    assert_eq!(fx.run("favourite list").out, " 1  profile       a   (missing)\n");
    assert_eq!(json_of(&fx.run("--json favourite list").out)[0]["exists"], json!(false));
    assert_eq!(fx.run("favourite apply 1").code, 1);
    assert_eq!(fx.run("favourite remove profile a").code, 0);
}

#[test]
fn move_reorders_and_prints_the_new_list() {
    let fx = with_profiles(&["a", "b", "c"]);
    for n in ["a", "b", "c"] {
        fx.run(&format!("favourite add profile {n}"));
    }
    assert_eq!(
        fx.run("favourite move 3 1").out,
        " 1  profile       c\n 2  profile       a\n 3  profile       b\n"
    );
    let bad = fx.run("favourite move 1 9");
    assert_eq!((bad.code, bad.err.as_str()), (1, "error: position out of range: favourites are 1..3\n"));
    assert_eq!(fx.run("favourite move 0 1").err, "error: positions count from 1\n");
    assert_eq!(fx.run("favourite move -1 1").err, "error: positions count from 1\n");
    assert_eq!(fx.run("favourite move x 1").code, 2);
    assert_eq!(
        json_of(&fx.run("--json favourite move 1 3").out).as_array().map(Vec::len),
        Some(3),
        "--json prints the list"
    );
}

#[test]
fn apply_by_position_or_by_kind_and_name() {
    let fx = with_profiles(&["gaming"]);
    fx.daemon().answer_ok("fan_curve", json!({"name": "truesense"}));
    fx.daemon().answer_ok("smart_profile", json!({"name": "truesense"}));
    fx.run("favourite add profile gaming");
    fx.run("favourite add fan_curve truesense");
    fx.run("favourite add smart_profile truesense");
    assert_eq!(fx.run("favourite apply 1").out, "Favourite 1/3: Profile gaming\n");
    assert_eq!(fx.run("favourite apply 2").out, "Favourite 2/3: Fan curve truesense\n");
    assert_eq!(fx.run("favourite apply 3").out, "Favourite 3/3: Smart profile truesense\n");
    assert_eq!(fx.rig.bundles.applied(), ["gaming"]);
    assert_eq!(fx.run("favourite apply look native").out, "Look: native\n");
    assert_eq!(fx.run("--json favourite apply 1").out, "\"Favourite 1/3: Profile gaming\"\n");
    let mut daemon_used = fx.daemon().commands();
    daemon_used.sort();
    daemon_used.dedup();
    assert_eq!(daemon_used, ["fan_curve", "smart_profile"]);
}

#[test]
fn apply_says_what_is_wrong_with_the_words() {
    let fx = with_profiles(&["gaming"]);
    fx.run("favourite add profile gaming");
    let table = [
        ("favourite apply 0", "error: no favourite 0: you have 1 (the most is 24)\n"),
        ("favourite apply 2", "error: no favourite 2: you have 1 (the most is 24)\n"),
        ("favourite apply 1 gaming", "error: apply N takes no name (or apply KIND NAME)\n"),
        ("favourite apply look", "error: apply look needs a NAME\n"),
        (
            "favourite apply sound x",
            "error: 'sound': the kind is one of look, fan_curve, smart_profile, profile, theme\n",
        ),
        (
            "favourite apply 99999999999999999999999",
            "error: '99999999999999999999999' is not a usable position\n",
        ),
        ("favourite apply profile ghost", "error: no profile 'ghost'\n"),
    ];
    for (line, want) in table {
        let ran = fx.run(line);
        assert_eq!((ran.code, ran.err.as_str()), (1, want), "{line}");
    }
}

#[test]
fn apply_with_nothing_starred_says_how_to_start() {
    let fx = Fx::new();
    let want = "error: no favourites yet (star one: truesense favourite add look NAME)\n";
    assert_eq!(fx.run("favourite apply 1").err, want);
    assert_eq!(fx.run("favourite next").err, want);
}

#[test]
fn next_cycles_through_the_list_and_wraps() {
    let fx = with_profiles(&["a", "b", "c"]);
    for n in ["a", "b", "c"] {
        fx.run(&format!("favourite add profile {n}"));
    }
    let said: Vec<String> = (0..4).map(|_| fx.run("favourite next").out).collect();
    assert_eq!(
        said,
        [
            "Favourite 1/3: Profile a\n",
            "Favourite 2/3: Profile b\n",
            "Favourite 3/3: Profile c\n",
            "Favourite 1/3: Profile a\n"
        ]
    );
}

#[test]
fn a_fan_curve_favourite_needs_the_service_like_fan_curve_use() {
    let fx = Fx::offline();
    fx.run("favourite add fan_curve truesense");
    let ran = fx.run("favourite apply 1");
    assert_eq!(ran.code, 1);
    assert!(ran.err.starts_with("error: "), "{}", ran.err);
}

#[test]
fn the_completions_know_the_area_and_its_verbs() {
    let fx = Fx::new();
    let bash = fx.run("completion bash").out;
    assert!(bash.contains("favourite)"), "{bash}");
    assert!(bash.contains("list add remove move apply next"), "{bash}");
    assert!(bash.contains("look fan_curve smart_profile profile"), "the kinds `add` completes");
    let fish = fx.run("completion fish").out;
    assert!(fish.contains("-n '__fish_use_subcommand' -a favourite"), "{fish}");
    for verb in ["list", "add", "remove", "move", "apply", "next"] {
        assert!(fish.contains(&format!("-n '__fish_seen_subcommand_from favourite' -a '{verb}'")), "{verb}");
    }
}

#[test]
fn the_command_table_lists_every_verb() {
    let table = crate::cli::command_table();
    for verb in ["list", "add", "remove", "move", "apply", "next"] {
        assert!(table.contains(&format!("truesense favourite {verb}")), "{verb}");
    }
}

#[test]
fn a_theme_is_starred_listed_and_applied_from_the_command_line() {
    let fx = Fx::new();
    let ran = fx.run("favourite add theme hearth");
    assert_eq!((ran.code, ran.out.as_str()), (0, "starred theme hearth\n"), "{}", ran.err);
    assert!(fx.run("favourite list").out.contains("theme"), "{}", fx.run("favourite list").out);
    assert_eq!(fx.run("favourite add theme ghost").err, "error: no theme 'ghost' (truesense theme list)\n");
    let ran = fx.run("favourite apply theme hearth");
    assert_eq!((ran.code, ran.out.as_str()), (0, "Theme: hearth\n"), "{}", ran.err);
    assert!(fx.run("theme list").out.ends_with("theme hearth is running\n"));
    assert_eq!(fx.run("favourite remove theme hearth").code, 0);
    assert!(fx.run("favourite list").out.contains("no favourites"));
}
