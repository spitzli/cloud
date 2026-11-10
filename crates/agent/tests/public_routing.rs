#![cfg(feature = "kubernetes")]
mod support;
use spitzli_agent::kubernetes::public_routing::{PublicRoutingConfig, ingress};
use spitzli_supervisor::{HostingBinding, PublicRoute, ResourceAllowance};
fn allowance() -> ResourceAllowance {
    ResourceAllowance {
        apps: 2,
        cpu_millicores: 1000,
        memory_bytes: 1073741824,
        volume_bytes: 0,
        ephemeral_bytes: 1073741824,
        replicas_per_app: 2,
        concurrent_deployments: 1,
    }
}
fn spec() -> spitzli_supervisor::RuntimeSpec {
    let mut spec = support::spec();
    spec.hosting_binding = Some(HostingBinding {
        cluster_id: spitzli_core::id::RecordId::parse("42").unwrap(),
        quota: allowance(),
        final_quota: allowance(),
        public_route: Some(PublicRoute {
            allocation_id: spitzli_core::id::RecordId::parse("43").unwrap(),
            hostname: "lunares.spitzli.site".into(),
            revision: 1,
        }),
    });
    spec
}
fn config() -> PublicRoutingConfig {
    PublicRoutingConfig {
        domain: "spitzli.site".into(),
        ingress_class: "traefik".into(),
        cluster_issuer: "webdock-cms-acme".into(),
    }
}
#[test]
fn exact_route_uses_existing_app_service_and_one_host_certificate() {
    let s = spec();
    let value = serde_json::to_value(ingress(&s, &config()).unwrap()).unwrap();
    assert_eq!(value["spec"]["rules"][0]["host"], "lunares.spitzli.site");
    assert_eq!(value["spec"]["rules"].as_array().unwrap().len(), 1);
    assert_eq!(
        value["spec"]["tls"][0]["hosts"],
        serde_json::json!(["lunares.spitzli.site"])
    );
    assert_eq!(
        value["spec"]["rules"][0]["http"]["paths"][0]["backend"]["service"]["name"],
        format!("app-{}", s.resource_id)
    );
    assert_eq!(
        value["metadata"]["annotations"]["cert-manager.io/cluster-issuer"],
        "webdock-cms-acme"
    );
    assert_eq!(
        value["metadata"]["labels"]["webdock.dev/allocation-id"],
        "43"
    );
    assert!(value["spec"].get("defaultBackend").is_none());
}
#[test]
fn no_implicit_route_or_suffix_lookalike_can_create_ingress() {
    for host in [
        "spitzli.site",
        "*.spitzli.site",
        "other.spitzli.cloud",
        "lunares.spitzli.site.evil",
        "nested.a.spitzli.site",
        "auth.spitzli.site:443",
    ] {
        let mut s = spec();
        s.hosting_binding
            .as_mut()
            .unwrap()
            .public_route
            .as_mut()
            .unwrap()
            .hostname = host.into();
        assert!(ingress(&s, &config()).is_err(), "{host}");
    }
    let mut s = spec();
    s.hosting_binding.as_mut().unwrap().public_route = None;
    assert!(ingress(&s, &config()).is_err());
}

#[test]
fn certificate_readiness_requires_the_qualified_solver_budget_and_clusterip() {
    use serde_json::json;
    use spitzli_agent::kubernetes::public_routing::compatible_issuer;
    let issuer = json!({"metadata":{"generation":1},"spec":{"acme":{"server":"https://acme-v02.api.letsencrypt.org/directory","solvers":[{"http01":{"ingress":{"ingressClassName":"traefik","serviceType":"ClusterIP","podTemplate":{"spec":{"resources":{"requests":{"cpu":"25m","memory":"32Mi","ephemeral-storage":"8Mi"},"limits":{"cpu":"200m","memory":"64Mi","ephemeral-storage":"64Mi"}}}}}}}]}},"status":{"conditions":[{"type":"Ready","status":"True","observedGeneration":1}]}});
    assert!(compatible_issuer(&issuer, &config()));
    let mut no_budget = issuer.clone();
    no_budget["spec"]["acme"]["solvers"][0]["http01"]["ingress"]["podTemplate"]["spec"]["resources"] =
        json!({});
    assert!(!compatible_issuer(&no_budget, &config()));
    let mut nodeport = issuer.clone();
    nodeport["spec"]["acme"]["solvers"][0]["http01"]["ingress"]["serviceType"] = json!("NodePort");
    assert!(!compatible_issuer(&nodeport, &config()));
    let mut stale = issuer.clone();
    stale["metadata"]["generation"] = json!(2);
    assert!(!compatible_issuer(&stale, &config()));
    let mut too_large = issuer;
    too_large["spec"]["acme"]["solvers"][0]["http01"]["ingress"]["podTemplate"]["spec"]["resources"]
        ["requests"]["ephemeral-storage"] = json!("128Mi");
    assert!(!compatible_issuer(&too_large, &config()));
}
