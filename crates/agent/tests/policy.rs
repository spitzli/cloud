#![cfg(all(feature = "docker", feature = "kubernetes"))]
mod support;
use support::spec;
use webdock_agent::{docker::container_config, kubernetes::helm_values};
#[test]
fn docker_is_nonprivileged_bounded_and_has_no_host_mounts_or_ports() {
    let config = container_config(&spec(), 1).unwrap();
    let value = serde_json::to_value(config).unwrap();
    assert_eq!(value["User"], "65532:65532");
    assert_eq!(value["HostConfig"]["ReadonlyRootfs"], true);
    assert_eq!(value["HostConfig"]["CapDrop"], serde_json::json!(["ALL"]));
    assert_eq!(
        value["HostConfig"]["SecurityOpt"],
        serde_json::json!(["no-new-privileges:true"])
    );
    assert_eq!(value["HostConfig"]["Memory"], 33554432);
    assert_eq!(value["HostConfig"]["NanoCpus"], 100000000);
    assert!(value["HostConfig"]["Binds"].is_null());
    assert!(value["HostConfig"]["PortBindings"].is_null());
    assert_ne!(value["HostConfig"]["NetworkMode"], "host");
}
#[test]
fn helm_values_only_describe_approved_workload_and_retention() {
    let value = helm_values(&spec(), 1, false).unwrap();
    assert_eq!(value["image"], spec().image);
    assert_eq!(value["replicas"], 1);
    assert_eq!(value["generation"], 1);
    assert_eq!(helm_values(&spec(), 2, true).unwrap()["replicas"], 0);
    assert_eq!(value["resources"]["limits"]["cpu"], "100m");
}
