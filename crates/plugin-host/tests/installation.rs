//! This suite owns its PostgreSQL container and every role. It never uses developer credentials.
use sea_orm::{
    ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement, TransactionTrait,
};
use std::{collections::BTreeMap, process::Command};
#[path = "support/catalog_fixture.rs"]
mod catalog_fixture;
use serde_json::json;
use std::sync::Arc;
use webdock_agent_protocol::plugin::*;
use webdock_browser_session::StoredAuthority;
use webdock_identity::{CookiePolicy, IdentityService};
use webdock_plugin_host::catalog::Catalog;
use webdock_plugin_host::{
    FileBindings, HostService, InstallRequest, ManagedBindings, PeerEnrollment,
    PreparedInstallation, RuntimeBinding, RuntimeDescription,
};
fn docker() -> Command {
    let mut command = Command::new("docker");
    command.args(["--host", "unix:///var/run/docker.sock"]);
    command
}
struct Fixture {
    name: String,
    url: url::Url,
    passwords: BTreeMap<String, String>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = docker().args(["rm", "-f", &self.name]).output();
    }
}
impl Fixture {
    async fn start() -> Self {
        let name = format!("wd-install-task1-{}", uuid::Uuid::new_v4().simple());
        assert!(
            docker()
                .args([
                    "run",
                    "-d",
                    "--rm",
                    "--name",
                    &name,
                    "-e",
                    "POSTGRES_HOST_AUTH_METHOD=trust",
                    "-e",
                    "POSTGRES_DB=webdock_install_test",
                    "-p",
                    "127.0.0.1::5432",
                    "postgres:18-alpine"
                ])
                .output()
                .unwrap()
                .status
                .success()
        );
        let output = docker().args(["port", &name, "5432/tcp"]).output().unwrap();
        let endpoint = String::from_utf8(output.stdout).unwrap();
        let endpoint = endpoint.trim();
        let url = url::Url::parse(&format!(
            "postgres://postgres@{endpoint}/webdock_install_test?sslmode=disable"
        ))
        .unwrap();
        let passwords = webdock_persistence::roles::LOGIN_ROLES
            .iter()
            .copied()
            .chain(["webdock_offline_migrate"])
            .map(|role| {
                (
                    role.to_owned(),
                    format!("private-{}", uuid::Uuid::new_v4().simple()),
                )
            })
            .collect();
        let fixture = Self {
            name,
            url,
            passwords,
        };
        let db = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                if let Ok(db) = Database::connect(fixture.url.as_str()).await {
                    break db;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("owned PostgreSQL connection readiness");
        webdock_persistence::roles::provision_roles(
            &db,
            "webdock_install_test",
            &fixture.passwords,
        )
        .await
        .unwrap();
        webdock_persistence::migrations::bootstrap(
            &db,
            "webdock_install_test",
            "owned-install-rls-secret-at-least32",
        )
        .await
        .unwrap();
        fixture
    }
    async fn owner(&self) -> DatabaseConnection {
        Database::connect(self.url.as_str()).await.unwrap()
    }
    async fn role(&self, role: &str) -> DatabaseConnection {
        let mut url = self.url.clone();
        url.set_username(role).unwrap();
        url.set_password(Some(&self.passwords[role])).unwrap();
        let mut options = sea_orm::ConnectOptions::new(url.as_str());
        options
            .max_connections(1)
            .min_connections(0)
            .sqlx_logging(false);
        Database::connect(options).await.unwrap()
    }
}
async fn sql(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .await
    .unwrap();
}
async fn scalar(db: &DatabaseConnection, sql: &str) -> i64 {
    db.query_one_raw(Statement::from_string(DbBackend::Postgres, sql))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "n")
        .unwrap()
}
#[tokio::test]
#[ignore = "owns a disposable Docker PostgreSQL container"]
async fn native_empty_bootstrap_has_plugin_state_without_products_or_dummy_owners() {
    let fixture = Fixture::start().await;
    let db = fixture.owner().await;
    assert_eq!(
        scalar(
            &db,
            "SELECT count(*)::bigint n FROM webdock_auth.plugin_bootstrap WHERE mode='fresh'"
        )
        .await,
        1
    );
    for table in [
        "webdock_auth.plugin_installation",
        "webdock_admin.customers",
        "webdock_admin.projects",
    ] {
        assert_eq!(
            scalar(&db, &format!("SELECT count(*)::bigint n FROM {table}")).await,
            0
        );
    }
}

