#![cfg(feature = "kubernetes-job")]

use http::{Request, Response};
use http_body_util::{BodyExt, Full};
use kube::{Client, client::Body};
use serde_json::{Value, json};
use spitzli_build::kubernetes::{
    KubernetesBuildConfig, KubernetesBuildLimits, KubernetesBuildProfile, KubernetesBuildRequest,
    KubernetesJobBuildExecutor, KubernetesJobPhase, KubernetesLogBounds, TrustedProject,
};
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};
use tower::service_fn;

fn config() -> KubernetesBuildConfig {
    KubernetesBuildConfig {
        profile: KubernetesBuildProfile::Restricted,
        namespace: "webdock-builds".into(),
        service_account: "build-runner".into(),
        sandbox_runtime_class: "webdock-build-sandbox".into(),
        runner_image: format!("registry.example/build-runner@sha256:{}", "a".repeat(64)),
        trusted_projects: BTreeSet::from([TrustedProject {
            customer_id: "customer-1".into(),
            project_id: "project-1".into(),
        }]),
        storage_cluster_id: None,
        hold_execution: false,
    }
}

fn request() -> KubernetesBuildRequest {
    KubernetesBuildRequest {
        customer_id: "customer-1".into(),
        project_id: "project-1".into(),
        build_id: "build-1".into(),
        generation: 1,
        attempt: 1,
        commit_sha: "b".repeat(40),
        volume: None,
        limits: KubernetesBuildLimits {
            cpu_millis: 2000,
            memory_mib: 2048,
            workspace_mib: 4096,
            cache_mib: 1024,
            deadline_seconds: 900,
        },
    }
}

fn hard_fixture() -> (KubernetesBuildConfig, KubernetesBuildRequest) {
    use spitzli_agent_protocol::build_storage::{
        BuildStorageAction, BuildStorageReservation, BuildStorageTarget, BuildVolumeHandle,
    };
    let mut config = config();
    config.profile = KubernetesBuildProfile::RootlessIdmapSandbox;
    config.namespace = "webdock-build-qualification".into();
    config.storage_cluster_id = Some("78".into());
    config.trusted_projects = BTreeSet::from([TrustedProject {
        customer_id: "12".into(),
        project_id: "34".into(),
    }]);
    let mut request = request();
    request.customer_id = "12".into();
    request.project_id = "34".into();
    request.build_id = "56".into();
    request.volume = Some(
        BuildVolumeHandle::new(
            BuildStorageReservation {
                version: 1,
                audience: "webdock-build-storage-v1".into(),
                volume_id: "1234".into(),
                customer_id: "12".into(),
                project_id: "34".into(),
                build_id: "56".into(),
                generation: 1,
                target: BuildStorageTarget {
                    cluster_id: "78".into(),
                    node: "build-node-1".into(),
                    pool_id: "123e4567-e89b-42d3-a456-426614174000".into(),
                },
                volume_bytes: 6 * 1024 * 1024 * 1024,
                action: BuildStorageAction::Reserve,
                epoch: 1,
                expires_at: chrono::Utc::now() + chrono::Duration::seconds(120),
            },
            "webdock-build-qualification",
            "pv-uid-1",
            "pvc-uid-1",
            "123e4567-e89b-42d3-a456-426614174001",
        )
        .unwrap(),
    );
    (config, request)
}

#[tokio::test]
async fn hard_volume_profile_requires_exact_attempt_scope_and_only_bounded_scratch_in_emptydir() {
    let (config, request) = hard_fixture();
    let executor = executor_with_config(config, |_, _, _| {
        unreachable!("manifest validation performs no I/O")
    });
    let mut missing = request.clone();
    missing.volume = None;
    assert!(executor.manifest(&missing).is_err());
    let manifest = serde_json::to_value(executor.manifest(&request).unwrap()).unwrap();
    assert!(
        manifest["metadata"]["labels"]
            .as_object()
            .unwrap()
            .values()
            .all(|v| v.as_str().unwrap().len() <= 63)
    );
    let spec = &manifest["spec"]["template"]["spec"];
    assert_eq!(
        spec["volumes"][0]["persistentVolumeClaim"]["claimName"],
        "wd-bv-56-1-1234"
    );
    for volume in spec["volumes"].as_array().unwrap() {
        if !volume["emptyDir"].is_null() {
            assert_eq!(volume["emptyDir"]["medium"], "Memory");
            assert_eq!(volume["emptyDir"]["sizeLimit"], "64Mi");
        }
        assert!(volume["hostPath"].is_null());
    }
    assert_eq!(
        spec["containers"][0]["volumeMounts"][0]["subPath"],
        "workspace"
    );
    assert_eq!(spec["containers"][0]["volumeMounts"][1]["subPath"], "cache");
    let mut foreign = request.clone();
    foreign.volume.as_mut().unwrap().reservation.customer_id = "99".into();
    assert!(executor.manifest(&foreign).is_err());
    let mut replaced = request.clone();
    replaced.volume.as_mut().unwrap().pvc_uid = "replacement".into();
    assert_ne!(
        executor.fingerprint(&request).unwrap(),
        executor.fingerprint(&replaced).unwrap()
    );
}

