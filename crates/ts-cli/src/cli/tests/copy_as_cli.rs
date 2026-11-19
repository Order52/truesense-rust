//! The Lighting page's "Copy as TrueSense CLI": every command it copies is run here through this
//! command line against the simulated laptop, and the lights are read back. A copied command must
//! put exactly what the page showed on the laptop, in one command.

use serde_json::{Value, json};
use ts_ui_model::vm::lighting::{self, Key};

use super::Fx;

fn json_of(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("not JSON ({e}): {text}"))
}

/// The page's command for `args`, run as a shell would hand it over (the words after `truesense`).
fn copy_and_run(fx: &Fx, args: Value) -> String {
    let cmd = lighting::call("cli", &args).unwrap_or_else(|e| panic!("{e}: {args}"));
    let words: Vec<String> =
        cmd["words"].as_array().unwrap().iter().map(|w| w.as_str().unwrap().to_owned()).collect();
    assert_eq!(words[0], "truesense");
    let ran = fx.run_args(&words[1..], "");
    assert_eq!(ran.code, 0, "{} -> {}", cmd["text"], ran.err);
    cmd["text"].as_str().unwrap().to_owned()
}

fn keyboard_look(fx: &Fx) -> Value {
    json_of(&fx.run("--json keyboard status").out)["look"].clone()
}

#[test]
fn a_keyboard_effect_is_one_command_and_lands_as_shown() {
    let fx = Fx::new();
    let text = copy_and_run(
        &fx,
        json!({"what": "keyboard", "effect": "wave", "colour": "ff0000", "speed": 7, "brightness": 30, "direction": 4}),
    );
    assert_eq!(text, "truesense keyboard effect wave --speed 7 --brightness 30 --direction clockwise");
    let look = keyboard_look(&fx);
    assert_eq!(
        (look["name"].clone(), look["speed"].clone(), look["brightness"].clone(), look["direction"].clone()),
        (json!("wave"), json!(7), json!(30), json!(4))
    );

    let text = copy_and_run(
        &fx,
        json!({"what": "keyboard", "effect": "breathing", "colour": "#11AA33", "speed": 3, "brightness": 50}),
    );
    assert_eq!(text, "truesense keyboard effect breathing --color 11aa33 --speed 3 --brightness 50");
    let look = keyboard_look(&fx);
    assert_eq!((look["name"].clone(), look["color"].clone()), (json!("breathing"), json!("11aa33")));

    // A static colour has no speed; off is its own command.
    assert_eq!(
        copy_and_run(
            &fx,
            json!({"what": "keyboard", "effect": "static", "colour": "0000ff", "brightness": 20})
        ),
        "truesense keyboard effect static --color 0000ff --brightness 20"
    );
    assert_eq!(copy_and_run(&fx, json!({"what": "keyboard_off"})), "truesense keyboard off");
}

#[test]
fn the_rear_bar_is_one_command_with_its_three_parts() {
    let fx = Fx::new();
    let text = copy_and_run(
        &fx,
        json!({"what": "rear", "on": true, "effect": "static", "colour": "00aec7",
        "zones": ["ff0000", "00ff00", "0000ff"], "brightness": 80}),
    );
    assert_eq!(text, "truesense rear set static --zones ff0000,00ff00,0000ff --brightness 80");
    // The command line remembers the three parts where the window reads them.
    let r = serde_json::to_value(fx.api.rear_status().unwrap()).unwrap();
    assert_eq!(
        (r["mode"].clone(), r["brightness"].clone(), r["zones"].clone()),
        (json!("static"), json!(80), json!(["ff0000", "00ff00", "0000ff"]))
    );

    let text = copy_and_run(
        &fx,
        json!({"what": "rear", "on": true, "effect": "wave", "colour": "aabbcc", "speed": 4, "brightness": 60, "direction": 0}),
    );
    assert_eq!(text, "truesense rear set wave --color aabbcc --speed 4 --brightness 60 --direction 0");
    let r = json_of(&fx.run("--json rear status").out);
    assert_eq!(
        (r["mode"].clone(), r["speed"].clone(), r["direction"].clone(), r["color"].clone()),
        (json!("wave"), json!(4), json!(0), json!("aabbcc"))
    );

    assert_eq!(copy_and_run(&fx, json!({"what": "rear", "on": false})), "truesense rear off");
    // (`rear off` runs the rear:off action, which keeps the colours for `rear on`; its effect is
    // tested with the actions.)
}

