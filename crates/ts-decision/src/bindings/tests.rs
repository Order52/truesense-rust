//! Tests of the data model, its validation and the edge function.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};

use super::*;

/// A stand-in for the lighting parser: a look is an object with a non-empty `keyboard` or `rear`.
fn parse(v: &Value) -> Result<String, String> {
    let has = |k: &str| v.get(k).and_then(Value::as_object).is_some_and(|o| !o.is_empty());
    if has("keyboard") || has("rear") {
        Ok(v.to_string())
    } else {
        Err("a look needs keyboard and/or rear settings".to_owned())
    }
}

fn look_data() -> Value {
    json!({"keyboard": {"mode": "effect", "name": "wave", "speed": 3}})
}

fn curve_data() -> Value {
    Curve::builtin("quiet").expect("builtin").to_value()
}

fn bound_look(name: &str) -> Bound {
    Bound::look(name, look_data(), parse).expect("look")
}

fn bound_curve(name: &str) -> Bound {
    Bound::fan_curve(name, &curve_data()).expect("curve")
}

/// Turbo has a look and a curve, eco a look only, quiet a curve only.
fn bindings() -> Bindings {
    let mut b = Bindings::default();
    b.set(Mode::Turbo, Some(bound_look("red")), Some(bound_curve("loud"))).expect("turbo");
    b.set(Mode::Eco, Some(bound_look("dim")), None).expect("eco");
    b.set(Mode::Quiet, None, Some(bound_curve("hush"))).expect("quiet");
    b
}

/// The actions reduced to `look:NAME`, `curve:NAME`, `skip-look:WHY` and `skip-curve:WHY`.
fn kinds(actions: &[Action<String>]) -> Vec<String> {
    actions
        .iter()
        .map(|a| match a {
            Action::ApplyLook { name, .. } => format!("look:{name}"),
            Action::ApplyCurve { name, .. } => format!("curve:{name}"),
            Action::Skip { target: Target::Look, why, .. } => format!("skip-look:{why:?}"),
            Action::Skip { target: Target::FanCurve, why, .. } => format!("skip-curve:{why:?}"),
        })
        .collect()
}

#[test]
fn the_edge_table() {
    use FanMode::{Firmware, Manual, Smart};
    use Mode::{Balanced, Eco, Performance, Quiet, Turbo};
    let table: &[(Option<Mode>, Mode, FanMode, &[&str])] = &[
        // No earlier mode (first observation after start) and no change: nothing, always.
        (None, Turbo, Smart, &[]),
        (None, Eco, Smart, &[]),
        (Some(Turbo), Turbo, Smart, &[]),
        (Some(Eco), Eco, Manual, &[]),
        // A change into a mode with a look and a curve, by the state of the fans.
        (Some(Balanced), Turbo, Smart, &["look:red", "curve:loud"]),
        (Some(Eco), Turbo, Smart, &["look:red", "curve:loud"]),
        (Some(Balanced), Turbo, Firmware, &["look:red", "skip-curve:FanNotSmart"]),
        (Some(Balanced), Turbo, Manual, &["look:red", "skip-curve:ManualFanKept"]),
        // A look only, whatever the fans do.
        (Some(Turbo), Eco, Smart, &["look:dim"]),
        (Some(Turbo), Eco, Manual, &["look:dim"]),
        // A curve only.
        (Some(Eco), Quiet, Smart, &["curve:hush"]),
        (Some(Eco), Quiet, Firmware, &["skip-curve:FanNotSmart"]),
        (Some(Eco), Quiet, Manual, &["skip-curve:ManualFanKept"]),
        // A mode without a binding is left alone: the user's choice is never touched.
        (Some(Turbo), Balanced, Smart, &[]),
        (Some(Quiet), Performance, Manual, &[]),
    ];
    let b = bindings();
    for (prev, now, fan, want) in table {
        let got = kinds(&edge(*prev, *now, *fan, &b, parse));
        assert_eq!(got, *want, "{prev:?} -> {now:?} with {fan:?}");
    }
}

#[test]
fn no_bindings_at_all_means_no_actions() {
    for mode in Mode::ALL {
        assert!(edge(Some(Mode::Balanced), mode, FanMode::Smart, &Bindings::default(), parse).is_empty());
    }
}

#[test]
fn plan_is_the_edge_without_the_change_test() {
    let b = bindings();
    assert_eq!(kinds(&plan(Mode::Turbo, FanMode::Smart, &b, parse)), ["look:red", "curve:loud"]);
    assert!(plan(Mode::Balanced, FanMode::Smart, &b, parse).is_empty());
}

#[test]
fn the_look_comes_first_and_the_curve_is_the_validated_one() {
    let b = bindings();
    let actions = edge(Some(Mode::Eco), Mode::Turbo, FanMode::Smart, &b, parse);
    let [Action::ApplyLook { look, .. }, Action::ApplyCurve { curve, .. }] = actions.as_slice() else {
        panic!("{actions:?}");
    };
    assert_eq!(look, &look_data().to_string());
    assert_eq!(curve, &curves::validate(&curve_data()).expect("valid"));
}

