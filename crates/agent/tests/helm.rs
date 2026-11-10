#![cfg(feature = "kubernetes")]
mod support;
use serde_json::Value;
use spitzli_agent::kubernetes::{ApprovedChart, helm_values};
use spitzli_supervisor::RetainedVolume;
use std::{io::Write, path::PathBuf};
fn chart() -> PathBuf {
    std::fs::canonicalize(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../deploy/helm/webdock-plugin"),
    )
    .unwrap()
}
#[test]
fn real_helm_probe_host_is_a_literal_header_only_when_requested() {
    for host in [None, Some("api.example.test:3130")] {
        let mut value = serde_json::to_value(support::spec()).unwrap();
        if let Some(host) = host {
            value["healthHost"] = serde_json::json!(host);
        }
        let spec: spitzli_supervisor::RuntimeSpec = serde_json::from_value(value).unwrap();
        let values = helm_values(&spec, 1, false).unwrap();
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(&serde_json::to_vec(&values).unwrap())
            .unwrap();
        let output = std::process::Command::new("helm")
            .args(["template", "test-runtime"])
            .arg(chart())
            .arg("--values")
            .arg(file.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let objects: Vec<Value> = serde_saphyr::from_slice_multiple(&output.stdout).unwrap();
        let deployment = objects.iter().find(|v| v["kind"] == "Deployment").unwrap();
        let probe =
            &deployment["spec"]["template"]["spec"]["containers"][0]["readinessProbe"]["httpGet"];
        assert!(
            probe.get("host").is_none(),
            "probe destination must remain its own Pod"
        );
        assert_eq!(
            probe["httpHeaders"],
            host.map(|host| serde_json::json!([{"name":"Host","value":host}]))
                .unwrap_or(Value::Null)
        );
    }
}
#[test]
fn real_helm_rejects_probe_header_injection_and_invalid_ports() {
    for host in [
        "api.example.test\r\nX-Test: injected",
        "user@api.test",
        "api.test/path",
        "api.test:65536",
    ] {
        let mut values = helm_values(&support::spec(), 1, false).unwrap();
        values["healthHost"] = serde_json::json!(host);
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(&serde_json::to_vec(&values).unwrap())
            .unwrap();
        let output = std::process::Command::new("helm")
            .args(["template", "test-runtime"])
            .arg(chart())
            .arg("--values")
            .arg(file.path())
            .output()
            .unwrap();
        assert!(!output.status.success(), "{host:?}");
    }
}
#[test]
fn approved_chart_rejects_changed_files_and_symlinks() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("Chart.yaml"), "name: test").unwrap();
    let digest = ApprovedChart::digest(directory.path()).unwrap();
    assert!(ApprovedChart::verify(directory.path(), &digest).is_ok());
    std::fs::write(directory.path().join("Chart.yaml"), "name: changed").unwrap();
    assert!(ApprovedChart::verify(directory.path(), &digest).is_err());
    std::os::unix::fs::symlink("/etc/passwd", directory.path().join("escape")).unwrap();
    assert!(ApprovedChart::digest(directory.path()).is_err());
}
#[test]
fn real_helm_render_produces_restricted_native_objects_and_retained_claim() {
    let mut spec = support::spec();
    spec.volume = Some(RetainedVolume {
        mount_path: "/data".into(),
        size_bytes: Some(67_108_864),
    });
    let values = helm_values(&spec, 1, false).unwrap();
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(&serde_json::to_vec(&values).unwrap())
        .unwrap();
    let output = std::process::Command::new("helm")
        .args(["template", "test-runtime"])
        .arg(chart())
        .arg("--namespace")
        .arg(spec.namespace())
        .arg("--values")
        .arg(file.path())
        .output()
        .expect("Helm is required for chart validation");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lint = std::process::Command::new("helm")
        .args(["lint", "--strict"])
        .arg(chart())
        .arg("--values")
        .arg(file.path())
        .output()
        .unwrap();
    assert!(
        lint.status.success(),
        "{}",
        String::from_utf8_lossy(&lint.stdout)
    );
    let documents: Vec<Value> =
        serde_saphyr::from_multiple(&String::from_utf8(output.stdout).unwrap()).unwrap();
    let deployment = documents
        .iter()
        .find(|v| v["kind"] == "Deployment")
        .unwrap();
    serde_json::from_value::<k8s_openapi::api::apps::v1::Deployment>(deployment.clone()).unwrap();
    let pod = &deployment["spec"]["template"]["spec"];
    assert_eq!(pod["automountServiceAccountToken"], false);
    assert_eq!(pod["securityContext"]["runAsNonRoot"], true);
    assert_eq!(
        pod["containers"][0]["securityContext"]["readOnlyRootFilesystem"],
        true
    );
    assert_eq!(
        pod["containers"][0]["securityContext"]["allowPrivilegeEscalation"],
        false
    );
    assert_eq!(pod["containers"][0]["image"], spec.image);
    let pvc = documents
        .iter()
        .find(|v| v["kind"] == "PersistentVolumeClaim")
        .unwrap();
    assert_eq!(
        pvc["metadata"]["annotations"]["helm.sh/resource-policy"],
        "keep"
    );
    serde_json::from_value::<k8s_openapi::api::core::v1::PersistentVolumeClaim>(pvc.clone())
        .unwrap();
    let service = documents.iter().find(|v| v["kind"] == "Service").unwrap();
    assert_eq!(service["spec"]["type"], "ClusterIP");
    let policy = documents
        .iter()
        .find(|v| v["kind"] == "NetworkPolicy")
        .unwrap();
    serde_json::from_value::<k8s_openapi::api::networking::v1::NetworkPolicy>(policy.clone())
        .unwrap();
    assert_eq!(
        policy["spec"]["policyTypes"],
        serde_json::json!(["Ingress", "Egress"])
    );
    assert!(documents.iter().all(|v| !matches!(
        v["kind"].as_str(),
        Some("ClusterRole" | "ClusterRoleBinding" | "Ingress")
    )));
}

