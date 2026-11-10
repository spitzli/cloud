use serde_json::{Value, json};
use spitzli_hosting::contracts::{
    admit_autoscaling, app_demand, apply_scaling_action, resolve_spec,
};

fn spec() -> Value {
    json!({"template":"custom","image":format!("registry.example/app@sha256:{}","a".repeat(64)),"args":[],"port":8080,"healthPath":"/","cpuMillicores":100,"memoryBytes":67108864,"ephemeralBytes":67108864,"replicas":2,"volumeBytes":0,"autoscaling":{"minReplicas":2,"maxReplicas":5,"targetCPUUtilizationPercent":70}})
}

#[test]
fn automatic_capacity_reserves_maximum_but_stop_and_delete_release_compute() {
    let mut s = resolve_spec(spec()).unwrap();
    let demand = app_demand(&s, false).unwrap();
    assert_eq!(demand["replicasPerApp"], 5);
    assert_eq!(demand["cpuMillicores"], 500);
    assert_eq!(demand["memoryBytes"], 335544320u64);
    assert_eq!(demand["ephemeralBytes"], 335544320u64);
    s["replicas"] = json!(0);
    assert!(resolve_spec(s.clone()).is_ok());
    assert_eq!(app_demand(&s, false).unwrap()["cpuMillicores"], 0);
    assert_eq!(app_demand(&spec(), true).unwrap()["replicasPerApp"], 0);
}

#[test]
fn automatic_policy_rejects_invalid_limits_and_local_storage() {
    for (key, value) in [
        ("minReplicas", json!(0)),
        ("minReplicas", json!(6)),
        ("maxReplicas", json!(21)),
        ("maxReplicas", json!(2.5)),
        ("targetCPUUtilizationPercent", json!(9)),
        ("targetCPUUtilizationPercent", json!(91)),
        ("unknown", json!(true)),
    ] {
        let mut s = spec();
        s["autoscaling"][key] = value;
        assert!(resolve_spec(s).is_err(), "accepted {key}");
    }
    let mut s = spec();
    s["volumeBytes"] = json!(67108864);
    assert!(resolve_spec(s).is_err());
    let mut s = spec();
    s["replicas"] = json!(1);
    assert!(resolve_spec(s).is_err());
    let mut fixed = spec();
    fixed.as_object_mut().unwrap().remove("autoscaling");
    assert!(resolve_spec(fixed).is_ok());
}

#[test]
fn automatic_lifecycle_requires_explicit_mode_switch_and_qualified_cluster() {
    let mut s = spec();
    assert!(apply_scaling_action(&mut s, "apps.scale", &json!(3)).is_err());
    apply_scaling_action(&mut s, "apps.stop", &Value::Null).unwrap();
    assert_eq!(s["replicas"], 0);
    assert!(s.get("autoscaling").is_some());
    apply_scaling_action(&mut s, "apps.start", &Value::Null).unwrap();
    assert_eq!(s["replicas"], 2);
    assert!(admit_autoscaling(false, "kubernetes", &json!(1)).is_err());
    assert!(admit_autoscaling(true, "docker", &json!(1)).is_err());
    for value in [Value::Null, json!(0), json!(true), json!(2)] {
        assert!(admit_autoscaling(true, "kubernetes", &value).is_err());
    }
    admit_autoscaling(true, "kubernetes", &json!(1)).unwrap();
}
