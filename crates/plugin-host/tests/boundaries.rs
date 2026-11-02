use serde_json::json;
use webdock_plugin_host::{data::validate_write, http::validate_path};
#[test]
fn document_boundary_rejects_unscoped_or_invalid_writes() {
    assert!(validate_write(json!({"key":"notes.v1","value":null,"expectedRevision":null,"idempotencyKey":"request-1"})).is_ok());
    for value in [
        json!({"key":"../secret","value":{},"expectedRevision":null,"idempotencyKey":"request-1"}),
        json!({"key":"notes","value":{},"expectedRevision":1,"idempotencyKey":"request-1"}),
        json!({"key":"notes","value":{},"expectedRevision":null,"idempotencyKey":"short"}),
        json!({"key":"notes","value":{},"expectedRevision":null,"idempotencyKey":"request-1","projectID":"other"}),
        json!({"key":"notes","value":"x".repeat(65537),"expectedRevision":null,"idempotencyKey":"request-1"}),
    ] {
        assert!(validate_write(value).is_err());
    }
}
#[test]
fn http_paths_cannot_escape_exact_origin_or_hide_segments() {
    for path in [
        "//evil.invalid/x",
        "https://evil.invalid",
        "/a/../b",
        "/a/./b",
        "/%2e%2e/admin",
        "/a\\b",
        "/a#secret",
        "/a?x=%2f",
        "/a b",
        "",
    ] {
        assert!(validate_path(path).is_err(), "{path}");
    }
    assert!(validate_path("/v1/items?limit=10").is_ok());
}
#[test]
fn retained_node_receipt_hashes_match_locale_numbers_and_key_order() {
    use sha2::{Digest, Sha256};
    let fixtures: serde_json::Value =
        serde_json::from_str(include_str!("node-canonical.json")).unwrap();
    for fixture in fixtures.as_array().unwrap() {
        let canonical =
            webdock_plugin_host::data::javascript_json(&fixture["input"], true).unwrap();
        assert_eq!(canonical, fixture["canonical"].as_str().unwrap());
        assert_eq!(hex::encode(Sha256::digest(canonical)), fixture["sha256"]);
    }
    assert_eq!(
        webdock_plugin_host::data::javascript_json(&serde_json::from_str("-0").unwrap(), false)
            .unwrap(),
        "0"
    );
}

#[test]
fn stored_v1_values_match_the_same_javascript_numbers_used_by_receipt_hashes() {
    let raw = json!({"key":"large-number","value":{"number":9007199254740993_u64},"expectedRevision":null,"idempotencyKey":"roundtrip-1"});
    let normalized = validate_write(raw).unwrap();
    assert_eq!(normalized["value"]["number"], json!(9007199254740992_u64));
}
