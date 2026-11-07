use super::Fx;

#[test]
fn profiles_list_with_source_and_description() {
    let fx = Fx::new();
    fx.rig.bundles.add("gaming", "turbo and loud");
    fx.rig.bundles.add("quiet", "");
    let list = fx.api.profiles();
    assert_eq!(list.len(), 2);
    assert_eq!(
        (list[0].name.as_str(), list[0].source.as_str(), list[0].description.as_str()),
        ("gaming", "yours", "turbo and loud")
    );
    assert_eq!(serde_json::to_value(&list[1]).unwrap()["description"], serde_json::json!(""));
    assert_eq!(fx.api.profile_names(), ["gaming", "quiet"]);
    assert!(
        crate::api::Api::profile_toml(&fx.api.profile("gaming").unwrap())
            .contains("description = \"turbo and loud\"")
    );
    assert!(fx.api.profile("nosuch").is_err());
    assert_eq!(fx.api.describe_profile("nosuch"), "");
}

#[test]
fn applying_goes_to_the_profile_store_and_next_steps_on() {
    let fx = Fx::new();
    fx.rig.bundles.add("a", "");
    fx.rig.bundles.add("b", "");
    assert_eq!(fx.api.apply_profile("a").unwrap(), "applied a");
    assert_eq!(fx.api.next_profile().unwrap(), "applied b");
    assert_eq!(fx.api.next_profile().unwrap(), "applied a");
    assert_eq!(fx.rig.bundles.applied(), ["a", "b", "a"]);
    assert!(fx.api.apply_profile("c").is_err());
}

#[test]
fn saving_captures_the_current_state_under_a_name() {
    let fx = Fx::new();
    assert_eq!(
        fx.api.save_profile("", "x").unwrap_err().to_string(),
        "profile names: letters, digits, - and _"
    );
    let file = fx.api.save_profile("mine", "my setup").unwrap();
    assert!(file.ends_with("mine.toml"));
    assert_eq!(fx.api.profiles()[0].description, "my setup");
    fx.api.delete_profile("mine").unwrap();
    assert!(fx.api.delete_profile("mine").is_err());
    assert!(fx.api.profiles().is_empty());
    assert!(fx.api.next_profile().is_err());
}