#[test]
fn a_per_key_picture_is_one_command_that_paints_every_key_back() {
    let fx = Fx::new();
    // The page's key picture, built from the machine's own key map.
    let layout = lighting::call("layout", &serde_json::to_value(fx.api.key_layout()).unwrap()).unwrap();
    let keys: Vec<Key> = serde_json::from_value(layout["keys"].clone()).unwrap();
    // A typical design: a dark base, WASD red, the arrows green, a few keys whose names need quotes.
    let mut cells = vec!["101010".to_owned(); 128];
    let paint = |cells: &mut Vec<String>, name: &str, colour: &str| {
        let k = keys.iter().find(|k| k.name == name).unwrap_or_else(|| panic!("{name}"));
        for c in &k.cells {
            cells[*c] = colour.to_owned();
        }
    };
    for k in ["w", "a", "s", "d"] {
        paint(&mut cells, k, "ff0000");
    }
    for k in ["up", "down", "left", "right"] {
        paint(&mut cells, k, "00ff00");
    }
    for k in ["`", "'", "\\", "space"] {
        paint(&mut cells, k, "3366ff");
    }
    let args = json!({"what": "perkey", "cells": cells, "keys": layout["keys"], "brightness": 40});
    let text = copy_and_run(&fx, args);
    assert!(text.starts_with("truesense keyboard keys "), "{text}");
    assert!(
        text.contains(" w=ff0000 ") && text.contains(" '`=3366ff' ") && text.contains(" ''\\''=3366ff' "),
        "{text}"
    );
    assert!(text.ends_with(" --base 101010 --brightness 40"), "{text}");
    // Every key reads back as painted.
    let got = fx.api.keyboard_cells();
    for k in &keys {
        for c in &k.cells {
            assert_eq!(got[*c], cells[*c], "{} (cell {c})", k.name);
        }
    }
    // One colour everywhere is a fill.
    let one = json!({"what": "perkey", "cells": vec!["ffffff"; 128], "keys": layout["keys"]});
    assert_eq!(copy_and_run(&fx, one), "truesense keyboard fill ffffff");
}

#[test]
fn themes_and_looks_are_applied_by_name() {
    let fx = Fx::new();
    assert_eq!(copy_and_run(&fx, json!({"what": "look", "name": "native"})), "truesense look apply native");
    // A theme exactly as the editor asks for it: the page's `theme_request`, then the command.
    let request = lighting::call(
        "theme_request",
        &json!({"name": "aurora", "colourway": 2,
        "values": {"speed": 70}, "speed": 40, "brightness": 90}),
    )
    .unwrap();
    let mut args = request.clone();
    args["what"] = json!("theme");
    let text = copy_and_run(&fx, args);
    assert_eq!(text, "truesense theme apply aurora --colourway 2 --set speed=70 --speed 40 --brightness 90");
    let active = json_of(&fx.run("--json theme list").out)["status"]["active"]["spec"].clone();
    assert_eq!(
        (active["name"].clone(), active["speed"].clone(), active["brightness"].clone()),
        (json!("aurora"), json!(40), json!(90)),
        "{active}"
    );
    // Own colours: one --custom per slot, quoted (a # would start a shell comment).
    let custom = lighting::call(
        "cli",
        &json!({"what": "theme", "name": "aurora", "colourway": "custom",
        "custom": ["ff0000", "00ff00", "0000ff"]}),
    )
    .unwrap();
    assert_eq!(
        custom["text"],
        json!(
            "truesense theme apply aurora --colourway custom --custom '#ff0000' --custom '#00ff00' --custom '#0000ff'"
        )
    );
    let words: Vec<String> =
        custom["words"].as_array().unwrap()[1..].iter().map(|w| w.as_str().unwrap().to_owned()).collect();
    assert_eq!(fx.run_args(&words, "").code, 0);
    assert!(lighting::call("cli", &json!({"what": "theme"})).is_err());
    assert!(lighting::call("cli", &json!({"what": "nope"})).is_err());
    assert!(lighting::call("cli", &json!({"what": "keyboard", "effect": "strobe"})).is_err());
}

#[test]
fn shell_words_survive_a_shell() {
    assert_eq!(lighting::shell_word("wave"), "wave");
    assert_eq!(lighting::shell_word("ff0000,00ff00"), "ff0000,00ff00");
    assert_eq!(lighting::shell_word("'=ff0000"), "''\\''=ff0000'");
    assert_eq!(lighting::shell_word("`=x"), "'`=x'");
    assert_eq!(lighting::shell_word("#ff0000"), "'#ff0000'", "a leading # would start a comment");
    assert_eq!(lighting::shell_word(""), "''");
}
