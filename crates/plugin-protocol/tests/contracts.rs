use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{Value, json};
use webdock_plugin_protocol::{Manifest, verify_invocation};

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/project-notes.json")).unwrap()
}

#[test]
fn manifest_preserves_existing_wire_fields_and_rejects_privileged_extensions() {
    let value = fixture();
    let parsed = Manifest::parse(&serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(parsed.as_json(), &value);
    for key in ["command", "namespace", "rbac", "mounts", "sql", "hostURL"] {
        let mut value = fixture();
        value[key] = json!("forbidden");
        assert!(
            Manifest::parse(&serde_json::to_vec(&value).unwrap()).is_err(),
            "{key}"
        );
    }
}

#[test]
fn manifest_rejects_unsafe_schemas_images_capabilities_and_lifecycle() {
    for (pointer, value) in [
        ("/image", json!("registry.example.com/notes:latest")),
        ("/name", json!("core")),
        ("/protocolVersion", json!(1)),
        (
            "/configuration",
            json!({"type":"object","properties":{"x":{"$ref":"https://evil"}}}),
        ),
        ("/configuration", json!({"type":"array"})),
        ("/configuration", json!({"type":"string","default":7})),
        (
            "/configuration",
            json!({"type":"object","required":["missing"]}),
        ),
        ("/actions/0/capabilities", json!(["plugin.data.write"])),
        ("/jobs/0/intervalSeconds", json!(59)),
        (
            "/dependencies",
            json!([{"id":"project-notes","version":"1.0.0"}]),
        ),
        ("/readableSchema", json!({"min":2,"max":1})),
        (
            "/areas",
            json!({"operator":{"pages":[{"id":"x","label":"X","action":"missing"}]}}),
        ),
        (
            "/migrations",
            json!([{"version":1,"checksum":"a".repeat(64),"steps":[{"op":"sql","sql":"DROP TABLE users"}]}]),
        ),
    ] {
        let mut manifest = fixture();
        *manifest.pointer_mut(pointer).unwrap() = value;
        assert!(
            Manifest::parse(&serde_json::to_vec(&manifest).unwrap()).is_err(),
            "{pointer}"
        );
    }
    assert!(Manifest::parse(br#"{"name":"a","name":"b"}"#).is_err());
    assert!(Manifest::parse(&vec![b' '; 262145]).is_err());
}

#[test]
fn declaration_is_distinct_from_installer_support() {
    let parsed = Manifest::parse(&serde_json::to_vec(&fixture()).unwrap()).unwrap();
    assert!(parsed.require_action_only_lifecycle().is_err());
    let mut manifest = fixture();
    manifest["jobs"] = json!([]);
    assert!(
        Manifest::parse(&serde_json::to_vec(&manifest).unwrap())
            .unwrap()
            .require_action_only_lifecycle()
            .is_ok()
    );
}

fn signed(value: &Value) -> String {
    let key = SigningKey::from_bytes(&[7; 32]);
    let body = URL_SAFE_NO_PAD.encode(serde_json::to_vec(value).unwrap());
    format!(
        "{body}.{}",
        URL_SAFE_NO_PAD.encode(key.sign(body.as_bytes()).to_bytes())
    )
}
fn claims() -> Value {
    json!({"v":1,"id":"123e4567-e89b-42d3-a456-426614174000","pluginID":"project-notes","generation":1,"action":"notes.read","projectID":"123","customerID":"101","iat":1000,"exp":1060})
}

#[test]
fn signed_customer_scope_is_required() {
    let key = SigningKey::from_bytes(&[7; 32]).verifying_key();
    assert!(verify_invocation(&signed(&claims()), &key, "project-notes", 1, 1001).is_ok());
    for customer in [
        json!(null),
        json!(""),
        json!("0"),
        json!("01"),
        json!("other"),
        json!("1".repeat(21)),
    ] {
        let mut value = claims();
        value["customerID"] = customer;
        assert!(verify_invocation(&signed(&value), &key, "project-notes", 1, 1001).is_err());
    }
    let mut missing = claims();
    missing.as_object_mut().unwrap().remove("customerID");
    assert!(verify_invocation(&signed(&missing), &key, "project-notes", 1, 1001).is_err());
}

#[test]
fn signatures_bind_runtime_identity_and_expiration() {
    let key = SigningKey::from_bytes(&[7; 32]).verifying_key();
    let token = signed(&claims());
    let result = verify_invocation(&token, &key, "project-notes", 1, 1001).unwrap();
    assert_eq!(result.project_id, "123");
    for (field, value) in [
        ("pluginID", json!("other")),
        ("generation", json!(2)),
        ("exp", json!(1061)),
        ("exp", json!(1000)),
        ("iat", json!(1002)),
        ("extra", json!(true)),
    ] {
        let mut value_claims = claims();
        value_claims[field] = value;
        assert!(
            verify_invocation(&signed(&value_claims), &key, "project-notes", 1, 1001).is_err(),
            "{field}"
        );
    }
    assert!(verify_invocation(&(token + "x"), &key, "project-notes", 1, 1001).is_err());
}

#[test]
fn verifies_node_crypto_golden_signature_over_encoded_body() {
    let key = SigningKey::from_bytes(&[7; 32]).verifying_key();
    let token = include_str!("fixtures/node-invocation.txt").trim();
    assert_eq!(
        verify_invocation(token, &key, "project-notes", 1, 1001)
            .unwrap()
            .action,
        "notes.read"
    );
}

#[test]
fn signed_jobs_are_strict_and_actions_keep_their_original_serialization() {
    let key = SigningKey::from_bytes(&[7; 32]).verifying_key();
    let action = verify_invocation(&signed(&claims()), &key, "project-notes", 1, 1001).unwrap();
    assert_eq!(
        serde_json::to_string(&action).unwrap(),
        r#"{"v":1,"id":"123e4567-e89b-42d3-a456-426614174000","pluginID":"project-notes","generation":1,"action":"notes.read","projectID":"123","customerID":"101","iat":1000,"exp":1060}"#
    );
    assert_eq!(serde_json::to_value(action).unwrap(), claims());
    let identity = json!({"id":format!("job-{}", "a".repeat(64)),"attempt":1});
    let mut value = claims();
    value["job"] = identity.clone();
    let job = verify_invocation(&signed(&value), &key, "project-notes", 1, 1001).unwrap();
    assert_eq!(serde_json::to_value(job).unwrap(), value);
    for invalid in [
        json!(null),
        json!({}),
        json!("job"),
        json!({"id":format!("job-{}", "A".repeat(64)),"attempt":1}),
        json!({"id":format!("job-{}", "a".repeat(63)),"attempt":1}),
        json!({"id":format!("job-{}", "a".repeat(65)),"attempt":1}),
        json!({"id":format!("job-{}", "a".repeat(64)),"attempt":0}),
        json!({"id":format!("job-{}", "a".repeat(64)),"attempt":6}),
        json!({"id":format!("job-{}", "a".repeat(64)),"attempt":1.5}),
        json!({"id":format!("job-{}", "a".repeat(64)),"attempt":1,"claimToken":"private"}),
    ] {
        value["job"] = invalid;
        assert!(
            verify_invocation(&signed(&value), &key, "project-notes", 1, 1001).is_err(),
            "{value}"
        );
    }
    value["job"] = identity;
    value["job"]["attempt"] = json!(5);
    assert!(verify_invocation(&signed(&value), &key, "project-notes", 1, 1001).is_ok());
}

#[test]
fn native_lifecycle_allows_only_immediate_data_http_jobs() {
    let mut value = fixture();
    value["jobs"] = json!([{"name":"notes.compact","capabilities":["plugin.data.write"],"input":{"type":"object"}}]);
    let manifest = Manifest::parse(&serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(manifest.require_action_only_lifecycle().is_ok());
    for cap in ["plugin.jobs.enqueue", "plugin.apps.read", "plugin.db.read"] {
        let mut unsupported = value.clone();
        unsupported["capabilities"]
            .as_array_mut()
            .unwrap()
            .push(json!(cap));
        unsupported["jobs"][0]["capabilities"] = json!([cap]);
        assert!(
            Manifest::parse(&serde_json::to_vec(&unsupported).unwrap())
                .unwrap()
                .require_action_only_lifecycle()
                .is_err()
        );
    }
    value["jobs"][0]["intervalSeconds"] = json!(60);
    assert!(
        Manifest::parse(&serde_json::to_vec(&value).unwrap())
            .unwrap()
            .require_action_only_lifecycle()
            .is_err()
    );
}

#[test]
fn availability_requires_all_live_authority_dimensions() {
    use webdock_plugin_protocol::{ActionPolicy, Availability};
    let manifest = Manifest::parse(&serde_json::to_vec(&fixture()).unwrap()).unwrap();
    let caps = vec!["plugin.data.read".into(), "plugin.data.write".into()];
    let valid = ActionPolicy {
        state: Availability::Enabled,
        ready: true,
        generation: 1,
        runtime_generation: 1,
        configuration_revision: 3,
        runtime_configuration_revision: 3,
        revoked: false,
        project_active: true,
        project_allowed: true,
        project_can_write: true,
        preview: false,
        approved: &caps,
        granted: &caps,
    };
    assert!(valid.authorize(&manifest, "notes.write").is_ok());
    for denied in [
        ActionPolicy {
            state: Availability::Draining,
            ..valid.clone()
        },
        ActionPolicy {
            state: Availability::Disabled,
            ..valid.clone()
        },
        ActionPolicy {
            ready: false,
            ..valid.clone()
        },
        ActionPolicy {
            runtime_generation: 2,
            ..valid.clone()
        },
        ActionPolicy {
            runtime_configuration_revision: 2,
            ..valid.clone()
        },
        ActionPolicy {
            revoked: true,
            ..valid.clone()
        },
        ActionPolicy {
            project_active: false,
            ..valid.clone()
        },
        ActionPolicy {
            project_allowed: false,
            ..valid.clone()
        },
        ActionPolicy {
            project_can_write: false,
            ..valid.clone()
        },
        ActionPolicy {
            preview: true,
            ..valid.clone()
        },
        ActionPolicy {
            granted: &[],
            ..valid.clone()
        },
        ActionPolicy {
            approved: &[],
            ..valid.clone()
        },
    ] {
        assert!(denied.authorize(&manifest, "notes.write").is_err());
    }
    assert!(valid.authorize(&manifest, "not-declared").is_err());
}

#[test]
fn values_enforce_nested_schemas_without_external_references() {
    use webdock_plugin_protocol::validate_value;
    let schema = json!({"type":"object","additionalProperties":false,"required":["items"],"properties":{"items":{"type":"array","maxItems":2,"items":{"type":"integer","minimum":1}}}});
    assert!(validate_value(Some(&schema), &json!({"items":[1,2]})).is_ok());
    for input in [
        json!({"items":[0]}),
        json!({"items":[1,2,3]}),
        json!({"items":[1],"secret":"x"}),
        json!({}),
    ] {
        assert!(validate_value(Some(&schema), &input).is_err());
    }
    assert!(
        validate_value(
            Some(&json!({"$ref":"https://example.invalid/schema"})),
            &json!({})
        )
        .is_err()
    );
}

#[test]
fn descriptor_preserves_exact_release_bytes_for_signature_checks() {
    let bytes = include_bytes!("fixtures/project-notes.json");
    let manifest = Manifest::parse(bytes).unwrap();
    assert_eq!(manifest.as_bytes(), bytes);
}

#[test]
fn required_services_fail_explicitly_but_optional_services_do_not_block() {
    let manifest = Manifest::parse(include_bytes!("fixtures/project-notes.json")).unwrap();
    assert!(manifest.require_host_services(&[]).is_err());
    assert!(manifest.require_host_services(&[("data", 1)]).is_ok());
    assert!(manifest.require_host_services(&[("data", 2)]).is_err());
}

#[test]
fn integer_valued_json_numbers_follow_javascript_manifest_semantics() {
    let bytes = include_str!("fixtures/project-notes.json")
        .replace("\"protocolVersion\": 2", "\"protocolVersion\": 2.0");
    assert!(Manifest::parse(bytes.as_bytes()).is_ok());
}

#[test]
fn schemas_and_migrations_reject_invalid_keys_defaults_and_foreign_references() {
    let table = json!({"name":"notes","columns":[{"name":"id","type":"string","nullable":false}],"primaryKey":["id"],"indexes":[],"foreignKeys":[]});
    let mut value = fixture();
    value["tables"] = json!([table.clone()]);
    value["migrations"] = json!([{"version":1,"checksum":"a".repeat(64),"steps":[{"op":"createTable","table":table}]}]);
    assert!(Manifest::parse(&serde_json::to_vec(&value).unwrap()).is_ok());
    for (pointer, invalid) in [
        ("/tables/0/primaryKey", json!(["missing"])),
        ("/tables/0/columns/0/nullable", json!(true)),
        (
            "/tables/0/foreignKeys",
            json!([{"name":"fk","columns":["id"],"references":{"table":"missing","columns":["id"]}}]),
        ),
        (
            "/tables/0/columns",
            json!([{"name":"id","type":"integer","nullable":false,"default":"bad"}]),
        ),
        ("/migrations/0/version", json!(2)),
    ] {
        let mut bad = value.clone();
        *bad.pointer_mut(pointer).unwrap() = invalid;
        assert!(
            Manifest::parse(&serde_json::to_vec(&bad).unwrap()).is_err(),
            "{pointer}"
        );
    }
}

#[test]
fn signed_integer_claims_accept_equivalent_json_number_syntax() {
    let key = SigningKey::from_bytes(&[7; 32]).verifying_key();
    let mut value = claims();
    value["generation"] = json!(1.0);
    value["iat"] = json!(1000.0);
    assert!(verify_invocation(&signed(&value), &key, "project-notes", 1, 1001).is_ok());
}

/// A v2 product manifest: one offering, one self-service feature and a resource page.
fn product() -> Value {
    let mut v = fixture();
    v["offerings"] =
        json!([{"id":"note","label":"Note","description":"A short note kept with the project."}]);
    v["selfService"] =
        json!({"features":[{"id":"create-notes","label":"Create notes","offering":"note"}]});
    let resource = json!({"type":"object","additionalProperties":false,"required":["resource_id"],"properties":{"resource_id":{"type":"string","maxLength":128}}});
    v["actions"] = json!([
        {"name":"notes.create","intent":"write","audience":"customer","capabilities":[],"offering":"note","feature":"create-notes","resource":"create",
         "input":{"type":"object","additionalProperties":false,"properties":{"name":{"type":"string","maxLength":80}}}},
        {"name":"notes.view","intent":"read","audience":"customer","capabilities":[],"offering":"note","resource":"use","input":resource},
        {"name":"notes.delete","intent":"write","audience":"customer","capabilities":[],"offering":"note","feature":"create-notes","resource":"delete","input":resource},
        {"name":"notes.operator","intent":"read","audience":"operator","capabilities":[]}
    ]);
    v["jobs"] = json!([]);
    v["areas"] = json!({
        "operator":{"pages":[{"id":"overview","label":"Overview","action":"notes.operator"}]},
        "selfService":{"pages":[{"id":"note","label":"Note","action":"notes.view","resource":true}]}
    });
    v
}
fn parses(v: &Value) -> bool {
    Manifest::parse(&serde_json::to_vec(v).unwrap()).is_ok()
}

#[test]
fn v2_manifest_accepts_reference() {
    let m = Manifest::parse(&serde_json::to_vec(&product()).unwrap()).unwrap();
    assert_eq!(m.offering("note").unwrap()["label"], "Note");
    assert_eq!(m.feature("create-notes").unwrap()["offering"], "note");
    assert_eq!(m.features().len(), 1);
    assert_eq!(m.self_service_pages()[0]["id"], "note");
    assert_eq!(m.operator_pages()[0]["action"], "notes.operator");
}

#[test]
fn rejects_v1_protocol() {
    let mut v = product();
    v["protocolVersion"] = json!(1);
    assert!(!parses(&v));
    let mut v = product();
    v["slots"] = json!([]);
    assert!(!parses(&v));
}

#[test]
fn customer_action_needs_gate() {
    let mut v = product();
    v["actions"][1].as_object_mut().unwrap().remove("resource");
    assert!(!parses(&v));
    let mut v = product();
    v["actions"][3]["audience"] = json!("everyone");
    assert!(!parses(&v));
    let mut v = product();
    v["actions"][3].as_object_mut().unwrap().remove("audience");
    assert!(!parses(&v));
}

#[test]
fn customer_action_needs_self_service_area() {
    let mut v = product();
    v["areas"].as_object_mut().unwrap().remove("selfService");
    assert!(!parses(&v));
}

#[test]
fn feature_must_exist_and_be_used() {
    let mut v = product();
    v["actions"][0]["feature"] = json!("missing");
    assert!(!parses(&v));
    let mut v = product();
    v["selfService"]["features"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"unused","label":"Unused"}));
    assert!(!parses(&v));
    let mut v = product();
    v["selfService"]["features"][0]["offering"] = json!("missing");
    assert!(!parses(&v));
    let mut v = product();
    v["actions"][3]["feature"] = json!("create-notes");
    assert!(!parses(&v), "operator actions are not self-service");
}

#[test]
fn create_needs_feature_and_offering() {
    for key in ["feature", "offering"] {
        let mut v = product();
        v["actions"][0].as_object_mut().unwrap().remove(key);
        assert!(!parses(&v), "{key}");
    }
    let mut v = product();
    v["actions"][0]["input"]["properties"]["resource_id"] =
        json!({"type":"string","maxLength":128});
    assert!(!parses(&v), "create must not take a resource id");
}

#[test]
fn use_needs_resource_input() {
    let mut v = product();
    v["actions"][1].as_object_mut().unwrap().remove("input");
    assert!(!parses(&v));
    let mut v = product();
    v["actions"][1]["input"]["required"] = json!([]);
    assert!(!parses(&v));
    let mut v = product();
    v["actions"][2]["input"]["properties"]["resource_id"]["maxLength"] = json!(129);
    assert!(!parses(&v));
    let mut v = product();
    v["actions"][2]["input"]["properties"]["resource_id"] = json!({"type":"integer"});
    assert!(!parses(&v));
}

#[test]
fn page_action_must_be_read_of_matching_audience() {
    let mut v = product();
    v["areas"]["operator"]["pages"][0]["action"] = json!("notes.view");
    assert!(!parses(&v));
    let mut v = product();
    v["areas"]["selfService"]["pages"][0]["action"] = json!("notes.delete");
    assert!(!parses(&v));
    let mut v = product();
    v["areas"]["selfService"]["pages"][0]["resource"] = json!(false);
    assert!(!parses(&v));
    let mut v = product();
    v["areas"]["selfService"]["pages"][0]["action"] = json!("notes.operator");
    assert!(!parses(&v));
}

#[test]
fn product_names_not_reserved() {
    for name in ["mail", "hosting", "databases", "storage", "cms"] {
        let mut v = product();
        v["name"] = json!(name);
        assert!(parses(&v), "{name}");
    }
    let mut v = product();
    v["name"] = json!("core");
    assert!(!parses(&v));
}

#[test]
fn ids_unique() {
    let mut v = product();
    v["offerings"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"note","label":"Again","description":"Same id twice."}));
    assert!(!parses(&v));
    let mut v = product();
    let page = v["areas"]["operator"]["pages"][0].clone();
    v["areas"]["operator"]["pages"]
        .as_array_mut()
        .unwrap()
        .push(page);
    assert!(!parses(&v));
}

