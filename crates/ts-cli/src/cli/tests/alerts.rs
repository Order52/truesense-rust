//! `truesense alerts show` and `alerts set`: the words, the output, and that a bad value writes
//! nothing. The command is new in 0.5, so there is no Python transcript to compare with.

use serde_json::{Value, json};

use super::Fx;

#[test]
fn show_prints_the_settings_as_lines_and_as_json() {
    let fx = Fx::offline();
    let ran = fx.run("alerts show");
    assert_eq!(ran.code, 0, "{}", ran.err);
    assert!(ran.out.starts_with("alerts: on\n  CPU limit      90 C"), "{}", ran.out);
    assert!(ran.out.contains("battery low    15 %"), "{}", ran.out);
    let json: Value = serde_json::from_str(&fx.run("--json alerts show").out).unwrap();
    assert_eq!(json["cpu_limit_c"], json!(90));
    assert_eq!(json["enabled"], json!(true));
    // The bare area is `show`.
    assert_eq!(fx.run("alerts").out, ran.out);
}

#[test]
fn set_changes_only_the_given_options() {
    let fx = Fx::offline();
    let ran = fx.run("alerts set --cpu-limit 85 --enabled off --battery-low 0 --sensor-lost off");
    assert_eq!(ran.code, 0, "{}", ran.err);
    assert!(ran.out.starts_with("alerts: off\n  CPU limit      85 C"), "{}", ran.out);
    assert!(ran.out.contains("battery low    off") && ran.out.contains("sensor lost    off"), "{}", ran.out);
    let text = fx.user_config();
    assert!(text.contains("cpu_limit_c = 85") && text.contains("sensor_lost = false"), "{text}");
    assert!(!text.contains("gpu_limit_c"), "{text}");
    let json: Value =
        serde_json::from_str(&fx.run("--json alerts set --gpu-limit 80 --throttling on").out).unwrap();
    assert_eq!(
        (json["gpu_limit_c"].clone(), json["cpu_limit_c"].clone(), json["throttling"].clone()),
        (json!(80), json!(85), json!(true))
    );
}

#[test]
fn set_refuses_bad_values_and_nothing_to_change() {
    let fx = Fx::offline();
    let ran = fx.run("alerts set --cpu-limit 50");
    assert_eq!(ran.code, 1);
    assert!(ran.err.contains("error: [alerts] cpu_limit_c must be 70..100"), "{}", ran.err);
    let ran = fx.run("alerts set");
    assert_eq!((ran.code, ran.err.as_str()), (1, "error: alerts set: nothing to change\n"));
    assert_eq!(fx.user_config(), "");
    // The parser's own refusals: a word that is not a number, a choice that is not offered.
    let ran = fx.run("alerts set --cpu-limit hot");
    assert_eq!(ran.code, 2, "{}", ran.err);
    let ran = fx.run("alerts set --enabled maybe");
    assert_eq!(ran.code, 2, "{}", ran.err);
    assert!(ran.err.contains("invalid choice: 'maybe'"), "{}", ran.err);
    assert_eq!(fx.run("alerts nope").code, 2);
}

#[test]
fn the_area_is_in_the_command_table_and_the_completions() {
    assert!(crate::cli::command_table().contains(&"truesense alerts set".to_owned()));
    let fx = Fx::offline();
    let bash = fx.run("completion bash").out;
    assert!(bash.contains("alerts) if"), "{bash}");
    assert!(bash.contains("--cpu-limit") && bash.contains("--sensor-lost"), "{bash}");
}
