use serde_json::json;
use spitzli_core::id::RecordId;
use spitzli_supervisor::{HostingBinding, PublicRoute};

#[test]
fn retained_binding_without_public_route_preserves_old_wire_bytes() {
    let quota = json!({"apps":2,"cpuMillicores":500,"memoryBytes":134217728,"volumeBytes":0,"ephemeralBytes":67108864,"replicasPerApp":1,"concurrentDeployments":1});
    let old = json!({"clusterId":"123","quota":quota,"finalQuota":quota});
    let binding: HostingBinding = serde_json::from_value(old.clone()).unwrap();
    assert!(binding.public_route.is_none());
    let allowance = serde_json::to_string(&binding.quota).unwrap();
    let expected =
        format!("{{\"clusterId\":\"123\",\"quota\":{allowance},\"finalQuota\":{allowance}}}");
    assert_eq!(serde_json::to_string(&binding).unwrap(), expected);
    assert_eq!(serde_json::to_value(binding).unwrap(), old);
}

#[test]
fn public_route_rejects_ports_wildcards_ips_ambiguous_names_and_unbounded_revisions() {
    let route = PublicRoute {
        allocation_id: RecordId::parse("456").unwrap(),
        hostname: "lunares.spitzli.site".into(),
        revision: 1,
    };
    route.validate().unwrap();
    for hostname in [
        "api.spitzli.site:443",
        "*.spitzli.site",
        "127.0.0.1",
        "localhost",
        "a..spitzli.site",
        "UPPER.spitzli.site",
        "x.spitzli.site/path",
        "x.spitzli.site.",
    ] {
        let bad = PublicRoute {
            hostname: hostname.into(),
            ..route.clone()
        };
        assert!(bad.validate().is_err(), "{hostname}");
    }
    for revision in [0, 9_007_199_254_740_992] {
        assert!(
            PublicRoute {
                revision,
                ..route.clone()
            }
            .validate()
            .is_err()
        );
    }
}
