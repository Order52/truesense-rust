//! The `theme` command: transcripts over the simulated services. The Python and Go editions have no
//! such command, so these are written by hand (the help pages are in `testdata/help-spec.txt`, the
//! completions in the golden transcripts).

use serde_json::{Value, json};

use super::Fx;

fn json_of(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("not JSON ({e}): {text}"))
}

/// Runs the words as they are, for arguments that hold spaces.
fn words(fx: &Fx, words: &[&str]) -> super::Ran {
    let args: Vec<String> = words.iter().map(|w| (*w).to_owned()).collect();
    fx.run_args(&args, "")
}

fn active(fx: &Fx) -> Value {
    json_of(&fx.run("--json theme list").out)["status"]["active"]["spec"].clone()
}

#[test]
fn the_list_has_every_built_in_theme_and_says_nothing_runs() {
    let fx = Fx::new();
    let ran = fx.run("theme list");
    assert_eq!(ran.code, 0);
    assert_eq!(ran.out.lines().count(), 33, "32 themes and the status line");
    assert!(ran.out.starts_with("  predator           Predator\n"), "{}", ran.out);
    assert!(ran.out.contains("  storm              Storm  (not available yet)\n"));
    assert!(ran.out.ends_with("no theme is running\n"));
    assert_eq!(fx.run("theme").out, ran.out, "list is the default verb");
    let v = json_of(&fx.run("--json theme list").out);
    assert_eq!(v["themes"].as_array().unwrap().len(), 32);
    assert_eq!(v["status"], json!({"active": null, "halted": null}));
}

#[test]
fn show_prints_the_colourways_slots_and_settings() {
    let fx = Fx::new();
    let out = fx.run("theme show hearth").out;
    assert!(out.starts_with("Hearth (hearth)\nA calm fire"), "{out}");
    assert!(out.contains("  1  Gas flame        #2A1AFF #0050FF #0098FF #00E5FF\n"), "{out}");
    assert!(out.contains("custom slots: Embers and smoulder, Flames, Bright flames, Flame tips\n"));
    assert!(out.contains("  height         Flame height  [20 to 90 %]\n"));
    assert!(out.contains("  embers         Embers  [on or off]"));
    assert!(fx.run("theme show grid").out.contains("[0=Both ways, 1=Left to right, 2=Right to left]"));
    assert!(fx.run("theme show focus").out.contains("[key names, separated by commas]"));
    assert!(fx.run("theme show storm").out.contains("not available yet"));
    let v = json_of(&fx.run("--json theme show hearth").out);
    assert_eq!((v["id"].as_str(), v["builtin"].as_bool()), (Some("hearth"), Some(true)));
    assert!(v["theme"]["keyboard"].is_object());
    let ran = fx.run("theme show nobody");
    assert_eq!((ran.code, ran.out.as_str()), (1, ""));
    assert_eq!(ran.err, "error: no theme 'nobody' (truesense theme list)\n");
}

#[test]
fn apply_starts_the_theme_and_the_list_shows_which_runs() {
    let fx = Fx::new();
    let ran = fx.run("theme apply hearth");
    assert_eq!((ran.code, ran.out.as_str()), (0, "theme hearth is running\n"));
    let list = fx.run("theme list").out;
    assert!(list.contains("* hearth             Hearth\n"), "{list}");
    assert!(list.ends_with("theme hearth is running\n"));
    assert_eq!(active(&fx)["name"], "hearth");
    assert_eq!(fx.run("theme stop").out, "theme stopped\n");
    assert_eq!(fx.run("theme stop").out, "no theme was running\n");
    assert_eq!(fx.run("--json theme stop").out, "false\n");
    assert_eq!(active(&fx), Value::Null);
}

#[test]
fn apply_takes_a_colourway_by_position_or_name_and_custom_colours() {
    let fx = Fx::new();
    fx.run("theme apply hearth --colourway 2");
    assert_eq!(active(&fx)["colourway"], 2);
    words(&fx, &["theme", "apply", "hearth", "--colourway", "gas flame"]);
    assert_eq!(active(&fx)["colourway"], 1);
    fx.run("theme apply hearth --colourway custom --custom #112233 --custom #445566");
    let spec = active(&fx);
    assert_eq!(spec["colourway"], "custom");
    assert_eq!(spec["custom"], json!(["#112233", "#445566"]));
    let ran = fx.run("theme apply hearth --colourway nothing");
    assert_eq!(ran.code, 1);
    assert!(ran.err.contains("--colourway nothing"), "{}", ran.err);
    assert_eq!(active(&fx)["colourway"], "custom", "a refusal leaves the running theme alone");
}