#[tokio::test]
async fn explicit_cluster_configuration_rejects_plaintext_or_unverified_tls() {
    use spitzli_agent::kubernetes::KubernetesAdapter;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("kubeconfig.yaml");
    for (server, insecure) in [("http://127.0.0.1:1", false), ("https://127.0.0.1:1", true)] {
        std::fs::write(&path,format!("apiVersion: v1\nkind: Config\nclusters:\n- name: disposable\n  cluster:\n    server: {server}\n    insecure-skip-tls-verify: {insecure}\nusers:\n- name: disposable\n  user:\n    token: test-only\ncontexts:\n- name: disposable\n  context:\n    cluster: disposable\n    user: disposable\n")).unwrap();
        let approved =
            ApprovedChart::verify(chart(), &ApprovedChart::digest(chart()).unwrap()).unwrap();
        assert!(
            KubernetesAdapter::connect(
                path.clone(),
                "disposable".into(),
                "test-install".into(),
                PathBuf::from("/usr/local/bin/helm"),
                approved
            )
            .await
            .is_err()
        );
    }
}

#[test]
fn retirement_manifest_only_accepts_exact_owned_chart_resources() {
    let spec = support::spec();
    let mut objects = Vec::new();
    for (version, kind) in [
        ("v1", "ServiceAccount"),
        ("apps/v1", "Deployment"),
        ("v1", "Service"),
        ("networking.k8s.io/v1", "NetworkPolicy"),
    ] {
        objects.push(serde_json::json!({"apiVersion":version,"kind":kind,"metadata":{"name":spec.name(),"labels":spec.labels()}}).to_string());
    }
    let manifest = objects.join("\n---\n");
    spitzli_agent::kubernetes::verify_retirement_manifest(&spec, manifest.as_bytes()).unwrap();
    let foreign = manifest.replace(&spec.name(), "foreign-workload");
    assert!(
        spitzli_agent::kubernetes::verify_retirement_manifest(&spec, foreign.as_bytes()).is_err()
    );
    let extra = format!(
        "{manifest}\n---\n{{\"apiVersion\":\"v1\",\"kind\":\"Namespace\",\"metadata\":{{\"name\":\"outside\"}}}}"
    );
    assert!(
        spitzli_agent::kubernetes::verify_retirement_manifest(&spec, extra.as_bytes()).is_err()
    );
}

#[test]
fn kubernetes_quantity_accepts_exact_exponents_from_helm_without_float_rounding() {
    use spitzli_agent::kubernetes::parse_quantity;
    assert_eq!(parse_quantity("6.7108864e+07", false).unwrap(), 67108864);
    assert_eq!(parse_quantity("67108864e0", false).unwrap(), 67108864);
    assert_eq!(parse_quantity("1e-3", true).unwrap(), 1);
    assert_eq!(parse_quantity("64Mi", false).unwrap(), 67108864);
    assert!(parse_quantity("1e-7", false).is_err());
    assert!(parse_quantity("9007199254740992", false).is_err());
    assert!(parse_quantity("1e100", false).is_err());
}

#[test]
fn helm_accepts_an_explicitly_absent_health_probe() {
    let mut spec = support::spec();
    spec.health_path = None;
    let values = helm_values(&spec, 1, false).unwrap();
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(&serde_json::to_vec(&values).unwrap())
        .unwrap();
    let output = std::process::Command::new("helm")
        .args(["template", "test-runtime"])
        .arg(chart())
        .arg("--values")
        .arg(file.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let objects: Vec<Value> = serde_saphyr::from_slice_multiple(&output.stdout).unwrap();
    let deployment = objects.iter().find(|v| v["kind"] == "Deployment").unwrap();
    assert!(deployment["spec"]["template"]["spec"]["containers"][0]["readinessProbe"].is_null());
}
