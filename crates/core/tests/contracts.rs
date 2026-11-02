use std::collections::HashMap;
use webdock_core::{
    config::PlatformConfig,
    features::{FeatureSet, OperationKind},
    id::RecordId,
};

#[test]
fn identifiers_preserve_full_signed_bigint_range_as_json_strings() {
    let id = RecordId::parse("9223372036854775807").unwrap();
    assert_eq!(
        serde_json::to_string(&id).unwrap(),
        "\"9223372036854775807\""
    );
    for invalid in [
        "0",
        "01",
        "-1",
        "1.2",
        "1e3",
        " 1",
        "9223372036854775808",
        "１２",
    ] {
        assert!(RecordId::parse(invalid).is_err(), "accepted {invalid}");
    }
    assert!(serde_json::from_str::<RecordId>("123").is_err());
    assert!(serde_json::from_str::<RecordId>("\"0\"").is_err());
}

#[test]
fn features_fail_closed_and_draining_allows_only_reads_and_accepted_work() {
    let empty = FeatureSet::default();
    assert!(empty.require("hosting", OperationKind::Read).is_err());
    let features = FeatureSet::from_json(r#"{"hosting":"draining","mail":"enabled"}"#).unwrap();
    assert!(features.require("hosting", OperationKind::Read).is_ok());
    assert!(
        features
            .require("hosting", OperationKind::ContinueAccepted)
            .is_ok()
    );
    assert!(
        features
            .require("hosting", OperationKind::Provision)
            .is_err()
    );
    assert!(features.require("mail", OperationKind::Provision).is_ok());
    assert!(features.require("arbitrary", OperationKind::Read).is_err());
    assert!(FeatureSet::from_json(r#"{"lumaserv":"enabled"}"#).is_err());
    assert!(FeatureSet::from_json(r#"{"hosting":"enable"}"#).is_err());
}

#[test]
fn canonical_origins_reject_credentials_paths_and_untrusted_http() {
    let default = PlatformConfig::from_map(&HashMap::new()).unwrap();
    assert_eq!(default.api_origin, "http://localhost:3130");
    assert_eq!(default.port, 3130);
    for invalid in [
        "http://example.com",
        "https://user:pass@example.com",
        "https://example.com/path",
        "https://example.com/?q=1",
        "https://example.com/#x",
        "ftp://example.com",
    ] {
        let env = HashMap::from([("WEBDOCK_API_ORIGIN".into(), invalid.into())]);
        assert!(
            PlatformConfig::from_map(&env).is_err(),
            "accepted {invalid}"
        );
    }
}

#[test]
fn retired_database_ordering_is_disabled_unless_explicitly_enabled() {
    assert!(
        !PlatformConfig::from_map(&HashMap::new())
            .unwrap()
            .database_new_orders_enabled
    );
    for (raw, enabled) in [("true", true), ("false", false)] {
        let config = PlatformConfig::from_map(&HashMap::from([(
            "WEBDOCK_DATABASE_NEW_ORDERS_ENABLED".into(),
            raw.into(),
        )]))
        .unwrap();
        assert_eq!(config.database_new_orders_enabled, enabled);
    }
    for invalid in ["TRUE", "1", "", " true", "false "] {
        assert!(
            PlatformConfig::from_map(&HashMap::from([(
                "WEBDOCK_DATABASE_NEW_ORDERS_ENABLED".into(),
                invalid.into()
            )]))
            .is_err()
        );
    }
}

#[test]
fn production_requires_explicit_secure_distinct_origins_and_valid_ports() {
    assert!(
        PlatformConfig::from_map(&HashMap::from([("NODE_ENV".into(), "production".into())]))
            .is_err()
    );
    for invalid in ["0", "65536", "-1", "3.5", ""] {
        assert!(
            PlatformConfig::from_map(&HashMap::from([("PORT".into(), invalid.into())])).is_err()
        );
    }
    let env = HashMap::from([("WEBDOCK_API_ORIGIN".into(), "http://localhost:3125".into())]);
    assert!(PlatformConfig::from_map(&env).is_err());
}