#[tokio::test]
async fn api_omitted_false_pvc_read_only_is_equivalent_but_true_is_foreign() {
    let (mut config, request) = hard_fixture();
    config.hold_execution = true;
    let offline = executor_with_config(config.clone(), |_, _, _| unreachable!());
    let job = serde_json::to_value(offline.manifest(&request).unwrap()).unwrap();
    let mut job = stored(job, "uid-original");
    job["spec"]["template"]["spec"]["volumes"][0]["persistentVolumeClaim"]
        .as_object_mut()
        .unwrap()
        .remove("readOnly");
    let current = Arc::new(Mutex::new(job));
    let state = current.clone();
    let executor = executor_with_config(config, move |method, _, _| {
        assert_eq!(method, "GET");
        (200, state.lock().unwrap().clone())
    });
    let handle = spitzli_build::kubernetes::KubernetesJobHandle {
        request,
        namespace: executor.namespace().into(),
        uid: "uid-original".into(),
    };
    assert_eq!(
        executor.inspect(&handle).await.unwrap(),
        Some(KubernetesJobPhase::Pending)
    );
    current.lock().unwrap()["spec"]["template"]["spec"]["volumes"][0]["persistentVolumeClaim"]["readOnly"] =
        json!(true);
    assert!(executor.inspect(&handle).await.is_err());
}

#[tokio::test]
#[ignore = "explicit private output and approved real runner image for server admission dry runs"]
async fn export_owned_fixed_volume_admission_fixture() {
    use std::{
        io::Write,
        os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    };
    let (mut config, mut request) = hard_fixture();
    config.runner_image = std::env::var("SPITZLI_VOLUME_TEST_RUNNER_IMAGE").unwrap();
    config.service_account = "webdock-build-job".into();
    config.hold_execution = true;
    if let Some(input) = std::env::var_os("SPITZLI_VOLUME_TEST_REQUEST") {
        let path = std::path::PathBuf::from(input);
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
            .unwrap();
        let meta = file.metadata().unwrap();
        assert!(meta.is_file() && meta.len() < 32768 && meta.mode() & 0o077 == 0);
        request = serde_json::from_reader(file).unwrap();
        config.storage_cluster_id = Some(
            request
                .volume
                .as_ref()
                .unwrap()
                .reservation
                .target
                .cluster_id
                .clone(),
        );
        config.trusted_projects = std::collections::BTreeSet::from([TrustedProject {
            customer_id: request.customer_id.clone(),
            project_id: request.project_id.clone(),
        }]);
    }

    let path = std::path::PathBuf::from(std::env::var("SPITZLI_VOLUME_TEST_MANIFEST").unwrap());
    let parent = path.parent().unwrap();
    let meta = parent.symlink_metadata().unwrap();
    assert!(
        path.is_absolute()
            && parent.canonicalize().unwrap() == parent
            && meta.is_dir()
            && meta.permissions().mode() & 0o077 == 0
    );
    // SAFETY: geteuid takes no arguments and does not retain pointers.
    assert_eq!(meta.uid(), unsafe { libc::geteuid() });
    let executor =
        executor_with_config(config, |_, _, _| unreachable!("offline fixture rendering"));
    let bytes = serde_json::to_vec(&executor.manifest(&request).unwrap()).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(&bytes).unwrap();
    file.sync_all().unwrap();
}

#[tokio::test]
async fn hard_volume_start_never_creates_job_for_replaced_foreign_deleting_or_unbound_claim() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    for variant in ["replacement", "foreign", "deleting", "pending", "ready"] {
        let (config, request) = hard_fixture();
        let volume = request.volume.as_ref().unwrap();
        let mut claim = json!({"apiVersion":"v1","kind":"PersistentVolumeClaim","metadata":{"name":volume.pvc_name,"namespace":volume.namespace,"uid":volume.pvc_uid,"resourceVersion":"1",
            "labels":{"app.kubernetes.io/managed-by":"webdock-build-storage","webdock.dev/customer-id":"12","webdock.dev/project-id":"34","webdock.dev/build-id":"56","webdock.dev/generation":"1","webdock.dev/volume-id":"1234"},
            "annotations":{"webdock.dev/allocation-sha256":volume.reservation.allocation_fingerprint().unwrap()}},
            "spec":{"volumeName":volume.pv_name,"accessModes":["ReadWriteOnce"],"volumeMode":"Filesystem","storageClassName":"","resources":{"requests":{"storage":"6Gi"}}},"status":{"phase":"Bound"}});
        match variant {
            "replacement" => claim["metadata"]["uid"] = json!("replacement"),
            "foreign" => claim["metadata"]["labels"]["webdock.dev/customer-id"] = json!("99"),
            "deleting" => claim["metadata"]["deletionTimestamp"] = json!("2026-10-30T07:00:00Z"),
            "pending" => claim["status"]["phase"] = json!("Pending"),
            _ => {}
        }
        let writes = Arc::new(AtomicUsize::new(0));
        let observed = writes.clone();
        let executor = executor_with_config(config, move |method, path, mut body| {
            if method == "GET" && path.contains("/persistentvolumeclaims/") {
                (200, claim.clone())
            } else if method == "POST" && variant == "ready" {
                observed.fetch_add(1, Ordering::SeqCst);
                body["metadata"]["uid"] = json!("job-uid-1");
                body["metadata"]["resourceVersion"] = json!("1");
                (201, body)
            } else {
                if method == "POST" {
                    observed.fetch_add(1, Ordering::SeqCst);
                }
                (
                    403,
                    json!({"kind":"Status","apiVersion":"v1","code":403,"reason":"Forbidden","message":"owned fixture"}),
                )
            }
        });
        let result = executor.start(&request).await;
        assert_eq!(result.is_ok(), variant == "ready", "{variant}: {result:?}");
        assert_eq!(
            writes.load(Ordering::SeqCst),
            usize::from(variant == "ready"),
            "{variant} claim controls Job publication"
        );
    }
}

