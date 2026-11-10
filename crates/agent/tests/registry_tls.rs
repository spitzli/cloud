#![cfg(feature = "docker")]
mod support;
use bollard::{Docker, models::*, query_parameters::*};
use futures_util::TryStreamExt;
use spitzli_agent::{
    docker::DockerAdapter,
    policy::{RuntimePolicy, RuntimePolicyConfig},
};
use spitzli_supervisor::*;
use std::{collections::HashMap, io::Write, os::unix::fs::PermissionsExt, sync::Arc};
const BUSYBOX: &str = "docker.io/library/busybox@sha256:bdf57e528e45e4433820e045b29b4597825a1c9e38353532d90a01445013f82e";
const REGISTRY: &str = "docker.io/library/registry@sha256:ddf754342cfc8acc51a56d5d0ab6af06826461864460636d8bd5c546dab2a7b8";
const NGINX: &str = "docker.io/library/nginx@sha256:970364228b1e4f932abc22d321932816782cca8a20b85a77552a7cfe9acfb3b5";
async fn pull(engine: &Docker, image: &str) {
    let mut stream = engine.create_image(
        Some(
            CreateImageOptionsBuilder::default()
                .from_image(image)
                .build(),
        ),
        None,
        None,
    );
    while let Some(progress) = stream.try_next().await.unwrap() {
        assert!(
            progress.error_detail.is_none(),
            "public fixture image pull failed"
        );
    }
}
fn private(path: &std::path::Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}
struct Authority;
#[async_trait::async_trait]
impl Authorizer for Authority {
    async fn authorize(&self, c: &Command) -> Result<()> {
        if c.spec.installation.starts_with("test-") {
            Ok(())
        } else {
            Err(ExecutionError::Denied)
        }
    }
}
#[tokio::test]
#[ignore = "explicit owned privileged nested Engine fixture; no host trust or socket mutations"]
async fn scoped_registry_requires_tls_and_readonly_credentials_even_for_cached_images() {
    assert_eq!(
        std::env::var("WEBDOCK_DISPOSABLE_REGISTRY_TEST").as_deref(),
        Ok("yes")
    );
    assert_eq!(rustix::process::geteuid().as_raw(), 0);
    assert!(std::path::Path::new("/.dockerenv").is_file());
    let engine = Docker::connect_with_unix(
        "/run/webdock-owned-docker.sock",
        10,
        bollard::API_DEFAULT_VERSION,
    )
    .unwrap()
    .negotiate_version()
    .await
    .unwrap();
    assert_eq!(
        engine.info().await.unwrap().docker_root_dir.as_deref(),
        Some("/var/lib/webdock-owned-docker")
    );
    let nonce = uuid::Uuid::new_v4();
    let suffix = &nonce.simple().to_string()[..12];
    let hostname = format!("registry-{suffix}.fixture");
    let authority = format!("{hostname}:5000");
    let network = format!("test-registry-{suffix}");
    let backend = format!("test-registry-backend-{suffix}");
    let proxy = format!("test-registry-proxy-{suffix}");
    let installation = format!("test-{suffix}");
    let numeric =
        u64::from_be_bytes(nonce.as_bytes()[..8].try_into().unwrap()) % 1_000_000_000_000 + 1000;
    let project = numeric.to_string();
    let repository = format!("owned/{project}/app");
    let base = tempfile::Builder::new()
        .prefix("webdock-registry-owned-")
        .tempdir()
        .unwrap();
    std::fs::set_permissions(base.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut ca_params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![
        rcgen::KeyUsagePurpose::KeyCertSign,
        rcgen::KeyUsagePurpose::DigitalSignature,
    ];
    let ca_key = rcgen::KeyPair::generate().unwrap();
    let ca = ca_params.self_signed(&ca_key).unwrap();
    let issuer = rcgen::Issuer::new(ca_params, ca_key);
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec![hostname.clone()])
        .unwrap()
        .signed_by(&key, &issuer)
        .unwrap();
    private(&base.path().join("cert.pem"), cert.pem().as_bytes());
    private(&base.path().join("key.pem"), key.serialize_pem().as_bytes());
    private(&base.path().join("ca.pem"), ca.pem().as_bytes());
    let password = "fixture-read-only";
    let mut hash = std::process::Command::new("/usr/bin/openssl")
        .args(["passwd", "-apr1", "-stdin"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    hash.stdin
        .take()
        .unwrap()
        .write_all(password.as_bytes())
        .unwrap();
    let output = hash.wait_with_output().unwrap();
    assert!(output.status.success());
    private(
        &base.path().join("htpasswd"),
        format!(
            "reader:{}\n",
            String::from_utf8(output.stdout).unwrap().trim()
        )
        .as_bytes(),
    );
    let labels = HashMap::from([
        ("dev.webdock.test".into(), "rust-registry-fixture".into()),
        ("dev.webdock.fixture".into(), nonce.to_string()),
    ]);
    pull(&engine, REGISTRY).await;
    pull(&engine, NGINX).await;
    pull(&engine, BUSYBOX).await;
    engine
        .create_network(NetworkCreateRequest {
            name: network.clone(),
            driver: Some("bridge".into()),
            internal: Some(false),
            labels: Some(labels.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .create_container(
            Some(
                CreateContainerOptionsBuilder::default()
                    .name(&backend)
                    .build(),
            ),
            ContainerCreateBody {
                image: Some(REGISTRY.into()),
                exposed_ports: Some(vec!["5000/tcp".into()]),
                env: Some(vec![
                    "GOMAXPROCS=2".into(),
                    "REGISTRY_LOG_LEVEL=error".into(),
                ]),
                labels: Some(labels.clone()),
                host_config: Some(HostConfig {
                    network_mode: Some(network.clone()),
                    readonly_rootfs: Some(true),
                    cap_drop: Some(vec!["ALL".into()]),
                    security_opt: Some(vec!["no-new-privileges:true".into()]),
                    tmpfs: Some(HashMap::from([(
                        "/var/lib/registry".into(),
                        "rw,nodev,nosuid,size=134217728".into(),
                    )])),
                    port_bindings: Some(HashMap::from([(
                        "5000/tcp".into(),
                        Some(vec![PortBinding {
                            host_ip: Some("127.0.0.1".into()),
                            host_port: Some("5001".into()),
                        }]),
                    )])),
                    memory: Some(268435456),
                    pids_limit: Some(64),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    engine
        .start_container(&backend, None::<StartContainerOptions>)
        .await
        .unwrap();
    let backend_ip = engine
        .inspect_container(&backend, None::<InspectContainerOptions>)
        .await
        .unwrap()
        .network_settings
        .unwrap()
        .networks
        .unwrap()[&network]
        .ip_address
        .clone()
        .unwrap();
    assert!(
        backend_ip
            .parse::<std::net::Ipv4Addr>()
            .unwrap()
            .is_private()
    );
    private(&base.path().join("nginx.conf"),format!("user root; worker_processes 1; pid /tmp/nginx.pid; events {{worker_connections 64;}} http {{access_log /dev/stdout; error_log /dev/stderr; client_body_temp_path /tmp/client; proxy_temp_path /tmp/proxy; server {{listen 5000 ssl; ssl_certificate /fixture/cert.pem; ssl_certificate_key /fixture/key.pem; auth_basic registry; auth_basic_user_file /fixture/htpasswd; location / {{limit_except GET HEAD {{deny all;}} proxy_pass http://{backend_ip}:5000; proxy_set_header Host $http_host;}}}}}}\n").as_bytes());
    engine
        .create_container(
            Some(
                CreateContainerOptionsBuilder::default()
                    .name(&proxy)
                    .build(),
            ),
            ContainerCreateBody {
                image: Some(NGINX.into()),
                exposed_ports: Some(vec!["5000/tcp".into()]),
                entrypoint: Some(vec!["nginx".into()]),
                cmd: Some(vec![
                    "-g".into(),
                    "daemon off;".into(),
                    "-c".into(),
                    "/fixture/nginx.conf".into(),
                ]),
                labels: Some(labels.clone()),
                host_config: Some(HostConfig {
                    network_mode: Some(network.clone()),
                    readonly_rootfs: Some(true),
                    cap_drop: Some(vec!["ALL".into()]),
                    cap_add: Some(vec!["SETGID".into()]),
                    security_opt: Some(vec!["no-new-privileges:true".into()]),
                    tmpfs: Some(HashMap::from([
                        ("/tmp".into(), "rw,nodev,nosuid,size=16777216".into()),
                        (
                            "/var/cache/nginx".into(),
                            "rw,nodev,nosuid,size=16777216".into(),
                        ),
                    ])),
                    mounts: Some(vec![Mount {
                        typ: Some(MountType::BIND),
                        source: Some(base.path().to_str().unwrap().into()),
                        target: Some("/fixture".into()),
                        read_only: Some(true),
                        ..Default::default()
                    }]),
                    port_bindings: Some(HashMap::from([(
                        "5000/tcp".into(),
                        Some(vec![PortBinding {
                            host_ip: Some("0.0.0.0".into()),
                            host_port: Some("5000".into()),
                        }]),
                    )])),
                    memory: Some(67108864),
                    pids_limit: Some(32),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    engine
        .start_container(&proxy, None::<StartContainerOptions>)
        .await
        .unwrap();
    let address = engine
        .inspect_container(&proxy, None::<InspectContainerOptions>)
        .await
        .unwrap()
        .network_settings
        .unwrap()
        .networks
        .unwrap()[&network]
        .ip_address
        .clone()
        .unwrap();
    let hosts_line = format!("{address} {hostname}");
    writeln!(
        std::fs::OpenOptions::new()
            .append(true)
            .open("/etc/hosts")
            .unwrap(),
        "{hosts_line}"
    )
    .unwrap();
    let ca_directory = std::path::PathBuf::from("/etc/docker/certs.d").join(&authority);
    assert!(!ca_directory.exists());
    std::fs::create_dir_all(&ca_directory).unwrap();
    let credentials =
        serde_json::json!({"auths":{authority.clone():{"username":"reader","password":password}}});
    let credential_file = base.path().join("credentials.json");
    private(&credential_file, &serde_json::to_vec(&credentials).unwrap());
    let policy_file = base.path().join("registry.json");
    private(&policy_file,&serde_json::to_vec(&serde_json::json!({"version":1,"euStorageEvidence":"owned local fixture; geography not asserted","projects":{project.clone():{"repository":format!("{authority}/{repository}"),"credentialFile":credential_file,"readOnly":true,"credentialScopeEvidence":"TLS proxy permits only GET/HEAD"}}})).unwrap());
    let mut spec = support::spec();
    spec.installation = installation.clone();
    spec.project_id = spitzli_core::id::RecordId::parse(&project).unwrap();
    spec.resource_id = spitzli_core::id::RecordId::parse(&(numeric + 1).to_string()).unwrap();
    spec.arguments = vec!["sleep".into(), "300".into()];
    spec.health_path = None;
    let name = spec.name();
    let test_engine = engine.clone();
    let test_base = base.path().to_path_buf();
    let test_ca_directory = ca_directory.clone();
    let test_authority = authority.clone();
    let test_repository = repository.clone();
    let test_ca = ca.pem();
    let test_address = address.clone();
    let test_hostname = hostname.clone();
    let test_credentials = credentials.clone();
    let test_project = project.clone();
    let result=tokio::spawn(async move {
        let probe=reqwest::Client::builder().no_proxy().timeout(std::time::Duration::from_secs(1)).build().unwrap();let mut ready=false;for _ in 0..50 {if probe.get("http://127.0.0.1:5001/v2/").send().await.is_ok_and(|r|r.status().is_success()){ready=true;break}tokio::time::sleep(std::time::Duration::from_millis(100)).await;}assert!(ready,"owned registry did not start");
        let writer=format!("127.0.0.1:5001/{test_repository}");test_engine.tag_image(BUSYBOX,Some(TagImageOptionsBuilder::default().repo(&writer).tag("fixture").build())).await.unwrap();let mut push=test_engine.push_image(&writer,Some(PushImageOptionsBuilder::default().tag("fixture").build()),None);while let Some(progress)=push.try_next().await.unwrap(){assert!(progress.error_detail.is_none(),"owned fixture registry push failed");}
        let digest=test_engine.inspect_image(&format!("{writer}:fixture")).await.unwrap().repo_digests.unwrap().into_iter().find_map(|value|value.strip_prefix(&format!("{writer}@")).map(str::to_owned)).unwrap();spec.image=format!("{test_authority}/{test_repository}@{digest}");
        let policy=RuntimePolicy::load(RuntimePolicyConfig{registry_config:Some(policy_file),..Default::default()}).unwrap();let adapter=DockerAdapter::connect("/run/webdock-owned-docker.sock",installation,None).await.unwrap().with_policy(Arc::new(policy));let command=Command{operation_id:"registry-apply".into(),generation:1,lease_until:timestamp().unwrap()+300,authorization:"fixture".into(),action:Action::Apply,spec:spec.clone(),expected_runtime_id:None,material:None};
        // The nested Engine's Go resolver caches the hosts file for five seconds.
        tokio::time::sleep(std::time::Duration::from_secs(6)).await;
        assert!(adapter.reconcile(&command,&LeaseGuard::new(command.clone(),Arc::new(Authority))).await.is_err(),"private CA must be explicitly trusted by nested Engine");
        private(&test_ca_directory.join("ca.crt"),test_ca.as_bytes());let observed=adapter.reconcile(&command,&LeaseGuard::new(command.clone(),Arc::new(Authority))).await.unwrap();assert_eq!(observed.phase,Phase::Running);assert_eq!(adapter.target_observation().await.unwrap().unwrap().registry_version,Some(1));
        let mut foreign=command.clone();foreign.spec.project_id=spitzli_core::id::RecordId::parse(&(numeric+2).to_string()).unwrap();assert!(adapter.reconcile(&foreign,&LeaseGuard::new(foreign.clone(),Arc::new(Authority))).await.is_err(),"cached private image still requires its project grant");
        let client=reqwest::Client::builder().no_proxy().add_root_certificate(reqwest::Certificate::from_pem(test_ca.as_bytes()).unwrap()).resolve(&test_hostname,std::net::SocketAddr::new(test_address.parse().unwrap(),5000)).build().unwrap();assert_eq!(client.post(format!("https://{test_authority}/v2/{test_repository}/blobs/uploads/")).basic_auth("reader",Some(password)).send().await.unwrap().status().as_u16(),403);
        let state=serde_json::json!({"authority":test_authority,"image":spec.image,"projectID":test_project,"ca":test_ca,"dockerConfig":test_credentials,"base":test_base});private(std::path::Path::new("/tmp/webdock-registry-fixture.json"),&serde_json::to_vec(&state).unwrap());
    }).await;
    if result.is_err() {
        for container in [&proxy] {
            let mut logs = engine.logs(
                container,
                Some(
                    LogsOptionsBuilder::default()
                        .stdout(true)
                        .stderr(true)
                        .tail("20")
                        .build(),
                ),
            );
            while let Ok(Some(line)) = logs.try_next().await {
                eprintln!("fixture {container}: {line}");
            }
        }
    }
    let _ = engine
        .remove_container(
            &name,
            Some(
                RemoveContainerOptionsBuilder::default()
                    .force(true)
                    .v(false)
                    .build(),
            ),
        )
        .await;
    let _ = engine.remove_network(&format!("{name}-net")).await;
    let keep =
        result.is_ok() && std::env::var("WEBDOCK_KEEP_REGISTRY_FIXTURE").as_deref() == Ok("yes");
    if keep {
        let _ = base.keep();
        eprintln!(
            "Scoped TLS registry fixture retained for owned Kind pull test; state is private"
        );
    } else {
        let _ = engine
            .remove_container(
                &proxy,
                Some(
                    RemoveContainerOptionsBuilder::default()
                        .force(true)
                        .v(false)
                        .build(),
                ),
            )
            .await;
        let _ = engine
            .remove_container(
                &backend,
                Some(
                    RemoveContainerOptionsBuilder::default()
                        .force(true)
                        .v(false)
                        .build(),
                ),
            )
            .await;
        let _ = engine.remove_network(&network).await;
        let _ = std::fs::remove_file("/tmp/webdock-registry-fixture.json");
        std::fs::remove_dir_all(&ca_directory).unwrap();
        let hosts = std::fs::read_to_string("/etc/hosts")
            .unwrap()
            .lines()
            .filter(|line| *line != hosts_line)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        std::fs::write("/etc/hosts", hosts).unwrap();
    }
    result.unwrap();
}

#[cfg(feature = "kubernetes")]
#[tokio::test]
#[ignore = "explicit owned Kind node plus retained scoped TLS registry fixture"]
async fn owned_kind_uses_project_pull_secret_and_private_registry_ca() {
    use k8s_openapi::api::core::v1::{Namespace, Pod, Secret};
    use kube::{
        Api, Client, Config,
        api::{DeleteParams, PostParams},
        config::{KubeConfigOptions, Kubeconfig},
    };
    use spitzli_agent::kubernetes::{ApprovedChart, KubernetesAdapter};
    assert_eq!(
        std::env::var("WEBDOCK_DISPOSABLE_NODE_TEST").as_deref(),
        Ok("yes")
    );
    assert_eq!(rustix::process::geteuid().as_raw(), 0);
    assert!(std::path::Path::new("/.dockerenv").is_file());
    let fixture = std::path::PathBuf::from(std::env::var("WEBDOCK_REGISTRY_FIXTURE_DIR").unwrap());
    let state: serde_json::Value = serde_json::from_slice(
        &spitzli_agent::config::read_private(&fixture.join("registry-state.json"), 65536).unwrap(),
    )
    .unwrap();
    let authority = state["authority"].as_str().unwrap().to_owned();
    let hostname = authority.strip_suffix(":5000").unwrap().to_owned();
    assert!(
        hostname.starts_with("registry-")
            && hostname.ends_with(".fixture")
            && hostname
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
    );
    let address: std::net::Ipv4Addr = state["address"].as_str().unwrap().parse().unwrap();
    assert!(address.is_private());
    let hosts_line = format!("{address} {hostname}");
    writeln!(
        std::fs::OpenOptions::new()
            .append(true)
            .open("/etc/hosts")
            .unwrap(),
        "{hosts_line}"
    )
    .unwrap();
    let cert_directory = std::path::PathBuf::from("/etc/containerd/certs.d").join(&authority);
    assert!(!cert_directory.exists());
    std::fs::create_dir_all(&cert_directory).unwrap();
    private(
        &cert_directory.join("ca.crt"),
        state["ca"].as_str().unwrap().as_bytes(),
    );
    private(&cert_directory.join("hosts.toml"),format!("server = \"https://{authority}\"\n[host.\"https://{authority}\"]\n  capabilities = [\"pull\", \"resolve\"]\n  ca = \"{}/ca.crt\"\n",cert_directory.display()).as_bytes());
    let kubeconfig = std::path::PathBuf::from(std::env::var("WEBDOCK_KUBE_TEST_CONFIG").unwrap());
    let context = std::env::var("WEBDOCK_KUBE_TEST_CONTEXT").unwrap();
    assert!(context.starts_with("kind-webdock-rust-"));
    let config = Config::from_custom_kubeconfig(
        Kubeconfig::read_from(&kubeconfig).unwrap(),
        &KubeConfigOptions {
            context: Some(context.clone()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let client = Client::try_from(config).unwrap();
    let project = state["projectID"].as_str().unwrap();
    let numeric = project.parse::<u64>().unwrap();
    let mut spec = support::spec();
    spec.project_id = spitzli_core::id::RecordId::parse(project).unwrap();
    spec.resource_id = spitzli_core::id::RecordId::parse(&(numeric + 100).to_string()).unwrap();
    spec.image = state["image"].as_str().unwrap().into();
    spec.arguments = vec![
        "sh".into(),
        "-c".into(),
        "printf registry-ok >/tmp/index.html; exec httpd -f -p 8080 -h /tmp".into(),
    ];
    let quota = ResourceAllowance {
        apps: 2,
        cpu_millicores: 1000,
        memory_bytes: 1073741824,
        volume_bytes: 0,
        ephemeral_bytes: 1073741824,
        replicas_per_app: 1,
        concurrent_deployments: 1,
    };
    spec.hosting_binding = Some(HostingBinding {
        public_route: None,
        cluster_id: spitzli_core::id::RecordId::parse("789").unwrap(),
        quota: quota.clone(),
        final_quota: quota,
    });
    let namespaces: Api<Namespace> = Api::all(client.clone());
    let owned_names = [format!("wd-{project}"), format!("wdv-{project}")];
    let labels = spitzli_agent::legacy::owner_labels(&spec, false).unwrap();
    for namespace in &owned_names {
        let mut ns_labels = labels.clone();
        ns_labels.insert(
            "pod-security.kubernetes.io/enforce".into(),
            "restricted".into(),
        );
        namespaces
            .create(
                &PostParams::default(),
                &Namespace {
                    metadata: kube::core::ObjectMeta {
                        name: Some(namespace.clone()),
                        labels: Some(ns_labels),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let secrets: Api<Secret> = Api::namespaced(client.clone(), namespace);
        secrets
            .create(
                &PostParams::default(),
                &Secret {
                    metadata: kube::core::ObjectMeta {
                        name: Some("registry-reader".into()),
                        labels: Some(labels.clone()),
                        ..Default::default()
                    },
                    type_: Some("kubernetes.io/dockerconfigjson".into()),
                    data: Some(std::collections::BTreeMap::from([(
                        ".dockerconfigjson".into(),
                        k8s_openapi::ByteString(
                            serde_json::to_vec(&state["dockerConfig"]).unwrap(),
                        ),
                    )])),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }
    let repository = spec.image.split('@').next().unwrap();
    let policy_path = fixture.join("registry.json");
    private(&policy_path,&serde_json::to_vec(&serde_json::json!({"version":1,"euStorageEvidence":"owned local fixture; geography not asserted","projects":{project:{"repository":repository,"pullSecret":"registry-reader","readOnly":true,"credentialScopeEvidence":"TLS proxy permits only GET/HEAD"}}})).unwrap());
    let policy = RuntimePolicy::load(RuntimePolicyConfig {
        registry_config: Some(policy_path),
        ..Default::default()
    })
    .unwrap();
    let chart_path = fixture.join("chart");
    let chart =
        ApprovedChart::verify(&chart_path, &ApprovedChart::digest(&chart_path).unwrap()).unwrap();
    let adapter = KubernetesAdapter::connect(
        kubeconfig,
        context,
        spec.installation.clone(),
        fixture.join("helm"),
        chart,
    )
    .await
    .unwrap()
    .with_policy(Arc::new(policy));
    let task_client = client.clone();
    let result = tokio::spawn(async move {
        assert_eq!(
            adapter
                .target_observation()
                .await
                .unwrap()
                .unwrap()
                .registry_version,
            Some(1)
        );
        let command = Command {
            operation_id: "private-registry-pull".into(),
            generation: 1,
            lease_until: timestamp().unwrap() + 300,
            authorization: "fixture".into(),
            action: Action::Apply,
            spec: spec.clone(),
            expected_runtime_id: None,
            material: None,
        };
        let observed = adapter
            .reconcile(
                &command,
                &LeaseGuard::new(command.clone(), Arc::new(Authority)),
            )
            .await
            .unwrap();
        assert!(observed.ready);
        let pods: Api<Pod> = Api::namespaced(task_client, &spitzli_agent::legacy::namespace(&spec));
        let pods = pods
            .list(
                &kube::api::ListParams::default()
                    .labels(&format!("webdock.dev/app-id={}", spec.resource_id)),
            )
            .await
            .unwrap();
        assert_eq!(pods.items.len(), 1);
        assert_eq!(
            pods.items[0]
                .spec
                .as_ref()
                .unwrap()
                .image_pull_secrets
                .as_ref()
                .unwrap()[0]
                .name,
            "registry-reader"
        );
        let mut denied = command.clone();
        denied.spec.project_id =
            spitzli_core::id::RecordId::parse(&(numeric + 1).to_string()).unwrap();
        assert!(
            adapter
                .reconcile(
                    &denied,
                    &LeaseGuard::new(denied.clone(), Arc::new(Authority))
                )
                .await
                .is_err()
        );
    })
    .await;
    for namespace in owned_names {
        let _ = namespaces
            .delete(&namespace, &DeleteParams::default())
            .await;
        for _ in 0..200 {
            if namespaces.get_opt(&namespace).await.unwrap().is_none() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    }
    std::fs::remove_dir_all(&cert_directory).unwrap();
    let hosts = std::fs::read_to_string("/etc/hosts")
        .unwrap()
        .lines()
        .filter(|line| *line != hosts_line)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write("/etc/hosts", hosts).unwrap();
    result.unwrap();
}
