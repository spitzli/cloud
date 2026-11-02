use serde_json::json;
use webdock_registry::contracts::{customer_input, project_input, record_id};
#[test]
fn ids_are_canonical_positive_signed_63_bit_strings() {
    for good in ["1", "9223372036854775807"] {
        assert!(record_id(good).is_ok());
    }
    for bad in ["", "0", "01", "-1", "1.0", "9223372036854775808", "１２"] {
        assert!(record_id(bad).is_err(), "{bad}");
    }
}
#[test]
fn customer_boundary_is_strict_and_preserves_omitted_vs_null() {
    let value =
        customer_input(json!({"name":"  Example  ","country":" ch ","notes":null})).unwrap();
    assert_eq!(value, json!({"name":"Example","country":"CH","notes":null}));
    assert!(value.get("contactName").is_none());
    for bad in [
        json!({"name":" "}),
        json!({"name":"x", "status":"archived"}),
        json!({"name":"x","id":"1"}),
        json!({"name":"x","customerType":null}),
        json!({"name":"x","country":"CHE"}),
        json!({"name":"x","contactEmail":"bad"}),
        json!({"name":7}),
    ] {
        assert!(customer_input(bad.clone()).is_err(), "{bad}");
    }
}
#[test]
fn project_urls_are_https_without_credentials_and_ownership_is_required() {
    assert!(
        project_input(
            json!({"name":"x","customer":"1","url":"https://example.invalid","repositoryURL":null})
        )
        .is_ok()
    );
    for bad in [
        json!({"name":"x","customer":1}),
        json!({"name":"x"}),
        json!({"name":"x","customer":"1","url":"http://example.invalid"}),
        json!({"name":"x","customer":"1","url":"https://u:p@example.invalid"}),
        json!({"name":"x","customer":"1","status":"archived"}),
    ] {
        assert!(project_input(bad.clone()).is_err(), "{bad}");
    }
}

#[test]
fn list_input_defaults_and_numeric_json_match_public_contract() {
    use webdock_registry::contracts::ListInput;
    let input =
        ListInput::parse(json!({"collection":"customers","page":1.0,"limit":20.0})).unwrap();
    assert_eq!(input.sort, "-updatedAt");
    for bad in [
        json!({"collection":"customers","page":0}),
        json!({"collection":"customers","limit":51}),
        json!({"collection":"customers","page":"1"}),
        json!({"collection":"customers","search":null}),
        json!({"collection":"customers","where":{}}),
    ] {
        assert!(ListInput::parse(bad).is_err());
    }
}
#[test]
fn retained_zod_email_and_ecmascript_trim_edges_match() {
    assert!(customer_input(json!({"name":"x","contactEmail":"a@domain-.invalid"})).is_ok());
    assert!(customer_input(json!({"name":"x","contactEmail":"a'@domain.invalid"})).is_err());
    assert_eq!(
        customer_input(json!({"name":"\u{0085}x\u{0085}"})).unwrap()["name"],
        "\u{0085}x\u{0085}"
    );
    assert_eq!(
        customer_input(json!({"name":"\u{feff}x\u{feff}"})).unwrap()["name"],
        "x"
    );
}