#[test]
fn a_part_that_fails_validation_is_skipped_and_never_applied() {
    // The state file was edited by hand: a look that is not a look, a curve that stops a fan.
    let damaged = json!({
        "turbo": {
            "look": {"name": "red", "data": {"nothing": true}},
            "fan_curve": {"name": "loud", "data": {"cpu": [[40, 0], [60, 0]], "gpu": [[40, 0], [60, 0]]}}
        }
    });
    let b = Bindings::from_value(&damaged).expect("shape is fine, parts are checked on use");
    let got = kinds(&edge(Some(Mode::Eco), Mode::Turbo, FanMode::Smart, &b, parse));
    assert_eq!(got.len(), 2);
    assert!(got[0].starts_with("skip-look:Invalid"), "{got:?}");
    assert!(got[1].starts_with("skip-curve:Invalid"), "{got:?}");
    // A damaged look does not take the good curve down with it.
    let mixed = json!({"turbo": {
        "look": {"name": "red", "data": {"nothing": true}},
        "fan_curve": {"name": "loud", "data": curve_data()}
    }});
    let b = Bindings::from_value(&mixed).expect("decode");
    let got = kinds(&edge(Some(Mode::Eco), Mode::Turbo, FanMode::Smart, &b, parse));
    assert!(got[0].starts_with("skip-look:Invalid") && got[1] == "curve:loud", "{got:?}");
}

#[test]
fn a_native_curve_is_not_a_binding() {
    let mut native = curve_data();
    native["native"] = json!(true);
    assert!(matches!(Bound::fan_curve("n", &native), Err(BindingError::NativeCurve)));
    // And a stored native curve is skipped, not applied, if it got there some other way.
    let stored = json!({"turbo": {"fan_curve": {"name": "n", "data": native}}});
    let b = Bindings::from_value(&stored).expect("decode");
    let got = kinds(&edge(Some(Mode::Eco), Mode::Turbo, FanMode::Smart, &b, parse));
    assert!(got[0].starts_with("skip-curve:Invalid"), "{got:?}");
}

#[test]
fn bound_parts_are_validated_when_made() {
    let bad_curves = [
        json!(null),
        json!([]),
        json!({"cpu": [[40, 30]], "gpu": [[40, 30], [60, 50]]}),
        json!({"cpu": [[40, 5], [95, 100]], "gpu": [[40, 30], [85, 100]]}), // a duty of 5 %
        json!({"cpu": [[40, 30], [60, 50]], "gpu": [[40, 30], [85, 100]]}), // not full by 95 C
        json!({"cpu": "fast", "gpu": "fast"}),
    ];
    for data in &bad_curves {
        assert!(matches!(Bound::fan_curve("x", data), Err(BindingError::Curve(_))), "{data}");
    }
    for name in ["", "a b", "a/b", "../x", "caf\u{e9}", &"n".repeat(41)] {
        assert!(matches!(
            Bound::fan_curve(name, &curve_data()),
            Err(BindingError::BadName(Target::FanCurve))
        ));
        assert!(matches!(Bound::look(name, look_data(), parse), Err(BindingError::BadName(Target::Look))));
    }
    assert!(Bound::fan_curve(&"n".repeat(40), &curve_data()).is_ok());
    assert!(matches!(Bound::look("x", json!({}), parse), Err(BindingError::Look(_))));
    assert!(matches!(Bound::look("x", json!("no"), parse), Err(BindingError::Look(_))));
    let huge = json!({"keyboard": {"cells": vec!["000000"; 4000]}});
    assert!(matches!(Bound::look("x", huge, parse), Err(BindingError::LookTooLarge)));
}

#[test]
fn a_curve_copy_is_stored_in_its_normal_form() {
    // Defaults filled in, so what is stored is what will run.
    let sparse = json!({"cpu": [[60, 50], [40, 30], [95, 100]], "gpu": [[40, 30], [85, 100]]});
    let bound = Bound::fan_curve("mine", &sparse).expect("valid");
    let curve = curves::validate(&bound.data).expect("round trip");
    assert_eq!(curve.cpu.first().map(|p| p.temp), Some(40.0), "sorted");
    assert!(bound.data.get("hysteresis").is_some());
}

#[test]
fn set_merges_parts_and_needs_at_least_one() {
    let mut b = Bindings::default();
    assert!(matches!(b.set(Mode::Eco, None, None), Err(BindingError::Nothing)));
    assert!(b.is_empty(), "a refused set leaves nothing behind");
    b.set(Mode::Eco, Some(bound_look("a")), None).expect("look");
    b.set(Mode::Eco, None, Some(bound_curve("c"))).expect("curve");
    let eco = b.get(Mode::Eco).expect("eco");
    assert_eq!(eco.look.as_ref().map(|l| l.name.as_str()), Some("a"));
    assert_eq!(eco.fan_curve.as_ref().map(|l| l.name.as_str()), Some("c"));
    b.set(Mode::Eco, Some(bound_look("b")), None).expect("replace the look");
    let eco = b.get(Mode::Eco).expect("eco");
    assert_eq!(eco.look.as_ref().map(|l| l.name.as_str()), Some("b"));
    assert!(eco.fan_curve.is_some(), "the curve stays");
}