#[test]
fn set_reads_each_value_as_the_kind_of_setting_it_is() {
    let fx = Fx::new();
    // A number, and a toggle by its words.
    fx.run("theme apply hearth --set height=60 --set embers=off");
    assert_eq!(active(&fx)["params"], json!({"height": 60.0, "embers": false}));
    fx.run("theme apply hearth --set embers=on");
    assert_eq!(active(&fx)["params"], json!({"embers": true}));
    // A choice by its label or its number.
    words(&fx, &["theme", "apply", "grid", "--set", "flow=left to right"]);
    assert_eq!(active(&fx)["params"], json!({"flow": 1}));
    fx.run("theme apply grid --set flow=2");
    assert_eq!(active(&fx)["params"], json!({"flow": 2.0}));
    // Keys.
    fx.run("theme apply focus --set keys=w_key,a_key,s_key,d_key");
    assert_eq!(active(&fx)["params"], json!({"keys": ["w_key", "a_key", "s_key", "d_key"]}));
}

#[test]
fn a_wrong_setting_or_value_is_an_error_with_the_reason_and_changes_nothing() {
    let fx = Fx::new();
    fx.run("theme apply hearth");
    for (line, expect) in [
        (
            "theme apply hearth --set nope=1",
            "theme hearth has no setting 'nope' (truesense theme show hearth)",
        ),
        ("theme apply hearth --set height", "--set height: use ID=VALUE"),
        ("theme apply hearth --set height=tall", "--set height: 'tall' is not a number"),
        ("theme apply hearth --set height=500", "setting 'height': 500 is outside"),
        ("theme apply hearth --set embers=maybe", "--set embers: 'maybe' is not on or off"),
        ("theme apply grid --set flow=sideways", "--set flow: 'sideways' is not one of the options"),
        ("theme apply focus --set keys=nokey", "not a key of this keyboard"),
        ("theme apply hearth --speed 5", "10 to 100"),
        ("theme apply hearth --brightness 0", "1 to 100"),
        ("theme apply storm", "flashing has not been measured"),
        ("theme apply nobody", "no theme 'nobody'"),
    ] {
        let ran = fx.run(line);
        assert_eq!((ran.code, ran.out.as_str()), (1, ""), "{line}");
        assert!(ran.err.starts_with("error: ") && ran.err.contains(expect), "{line}: {}", ran.err);
    }
    assert_eq!(active(&fx)["name"], "hearth");
    assert_eq!(active(&fx).get("params"), None);
}

#[test]
fn speed_and_brightness_are_recorded() {
    let fx = Fx::new();
    fx.run("theme apply hearth --speed 40 --brightness 25");
    let spec = active(&fx);
    assert_eq!((spec["speed"].as_i64(), spec["brightness"].as_i64()), (Some(40), Some(25)));
    let ran = fx.run("theme apply hearth --speed fast");
    assert_eq!(ran.code, 2, "a usage error: {}", ran.err);
}

#[test]
fn a_look_with_a_theme_shows_it_and_applying_the_look_starts_it() {
    let fx = Fx::new();
    fx.run("theme apply hearth --speed 60");
    // Save what is on the lights now (the keyboard look and the running theme).
    fx.run("look apply native");
    fx.run("theme apply hearth --speed 60");
    let saved = fx.run("look save fire");
    assert_eq!(saved.code, 0, "{}", saved.err);
    fx.run("theme stop");
    assert!(fx.run("look show fire").out.contains("theme hearth"), "{}", fx.run("look show fire").out);
    fx.run("look apply fire");
    assert_eq!(active(&fx)["name"], "hearth");
    assert_eq!(active(&fx)["speed"], 60);
}

#[test]
fn an_unknown_verb_is_a_usage_error() {
    let fx = Fx::new();
    let ran = fx.run("theme dance");
    assert_eq!(ran.code, 2);
    assert!(ran.err.contains("invalid choice: 'dance'"), "{}", ran.err);
}
