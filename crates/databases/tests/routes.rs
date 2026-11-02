use serde_json::json;
use webdock_databases::{DatabaseBindingRoute, DatabaseRouteApproval};
fn approval() -> serde_json::Value {
    json!({"bindingID":"1","bindingRevision":2,"databaseID":"3","databaseRevision":4,"backendID":"5","backendRevision":6,"credentialRevision":7,"sourceClusterID":"8","customerID":"9","projectID":"10","hostname":"db-5.database.example.invalid","port":32000,"caSha256":"ab".repeat(32),"addresses":["192.0.2.10"],"expectedRouteRevision":null,"idempotencyKey":"reviewed-route-fixture"})
}
#[test]
fn reviewed_route_pins_canonical_host_and_literal_unicast_addresses() {
    let value: DatabaseRouteApproval = serde_json::from_value(approval()).unwrap();
    value.validate().unwrap();
    for address in [
        "localhost",
        "127.0.0.1",
        "0.0.0.0",
        "224.0.0.1",
        "255.255.255.255",
        "::1",
        "::",
        "ff02::1",
        "::ffff:127.0.0.1",
        "192.0.2.1/24",
    ] {
        let mut bad = approval();
        bad["addresses"] = json!([address]);
        assert!(
            serde_json::from_value::<DatabaseRouteApproval>(bad)
                .map_or(true, |p| p.validate().is_err()),
            "{address}"
        );
    }
    for hostname in [
        "https://db.example.invalid",
        "db.example.invalid/path",
        "user@db.example.invalid",
        "192.0.2.10",
        "DB.example.invalid",
        "db..example.invalid",
    ] {
        let mut bad = approval();
        bad["hostname"] = json!(hostname);
        assert!(
            serde_json::from_value::<DatabaseRouteApproval>(bad)
                .unwrap()
                .validate()
                .is_err()
        );
    }
    let mut duplicate = approval();
    duplicate["addresses"] = json!(["192.0.2.10", "192.0.2.10"]);
    assert!(
        serde_json::from_value::<DatabaseRouteApproval>(duplicate)
            .unwrap()
            .validate()
            .is_err()
    );
    let mut extra = approval();
    extra["verified"] = json!(true);
    assert!(serde_json::from_value::<DatabaseRouteApproval>(extra).is_err());
}
#[test]
fn route_wire_contains_only_the_approved_public_transport_reference() {
    let route:DatabaseBindingRoute=serde_json::from_value(json!({"id":"1","revision":2,"customerID":"3","projectID":"4","sourceClusterID":"5","backendRevision":6,"hostname":"db-7.database.example.invalid","port":32000,"addresses":["192.0.2.10"]})).unwrap();
    route.validate().unwrap();
    let encoded = serde_json::to_value(route).unwrap();
    assert_eq!(encoded.as_object().unwrap().len(), 9);
    assert!(encoded.get("password").is_none());
}
