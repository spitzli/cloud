use super::database::*;
use serde_json::json;

#[test]
fn projections_drop_passwords_uris_and_unknown_nested_data() {
    for (path, input, expected) in [
        (
            "/projects/p-one/branches/b-one/roles",
            json!({"roles":[{"name":"app","branch_id":"b-one","password":"private-password","connection_uri":"postgres://private","protected":false}]}),
            json!({"roles":[{"name":"app","branch_id":"b-one","protected":false}],"pagination":{"cursor":null}}),
        ),
        (
            "/projects/p-one/endpoints",
            json!({"endpoints":[{"id":"ep-one","branch_id":"b-one","type":"read_write","host":"ep-one.eu.neon.tech","current_state":"active","password":"private-password"}]}),
            json!({"endpoints":[{"id":"ep-one","branch_id":"b-one","type":"read_write","host":"ep-one.eu.neon.tech","current_state":"active"}],"pagination":{"cursor":null}}),
        ),
    ] {
        assert_eq!(project_inventory(path, input).unwrap(), expected);
    }
    assert!(
        project_inventory(
            "/projects/p-one/branches/b-one/roles",
            json!({"roles":[{"password":"private-password"}]})
        )
        .is_err()
    );
}

#[test]
fn inventory_path_allowlist_cannot_reach_uri_or_role_passwords() {
    for path in [
        "/projects/p-one/connection_uri",
        "/projects/p-one/branches/b-one/roles/app/reveal_password",
        "/projects/p-one/../other",
        "/projects/p-one?org_id=other",
        "/projects/%70-one",
    ] {
        assert!(inventory_path(path).is_err());
    }
    for path in [
        "/projects",
        "/projects/p-one",
        "/projects/p-one/branches",
        "/projects/p-one/branches/b-one/databases",
        "/projects/p-one/branches/b-one/roles",
        "/projects/p-one/endpoints",
    ] {
        assert!(inventory_path(path).is_ok());
    }
}

#[test]
fn revealed_uri_matches_stored_role_database_endpoint_and_ssl() {
    let binding = json!({"role_name":"app","database_name":"dbname","endpoint_id":"ep-one"});
    let valid = "postgresql://app:private-password@ep-one-pooler.eu.neon.tech/dbname?sslmode=require&channel_binding=require";
    assert!(validate_uri(valid, &binding, true).is_ok());
    for uri in [
        "postgresql://other:private@ep-one-pooler.eu.neon.tech/dbname?sslmode=require",
        "postgresql://app:private@ep-other-pooler.eu.neon.tech/dbname?sslmode=require",
        "postgresql://app:private@ep-one-pooler.eu.neon.tech/other?sslmode=require",
        "postgresql://app:private@ep-one-pooler.eu.neon.tech/dbname?sslmode=disable",
        "postgresql://app:private@ep-one-pooler.attacker.invalid/dbname?sslmode=require",
        "postgresql://app:private@ep-one-pooler.eu.neon.tech/dbname?sslmode=require&sslmode=disable",
        "postgresql://app:private@ep-one-pooler.eu.neon.tech/dbname?sslmode=require&host=attacker.invalid",
        "postgresql://app:private@ep-one-pooler.eu.neon.tech/dbname?sslmode=require&hostaddr=127.0.0.1",
        "postgresql://app:private@ep-one-pooler.eu.neon.tech/dbname?sslmode=require&user=other&dbname=other",
        "postgresql://app:private@ep-one-pooler.eu.neon.tech/dbname?sslmode=require&options=-csearch_path=other",
        "postgresql://app:private@ep-one-pooler.eu.neon.tech/dbname?sslmode=require&channel_binding=disable",
        "postgresql://app:private@ep-one-pooler.eu.neon.tech/dbname?sslmode=require&channel_binding=require&channel_binding=require",
        "postgresql://app:private@ep-one-pooler.eu.neon.tech/dbname?sslmode=require&%68ost=attacker.invalid",
        "postgresql://app:private@ep-one-pooler.attacker.invalid,foo.neon.tech/dbname?sslmode=require",
        "postgresql://app:private@ep-one-pooler.attacker.invalid%2Cfoo.neon.tech/dbname?sslmode=require",
        "postgresql://app:private@ep-one-pooler.attacker.invalid%2cfoo.neon.tech/dbname?sslmode=require",
        "postgresql://app:private@ep-one-pooler.eu.neon.tech/dbname?sslmode=require\n",
    ] {
        assert!(
            validate_uri(uri, &binding, true).is_err(),
            "URI must match stored tuple"
        );
    }
    assert!(validate_uri(valid, &binding, false).is_err());
}

#[test]
fn encoded_database_and_role_names_match_their_original_tuple() {
    let binding = json!({"role_name":"app/role","database_name":"db/part","endpoint_id":"ep-one"});
    assert!(
        validate_uri(
            "postgresql://app%2Frole:private@ep-one.eu.neon.tech/db%2Fpart?sslmode=require",
            &binding,
            false
        )
        .is_ok()
    );
}

#[test]
fn initial_database_region_is_proven_from_provider_metadata() {
    assert!(require_database_region(Some("aws-eu-central-1")).is_ok());
    for region in [
        None,
        Some(""),
        Some("aws-us-east-1"),
        Some("aws-eu-west-2"),
        Some("aws-eu-central-1.attacker"),
        Some("azure-gwc"),
    ] {
        assert!(require_database_region(region).is_err());
    }
}