#[tokio::test]
async fn held_job_requires_exact_uid_and_resource_version_before_execution_release() {
    let mut cfg = config();
    cfg.hold_execution = true;
    let stored = Arc::new(Mutex::new(None::<Value>));
    let remote = stored.clone();
    let executor = executor_with_config(cfg, move |method, _, mut body| {
        let mut job = remote.lock().unwrap();
        match method {
            "POST" => {
                body["metadata"]["uid"] = json!("held-uid");
                body["metadata"]["resourceVersion"] = json!("7");
                assert_eq!(body["spec"]["suspend"], true);
                *job = Some(body.clone());
                (201, body)
            }
            "PATCH" => {
                assert_eq!(body["metadata"]["uid"], "held-uid");
                assert_eq!(body["metadata"]["resourceVersion"], "7");
                assert_eq!(body["spec"]["suspend"], false);
                let value = job.as_mut().unwrap();
                value["spec"]["suspend"] = json!(false);
                (200, value.clone())
            }
            _ => (200, job.as_ref().unwrap().clone()),
        }
    });
    let handle = executor.start(&request()).await.unwrap();
    assert_eq!(
        stored.lock().unwrap().as_ref().unwrap()["spec"]["suspend"],
        true
    );
    executor.release(&handle).await.unwrap();
    executor.release(&handle).await.unwrap();
    assert_eq!(
        stored.lock().unwrap().as_ref().unwrap()["spec"]["suspend"],
        false
    );
    let mut wrong = handle;
    wrong.uid = "foreign-uid".into();
    assert!(executor.release(&wrong).await.is_err());
}

// The real client serializes/parses HTTP; the fake replaces only the remote API server.
fn executor_with(
    handler: impl Fn(&str, &str, Value) -> (u16, Value) + Send + Sync + 'static,
) -> KubernetesJobBuildExecutor {
    executor_with_config(config(), handler)
}

fn executor_with_config(
    config: KubernetesBuildConfig,
    handler: impl Fn(&str, &str, Value) -> (u16, Value) + Send + Sync + 'static,
) -> KubernetesJobBuildExecutor {
    let handler = Arc::new(handler);
    let client = Client::new(
        service_fn(move |req: Request<Body>| {
            let handler = handler.clone();
            async move {
                let method = req.method().to_string();
                let path = req.uri().to_string();
                let bytes = req.into_body().collect().await.unwrap().to_bytes();
                let body = if bytes.is_empty() {
                    Value::Null
                } else {
                    serde_json::from_slice(&bytes).unwrap()
                };
                let (status, body) = handler(&method, &path, body);
                Ok::<_, std::convert::Infallible>(
                    Response::builder()
                        .status(status)
                        .header("content-type", "application/json")
                        .body(Full::new(serde_json::to_vec(&body).unwrap().into()))
                        .unwrap(),
                )
            }
        }),
        "ignored-ambient-namespace",
    );
    KubernetesJobBuildExecutor::new(client, config).unwrap()
}

fn offline() -> KubernetesJobBuildExecutor {
    executor_with(|_, _, _| panic!("validation must not contact Kubernetes"))
}

fn status(code: u16) -> Value {
    json!({"apiVersion":"v1", "kind":"Status", "status":"Failure", "code":code,
        "reason":if code == 409 {"AlreadyExists"} else {"NotFound"}, "message":"test"})
}

fn stored(mut job: Value, uid: &str) -> Value {
    job["metadata"]["uid"] = json!(uid);
    job["metadata"]["resourceVersion"] = json!("10");
    // Go API JSON omits false non-pointer host flags and emits null template time.
    for key in ["hostNetwork", "hostPID", "hostIPC"] {
        job["spec"]["template"]["spec"]
            .as_object_mut()
            .unwrap()
            .remove(key);
    }
    job["spec"]["template"]["metadata"]["creationTimestamp"] = Value::Null;
    job["spec"]["completionMode"] = json!("NonIndexed");
    job["spec"]["manualSelector"] = json!(false);
    job["spec"]["suspend"] = json!(false);
    job["spec"]["podReplacementPolicy"] = json!("TerminatingOrFailed");
    job["spec"]["selector"] = json!({"matchLabels":{"batch.kubernetes.io/controller-uid":uid}});
    job["spec"]["template"]["metadata"]["labels"]["batch.kubernetes.io/controller-uid"] =
        json!(uid);
    job["spec"]["template"]["metadata"]["labels"]["batch.kubernetes.io/job-name"] =
        job["metadata"]["name"].clone();
    job
}