#[test]
fn clear_removes_parts_and_drops_empty_modes() {
    let mut b = bindings();
    assert!(b.clear(Mode::Turbo, Clear::Look));
    assert!(!b.clear(Mode::Turbo, Clear::Look), "nothing left to clear");
    assert!(b.get(Mode::Turbo).is_some_and(|t| t.look.is_none() && t.fan_curve.is_some()));
    assert!(b.clear(Mode::Turbo, Clear::FanCurve));
    assert!(b.get(Mode::Turbo).is_none(), "an empty mode is dropped");
    assert!(b.clear(Mode::Eco, Clear::All));
    assert!(!b.clear(Mode::Balanced, Clear::All), "an unbound mode");
    assert_eq!(b.iter().map(|(m, _)| m).collect::<Vec<_>>(), [Mode::Quiet]);
}

#[test]
fn refresh_replaces_every_copy_with_that_name_and_nothing_else() {
    let mut b = Bindings::default();
    b.set(Mode::Eco, Some(bound_look("same")), None).expect("eco");
    b.set(Mode::Turbo, Some(bound_look("same")), Some(bound_curve("same"))).expect("turbo");
    b.set(Mode::Quiet, Some(bound_look("other")), None).expect("quiet");
    let fresh =
        Bound::look("same", json!({"rear": {"mode": "static", "color": "ff0000"}}), parse).expect("fresh");
    assert_eq!(b.refresh(Target::Look, &fresh), 2);
    for mode in [Mode::Eco, Mode::Turbo] {
        assert_eq!(b.get(mode).and_then(|x| x.look.as_ref()), Some(&fresh));
    }
    assert_eq!(b.get(Mode::Quiet).and_then(|x| x.look.as_ref()), Some(&bound_look("other")));
    assert_eq!(b.get(Mode::Turbo).and_then(|x| x.fan_curve.as_ref()), Some(&bound_curve("same")));
    assert_eq!(b.refresh(Target::Look, &bound_look("absent")), 0);
}

#[test]
fn the_stored_form_round_trips() {
    let b = bindings();
    let value = b.to_value();
    assert_eq!(value["turbo"]["look"]["name"], json!("red"));
    assert_eq!(value["eco"]["fan_curve"], Value::Null, "an absent part is null on the wire");
    assert!(value.get("balanced").is_none());
    assert_eq!(Bindings::from_value(&value).expect("decode"), b);
}

#[test]
fn decoding_is_strict_about_shape_and_forgiving_about_empties() {
    assert!(Bindings::from_value(&Value::Null).expect("null").is_empty());
    assert!(Bindings::from_value(&json!({})).expect("empty").is_empty());
    assert!(Bindings::from_value(&json!({"turbo": {}})).expect("empty mode").is_empty());
    assert!(
        Bindings::from_value(&json!({"turbo": {"look": null, "fan_curve": null}})).expect("nulls").is_empty()
    );
    let hostile = [
        json!([]),
        json!("turbo"),
        json!(7),
        json!({"turbo": []}),
        json!({"turbo": {"look": "red"}}),
        json!({"turbo": {"look": {"name": 1, "data": {}}}}),
        json!({"turbo": {"look": {"name": "x"}}}),
        json!({"turbo": {"extra": 1}}),
        json!({"turbo": {"look": {"name": "x", "data": {}, "extra": 1}}}),
    ];
    for value in &hostile {
        assert!(matches!(Bindings::from_value(value), Err(BindingError::Malformed(_))), "{value}");
    }
    for mode in ["Turbo", "ludicrous", "", "turbo "] {
        let value = json!({ mode: {"look": {"name": "x", "data": look_data()}} });
        assert!(matches!(Bindings::from_value(&value), Err(BindingError::UnknownMode(_))), "{mode:?}");
    }
}

#[test]
fn messages_read_well() {
    assert_eq!(
        BindingError::UnknownMode("x".to_owned()).to_string(),
        "unknown power mode 'x' (use eco, quiet, balanced, performance or turbo)"
    );
    assert_eq!(BindingError::Nothing.to_string(), "a binding needs a look and/or a fan curve");
    assert_eq!(Skip::ManualFanKept.to_string(), "a manual fan setting is held");
    assert_eq!(Target::FanCurve.to_string(), "fan curve");
    assert_eq!(Target::parse(Target::FanCurve.key()), Some(Target::FanCurve));
    assert_eq!(Clear::parse("all"), Some(Clear::All));
    assert_eq!(Clear::parse("both"), None);
}
