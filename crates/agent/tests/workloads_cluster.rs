//! The workload executor against a real disposable cluster: the local dev k3s
//! (`WEBDOCK_WORKLOAD_TEST_KUBECONFIG`, context `webdock-dev`). Everything lives in one unique
//! namespace that the test removes.
#![cfg(feature = "kubernetes")]
use kube::{
    Client, Config,
    api::{Api, DeleteParams, ListParams},
    config::{KubeConfigOptions, Kubeconfig},
    core::{ApiResource, DynamicObject, GroupVersionKind},
};
use serde_json::{Value, json};
use std::time::Duration;
use webdock_agent::workloads::WorkloadExecutor;

async fn client() -> Client {
    let path = std::env::var("WEBDOCK_WORKLOAD_TEST_KUBECONFIG").expect("local dev kubeconfig");
    let options = KubeConfigOptions {
        context: Some("webdock-dev".into()),
        ..Default::default()
    };
    let config = Config::from_custom_kubeconfig(Kubeconfig::read_from(path).unwrap(), &options)
        .await
        .unwrap();
    assert!(
        config.cluster_url.host().is_some_and(|h| h == "127.0.0.1"),
        "only the local cluster"
    );
    Client::try_from(config).unwrap()
}
fn labelled(mut object: Value, namespace: &str, unit: &str) -> Value {
    let labels = json!({"webdock.dev/managed-by":"webdock","webdock.dev/plugin":"fixture","webdock.dev/project-id":"1","webdock.dev/unit":unit,"app":unit});
    object["metadata"]["namespace"] = json!(namespace);
    object["metadata"]["labels"] = labels.clone();
    if object["kind"] == "Deployment" {
        object["spec"]["template"]["metadata"]["labels"] = labels;
    }
    object
}