#[tokio::test]
async fn rejects_untrusted_projects_and_hostile_or_unbounded_requests_before_io() {
    let executor = offline();
    let mut hostile = Vec::new();
    let mut r = request();
    r.project_id = "other-project".into();
    hostile.push(r);
    let mut r = request();
    r.build_id = "../../secret".into();
    hostile.push(r);
    let mut r = request();
    r.commit_sha = "main; curl evil".into();
    hostile.push(r);
    let mut r = request();
    r.generation = 0;
    hostile.push(r);
    let mut r = request();
    r.attempt = 2;
    hostile.push(r);
    let mut r = request();
    r.limits.cpu_millis = 0;
    hostile.push(r);
    let mut r = request();
    r.limits.memory_mib = 65536;
    hostile.push(r);
    let mut r = request();
    r.limits.workspace_mib = u32::MAX;
    hostile.push(r);
    let mut r = request();
    r.limits.cache_mib = 0;
    hostile.push(r);
    let mut r = request();
    r.limits.deadline_seconds = 86400;
    hostile.push(r);
    for r in hostile {
        assert!(executor.manifest(&r).is_err(), "admitted {r:?}");
    }
    for field in [
        "namespace",
        "service_account",
        "sandbox_runtime_class",
        "runner_image",
    ] {
        let mut c = config();
        match field {
            "namespace" => c.namespace = "default".into(),
            "service_account" => c.service_account = "default".into(),
            "sandbox_runtime_class" => c.sandbox_runtime_class = "runc".into(),
            "runner_image" => c.runner_image = "runner:latest".into(),
            _ => unreachable!(),
        }
        let client = Client::new(
            service_fn(|_: Request<Body>| async {
                Ok::<_, std::convert::Infallible>(Response::new(Full::new(Vec::<u8>::new().into())))
            }),
            "default",
        );
        assert!(
            KubernetesJobBuildExecutor::new(client, c).is_err(),
            "admitted {field}"
        );
    }
}

#[tokio::test]
async fn native_railpack_cache_has_a_finite_sixteen_gib_budget_in_pod_storage() {
    let executor = offline();
    let mut input = request();
    input.limits.cache_mib = 16384;
    let job = serde_json::to_value(executor.manifest(&input).unwrap()).unwrap();
    let pod = &job["spec"]["template"]["spec"];
    assert_eq!(pod["volumes"][1]["emptyDir"]["sizeLimit"], "16Gi");
    assert_eq!(
        pod["containers"][0]["resources"]["limits"]["ephemeral-storage"],
        "20544Mi"
    );
    input.limits.cache_mib = 16385;
    assert!(executor.manifest(&input).is_err());
}

