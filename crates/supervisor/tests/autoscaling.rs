use serde_json::{Value, json};
use webdock_supervisor::RuntimeSpec;

fn spec() -> Value {
    json!({"version":1,"installation":"test","projectId":"123","resourceId":"456","revision":1,"image":format!("registry.example/app@sha256:{}","a".repeat(64)),"arguments":[],"cpuMillicores":100,"memoryBytes":67108864,"ephemeralBytes":67108864,"replicas":2,"port":8080,"healthPath":"/","volume":null,"autoscaling":{"minReplicas":2,"maxReplicas":5,"targetCPUUtilizationPercent":70}})
}

#[test]
fn signed_runtime_carries_strict_optional_scaling_policy() {
    let s: RuntimeSpec = serde_json::from_value(spec()).unwrap();
    s.validate().unwrap();
    assert_eq!(
        serde_json::to_value(&s).unwrap()["autoscaling"],
        spec()["autoscaling"]
    );
    for (key, value) in [
        ("minReplicas", json!(0)),
        ("maxReplicas", json!(21)),
        ("minReplicas", json!(6)),
        ("targetCPUUtilizationPercent", json!(91)),
    ] {
        let mut v = spec();
        v["autoscaling"][key] = value;
        assert!(
            serde_json::from_value::<RuntimeSpec>(v)
                .unwrap()
                .validate()
                .is_err()
        );
    }
    let mut v = spec();
    v["autoscaling"]["foreign"] = json!(true);
    assert!(serde_json::from_value::<RuntimeSpec>(v).is_err());
    let mut v = spec();
    v["volume"] = json!({"mountPath":"/data","sizeBytes":67108864});
    assert!(
        serde_json::from_value::<RuntimeSpec>(v)
            .unwrap()
            .validate()
            .is_err()
    );
    let mut old = spec();
    old.as_object_mut().unwrap().remove("autoscaling");
    let old: RuntimeSpec = serde_json::from_value(old).unwrap();
    old.validate().unwrap();
    assert!(
        serde_json::to_value(old)
            .unwrap()
            .get("autoscaling")
            .is_none()
    );
}

#[test]
fn changing_scaling_policy_changes_the_signed_command_fingerprint() {
    use webdock_supervisor::{Action, Command};
    let spec: RuntimeSpec = serde_json::from_value(spec()).unwrap();
    let mut command = Command {
        operation_id: "owned-policy".into(),
        generation: 1,
        lease_until: webdock_supervisor::timestamp().unwrap() + 60,
        authorization: "fixture".into(),
        action: Action::Apply,
        spec,
        material: None,
        expected_runtime_id: None,
    };
    let before = command.fingerprint().unwrap();
    command.spec.autoscaling.as_mut().unwrap().max_replicas = 4;
    assert_ne!(before, command.fingerprint().unwrap());
}