#[tokio::test]
#[ignore = "requires the local dev k3s"]
async fn applies_prunes_observes_and_removes_a_unit() {
    let client = client().await;
    let executor = WorkloadExecutor::new(client.clone());
    let namespace = format!("wd-fixture-{}", std::process::id());
    let unit = "web";
    let deployment = labelled(
        json!({"apiVersion":"apps/v1","kind":"Deployment","metadata":{"name":"web"},
        "spec":{"replicas":1,"selector":{"matchLabels":{"app":"web"}},"template":{"metadata":{},
        "spec":{"automountServiceAccountToken":false,"containers":[{"name":"web","image":"nginx:stable-alpine","ports":[{"containerPort":80}]}]}}}}),
        &namespace,
        unit,
    );
    let service = labelled(
        json!({"apiVersion":"v1","kind":"Service","metadata":{"name":"web"},"spec":{"selector":{"app":"web"},"ports":[{"port":80}]}}),
        &namespace,
        unit,
    );
    let both = serde_json::to_vec(&json!([deployment, service])).unwrap();

    // Objects outside the unit are refused before anything is applied.
    let foreign =
        serde_json::to_vec(&json!([labelled(service.clone(), "kube-system", unit)])).unwrap();
    assert!(
        executor
            .handle("fixture", unit, &namespace, &foreign, false)
            .await
            .error
            .is_some()
    );

    let mut outcome = executor
        .handle("fixture", unit, &namespace, &both, false)
        .await;
    assert_eq!(outcome.error, None);
    for _ in 0..120 {
        if outcome.observed["ready"] == true {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        outcome = executor
            .handle("fixture", unit, &namespace, &both, false)
            .await;
    }
    assert_eq!(outcome.observed["ready"], true, "{}", outcome.observed);
    // Recent pod output travels with the observation.
    let mut logged = outcome;
    for _ in 0..20 {
        if logged.observed["logs"]
            .as_array()
            .is_some_and(|l| !l.is_empty())
        {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        logged = executor
            .handle("fixture", unit, &namespace, &both, false)
            .await;
    }
    assert!(
        logged.observed["logs"]
            .as_array()
            .is_some_and(|l| !l.is_empty()),
        "{}",
        logged.observed
    );

    // What the pods use comes from the metrics API once it has sampled them.
    let mut measured = logged;
    for _ in 0..90 {
        if measured.observed["usage"]["memory"]
            .as_u64()
            .is_some_and(|m| m > 0)
        {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        measured = executor
            .handle("fixture", unit, &namespace, &both, false)
            .await;
    }
    assert!(
        measured.observed["usage"]["memory"]
            .as_u64()
            .is_some_and(|m| m > 0),
        "{}",
        measured.observed
    );

    // A stop is observed only after the Kubernetes controller acknowledges scale-to-zero.
    let mut stopped = deployment.clone();
    stopped["spec"]["replicas"] = json!(0);
    let zero = serde_json::to_vec(&json!([stopped, service])).unwrap();
    let mut stopping = executor
        .handle("fixture", unit, &namespace, &zero, false)
        .await;
    for _ in 0..60 {
        assert_eq!(stopping.error, None);
        if stopping.observed["ready"] == true {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        stopping = executor
            .handle("fixture", unit, &namespace, &zero, false)
            .await;
    }
    assert_eq!(stopping.observed["ready"], true, "{}", stopping.observed);
    assert!(
        stopping.observed["objects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["kind"] == "apps/v1/Deployment" && o["message"] == "Stopped")
    );

    // Dropping the service from the unit prunes it.
    let only = serde_json::to_vec(&json!([deployment])).unwrap();
    assert_eq!(
        executor
            .handle("fixture", unit, &namespace, &only, false)
            .await
            .error,
        None
    );
    let services: Api<DynamicObject> = Api::namespaced_with(
        client.clone(),
        &namespace,
        &ApiResource::from_gvk(&GroupVersionKind::gvk("", "v1", "Service")),
    );
    assert!(services.get_opt("web").await.unwrap().is_none());

    // Removal deletes everything of the unit and says so once nothing is left.
    let mut removal = executor
        .handle("fixture", unit, &namespace, b"[]", true)
        .await;
    for _ in 0..60 {
        if removal.removed {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        removal = executor
            .handle("fixture", unit, &namespace, b"[]", true)
            .await;
    }
    assert!(removal.removed);
    let namespaces: Api<DynamicObject> = Api::all_with(
        client,
        &ApiResource::from_gvk(&GroupVersionKind::gvk("", "v1", "Namespace")),
    );
    namespaces
        .delete(&namespace, &DeleteParams::default())
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires local dev k3s with CloudNativePG and metrics-server"]
async fn cnpg_pods_are_measured_without_inheriting_workload_ownership() {
    let client = client().await;
    let executor = WorkloadExecutor::new(client.clone());
    let namespace = format!("wd-fixture-cnpg-{}", std::process::id());
    let cluster = labelled(
        json!({"apiVersion":"postgresql.cnpg.io/v1","kind":"Cluster",
        "metadata":{"name":"database"},"spec":{"instances":1,"storage":{"size":"256Mi"},
        "resources":{"requests":{"cpu":"50m","memory":"256Mi"},"limits":{"memory":"256Mi"}}}}),
        &namespace,
        "database",
    );
    let objects = serde_json::to_vec(&json!([cluster])).unwrap();
    let metrics: Api<DynamicObject> = Api::namespaced_with(
        client.clone(),
        &namespace,
        &ApiResource {
            group: "metrics.k8s.io".into(),
            version: "v1beta1".into(),
            api_version: "metrics.k8s.io/v1beta1".into(),
            kind: "PodMetrics".into(),
            plural: "pods".into(),
        },
    );
    let mut observed = executor
        .handle("fixture", "database", &namespace, &objects, false)
        .await;
    for _ in 0..150 {
        assert!(observed.error.is_none(), "CNPG apply failed");
        if observed.observed["ready"] == true
            && let Ok(sample) = metrics
                .list(&ListParams::default().labels("cnpg.io/cluster=database"))
                .await
            && !sample.items.is_empty()
        {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        observed = executor
            .handle("fixture", "database", &namespace, &objects, false)
            .await;
    }
    let sample = metrics
        .list(&ListParams::default().labels("cnpg.io/cluster=database"))
        .await
        .unwrap();
    assert!(
        !sample.items.is_empty(),
        "the independent metrics query must contain the database pod"
    );
    assert!(
        sample.items.iter().all(|p| p
            .metadata
            .labels
            .as_ref()
            .is_none_or(|l| !l.contains_key("webdock.dev/unit"))),
        "CNPG pods must not inherit the agent's object-pruning labels"
    );
    let observed = executor
        .handle("fixture", "database", &namespace, &objects, false)
        .await;
    let measured = observed.observed["usage"]["memory"]
        .as_u64()
        .is_some_and(|m| m > 0);
    let logged = observed.observed["logs"]
        .as_array()
        .is_some_and(|l| !l.is_empty());
    // Remove only this test's namespace, including CNPG-generated PVCs, even on assertion failure.
    let namespaces: Api<DynamicObject> = Api::all_with(
        client,
        &ApiResource::from_gvk(&GroupVersionKind::gvk("", "v1", "Namespace")),
    );
    namespaces
        .delete(&namespace, &DeleteParams::default())
        .await
        .unwrap();
    assert!(
        measured,
        "a busy CNPG pod must not be reported as zero memory"
    );
    assert!(logged, "CNPG pod logs must be observed");
}