#[tokio::test]
async fn manifest_confines_processes_storage_and_identity_without_credentials() {
    let executor = offline();
    let job = serde_json::to_value(executor.manifest(&request()).unwrap()).unwrap();
    let spec = &job["spec"]["template"]["spec"];
    assert_eq!(spec["runtimeClassName"], "webdock-build-sandbox");
    assert_eq!(spec["automountServiceAccountToken"], false);
    assert_eq!(spec["hostNetwork"], false);
    assert_eq!(spec["hostPID"], false);
    assert_eq!(spec["hostIPC"], false);
    assert_eq!(
        spec["containers"][0]["securityContext"]["privileged"],
        false
    );
    assert_eq!(
        spec["containers"][0]["securityContext"]["allowPrivilegeEscalation"],
        false
    );
    assert_eq!(
        spec["securityContext"]["seccompProfile"]["type"],
        "RuntimeDefault"
    );
    assert_eq!(
        spec["containers"][0]["securityContext"]["capabilities"]["drop"],
        json!(["ALL"])
    );
    assert_eq!(
        spec["containers"][0]["securityContext"]["readOnlyRootFilesystem"],
        true
    );
    assert_eq!(spec["volumes"].as_array().unwrap().len(), 3);
    assert!(
        spec["volumes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v.get("emptyDir").is_some())
    );
    assert_eq!(job["spec"]["backoffLimit"], 0);
    assert_eq!(job["spec"]["activeDeadlineSeconds"], 900);
    assert_eq!(spec["activeDeadlineSeconds"], 900);
    assert!(
        spec["containers"][0]["env"]
            .as_array()
            .unwrap()
            .contains(&json!({"name":"TMPDIR","value":"/workspace"}))
    );
    assert_eq!(
        job["spec"]["template"]["metadata"]["labels"]["webdock.dev/build-id"],
        "build-1"
    );
    assert_ne!(executor.fingerprint(&request()).unwrap(), {
        let mut r = request();
        r.commit_sha = "c".repeat(40);
        executor.fingerprint(&r).unwrap()
    });
    assert_eq!(executor.job_name(&request()).unwrap().len(), 51);
}

#[tokio::test]
async fn adopts_only_exact_job_after_create_conflict_and_never_calls_it_verified_output() {
    let existing = Arc::new(Mutex::new(Value::Null));
    let state = existing.clone();
    let executor = executor_with(move |method, _, body| {
        if method == "POST" {
            let mut value = stored(body, "uid-original");
            value["spec"]["template"]["spec"]["dnsPolicy"] = json!("ClusterFirst");
            // Kubernetes canonicalizes resource quantities before returning them.
            for kind in ["limits", "requests"] {
                value["spec"]["template"]["spec"]["containers"][0]["resources"][kind]["cpu"] =
                    json!("2");
                value["spec"]["template"]["spec"]["containers"][0]["resources"][kind]["memory"] =
                    json!("2Gi");
            }
            value["spec"]["template"]["spec"]["volumes"][0]["emptyDir"]["sizeLimit"] = json!("4Gi");
            value["spec"]["template"]["spec"]["volumes"][1]["emptyDir"]["sizeLimit"] = json!("1Gi");
            value["status"] =
                json!({"succeeded":1,"conditions":[{"type":"Complete","status":"True"}]});
            *state.lock().unwrap() = value;
            (409, status(409))
        } else {
            (200, state.lock().unwrap().clone())
        }
    });
    let handle = executor.start(&request()).await.unwrap();
    assert_eq!(handle.uid, "uid-original");
    assert_eq!(
        executor.inspect(&handle).await.unwrap(),
        Some(KubernetesJobPhase::SucceededAwaitingRegistryVerification)
    );
    existing.lock().unwrap()["spec"]["template"]["spec"]["containers"][0]["securityContext"]["privileged"] =
        json!(true);
    assert!(executor.inspect(&handle).await.is_err());
}

#[tokio::test]
async fn refuses_spoofed_fingerprint_and_additional_containers_on_adoption() {
    for attack in [
        "image",
        "sidecar",
        "init",
        "environment",
        "future-field",
        "fingerprint",
    ] {
        let executor = executor_with(move |method, _, body| {
            assert_eq!(method, "POST");
            let mut job = stored(body, "uid-malicious");
            match attack {
                "image" => {
                    job["spec"]["template"]["spec"]["containers"][0]["image"] = json!("evil:latest")
                }
                "sidecar" => job["spec"]["template"]["spec"]["containers"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"name":"evil", "image":"evil:latest"})),
                "init" => {
                    job["spec"]["template"]["spec"]["initContainers"] =
                        json!([{"name":"evil","image":"evil:latest"}])
                }
                "environment" => job["spec"]["template"]["spec"]["containers"][0]["env"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"name":"AWS_SECRET_ACCESS_KEY","value":"secret"})),
                "future-field" => {
                    job["spec"]["template"]["spec"]["futureSecurityPolicy"] = json!("unconfined")
                }
                _ => {
                    job["metadata"]["annotations"]["webdock.dev/build-fingerprint"] =
                        json!("forged")
                }
            }
            (201, job)
        });
        assert!(
            executor.start(&request()).await.is_err(),
            "accepted {attack}"
        );
    }
}

#[tokio::test]
async fn cancellation_uses_foreground_uid_and_resource_version_preconditions() {
    let state = Arc::new(Mutex::new(Value::Null));
    let shared = state.clone();
    let executor = executor_with(move |method, _, body| match method {
        "POST" => {
            let job = stored(body, "uid-original");
            *shared.lock().unwrap() = job.clone();
            (201, job)
        }
        "GET" => (200, shared.lock().unwrap().clone()),
        "DELETE" => {
            assert_eq!(body["propagationPolicy"], "Foreground");
            assert_eq!(body["preconditions"]["uid"], "uid-original");
            assert_eq!(body["preconditions"]["resourceVersion"], "10");
            // Replacement between read and delete must remain a conflict.
            (409, status(409))
        }
        _ => panic!("unexpected method"),
    });
    let handle = executor.start(&request()).await.unwrap();
    assert!(executor.cancel(&handle).await.is_err());
    state.lock().unwrap()["metadata"]["uid"] = json!("uid-replacement");
    assert!(executor.inspect(&handle).await.is_err());
    assert!(executor.cancel(&handle).await.is_err());
}

#[tokio::test]
async fn deletion_absence_and_api_failures_are_not_execution_success() {
    let state = Arc::new(Mutex::new(Value::Null));
    let shared = state.clone();
    let executor = executor_with(move |method, _, body| {
        if method == "POST" {
            let job = stored(body, "uid-original");
            *shared.lock().unwrap() = job.clone();
            (201, job)
        } else {
            (404, status(404))
        }
    });
    let handle = executor.start(&request()).await.unwrap();
    assert_eq!(executor.inspect(&handle).await.unwrap(), None);
    assert!(executor.cancel(&handle).await.unwrap().is_none());
}

