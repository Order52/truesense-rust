//! `keyboard effect wave --direction ...`: the six wave directions by name or number.

use serde_json::Value;

use super::Fx;

fn look(fx: &Fx) -> Value {
    let v: Value = serde_json::from_str(&fx.run("--json keyboard status").out).expect("json");
    v["look"].clone()
}

#[test]
fn every_direction_name_and_number_is_stored_as_its_logical_number() {
    for (arg, want) in [
        ("left-to-right", 0),
        ("right-to-left", 1),
        ("bottom-to-top", 2),
        ("top-to-bottom", 3),
        ("clockwise", 4),
        ("counter-clockwise", 5),
        ("cw", 4),
        ("ccw", 5),
        ("0", 0),
        ("4", 4),
        ("5", 5),
    ] {
        let fx = Fx::offline();
        let ran = fx.run(&format!("keyboard effect wave --direction {arg}"));
        assert_eq!((ran.code, ran.out.as_str()), (0, "sent\n"), "{arg}: {}", ran.err);
        let look = look(&fx);
        assert_eq!(look["name"], "wave");
        assert_eq!(look["direction"], want, "{arg}");
    }
}

#[test]
fn the_equals_form_and_the_default_work() {
    let fx = Fx::offline();
    assert_eq!(fx.run("keyboard effect wave --direction=counter-clockwise").code, 0);
    assert_eq!(look(&fx)["direction"], 5);
    assert_eq!(fx.run("keyboard effect wave").code, 0);
    assert_eq!(look(&fx)["direction"], 0, "left to right unless asked");
}

#[test]
fn a_wrong_direction_is_refused_with_what_is_allowed_and_nothing_changes() {
    let fx = Fx::offline();
    for bad in ["sideways", "6", "clock-wise-ish"] {
        let ran = fx.run(&format!("keyboard effect wave --direction {bad}"));
        assert_eq!(ran.code, 1, "{bad}");
        assert!(ran.err.contains("0..5") && ran.err.contains("counter-clockwise"), "{bad}: {}", ran.err);
    }
    assert_eq!(look(&fx), serde_json::json!({}), "nothing was sent");
}

#[test]
fn another_effect_keeps_the_numeric_rule() {
    let fx = Fx::offline();
    assert_eq!(fx.run("keyboard effect snake --direction 7").code, 0);
    assert_eq!(look(&fx)["direction"], 7);
    assert_eq!(fx.run("keyboard effect snake --direction 255").code, 1);
}

#[test]
fn the_help_names_the_six() {
    let fx = Fx::offline();
    let out = fx.run("keyboard effect -h").out;
    let flat = out.split_whitespace().collect::<Vec<_>>().join(" ");
    for name in ["clockwise", "counter-clockwise", "left-to-right"] {
        assert!(flat.contains(name), "{name}");
    }
}

#[test]
fn the_rotating_sweeps_are_per_key_animations_on_the_command_line() {
    let fx = Fx::offline();
    for mode in ["wave-clockwise", "wave-counter-clockwise"] {
        let ran = fx.run(&format!("keyboard fx {mode} --speed 7 --keys wasd,arrows"));
        assert_eq!(ran.code, 0, "{mode}: {}", ran.err);
        assert_eq!(
            ran.out,
            format!(
                "per-key animation: {mode} on wasd,arrows (needs the key service: systemctl --user status truesense-keys)\n"
            )
        );
    }
    let all = fx.run("keyboard fx wave-clockwise");
    assert!(all.out.contains("on all keys"), "{}", all.out);
    assert_eq!(fx.run("keyboard fx wave-sideways").code, 2, "an unknown mode is a usage error");
    assert_eq!(fx.run("keyboard fx wave-clockwise --speed 11").code, 1);
    assert_eq!(fx.run("keyboard fx off").code, 0);
}