#[test]
fn create_and_delete_must_be_writes() {
    for index in [0, 2] {
        let mut v = product();
        v["actions"][index]["intent"] = json!("read");
        assert!(!parses(&v), "{}", v["actions"][index]["name"]);
    }
}

#[test]
fn offerings_explain_themselves() {
    let mut v = product();
    v["offerings"][0]["description"] = json!("A short note kept with the project.");
    v["offerings"][0]["highlights"] = json!(["Ready at once", "Visible to every member"]);
    assert!(parses(&v));
    // Every offering must say what it is.
    v["offerings"][0]
        .as_object_mut()
        .unwrap()
        .remove("description");
    assert!(!parses(&v));
    for bad in [
        json!({"description": "x".repeat(281)}),
        json!({"highlights": ["x".repeat(141)]}),
        json!({"highlights": ["a", "b", "c", "d", "e", "f", "g"]}),
        json!({"highlights": [""]}),
        json!({"highlights": "not a list"}),
        json!({"description": ""}),
    ] {
        let mut v = product();
        for (key, value) in bad.as_object().unwrap() {
            v["offerings"][0][key] = value.clone();
        }
        assert!(!parses(&v), "{bad}");
    }
}

#[test]
fn workloads_are_declared_kinds_with_the_capability() {
    let mut v = product();
    v["capabilities"] = json!(["plugin.workloads"]);
    v["workloads"] =
        json!({"kinds": ["apps/v1/Deployment", "v1/Service", "traefik.io/v1alpha1/IngressRoute"]});
    let m = Manifest::parse(&serde_json::to_vec(&v).unwrap()).unwrap();
    assert!(m.workload_kind("apps/v1", "Deployment"));
    assert!(!m.workload_kind("v1", "Pod"));
    for (capabilities, workloads) in [
        (json!(["plugin.workloads"]), Value::Null),
        (json!([]), json!({"kinds": ["v1/Service"]})),
        (json!(["plugin.workloads"]), json!({"kinds": ["v1/Pod"]})),
        (
            json!(["plugin.workloads"]),
            json!({"kinds": ["rbac.authorization.k8s.io/v1/ClusterRole"]}),
        ),
        (json!(["plugin.workloads"]), json!({"kinds": []})),
        (
            json!(["plugin.workloads"]),
            json!({"kinds": ["v1/Service"], "namespace": "kube-system"}),
        ),
    ] {
        let mut v = product();
        v["capabilities"] = capabilities.clone();
        if workloads.is_null() {
            v.as_object_mut().unwrap().remove("workloads");
        } else {
            v["workloads"] = workloads.clone();
        }
        assert!(!parses(&v), "{capabilities} {workloads}");
    }
}

#[test]
fn schema_fields_support_descriptor_camel_case_without_widening_action_names() {
    let mut v = fixture();
    v["actions"][0]["output"] = json!({"type":"object","properties":{"expiresAt":{"type":"integer"},"exactBytes":{"type":"integer"},"expectedSha256":{"type":"string"},"attachment":{"type":"object","properties":{"contentType":{"type":"string"}},"required":["contentType"]}},"required":["expiresAt","exactBytes"]});
    assert!(Manifest::parse(&serde_json::to_vec(&v).unwrap()).is_ok());
    for key in [
        "constructor",
        "prototype",
        "__proto__",
        "ExpiresAt",
        "a.constructor",
        "a.__proto__.b",
    ] {
        let mut bad = v.clone();
        bad["actions"][0]["output"] = json!({"type":"object","properties":{key:{"type":"string"}}});
        assert!(Manifest::parse(&serde_json::to_vec(&bad).unwrap()).is_err());
    }
    v["actions"][0]["name"] = json!("downloadObject");
    assert!(Manifest::parse(&serde_json::to_vec(&v).unwrap()).is_err());
}