#[tokio::test]
async fn retirement_waits_for_pods_even_after_the_job_disappears() {
    let pods_present = Arc::new(Mutex::new(true));
    let state = pods_present.clone();
    let executor = executor_with(move |method, path, _| {
        assert_eq!(method, "GET");
        if path.contains("/pods?") {
            assert!(path.contains("limit=1"));
            assert!(path.contains("labelSelector="));
            (
                200,
                json!({"apiVersion":"v1","kind":"PodList","metadata":{},
                "items":if *state.lock().unwrap() {vec![json!({"metadata":{"name":"retained-pod"}})]} else {vec![]}}),
            )
        } else {
            (404, status(404))
        }
    });
    assert!(
        !executor
            .absent(&request(), Some("uid-original"))
            .await
            .unwrap()
    );
    *pods_present.lock().unwrap() = false;
    assert!(
        executor
            .absent(&request(), Some("uid-original"))
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn logs_reject_foreign_owner_and_never_trust_server_byte_limits() {
    let job_state = Arc::new(Mutex::new(Value::Null));
    let foreign = Arc::new(Mutex::new(false));
    let replacement = Arc::new(Mutex::new(false));
    let waiting = Arc::new(Mutex::new(false));
    let shared = job_state.clone();
    let owner = foreign.clone();
    let race = replacement.clone();
    let not_started = waiting.clone();
    let executor = executor_with(move |method, path, body| {
        if method == "POST" {
            let job = stored(body, "uid-original");
            *shared.lock().unwrap() = job.clone();
            return (201, job);
        }
        if path.contains("/log?") {
            assert!(
                !*not_started.lock().unwrap(),
                "must not request logs before runner starts"
            );
            assert!(path.contains("limitBytes=16"));
            assert!(path.contains("tailLines=10"));
            assert!(!path.contains("follow=true"));
            // Encoded JSON exceeds requested bytes; executor must enforce its own cap.
            return (200, json!("x".repeat(100000)));
        }
        let job = shared.lock().unwrap().clone();
        if path.contains("/pods") {
            let mut pod = json!({"apiVersion":"v1","kind":"Pod","metadata":job["spec"]["template"]["metadata"],"spec":job["spec"]["template"]["spec"]});
            pod["metadata"]["name"] = json!("runner-pod");
            pod["metadata"]["namespace"] = json!("webdock-builds");
            pod["metadata"]["uid"] = json!("uid-pod");
            pod["metadata"]["ownerReferences"] = json!([{"apiVersion":"batch/v1","kind":"Job","name":job["metadata"]["name"],"uid":if *owner.lock().unwrap(){"uid-foreign"}else{"uid-original"},"controller":true}]);
            if *not_started.lock().unwrap() {
                pod["status"] = json!({"containerStatuses":[{"name":"runner","image":"runner","imageID":"","ready":false,"restartCount":0,"state":{"waiting":{"reason":"PodInitializing"}}}]});
            }
            if path.contains("/pods?") {
                return (
                    200,
                    json!({"apiVersion":"v1","kind":"PodList","metadata":{},"items":[pod]}),
                );
            }
            if *race.lock().unwrap() {
                pod["metadata"]["uid"] = json!("uid-replaced");
            }
            return (200, pod);
        }
        (200, job)
    });
    let handle = executor.start(&request()).await.unwrap();
    let logs = executor
        .logs(
            &handle,
            KubernetesLogBounds {
                max_bytes: 16,
                tail_lines: 10,
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(logs.bytes.len(), 16);
    assert!(logs.truncated);
    *replacement.lock().unwrap() = true;
    assert!(
        executor
            .logs(
                &handle,
                KubernetesLogBounds {
                    max_bytes: 16,
                    tail_lines: 10
                }
            )
            .await
            .is_err()
    );
    *replacement.lock().unwrap() = false;
    *foreign.lock().unwrap() = true;
    assert!(
        executor
            .logs(
                &handle,
                KubernetesLogBounds {
                    max_bytes: 16,
                    tail_lines: 10
                }
            )
            .await
            .is_err()
    );
    assert!(
        executor
            .logs(
                &handle,
                KubernetesLogBounds {
                    max_bytes: u32::MAX,
                    tail_lines: 10
                }
            )
            .await
            .is_err()
    );
    *foreign.lock().unwrap() = false;
    *waiting.lock().unwrap() = true;
    assert!(
        executor
            .logs(
                &handle,
                KubernetesLogBounds {
                    max_bytes: 16,
                    tail_lines: 10
                }
            )
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn transport_errors_and_duplicate_starts_do_not_change_attempt_identity() {
    let existing = Arc::new(Mutex::new(Value::Null));
    let unavailable = Arc::new(Mutex::new(false));
    let shared = existing.clone();
    let failure = unavailable.clone();
    let executor = executor_with(move |method, _, body| {
        if *failure.lock().unwrap() {
            return (500, status(500));
        }
        if method == "POST" {
            let mut stored_job = shared.lock().unwrap();
            if !stored_job.is_null() {
                return (409, status(409));
            }
            *stored_job = stored(body, "uid-once");
            return (201, stored_job.clone());
        }
        (200, shared.lock().unwrap().clone())
    });
    let first = executor.start(&request()).await.unwrap();
    let second = executor.start(&request()).await.unwrap();
    assert_eq!(first.uid, second.uid);
    let mut changed = request();
    changed.commit_sha = "c".repeat(40);
    assert!(executor.start(&changed).await.is_err());
    *unavailable.lock().unwrap() = true;
    assert!(executor.inspect(&first).await.is_err());
    assert!(executor.cancel(&first).await.is_err());
    assert!(executor.absent(&request(), Some("uid-once")).await.is_err());
}

#[tokio::test]
async fn revoking_all_project_trust_blocks_starts_but_allows_exact_bound_cleanup() {
    let state = Arc::new(Mutex::new(Value::Null));
    let shared = state.clone();
    let admitted = executor_with(move |method, _, body| {
        assert_eq!(method, "POST");
        let job = stored(body, "uid-original");
        *shared.lock().unwrap() = job.clone();
        (201, job)
    });
    let handle = admitted.start(&request()).await.unwrap();
    let fingerprint = admitted.fingerprint(&request()).unwrap();
    let mut revoked_config = config();
    revoked_config.trusted_projects.clear();
    let shared = state.clone();
    let revoked = executor_with_config(revoked_config, move |method, path, body| match method {
        "GET" if path.contains("/pods?") => (
            200,
            json!({"apiVersion":"v1","kind":"PodList","metadata":{},"items":[]}),
        ),
        "GET" => {
            let job = shared.lock().unwrap().clone();
            if job.is_null() {
                (404, status(404))
            } else {
                (200, job)
            }
        }
        "DELETE" => {
            assert_eq!(body["preconditions"]["uid"], "uid-original");
            *shared.lock().unwrap() = Value::Null;
            (
                200,
                json!({"apiVersion":"v1","kind":"Status","status":"Success","code":200}),
            )
        }
        _ => panic!("revoked project must never create a Job"),
    });
    assert!(revoked.start(&request()).await.is_err());
    assert!(revoked.inspect(&handle).await.unwrap().is_some());
    let observed = revoked
        .observe(&request(), &fingerprint)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(observed.uid, "uid-original");
    state.lock().unwrap()["metadata"]["uid"] = json!("uid-foreign");
    assert!(revoked.cancel(&handle).await.is_err());
    state.lock().unwrap()["metadata"]["uid"] = json!("uid-original");
    assert_eq!(revoked.cancel(&handle).await.unwrap(), Some(()));
    assert!(
        revoked
            .absent(&request(), Some("uid-original"))
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn rootless_idmap_exception_is_typed_and_bound_to_qualified_namespace_and_runtime() {
    let mut qualified = config();
    qualified.profile = KubernetesBuildProfile::RootlessIdmapSandbox;
    qualified.namespace = "webdock-build-qualification".into();
    let executor = executor_with_config(qualified, |_, _, _| panic!("manifest needs no API"));
    let manifest = serde_json::to_value(executor.manifest(&request()).unwrap()).unwrap();
    let spec = &manifest["spec"]["template"]["spec"];
    assert_eq!(spec["securityContext"]["runAsUser"], 1000);
    assert_eq!(
        spec["volumes"][3]["secret"]["secretName"],
        executor.handoff_name(&request()).unwrap()
    );
    assert_eq!(
        spec["containers"][0]["volumeMounts"][3],
        json!({"name":"handoff","mountPath":"/handoff/attempt.json","subPath":"attempt.json","readOnly":true})
    );
    assert_eq!(
        spec["initContainers"][0]["args"],
        json!([
            "download",
            "--config",
            "/handoff/attempt.json",
            "--policy",
            "/etc/webdock-build/transfer-policy.json",
            "--input-dir",
            "/workspace/input"
        ])
    );
    assert_eq!(
        spec["initContainers"][0]["securityContext"]["allowPrivilegeEscalation"],
        false
    );
    assert!(spec["containers"][0]["env"].as_array().unwrap().contains(&json!({"name":"SPITZLI_JOB_UID","valueFrom":{"fieldRef":{"apiVersion":"v1","fieldPath":"metadata.labels['batch.kubernetes.io/controller-uid']"}}})));
    assert_eq!(
        spec["containers"][0]["command"],
        json!(["/usr/local/bin/webdock-build-entrypoint"])
    );
    assert_eq!(
        spec["containers"][0]["securityContext"]["allowPrivilegeEscalation"],
        true
    );
    assert_eq!(
        spec["containers"][0]["securityContext"]["capabilities"],
        json!({"drop":["ALL"],"add":["SETUID","SETGID"]})
    );
    assert_eq!(
        spec["containers"][0]["securityContext"]["privileged"],
        false
    );
    assert_eq!(
        spec["securityContext"]["seccompProfile"]["type"],
        "RuntimeDefault"
    );
    assert_ne!(
        executor.runtime_fingerprint(),
        offline().runtime_fingerprint()
    );
    let mut wrong_namespace = config();
    wrong_namespace.profile = KubernetesBuildProfile::RootlessIdmapSandbox;
    let mut wrong_runtime = wrong_namespace.clone();
    wrong_runtime.namespace = "webdock-build-qualification".into();
    wrong_runtime.sandbox_runtime_class = "other-sandbox".into();
    for invalid in [wrong_namespace, wrong_runtime] {
        // Constructor must reject an exception outside its fixed qualification.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            executor_with_config(invalid, |_, _, _| panic!("no API"))
        }));
        assert!(result.is_err());
    }
}

#[tokio::test]
async fn immutable_handoff_adopts_only_identical_capabilities_and_cleanup_uses_exact_uid() {
    let state = Arc::new(Mutex::new(Value::Null));
    let shared = state.clone();
    let handler = move |method: &str, path: &str, body: Value| {
        assert!(path.contains("/secrets"));
        let mut stored = shared.lock().unwrap();
        match method {
            "POST" if stored.is_null() => {
                *stored = body;
                stored["metadata"]["uid"] = json!("uid-secret");
                stored["metadata"]["resourceVersion"] = json!("10");
                (201, stored.clone())
            }
            "POST" => (409, status(409)),
            "GET" if stored.is_null() => (404, status(404)),
            "GET" => (200, stored.clone()),
            "DELETE" => {
                assert_eq!(body["preconditions"]["uid"], "uid-secret");
                assert_eq!(body["preconditions"]["resourceVersion"], "10");
                *stored = Value::Null;
                (
                    200,
                    json!({"kind":"Status","apiVersion":"v1","status":"Success","code":200}),
                )
            }
            _ => panic!("unexpected request"),
        }
    };
    let mut c = config();
    c.profile = KubernetesBuildProfile::RootlessIdmapSandbox;
    c.namespace = "webdock-build-qualification".into();
    let executor = executor_with_config(c.clone(), handler.clone());
    let handle = executor
        .stage_handoff(&request(), br#"{"attempt":"exact-scoped-capability"}"#)
        .await
        .unwrap();
    let adopted = executor
        .stage_handoff(&request(), br#"{"attempt":"exact-scoped-capability"}"#)
        .await
        .unwrap();
    assert_eq!(handle.uid, adopted.uid);
    assert!(
        executor
            .stage_handoff(&request(), br#"{"attempt":"changed-capability"}"#)
            .await
            .is_err()
    );
    c.trusted_projects.clear();
    let revoked = executor_with_config(c, handler);
    assert!(revoked.stage_handoff(&request(), b"{}").await.is_err());
    assert_eq!(
        revoked
            .observe_handoff(&request(), &handle.sha256)
            .await
            .unwrap()
            .unwrap()
            .uid,
        "uid-secret"
    );
    state.lock().unwrap()["metadata"]["uid"] = json!("uid-foreign");
    assert!(revoked.delete_handoff(&handle).await.is_err());
    state.lock().unwrap()["metadata"]["uid"] = json!("uid-secret");
    assert_eq!(revoked.delete_handoff(&handle).await.unwrap(), Some(()));
    assert!(
        revoked
            .observe_handoff(&request(), &handle.sha256)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn qualified_jobs_accept_only_harmless_init_container_api_defaults() {
    let mut c = config();
    c.profile = KubernetesBuildProfile::RootlessIdmapSandbox;
    c.namespace = "webdock-build-qualification".into();
    let executor = executor_with_config(c, |method, _, body| {
        assert_eq!(method, "POST");
        let mut job = stored(body, "uid-job");
        for key in ["containers", "initContainers"] {
            for container in job["spec"]["template"]["spec"][key].as_array_mut().unwrap() {
                container["terminationMessagePath"] = json!("/dev/termination-log");
                container["terminationMessagePolicy"] = json!("File");
            }
        }
        (201, job)
    });
    assert!(executor.start(&request()).await.is_ok());
}

#[tokio::test]
async fn availability_probe_only_lists_one_job_in_the_pinned_namespace() {
    let calls = Arc::new(Mutex::new(0));
    let observed = calls.clone();
    let executor = executor_with(move |method, path, _| {
        assert_eq!(method, "GET");
        assert!(path.starts_with("/apis/batch/v1/namespaces/webdock-builds/jobs?"));
        assert!(path.contains("limit=1"));
        *observed.lock().unwrap() += 1;
        (
            200,
            json!({"apiVersion":"batch/v1","kind":"JobList","metadata":{},"items":[]}),
        )
    });
    executor.available().await.unwrap();
    assert_eq!(*calls.lock().unwrap(), 1);
}
#[tokio::test]
async fn forbidden_or_unavailable_namespace_api_never_counts_as_available() {
    for code in [403, 503] {
        let executor = executor_with(move |method, path, _| {
            assert_eq!(method, "GET");
            assert!(path.contains("/namespaces/webdock-builds/jobs?"));
            (
                code,
                json!({"apiVersion":"v1","kind":"Status","status":"Failure","reason":"Unavailable","message":"owned API probe denial","code":code}),
            )
        });
        assert!(executor.available().await.is_err());
    }
}
