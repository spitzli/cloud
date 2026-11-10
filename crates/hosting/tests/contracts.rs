use serde_json::json;
use spitzli_hosting::contracts::{app_demand, parse_command, resolve_spec};
fn spec() -> serde_json::Value {
    json!({"template":"custom","image":format!("registry.invalid/app@sha256:{}","a".repeat(64)),"args":[],"port":8080,"healthPath":"/health","cpuMillicores":100,"memoryBytes":16777216,"ephemeralBytes":1048576,"replicas":1,"volumeBytes":0})
}
#[test]
fn probe_host_is_an_optional_canonical_authority_in_strict_app_commands() {
    assert!(resolve_spec(spec()).unwrap().get("healthHost").is_none());
    for host in ["api.example.test", "api.example.test:3130"] {
        let mut value = spec();
        value["healthHost"] = json!(host);
        assert_eq!(resolve_spec(value.clone()).unwrap()["healthHost"], host);
        parse_command(json!({"action":"apps.create","projectID":"1","name":"Core","spec":value,"subscriptionRevision":1,"idempotencyKey":"probe-host-create"})).unwrap();
    }
    for host in [
        "api.test/path",
        "api.test\r\nX-Test: yes",
        "user@api.test",
        "API.test",
        "api.test:65536",
    ] {
        let mut value = spec();
        value["healthHost"] = json!(host);
        assert!(resolve_spec(value).is_err(), "{host:?}");
    }
}
#[test]
fn strict_commands_do_not_accept_authority_or_mutation_fields() {
    assert!(parse_command(json!({"action":"apps.list","projectID":"1"})).is_ok());
    for input in [
        json!({"action":"apps.get","appID":1}),
        json!({"action":"apps.get","appID":"1","operator":true}),
        json!({"action":"apps.create","projectID":"1","name":"x","spec":spec(),"subscriptionRevision":0,"idempotencyKey":"short"}),
    ] {
        assert!(parse_command(input).is_err());
    }
}
#[test]
fn storage_is_retained_when_deleted_and_not_multiplied_by_replicas() {
    let mut value = spec();
    value["volumeBytes"] = json!(67108864);
    let demand = app_demand(&value, true).unwrap();
    assert_eq!(demand["apps"], 0);
    assert_eq!(demand["cpuMillicores"], 0);
    assert_eq!(demand["volumeBytes"], 67108864);
    value["replicas"] = json!(2);
    assert!(resolve_spec(value).is_err());
}
#[test]
fn healthcheck_uses_pinned_maintained_defaults() {
    let mut value = spec();
    value["template"] = json!("healthcheck");
    let resolved = resolve_spec(value).unwrap();
    assert_eq!(resolved["healthPath"], "/ping");
    assert!(
        resolved["image"]
            .as_str()
            .unwrap()
            .starts_with("docker.io/rancher/")
    );
}

#[test]
fn public_addresses_are_explicit_and_cannot_supply_route_authority() {
    let base = json!({"action":"apps.create","projectID":"1","name":"App","spec":spec(),"subscriptionRevision":1,"idempotencyKey":"public-address-create"});
    parse_command(base.clone()).unwrap();
    for public in [json!({}), json!({"label":"lunares"})] {
        let mut value = base.clone();
        value["publicAddress"] = public;
        parse_command(value).unwrap();
    }
    for public in [
        json!(true),
        json!(null),
        json!({"hostname":"other.spitzli.site"}),
        json!({"label":"../escape"}),
        json!({"label":"a.b"}),
        json!({"label":"*"}),
        json!({"label":"x","customerID":"2"}),
    ] {
        let mut value = base.clone();
        value["publicAddress"] = public;
        assert!(parse_command(value).is_err());
    }
    parse_command(json!({"action":"addresses.list","customerID":"1"})).unwrap();
    parse_command(json!({"action":"clusters.publicSites.configure","clusterID":"1","domain":"spitzli.site","enabled":true,"policyRevision":0,"tenantDailyLimit":1,"globalDailyLimit":2,"trustedCustomerIDs":["1"]})).unwrap();
}