#[tokio::test]
#[ignore = "owns a disposable Docker PostgreSQL container"]
async fn restricted_install_replay_fencing_promotion_and_retained_retry() {
    let fixture = Fixture::start().await;
    let owner = fixture.owner().await;
    let auth = fixture.role("webdock_auth_bootstrap").await;
    let business = fixture.role("webdock_auth_runtime").await;
    let worker = fixture.role("webdock_worker_runtime").await;
    sql(&owner,"INSERT INTO webdock_auth.\"user\"(id,name,email,\"emailVerified\",role,\"twoFactorEnabled\",\"mustChangePassword\") VALUES('operator','Operator','operator@example.test',true,'operator',true,false)",vec![]).await;
    sql(&owner,"INSERT INTO webdock_auth.session(id,token,\"userId\",\"expiresAt\",\"updatedAt\") VALUES('session','private-token','operator',now()+interval '1 hour',now())",vec![]).await;
    sql(&owner,"INSERT INTO webdock_admin.users(id,name,email,role,auth_subject) VALUES('100','Operator','operator@example.test','operator','operator')",vec![]).await;
    sql(&owner,"INSERT INTO webdock_auth.hosting_cluster(id,name,provider,region,location_evidence,verified,runtime_installation,runtime_kind) VALUES('42','Owned fixture','fixture','eu','fixture',true,'fixture','kubernetes')",vec![]).await;
    sql(&owner,"INSERT INTO webdock_auth.hosting_agent(cluster_id,generation,credential_hash,revoked) VALUES('42',1,$1,false)",vec!["a".repeat(64).into()]).await;
    // Synthetic transport enrollment is fixture setup; acceptance uses the separately owned offline CLI.
    sql(&owner,"INSERT INTO webdock_auth.agent_transport_enrollment(installation,agent_id,generation,cluster_id,credential_hash,transport,unix_uid) VALUES('fixture','agent',1,'42',$1,'unix',0)",vec!["a".repeat(64).into()]).await;
    let identity = Arc::new(IdentityService::new(
        auth.clone(),
        CookiePolicy::new(false, "owned-install-auth-secret-atleast32").unwrap(),
    ));
    let identity_tx = auth.begin().await.unwrap();
    let principal = identity
        .authenticate_session_in(&identity_tx, "operator", "session")
        .await
        .unwrap()
        .unwrap();
    identity_tx.commit().await.unwrap();
    let directory = tempfile::tempdir().unwrap();
    let fallback = Arc::new(FileBindings::new(directory.path().join("missing.json")));
    // Empty file enrollment is explicit, rather than treating a missing/unreadable fallback as empty.
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(directory.path().join("missing.json"), b"{\"runtimes\":{}}").unwrap();
    std::fs::set_permissions(
        directory.path().join("missing.json"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let bindings = Arc::new(
        ManagedBindings::new(
            fixture.role("webdock_worker_runtime").await,
            "owned-install-rls-secret-at-least32".into(),
            fallback,
        )
        .await
        .unwrap()
        .with_encryption_secret("owned-install-encryption-key-at-least32")
        .unwrap(),
    );
    let ids = Arc::new(
        webdock_persistence::IdAllocator::connect(
            fixture.url.as_str(),
            webdock_ids::Identity::platform(0, 2).unwrap(),
        )
        .await
        .unwrap(),
    );
    let catalog = Arc::new(
        Catalog::from_bytes(&serde_json::to_vec(&catalog_fixture::with_releases()).unwrap())
            .unwrap(),
    );
    let host = HostService::new(
        business.clone(),
        worker.clone(),
        "owned-install-rls-secret-at-least32".into(),
        ids,
        bindings.clone(),
        ed25519_dalek::SigningKey::from_bytes(&[19; 32]),
    )
    .await
    .unwrap()
    .with_session_authority(auth.clone(), identity.clone(), None)
    .await
    .unwrap();
    let authority = StoredAuthority::LocalSession {};
    assert!(
        host.catalog_with_authority(&principal, &authority)
            .await
            .unwrap()
            .is_empty()
    );
    let host = host
        .with_installation(catalog.clone(), "owned-install-encryption-key-at-least32")
        .unwrap();
    let descriptor = &catalog.descriptors()[0];
    let request = InstallRequest {
        catalog_id: descriptor.catalog_id.clone(),
        version: descriptor.version.clone(),
        review_sha256: descriptor.targets[0].review_sha256.clone(),
        target_id: "local-eu".into(),
        configuration: json!({}),
        approved_capabilities: descriptor.capabilities.clone(),
        idempotency_key: "install-1".into(),
    };
    let authority = StoredAuthority::LocalSession {};
    sql(
        &owner,
        "DELETE FROM webdock_auth.agent_transport_enrollment WHERE cluster_id='42'",
        vec![],
    )
    .await;
    assert!(
        host.catalog_with_authority(&principal, &authority)
            .await
            .unwrap()[0]
            .targets
            .is_empty()
    );
    assert!(
        host.request_install_with_authority(&principal, &authority, request.clone())
            .await
            .is_err(),
        "native admission requires offline transport enrollment, not just a verified cluster"
    );
    assert_eq!(
        scalar(
            &owner,
            "SELECT count(*)::bigint n FROM webdock_auth.plugin_installation"
        )
        .await,
        0
    );
    sql(&owner,"INSERT INTO webdock_auth.agent_transport_enrollment(installation,agent_id,generation,cluster_id,credential_hash,transport,unix_uid) VALUES('fixture','agent',1,'42',$1,'unix',0)",vec!["a".repeat(64).into()]).await;
    std::fs::write(directory.path().join("missing.json"),serde_json::to_vec(&json!({"runtimes":{"postgres":{"generation":1,"namespace":"legacy","service":"runtime","credentialSha256":"a".repeat(64),"peers":[{"kind":"unix","uid":rustix::process::geteuid().as_raw()}]}}})).unwrap()).unwrap();
    assert!(
        host.request_install_with_authority(&principal, &authority, request.clone())
            .await
            .is_err(),
        "legacy enrollment must prevent managed admission"
    );
    std::fs::write(directory.path().join("missing.json"), b"{\"runtimes\":{}}").unwrap();
    let accepted = host
        .request_install_with_authority(&principal, &authority, request.clone())
        .await
        .unwrap();
    assert_eq!(
        host.operation_with_authority(&principal, &authority, &accepted.id)
            .await
            .unwrap()
            .kind,
        "install"
    );
    assert_eq!(
        host.installed_with_authority(&principal, &authority)
            .await
            .unwrap()[0]["operationID"],
        accepted.id
    );
    assert_eq!(
        host.request_install_with_authority(&principal, &authority, request.clone())
            .await
            .unwrap()
            .id,
        accepted.id
    );
    let mut changed_catalog = catalog_fixture::catalog();
    changed_catalog["targets"][0]["policy"]["secretRef"] = json!("changed-after-admission");
    let host = host
        .with_installation(
            Arc::new(Catalog::from_bytes(&serde_json::to_vec(&changed_catalog).unwrap()).unwrap()),
            "owned-install-encryption-key-at-least32",
        )
        .unwrap();
    assert_eq!(
        host.request_install_with_authority(&principal, &authority, request.clone())
            .await
            .unwrap()
            .id,
        accepted.id,
        "exact replay does not re-admit against a changed catalog"
    );
    let host = host
        .with_installation(catalog.clone(), "owned-install-encryption-key-at-least32")
        .unwrap();
    let mut changed = request.clone();
    changed.configuration = json!({"changed":true});
    assert!(
        host.request_install_with_authority(&principal, &authority, changed)
            .await
            .is_err()
    );
    assert_eq!(
        scalar(
            &owner,
            "SELECT count(*)::bigint n FROM webdock_admin.projects"
        )
        .await,
        0
    );
    assert_eq!(
        scalar(
            &business,
            "SELECT count(*)::bigint n FROM webdock_auth.plugin_install_execution"
        )
        .await,
        0
    );
    // Reproduce the terminal pre-claim authority loss from review: availability must not
    // bump the installation generation and make its own retained retry impossible.
    sql(&owner,"UPDATE webdock_auth.session SET \"expiresAt\"=now()-interval '1 second' WHERE id='session'",vec![]).await;
    assert!(host.claim_installation(None).await.unwrap().is_none());
    sql(
        &owner,
        "INSERT INTO webdock_auth.session(id,token,\"userId\",\"expiresAt\",\"updatedAt\") VALUES('recovery-session','recovery-token','operator',now()+interval '1 hour',now())",
        vec![],
    )
    .await;
    let recovered = auth.begin().await.unwrap();
    let principal = identity
        .authenticate_session_in(&recovered, "operator", "recovery-session")
        .await
        .unwrap()
        .unwrap();
    recovered.commit().await.unwrap();
    let before = host
        .installed_with_authority(&principal, &authority)
        .await
        .unwrap()[0]
        .clone();
    let generation = scalar(
        &owner,
        "SELECT generation AS n FROM webdock_auth.plugin_installation WHERE id='postgres'",
    )
    .await;
    let operations = scalar(
        &owner,
        "SELECT count(*)::bigint n FROM webdock_auth.plugin_operation",
    )
    .await;
    for desired in ["enabled", "disabled", "draining"] {
        let error = host
            .request_state_with_authority(
                &principal,
                &authority,
                "postgres",
                desired,
                before["revision"].as_str().unwrap(),
                &format!("unpromoted-state-{desired}"),
            )
            .await
            .expect_err("unpromoted managed installations must reject availability commands");
        assert_eq!(error.code, webdock_plugin_protocol::ErrorCode::Conflict);
    }
    let after = host
        .installed_with_authority(&principal, &authority)
        .await
        .unwrap()[0]
        .clone();
    assert_eq!(after["installRecovery"], true);
    assert_eq!(after["revision"], before["revision"]);
    assert_eq!(
        scalar(
            &owner,
            "SELECT generation AS n FROM webdock_auth.plugin_installation WHERE id='postgres'"
        )
        .await,
        generation
    );
    assert_eq!(
        scalar(
            &owner,
            "SELECT count(*)::bigint n FROM webdock_auth.plugin_operation"
        )
        .await,
        operations
    );
    let accepted = host
        .retry_install_with_authority(
            &principal,
            &authority,
            &accepted.id,
            "retry-preclaim-authority",
        )
        .await
        .unwrap();
    let old = host.claim_installation(None).await.unwrap().unwrap();
    host.persist_install_components(
        &old,
        BTreeMap::from([
            ("storage-password".into(), "private-fixed-password".into()),
            ("storagePassword".into(), "fixed-master".into()),
            ("sealingKey".into(), "fixed-sealer".into()),
        ]),
    )
    .await
    .unwrap();
    assert_eq!(scalar(&owner,"SELECT count(*)::bigint n FROM webdock_auth.plugin_install_execution WHERE private_components::text LIKE '%private-fixed-password%'").await,0);
    sql(
        &owner,
        "UPDATE webdock_auth.agent_transport_enrollment SET revoked=true WHERE cluster_id='42'",
        vec![],
    )
    .await;
    assert!(
        !host.guard_installation(&old).await.unwrap(),
        "revocation after admission must stop private storage preparation"
    );
    assert!(host.install_work(&old).await.is_err());
    sql(
        &owner,
        "UPDATE webdock_auth.agent_transport_enrollment SET revoked=false WHERE cluster_id='42'",
        vec![],
    )
    .await;
    sql(
        &owner,
        "UPDATE webdock_auth.\"user\" SET \"twoFactorEnabled\"=false WHERE id='operator'",
        vec![],
    )
    .await;
    assert!(!host.guard_installation(&old).await.unwrap());
    sql(
        &owner,
        "UPDATE webdock_auth.\"user\" SET \"twoFactorEnabled\"=true WHERE id='operator'",
        vec![],
    )
    .await;
    assert!(
        host.persist_install_components(
            &old,
            BTreeMap::from([("storage-password".into(), "replacement".into())])
        )
        .await
        .is_err()
    );
    sql(&owner,"UPDATE webdock_auth.plugin_operation SET lease_until=now()-interval '1 second' WHERE id=$1",vec![accepted.id.clone().into()]).await;
    let claim = host.claim_installation(None).await.unwrap().unwrap();
    assert!(!host.guard_installation(&old).await.unwrap());
    let held_auth = auth.begin().await.unwrap();
    let mut guarded = Box::pin(host.guard_installation(&claim));
    tokio::select! {result=&mut guarded=>panic!("guard unexpectedly completed while auth pool held: {}",result.is_ok()),_=tokio::time::sleep(std::time::Duration::from_millis(50))=>{}}
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            worker.query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT 1 AS available"
            ))
        )
        .await
        .is_ok(),
        "operator revalidation must not hold the only worker connection"
    );
    held_auth.rollback().await.unwrap();
    assert!(guarded.await.unwrap());
    let work = host.install_work(&claim).await.unwrap();
    assert_eq!(
        work.components["storage-password"],
        "private-fixed-password"
    );
    let files = BTreeMap::from([
        ("manifest.json".into(), work.manifest_bytes.clone()),
        (
            "runtime.json".into(),
            json!({"private":"x".repeat(30000)}).to_string(),
        ),
        ("runtime.crt".into(), "fixture-cert".into()),
        ("runtime.key".into(), "private-key".into()),
        ("core-ca.crt".into(), "fixture-ca".into()),
        ("storage-ca.crt".into(), "fixture-ca".into()),
    ]);
    let spec = PluginRuntimeSpec {
        version: 1,
        platform_id: "fixture".into(),
        plugin_id: "postgres".into(),
        runtime_generation: 1,
        configuration_revision: 1,
        cluster_id: "42".into(),
        manifest_sha256: work.manifest_sha256.clone(),
        material_sha256: PluginRuntimeMaterial {
            files: files.clone(),
        }
        .fingerprint()
        .unwrap(),
        image: work.image.clone(),
        namespace: PluginRuntimeSpec::namespace_name("fixture", "postgres"),
        service: "runtime".into(),
        runtime_host: "notes.example.test".into(),
        port: 3443,
        entry_point: "plugins".into(),
        cpu_millicores: 100,
        memory_bytes: 134217728,
        egress: vec![PluginRuntimeEgress {
            cidr: "192.0.2.1/32".into(),
            port: 443,
        }],
        host_aliases: vec![],
    };
    let binding = RuntimeBinding {
        storage: Default::default(),
        generation: 1,
        namespace: spec.namespace.clone(),
        service: "runtime".into(),
        credential_sha256: "b".repeat(64),
        peers: vec![PeerEnrollment::Unix {
            uid: rustix::process::geteuid().as_raw(),
        }],
        public_configuration: BTreeMap::new(),
        projects: BTreeMap::new(),
        public_customer_configuration: BTreeMap::new(),
        customers: BTreeMap::new(),
        workload_cluster: Some("42".into()),
        route_domains: vec![],
    };
    host.prepare_installation(
        &claim,
        PreparedInstallation {
            spec: serde_json::to_value(&spec).unwrap(),
            files,
            binding: binding.clone(),
            endpoint: json!({"generation":1,"private":"endpoint"}),
        },
    )
    .await
    .unwrap();
    assert!(
        bindings
            .managed_endpoint("postgres", 1)
            .await
            .unwrap()
            .is_some()
    );
    use webdock_plugin_host::RuntimeBindings;
    std::fs::write(
        directory.path().join("missing.json"),
        serde_json::to_vec(&json!({"runtimes":{"postgres":binding}})).unwrap(),
    )
    .unwrap();
    assert!(bindings.runtime("postgres").await.is_err());
    assert!(
        bindings.managed_endpoint("postgres", 2).await.is_err(),
        "a generation miss cannot fall through a managed/file collision"
    );
    std::fs::write(directory.path().join("missing.json"), b"{\"runtimes\":{}}").unwrap();
    assert!(
        bindings.runtime("postgres").await.unwrap().is_none(),
        "candidate must not have normal callback authority"
    );
    // Use the real opaque enrollment identity; the backend must independently recheck it.
    let hosting = webdock_hosting::HostingService::new(
        business,
        auth,
        worker,
        "owned-install-rls-secret-at-least32".into(),
        "owned-install-environment-key-at-least32".into(),
        Arc::new(
            webdock_persistence::IdAllocator::connect(
                fixture.url.as_str(),
                webdock_ids::Identity::platform(0, 3).unwrap(),
            )
            .await
            .unwrap(),
        ),
    )
    .await
    .unwrap();
    let agent = hosting
        .authenticate_enrolled_agent("42", 1, &"a".repeat(64))
        .await
        .unwrap();
    let dispatch = host.poll_plugin_runtime(&agent, 1).await.unwrap().unwrap();
    assert!(
        host.guard_plugin_runtime(&agent, &dispatch.spec, &dispatch.lease)
            .await
            .unwrap()
    );
    sql(&owner,"UPDATE webdock_auth.plugin_configuration SET configuration='{\"changed\":true}'::jsonb WHERE plugin_id='postgres'",vec![]).await;
    assert!(
        !host
            .guard_plugin_runtime(&agent, &dispatch.spec, &dispatch.lease)
            .await
            .unwrap(),
        "the approved public configuration is bound by value as well as revision"
    );
    sql(&owner,"UPDATE webdock_auth.plugin_configuration SET configuration='{}'::jsonb WHERE plugin_id='postgres'",vec![]).await;
    let mut tampered = dispatch.spec.clone();
    tampered.runtime_generation = 2;
    assert!(
        !host
            .guard_plugin_runtime(&agent, &tampered, &dispatch.lease)
            .await
            .unwrap()
    );
    sql(
        &owner,
        "UPDATE webdock_auth.\"user\" SET \"twoFactorEnabled\"=false WHERE id='operator'",
        vec![],
    )
    .await;
    assert!(
        !host
            .guard_plugin_runtime(&agent, &dispatch.spec, &dispatch.lease)
            .await
            .unwrap()
    );
    sql(&owner,"UPDATE webdock_auth.plugin_operation SET lease_until=now()-interval '1 second' WHERE id=$1",vec![accepted.id.clone().into()]).await;
    assert!(
        host.poll_plugin_runtime(&agent, 1).await.unwrap().is_none(),
        "operator revocation is not an agent transport denial"
    );
    let revoked_checkpoint = PluginRuntimeCheckpoint {
        namespace_uid: Some("revoked-ns".into()),
        ..Default::default()
    };
    assert!(
        host.checkpoint_plugin_runtime(
            &agent,
            &dispatch.spec,
            &dispatch.lease,
            &revoked_checkpoint
        )
        .await
        .unwrap()
    );
    sql(
        &owner,
        "UPDATE webdock_auth.\"user\" SET \"twoFactorEnabled\"=true WHERE id='operator'",
        vec![],
    )
    .await;
    let lost_authority = host
        .install_operation_with_authority(&principal, &authority, &accepted.id)
        .await
        .unwrap();
    assert_eq!(lost_authority.phase, "blocked");
    assert_eq!(
        lost_authority.condition.as_deref(),
        Some("authority_changed")
    );
    let accepted = host
        .retry_install_with_authority(&principal, &authority, &accepted.id, "retry-authority")
        .await
        .unwrap();
    let dispatch = host.poll_plugin_runtime(&agent, 1).await.unwrap().unwrap();
    assert_eq!(dispatch.spec, spec);
    sql(&owner,"UPDATE webdock_auth.plugin_operation SET lease_until=now()-interval '1 second' WHERE id=$1",vec![accepted.id.clone().into()]).await;
    let obsolete_checkpoint = PluginRuntimeCheckpoint {
        namespace_uid: Some("obsolete-ns".into()),
        ..Default::default()
    };
    assert!(
        host.checkpoint_plugin_runtime(
            &agent,
            &dispatch.spec,
            &dispatch.lease,
            &obsolete_checkpoint
        )
        .await
        .unwrap(),
        "recognized expired custody is acknowledged without installing it"
    );
    assert!(
        host.checkpoint_plugin_runtime(
            &agent,
            &dispatch.spec,
            &dispatch.lease,
            &obsolete_checkpoint
        )
        .await
        .unwrap()
    );
    let changed_obsolete = PluginRuntimeCheckpoint {
        namespace_uid: Some("changed-obsolete".into()),
        ..Default::default()
    };
    assert!(
        !host
            .checkpoint_plugin_runtime(&agent, &dispatch.spec, &dispatch.lease, &changed_obsolete)
            .await
            .unwrap()
    );
    assert_eq!(scalar(&owner,"SELECT count(*)::bigint n FROM webdock_auth.plugin_install_execution WHERE checkpoint IS NOT NULL").await,0);
    assert_eq!(
        scalar(
            &owner,
            "SELECT count(*)::bigint n FROM webdock_auth.plugin_operation WHERE lease_until>now()"
        )
        .await,
        0
    );
    let dispatch = host.poll_plugin_runtime(&agent, 1).await.unwrap().unwrap();
    let checkpoint = PluginRuntimeCheckpoint {
        namespace_uid: Some("ns-uid".into()),
        secret_uid: Some("secret-uid".into()),
        deployment_uid: Some("deploy-uid".into()),
        service_uid: Some("service-uid".into()),
        network_policy_uid: Some("policy-uid".into()),
        route_uid: Some("route-uid".into()),
    };
    assert!(
        host.checkpoint_plugin_runtime(&agent, &dispatch.spec, &dispatch.lease, &checkpoint)
            .await
            .unwrap()
    );
    let completion = PluginRuntimeCompletion {
        outcome: PluginRuntimeOutcome::Succeeded,
        checkpoint: checkpoint.clone(),
        converged: true,
    };
    assert!(
        host.complete_plugin_runtime(&agent, &dispatch.spec, &dispatch.lease, &completion)
            .await
            .unwrap()
    );
    assert!(
        host.complete_plugin_runtime(&agent, &dispatch.spec, &dispatch.lease, &completion)
            .await
            .unwrap()
    );
    let mut changed_lease = dispatch.lease.clone();
    changed_lease.lease_until += 1;
    assert!(
        !host
            .complete_plugin_runtime(&agent, &dispatch.spec, &changed_lease, &completion)
            .await
            .unwrap(),
        "an exact ACK binds the complete issued lease"
    );
    let verify = host.claim_installation(None).await.unwrap().unwrap();
    assert_eq!(verify.phase(), "verifying_runtime");
    for mismatch in ["version", "generation", "protocol", "bytes", "peer"] {
        let mut invalid_proof = RuntimeDescription {
            manifest: work.manifest_bytes.as_bytes().to_vec(),
            plugin_id: "postgres".into(),
            version: "0.1.0".into(),
            generation: 1,
            protocol_version: 1,
            peer: webdock_plugin_runtime::PeerIdentity::Unix {
                uid: rustix::process::geteuid().as_raw(),
            },
        };
        match mismatch {
            "version" => invalid_proof.version = "9.9.9".into(),
            "generation" => invalid_proof.generation = 2,
            "protocol" => invalid_proof.protocol_version = 2,
            "bytes" => invalid_proof.manifest.push(b' '),
            _ => {
                invalid_proof.peer = webdock_plugin_runtime::PeerIdentity::Unix {
                    uid: rustix::process::geteuid().as_raw() + 1,
                }
            }
        }
        assert!(
            host.promote_installation(&verify, invalid_proof)
                .await
                .is_err(),
            "mismatch {mismatch}"
        );
    }
    let wrong = RuntimeDescription {
        manifest: work.manifest_bytes.as_bytes().to_vec(),
        plugin_id: "wrong".into(),
        version: "0.1.0".into(),
        generation: 1,
        protocol_version: 1,
        peer: webdock_plugin_runtime::PeerIdentity::Unix {
            uid: rustix::process::geteuid().as_raw(),
        },
    };
    assert!(host.promote_installation(&verify, wrong).await.is_err());
    host.fail_installation(&verify, "runtime_mismatch", true)
        .await
        .unwrap();
    assert_eq!(
        scalar(
            &owner,
            "SELECT count(*)::bigint n FROM webdock_auth.plugin_installation WHERE runtime='failed'"
        )
        .await,
        1
    );
    let retry = host
        .retry_install_with_authority(&principal, &authority, &accepted.id, "retry-1")
        .await
        .unwrap();
    assert_ne!(retry.id, accepted.id);
    assert_eq!(
        scalar(
            &owner,
            "SELECT count(*)::bigint n FROM webdock_auth.plugin_runtime"
        )
        .await,
        1
    );
    let dispatch = host.poll_plugin_runtime(&agent, 1).await.unwrap().unwrap();
    assert_eq!(dispatch.spec, spec);
    assert!(
        host.complete_plugin_runtime(&agent, &dispatch.spec, &dispatch.lease, &completion)
            .await
            .unwrap()
    );
    let verify = host.claim_installation(None).await.unwrap().unwrap();
    let proof = RuntimeDescription {
        manifest: work.manifest_bytes.as_bytes().to_vec(),
        plugin_id: "postgres".into(),
        version: "0.1.0".into(),
        generation: 1,
        protocol_version: 1,
        peer: webdock_plugin_runtime::PeerIdentity::Unix {
            uid: rustix::process::geteuid().as_raw(),
        },
    };
    assert_eq!(
        host.promote_installation(&verify, proof)
            .await
            .unwrap()
            .phase,
        "complete"
    );
    assert!(bindings.runtime("postgres").await.unwrap().is_some());
    assert_eq!(scalar(&owner,"SELECT schema_high_water AS n FROM webdock_auth.plugin_installation WHERE id='postgres'").await,1,"initial signed release reserves logical schema before execution");
    let policy = host
        .self_service(&principal, Some(&authority), "postgres")
        .await
        .unwrap();
    assert!(
        host.set_feature_mode(
            &principal,
            Some(&authority),
            "postgres",
            "create-shared",
            "everyone",
            policy["revision"].as_str().unwrap()
        )
        .await
        .is_err(),
        "unsupported features are denied even to operators"
    );
    assert!(
        host.complete_plugin_runtime(&agent, &dispatch.spec, &dispatch.lease, &completion)
            .await
            .unwrap(),
        "exact lost ACK must not require a live lease"
    );
    let public = serde_json::to_string(
        &host
            .install_operation_with_authority(&principal, &authority, &retry.id)
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(!public.contains("private"));
    assert_eq!(scalar(&owner,"SELECT count(*)::bigint n FROM webdock_auth.plugin_configuration WHERE configuration::text LIKE '%private%'").await,0);
    sql(
        &owner,
        "UPDATE webdock_auth.agent_transport_enrollment SET revoked=true",
        vec![],
    )
    .await;
    assert!(
        !host
            .complete_plugin_runtime(&agent, &dispatch.spec, &dispatch.lease, &completion)
            .await
            .unwrap(),
        "revoked agent cannot replay a receipt"
    );
    let promoted = host
        .installed_with_authority(&principal, &authority)
        .await
        .unwrap()[0]
        .clone();
    assert_eq!(promoted["installRecovery"], false);
    let state = host
        .request_state_with_authority(
            &principal,
            &authority,
            "postgres",
            "disabled",
            promoted["revision"].as_str().unwrap(),
            "promoted-state-disable",
        )
        .await
        .unwrap();
    assert_eq!(
        state["status"], "pending",
        "promoted runtime availability remains manageable"
    );
    // The same real operator/agent flow must preserve generation1 and reserve schema before startup.
    sql(
        &owner,
        "UPDATE webdock_auth.agent_transport_enrollment SET revoked=false",
        vec![],
    )
    .await;
    sql(&owner,r#"UPDATE webdock_auth.hosting_agent SET observation='{"capabilities":{"pluginRuntimeVersion":2}}'::jsonb"#,vec![]).await;
    struct NoProbe;
    #[tonic::async_trait]
    impl webdock_plugin_host::StateProbe for NoProbe {
        async fn describe(
            &self,
            _: &str,
            _: u64,
        ) -> webdock_plugin_protocol::Result<RuntimeDescription> {
            panic!("disable never probes")
        }
    }
    let state_claim = host.claim_state_operation(None).await.unwrap().unwrap();
    host.advance_state_operation(&state_claim, &NoProbe)
        .await
        .unwrap();
    let installed = host
        .installed_with_authority(&principal, &authority)
        .await
        .unwrap();
    let descriptor = catalog
        .descriptors()
        .into_iter()
        .find(|r| r.version == "0.2.0")
        .unwrap();
    let update = webdock_plugin_host::UpdateRequest {
        version: "0.2.0".into(),
        review_sha256: descriptor.targets[0].review_sha256.clone(),
        expected_revision: installed[0]["revision"].as_str().unwrap().into(),
        approved_capabilities: descriptor.capabilities,
        idempotency_key: "update-1".into(),
    };
    for field in ["review", "revision", "capabilities"] {
        let mut invalid = update.clone();
        invalid.idempotency_key = format!("invalid-{field}");
        match field {
            "review" => invalid.review_sha256 = "0".repeat(64),
            "revision" => invalid.expected_revision = "0".into(),
            _ => invalid.approved_capabilities.clear(),
        }
        assert!(
            host.request_update_with_authority(&principal, &authority, "postgres", invalid)
                .await
                .is_err(),
            "reject changed {field}"
        );
    }
    // A writer of schema2 that only reads2 cannot consume the retained schema1.
    let forward_only = catalog
        .descriptors()
        .into_iter()
        .find(|release| release.version == "0.4.0")
        .unwrap();
    let counters = [
        "SELECT schema_high_water n FROM webdock_auth.plugin_installation WHERE id='postgres'",
        "SELECT generation n FROM webdock_auth.plugin_installation WHERE id='postgres'",
        "SELECT count(*)::bigint n FROM webdock_auth.plugin_operation",
        "SELECT count(*)::bigint n FROM webdock_auth.plugin_runtime",
        "SELECT count(*)::bigint n FROM webdock_auth.plugin_install_execution",
    ];
    let mut before = Vec::new();
    for query in counters {
        before.push(scalar(&owner, query).await);
    }
    let rejection = host
        .request_update_with_authority(
            &principal,
            &authority,
            "postgres",
            webdock_plugin_host::UpdateRequest {
                version: forward_only.version,
                review_sha256: forward_only.targets[0].review_sha256.clone(),
                expected_revision: update.expected_revision.clone(),
                approved_capabilities: forward_only.capabilities,
                idempotency_key: "forward-only-reader".into(),
            },
        )
        .await
        .err()
        .expect("reader2..2 must reject retained schema1 before reserving schema2");
    assert_eq!(rejection.code, webdock_plugin_protocol::ErrorCode::Conflict);
    for (query, expected) in counters.into_iter().zip(before) {
        assert_eq!(
            scalar(&owner, query).await,
            expected,
            "incompatible old-schema reader cannot mutate lifecycle state"
        );
    }
    assert_eq!(
        host.installed_with_authority(&principal, &authority)
            .await
            .unwrap(),
        installed
    );
    let mut competing = update.clone();
    competing.idempotency_key = "competing-update".into();
    let (first, second) = tokio::join!(
        host.request_update_with_authority(&principal, &authority, "postgres", update.clone()),
        host.request_update_with_authority(&principal, &authority, "postgres", competing.clone())
    );
    assert_eq!(
        u8::from(first.is_ok()) + u8::from(second.is_ok()),
        1,
        "admission serializes lifecycle changes"
    );
    let (updated, update) = if let Ok(updated) = first {
        (updated, update)
    } else {
        (second.unwrap(), competing)
    };

    assert_eq!(updated.kind, "update");
    assert_eq!(
        host.request_update_with_authority(&principal, &authority, "postgres", update)
            .await
            .unwrap()
            .id,
        updated.id
    );
    assert_eq!(
        scalar(
            &owner,
            "SELECT count(*)::bigint n FROM webdock_auth.plugin_install_execution"
        )
        .await,
        2
    );
    let claim = host.claim_installation(None).await.unwrap().unwrap();
    let next = host.install_work(&claim).await.unwrap();
    assert_eq!(next.runtime_generation, 2);
    assert_eq!(next.source_runtime_generation, Some(1));
    assert_eq!(next.components["storagePassword"], "fixed-master");
    assert_eq!(next.components["sealingKey"], "fixed-sealer");
    assert_eq!(
        host.installed_with_authority(&principal, &authority)
            .await
            .unwrap()[0]["version"],
        "0.1.0"
    );

    host.persist_install_components(
        &claim,
        BTreeMap::from([("certificate".into(), "new-tls".into())]),
    )
    .await
    .unwrap();
    assert!(
        host.persist_install_components(
            &claim,
            BTreeMap::from([("storagePassword".into(), "replacement".into())])
        )
        .await
        .is_err()
    );
    let (next_spec, _) = prepare_generation(&host, &claim, &next, &spec, &binding).await;
    assert!(
        host.poll_plugin_runtime(&agent, 1).await.unwrap().is_none(),
        "old binary request cannot consume lifecycle despite retained cap2"
    );
    let stop = host.poll_plugin_runtime(&agent, 2).await.unwrap().unwrap();
    assert_eq!(stop.lease.action, PluginRuntimeAction::Suspend);
    assert_eq!(stop.spec, spec);
    assert_eq!(stop.checkpoint.as_ref(), Some(&checkpoint));
    // Capability loss stops mutation, but cannot strand the exact obsolete receipt in an outbox.
    sql(&owner,r#"UPDATE webdock_auth.hosting_agent SET observation='{"capabilities":{"pluginRuntimeVersion":1}}'::jsonb"#,vec![]).await;
    assert!(
        !host
            .guard_plugin_runtime(&agent, &stop.spec, &stop.lease)
            .await
            .unwrap()
    );
    assert!(
        host.complete_plugin_runtime(&agent, &stop.spec, &stop.lease, &completion)
            .await
            .unwrap()
    );
    assert_eq!(
        scalar(
            &owner,
            "SELECT count(*)::bigint n FROM webdock_auth.plugin_install_execution WHERE stopped"
        )
        .await,
        0
    );
    sql(&owner,r#"UPDATE webdock_auth.hosting_agent SET observation='{"capabilities":{"pluginRuntimeVersion":2}}'::jsonb"#,vec![]).await;
    let retry_update = host
        .retry_install_with_authority(&principal, &authority, &updated.id, "retry-update")
        .await
        .unwrap();
    assert_eq!(retry_update.kind, "update");
    let stop = host.poll_plugin_runtime(&agent, 2).await.unwrap().unwrap();
    assert!(
        host.complete_plugin_runtime(&agent, &stop.spec, &stop.lease, &completion)
            .await
            .unwrap()
    );
    let apply = host.poll_plugin_runtime(&agent, 2).await.unwrap().unwrap();
    assert_eq!(apply.spec, next_spec);
    assert_eq!(apply.lease.action, PluginRuntimeAction::Apply);
    assert!(
        host.complete_plugin_runtime(&agent, &stop.spec, &stop.lease, &completion)
            .await
            .unwrap(),
        "late source exact ACK cannot advance candidate"
    );
    assert_eq!(
        host.install_operation_with_authority(&principal, &authority, &retry_update.id)
            .await
            .unwrap()
            .phase,
        "deploying"
    );
    let candidate_checkpoint = generation_checkpoint(2);
    let candidate_completion = PluginRuntimeCompletion {
        outcome: PluginRuntimeOutcome::Succeeded,
        checkpoint: candidate_checkpoint,
        converged: true,
    };
    assert!(
        host.complete_plugin_runtime(&agent, &apply.spec, &apply.lease, &candidate_completion)
            .await
            .unwrap()
    );
    let verify = host.claim_installation(None).await.unwrap().unwrap();
    sql(
        &owner,
        "UPDATE webdock_auth.plugin_install_execution SET stopped=false WHERE runtime_generation=1",
        vec![],
    )
    .await;
    assert!(
        host.promote_installation(&verify, generation_proof(&next, "0.2.0"))
            .await
            .is_err(),
        "promotion must recheck predecessor stop proof"
    );
    sql(
        &owner,
        "UPDATE webdock_auth.plugin_install_execution SET stopped=true WHERE runtime_generation=1",
        vec![],
    )
    .await;
    host.promote_installation(&verify, generation_proof(&next, "0.2.0"))
        .await
        .unwrap();
    assert_eq!(
        bindings
            .runtime("postgres")
            .await
            .unwrap()
            .unwrap()
            .generation,
        2
    );
    sql(&owner,r#"UPDATE webdock_auth.plugin_install_execution SET supported_features='["create-dedicated","create-shared"]'::jsonb WHERE plugin_id='postgres' AND runtime_generation=1"#,vec![]).await;
    let policy = host
        .self_service(&principal, Some(&authority), "postgres")
        .await
        .unwrap();
    assert!(
        host.set_feature_mode(
            &principal,
            Some(&authority),
            "postgres",
            "create-shared",
            "everyone",
            policy["revision"].as_str().unwrap()
        )
        .await
        .is_err(),
        "historical supported features never grant current authority"
    );
    sql(&owner,r#"UPDATE webdock_auth.plugin_install_execution SET supported_features='["create-dedicated"]'::jsonb WHERE plugin_id='postgres' AND runtime_generation=1"#,vec![]).await;

    assert!(
        bindings
            .managed_endpoint("postgres", 1)
            .await
            .unwrap()
            .is_some()
    );
    let history = host
        .release_history_with_authority(&principal, &authority, "postgres")
        .await
        .unwrap();
    assert_eq!(history["currentGeneration"], "2");
    assert_eq!(history["previousGeneration"], "1");
    assert_eq!(history["releases"].as_array().unwrap().len(), 2);
    assert!(!history.to_string().contains("private"));
    let installed = host
        .installed_with_authority(&principal, &authority)
        .await
        .unwrap();
    assert_eq!(installed[0]["version"], "0.2.0");
    assert_eq!(installed[0]["managed"], true);
    host.request_state_with_authority(
        &principal,
        &authority,
        "postgres",
        "disabled",
        installed[0]["revision"].as_str().unwrap(),
        "disable-v2",
    )
    .await
    .unwrap();
    let disable = host.claim_state_operation(None).await.unwrap().unwrap();
    host.advance_state_operation(&disable, &NoProbe)
        .await
        .unwrap();
    let installed = host
        .installed_with_authority(&principal, &authority)
        .await
        .unwrap();
    let first = catalog
        .descriptors()
        .into_iter()
        .find(|r| r.version == "0.1.0")
        .unwrap();
    let rollback = webdock_plugin_host::RollbackRequest {
        release_generation: "1".into(),
        review_sha256: first.targets[0].review_sha256.clone(),
        expected_revision: installed[0]["revision"].as_str().unwrap().into(),
        approved_capabilities: first.capabilities.clone(),
        idempotency_key: "rollback-1".into(),
    };
    let rolled = host
        .request_rollback_with_authority(&principal, &authority, "postgres", rollback.clone())
        .await
        .unwrap();
    assert_eq!(rolled.kind, "rollback");
    let prepare = host.claim_installation(None).await.unwrap().unwrap();
    let restored = host.install_work(&prepare).await.unwrap();
    assert_eq!(restored.runtime_generation, 3);
    assert_eq!(restored.components["storagePassword"], "fixed-master");
    assert_eq!(restored.components["sealingKey"], "fixed-sealer");
    prepare_generation(&host, &prepare, &restored, &spec, &binding).await;
    let stop = host.poll_plugin_runtime(&agent, 2).await.unwrap().unwrap();
    assert_eq!(stop.spec.runtime_generation, 2);
    assert!(
        host.complete_plugin_runtime(&agent, &stop.spec, &stop.lease, &candidate_completion)
            .await
            .unwrap()
    );
    let apply = host.poll_plugin_runtime(&agent, 2).await.unwrap().unwrap();
    assert_eq!(apply.spec.runtime_generation, 3);
    let third_completion = PluginRuntimeCompletion {
        outcome: PluginRuntimeOutcome::Succeeded,
        checkpoint: generation_checkpoint(3),
        converged: true,
    };
    assert!(
        host.complete_plugin_runtime(&agent, &apply.spec, &apply.lease, &third_completion)
            .await
            .unwrap()
    );
    let verify = host.claim_installation(None).await.unwrap().unwrap();
    host.promote_installation(&verify, generation_proof(&restored, "0.1.0"))
        .await
        .unwrap();
    assert_eq!(
        bindings
            .runtime("postgres")
            .await
            .unwrap()
            .unwrap()
            .generation,
        3
    );
    assert_eq!(
        scalar(
            &owner,
            "SELECT count(*)::bigint n FROM webdock_auth.plugin_install_execution WHERE stopped"
        )
        .await,
        2
    );
    let installed = host
        .installed_with_authority(&principal, &authority)
        .await
        .unwrap();
    host.request_state_with_authority(
        &principal,
        &authority,
        "postgres",
        "disabled",
        installed[0]["revision"].as_str().unwrap(),
        "disable-v3",
    )
    .await
    .unwrap();
    let disable = host.claim_state_operation(None).await.unwrap().unwrap();
    host.advance_state_operation(&disable, &NoProbe)
        .await
        .unwrap();
    let installed = host
        .installed_with_authority(&principal, &authority)
        .await
        .unwrap();
    let compatible = catalog
        .descriptors()
        .into_iter()
        .find(|r| r.version == "0.2.0")
        .unwrap();
    let unknown = host
        .request_update_with_authority(
            &principal,
            &authority,
            "postgres",
            webdock_plugin_host::UpdateRequest {
                version: "0.2.0".into(),
                review_sha256: compatible.targets[0].review_sha256.clone(),
                expected_revision: installed[0]["revision"].as_str().unwrap().into(),
                approved_capabilities: compatible.capabilities,
                idempotency_key: "unknown-start".into(),
            },
        )
        .await
        .unwrap();
    let claim = host.claim_installation(None).await.unwrap().unwrap();
    let work = host.install_work(&claim).await.unwrap();
    prepare_generation(&host, &claim, &work, &spec, &binding).await;
    let stop = host.poll_plugin_runtime(&agent, 2).await.unwrap().unwrap();
    assert_eq!(stop.spec.runtime_generation, 3);
    assert!(
        host.complete_plugin_runtime(&agent, &stop.spec, &stop.lease, &third_completion)
            .await
            .unwrap()
    );
    let apply = host.poll_plugin_runtime(&agent, 2).await.unwrap().unwrap();
    assert_eq!(apply.spec.runtime_generation, 4);
    let uncertain = PluginRuntimeCompletion {
        outcome: PluginRuntimeOutcome::NeedsReconciliation,
        checkpoint: PluginRuntimeCheckpoint {
            namespace_uid: generation_checkpoint(4).namespace_uid,
            ..Default::default()
        },
        converged: false,
    };
    assert!(
        host.complete_plugin_runtime(&agent, &apply.spec, &apply.lease, &uncertain)
            .await
            .unwrap()
    );
    let installed = host
        .installed_with_authority(&principal, &authority)
        .await
        .unwrap();
    let mut refused = rollback.clone();
    refused.idempotency_key = "unknown-custody-rollback".into();
    refused.expected_revision = installed[0]["revision"].as_str().unwrap().into();
    assert!(
        host.request_rollback_with_authority(&principal, &authority, "postgres", refused)
            .await
            .is_err(),
        "unknown candidate writer requires complete custody"
    );
    let retry = host
        .retry_install_with_authority(&principal, &authority, &unknown.id, "reconcile-unknown")
        .await
        .unwrap();
    let apply = host.poll_plugin_runtime(&agent, 2).await.unwrap().unwrap();
    assert_eq!(apply.spec.runtime_generation, 4);
    let fourth_completion = PluginRuntimeCompletion {
        outcome: PluginRuntimeOutcome::Succeeded,
        checkpoint: generation_checkpoint(4),
        converged: true,
    };
    assert!(
        host.complete_plugin_runtime(&agent, &apply.spec, &apply.lease, &fourth_completion)
            .await
            .unwrap()
    );
    let verify = host.claim_installation(None).await.unwrap().unwrap();
    host.fail_installation(&verify, "candidate_rejected", true)
        .await
        .unwrap();
    let installed = host
        .installed_with_authority(&principal, &authority)
        .await
        .unwrap();
    let mut recovered = rollback.clone();
    recovered.idempotency_key = "recover-failed-candidate".into();
    recovered.expected_revision = installed[0]["revision"].as_str().unwrap().into();
    host.request_rollback_with_authority(&principal, &authority, "postgres", recovered)
        .await
        .unwrap();
    let claim = host.claim_installation(None).await.unwrap().unwrap();
    let work = host.install_work(&claim).await.unwrap();
    prepare_generation(&host, &claim, &work, &spec, &binding).await;
    let stop = host.poll_plugin_runtime(&agent, 2).await.unwrap().unwrap();
    assert_eq!(
        stop.spec.runtime_generation, 4,
        "the failed candidate is stopped even though current generation is3"
    );
    assert!(
        host.complete_plugin_runtime(&agent, &stop.spec, &stop.lease, &fourth_completion)
            .await
            .unwrap()
    );
    let apply = host.poll_plugin_runtime(&agent, 2).await.unwrap().unwrap();
    assert_eq!(apply.spec.runtime_generation, 5);
    let fifth_completion = PluginRuntimeCompletion {
        outcome: PluginRuntimeOutcome::Succeeded,
        checkpoint: generation_checkpoint(5),
        converged: true,
    };
    assert!(
        host.complete_plugin_runtime(&agent, &apply.spec, &apply.lease, &fifth_completion)
            .await
            .unwrap()
    );
    let verify = host.claim_installation(None).await.unwrap().unwrap();
    host.promote_installation(&verify, generation_proof(&work, "0.1.0"))
        .await
        .unwrap();
    assert_eq!(
        host.install_operation_with_authority(&principal, &authority, &retry.id)
            .await
            .unwrap()
            .status,
        webdock_plugin_host::OperationStatus::Failed
    );
    let installed = host
        .installed_with_authority(&principal, &authority)
        .await
        .unwrap();
    host.request_state_with_authority(
        &principal,
        &authority,
        "postgres",
        "disabled",
        installed[0]["revision"].as_str().unwrap(),
        "disable-v5",
    )
    .await
    .unwrap();
    let disable = host.claim_state_operation(None).await.unwrap().unwrap();
    host.advance_state_operation(&disable, &NoProbe)
        .await
        .unwrap();
    let installed = host
        .installed_with_authority(&principal, &authority)
        .await
        .unwrap();
    let advanced = catalog
        .descriptors()
        .into_iter()
        .find(|r| r.version == "0.3.0")
        .unwrap();
    let advanced = host
        .request_update_with_authority(
            &principal,
            &authority,
            "postgres",
            webdock_plugin_host::UpdateRequest {
                version: "0.3.0".into(),
                review_sha256: advanced.targets[0].review_sha256.clone(),
                expected_revision: installed[0]["revision"].as_str().unwrap().into(),
                approved_capabilities: advanced.capabilities,
                idempotency_key: "reserve-schema2".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        scalar(
            &owner,
            "SELECT schema_high_water n FROM webdock_auth.plugin_installation WHERE id='postgres'"
        )
        .await,
        2
    );
    let preparing = host.claim_installation(None).await.unwrap().unwrap();
    host.fail_installation(&preparing, "before_start", true)
        .await
        .unwrap();
    assert_eq!(scalar(&owner,"SELECT count(*)::bigint n FROM webdock_auth.plugin_install_execution WHERE runtime_generation=6 AND apply_dispatched").await,0);
    let installed = host
        .installed_with_authority(&principal, &authority)
        .await
        .unwrap();
    let mut incompatible = rollback;
    incompatible.expected_revision = installed[0]["revision"].as_str().unwrap().into();
    incompatible.idempotency_key = "incompatible-rollback".into();
    assert!(
        host.request_rollback_with_authority(&principal, &authority, "postgres", incompatible)
            .await
            .is_err()
    );
    assert_eq!(
        scalar(
            &owner,
            "SELECT count(*)::bigint n FROM webdock_auth.plugin_runtime"
        )
        .await,
        6
    );
    assert_eq!(
        host.install_operation_with_authority(&principal, &authority, &advanced.id)
            .await
            .unwrap()
            .status,
        webdock_plugin_host::OperationStatus::Blocked
    );
    let history = host
        .release_history_with_authority(&principal, &authority, "postgres")
        .await
        .unwrap();
    assert_eq!(history["schemaHighWater"], 2);
    assert!(owner.execute_unprepared("UPDATE webdock_auth.plugin_installation SET schema_high_water=1 WHERE id='postgres'").await.is_err(),"schema watermark cannot move backwards even when work failed");

    assert_eq!(
        history["releases"][1]["rollbackBlocker"],
        "schema_incompatible"
    );
}

fn generation_checkpoint(generation: u64) -> PluginRuntimeCheckpoint {
    let uid = |name: &str| Some(format!("{name}-{generation}"));
    PluginRuntimeCheckpoint {
        namespace_uid: uid("ns"),
        secret_uid: uid("secret"),
        deployment_uid: uid("deployment"),
        service_uid: uid("service"),
        network_policy_uid: uid("policy"),
        route_uid: uid("route"),
    }
}
fn generation_proof(work: &webdock_plugin_host::InstallWork, version: &str) -> RuntimeDescription {
    RuntimeDescription {
        manifest: work.manifest_bytes.as_bytes().to_vec(),
        plugin_id: "postgres".into(),
        version: version.into(),
        generation: work.runtime_generation,
        protocol_version: 1,
        peer: webdock_plugin_runtime::PeerIdentity::Unix {
            uid: rustix::process::geteuid().as_raw(),
        },
    }
}
async fn prepare_generation(
    host: &HostService,
    claim: &webdock_plugin_host::InstallClaim,
    work: &webdock_plugin_host::InstallWork,
    template: &PluginRuntimeSpec,
    binding: &RuntimeBinding,
) -> (PluginRuntimeSpec, RuntimeBinding) {
    let mut spec = template.clone();
    spec.version = 2;
    spec.runtime_generation = work.runtime_generation;
    spec.manifest_sha256 = work.manifest_sha256.clone();
    spec.image = work.image.clone();
    spec.namespace =
        PluginRuntimeSpec::namespace_name_v2("fixture", "postgres", work.runtime_generation);
    let files: BTreeMap<String, String> = MATERIAL_FILES
        .iter()
        .map(|name| {
            (
                (*name).into(),
                if *name == "manifest.json" {
                    work.manifest_bytes.clone()
                } else {
                    format!("private-{name}-{}", work.runtime_generation)
                },
            )
        })
        .collect();
    spec.material_sha256 = PluginRuntimeMaterial {
        files: files.clone(),
    }
    .fingerprint()
    .unwrap();
    let mut binding = binding.clone();
    binding.generation = work.runtime_generation;
    binding.namespace = spec.namespace.clone();
    host.prepare_installation(
        claim,
        PreparedInstallation {
            spec: serde_json::to_value(&spec).unwrap(),
            files,
            binding: binding.clone(),
            endpoint: json!({"generation":work.runtime_generation,"private":"endpoint"}),
        },
    )
    .await
    .unwrap();
    (spec, binding)
}
