#![cfg(feature = "serde")]

use maylily::{Error, Maylily, Options};

const CONFIG: &str = r#"{"radix":16,"time_base":100,"machine_id":1,"machine_bits":2,"generator_id":2,"generator_bits":2,"sequence_bits":2}"#;

#[test]
fn configuration_roundtrips_and_controls_generated_ids() {
    let options: Options = serde_json::from_str(CONFIG).unwrap();
    let json = serde_json::to_string(&options).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&json).unwrap(),
        serde_json::from_str::<serde_json::Value>(CONFIG).unwrap()
    );
    let mut generator = Maylily::new(options).unwrap();
    assert_eq!(generator.generate_at(101).unwrap(), "58");
    assert_eq!(generator.generate_at(101).unwrap(), "59");
}

#[test]
fn configuration_rejects_typo_fields_missing_identity_and_wrong_types() {
    for json in [
        CONFIG.replace("\"radix\":16", "\"radix\":16,\"generator_bit\":2"),
        CONFIG.replace("\"generator_id\":2,", ""),
        CONFIG.replace("\"machine_id\":1", "\"machine_id\":-1"),
        CONFIG.replace("\"machine_bits\":2", "\"machine_bits\":256"),
        CONFIG.replace("\"radix\":16", "\"radix\":\"16\""),
    ] {
        assert!(serde_json::from_str::<Options>(&json).is_err(), "{json}");
    }
}

#[test]
fn deserialized_options_still_pass_through_generation_validation() {
    let options: Options =
        serde_json::from_str(&CONFIG.replace("\"machine_id\":1", "\"machine_id\":4")).unwrap();
    assert!(matches!(
        Maylily::new(options),
        Err(Error::IdentityOverflow)
    ));
    assert_eq!(options.pack_at(101, 0), Err(Error::IdentityOverflow));
}
