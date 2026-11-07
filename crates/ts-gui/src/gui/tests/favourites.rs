//! The `favourite` commands of the bus against the simulated laptop: the list with its `exists`
//! flag, the star toggle, reordering, and applying by position or by item.

use serde_json::json;

use super::harness::Machine;

fn names(list: &serde_json::Value) -> Vec<String> {
    list.as_array()
        .expect("a list")
        .iter()
        .map(|f| {
            format!(
                "{}:{}:{}:{}",
                f["position"],
                f["kind"].as_str().unwrap_or("?"),
                f["name"].as_str().unwrap_or("?"),
                f["exists"]
            )
        })
        .collect()
}

#[test]
fn the_list_starts_empty_and_follows_the_stars() {
    let m = Machine::start();
    m.rig.bundles.add("gaming", "");
    assert_eq!(m.ok("favourite", "list", &json!({})), json!([]));
    assert_eq!(
        m.ok("favourite", "toggle", &json!({"kind": "profile", "name": "gaming"})),
        json!({"starred": true})
    );
    assert_eq!(
        m.ok("favourite", "toggle", &json!({"kind": "look", "name": "native"})),
        json!({"starred": true})
    );
    assert_eq!(
        names(&m.ok("favourite", "list", &json!({}))),
        ["1:profile:gaming:true", "2:look:native:true"]
    );
    assert_eq!(
        m.ok("favourite", "toggle", &json!({"kind": "profile", "name": "gaming"})),
        json!({"starred": false})
    );
    assert_eq!(names(&m.ok("favourite", "list", &json!({}))), ["1:look:native:true"]);
}

#[test]
fn a_deleted_item_stays_listed_as_missing_until_removed() {
    let m = Machine::start();
    m.rig.bundles.add("gaming", "");
    m.ok("favourite", "toggle", &json!({"kind": "profile", "name": "gaming"}));
    m.ok("profile", "delete", &json!({"name": "gaming"}));
    assert_eq!(names(&m.ok("favourite", "list", &json!({}))), ["1:profile:gaming:false"]);
    assert!(m.refused("favourite", "apply", &json!({"position": 1})).contains("gaming"));
    assert_eq!(
        m.ok("favourite", "remove", &json!({"kind": "profile", "name": "gaming"})),
        json!({"removed": true})
    );
    assert_eq!(
        m.ok("favourite", "remove", &json!({"kind": "profile", "name": "gaming"})),
        json!({"removed": false})
    );
    assert_eq!(m.ok("favourite", "list", &json!({})), json!([]));
}

#[test]
fn starring_a_missing_item_or_a_wrong_kind_is_refused_with_a_message() {
    let m = Machine::start();
    assert_eq!(
        m.refused("favourite", "toggle", &json!({"kind": "look", "name": "ghost"})),
        "no saved look 'ghost' (truesense look list)"
    );
    assert_eq!(
        m.refused("favourite", "toggle", &json!({"kind": "undervolt", "name": "x"})),
        "'undervolt': the kind is one of look, fan_curve, smart_profile, profile"
    );
    assert_eq!(m.refused("favourite", "toggle", &json!({"kind": "look"})), "name is needed");
    assert_eq!(m.refused("favourite", "toggle", &json!({"name": "native"})), "kind is needed");
    assert_eq!(
        m.refused("favourite", "toggle", &json!({"kind": 3, "name": "native"})),
        "kind: wrong kind of value"
    );
    assert_eq!(m.ok("favourite", "list", &json!({})), json!([]));
}

#[test]
fn move_reorders_with_positions_from_one_and_answers_the_new_list() {
    let m = Machine::start();
    for n in ["a", "b", "c"] {
        m.rig.bundles.add(n, "");
        m.ok("favourite", "toggle", &json!({"kind": "profile", "name": n}));
    }
    let moved = m.ok("favourite", "move", &json!({"from": 3, "to": 1}));
    assert_eq!(names(&moved), ["1:profile:c:true", "2:profile:a:true", "3:profile:b:true"]);
    assert_eq!(names(&m.ok("favourite", "list", &json!({}))), names(&moved));
    assert_eq!(m.refused("favourite", "move", &json!({"from": 0, "to": 1})), "positions count from 1");
    assert_eq!(m.refused("favourite", "move", &json!({"from": -1, "to": 1})), "from: positions count from 1");
    assert_eq!(
        m.refused("favourite", "move", &json!({"from": 1, "to": 9})),
        "position out of range: favourites are 1..3"
    );
    assert_eq!(m.refused("favourite", "move", &json!({"from": 1})), "to is needed");
}

#[test]
fn apply_by_position_or_by_item_goes_through_the_librarys_own_operation() {
    let m = Machine::start();
    m.rig.bundles.add("gaming", "");
    m.rig.daemon.answer_ok("fan_curve", json!({"name": "truesense"}));
    m.ok("favourite", "toggle", &json!({"kind": "profile", "name": "gaming"}));
    m.ok("favourite", "toggle", &json!({"kind": "curve", "name": "truesense"}));
    assert_eq!(m.ok("favourite", "apply", &json!({"position": 1})), json!("Favourite 1/2: Profile gaming"));
    assert_eq!(m.rig.bundles.applied(), ["gaming"]);
    assert_eq!(
        m.ok("favourite", "apply", &json!({"position": 2})),
        json!("Favourite 2/2: Fan curve truesense")
    );
    assert_eq!(
        m.rig.daemon.last_args("fan_curve").and_then(|a| a.get("action").cloned()),
        Some(json!("use"))
    );
    assert_eq!(m.ok("favourite", "apply", &json!({"kind": "look", "name": "native"})), json!("Look: native"));
    assert_eq!(
        m.refused("favourite", "apply", &json!({"position": 3})),
        "no favourite 3: you have 2 (the most is 24)"
    );
    assert_eq!(m.refused("favourite", "apply", &json!({})), "kind is needed");
}

#[test]
fn the_list_is_capped_at_twenty_four() {
    let m = Machine::start();
    for i in 0..30 {
        let name = format!("p{i}");
        m.rig.bundles.add(&name, "");
        let (status, body) = m.run("favourite", "toggle", &json!({"kind": "profile", "name": name}));
        if i < 24 {
            assert_eq!(status, 200, "{body}");
        } else {
            assert_eq!(status, 422, "{body}");
            assert_eq!(body["error"]["message"], json!("favourites are full (24): remove one first"));
        }
    }
    assert_eq!(m.ok("favourite", "list", &json!({})).as_array().expect("list").len(), 24);
}
