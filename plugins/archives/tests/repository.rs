//! Existing normalized repository protocol under exact archives job/attempt authority.
use sea_orm::{ConnectionTrait, DbBackend, Statement, TransactionTrait};
use serde_json::{Value, json};
use std::{
    os::unix::fs::PermissionsExt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use webdock_export_protocol::{SourceKind, SourceRef};
use webdock_identity::{CookiePolicy, IdentityService};
use webdock_plugin_host::{
    AdmissionInput, ExportJob, ExportSources, FileBindings, HostService, InvocationScope,
};
use webdock_plugin_runtime::{PeerIdentity, UnixSocket};
use webdock_repository_protocol::{self as repository, runtime};
#[allow(
    dead_code,
    reason = "existing owned Core fixture also supplies other job helpers"
)]
#[path = "../../../crates/plugin-host/tests/support/jobs_fixture.rs"]
mod fixture;
fn peer() -> PeerIdentity {
    PeerIdentity::Unix {
        uid: rustix::process::geteuid().as_raw(),
    }
}
async fn sql(db: &impl ConnectionTrait, text: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        text,
        values,
    ))
    .await
    .unwrap();
}
struct Task(tokio::task::JoinHandle<()>);
impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}
struct Provider {
    pause: AtomicBool,
    entered: tokio::sync::Notify,
    last: Mutex<Option<repository::LeaseClaims>>,
}
#[async_trait::async_trait]
impl runtime::Handler for Provider {
    async fn resolve(
        &self,
        _: &repository::LeaseClaims,
        input: Value,
        _: Value,
    ) -> repository::Result<Value> {
        Ok(json!({"repositoryID":input["repositoryID"],"sha":"b".repeat(40)}))
    }
    async fn archive(
        &self,
        c: &repository::LeaseClaims,
        _: Value,
        _: Value,
    ) -> repository::Result<Vec<u8>> {
        *self.last.lock().unwrap() = Some(c.clone());
        if self.pause.load(Ordering::SeqCst) {
            self.entered.notify_one();
            std::future::pending::<()>().await;
        }
        Ok(
            webdock_build::artifact::Archive::from_files(std::collections::BTreeMap::from([(
                "README.md".into(),
                b"owned committed source\n".to_vec(),
            )]))
            .unwrap()
            .encode()
            .unwrap(),
        )
    }
}
struct Adapter {
    gate: Arc<webdock_build::RepositoryAdmission>,
    proof: Mutex<Option<ExportJob>>,
    hold_data: AtomicBool,
    validations: AtomicUsize,
    entered: tokio::sync::Notify,
    resume: tokio::sync::Notify,
}
#[async_trait::async_trait]
impl ExportSources for Adapter {
    async fn bind(
        &self,
        s: &InvocationScope,
        t: &sea_orm::DatabaseTransaction,
        k: SourceKind,
        id: &str,
    ) -> webdock_plugin_protocol::Result<SourceRef> {
        if k != SourceKind::Project {
            return Err(webdock_plugin_protocol::PluginError::denied());
        }
        self.gate
            .export_binding(s, t, id)
            .await
            .map_err(|_| webdock_plugin_protocol::PluginError::denied())
    }
    async fn validate(
        &self,
        s: &InvocationScope,
        t: &sea_orm::DatabaseTransaction,
        r: &SourceRef,
    ) -> webdock_plugin_protocol::Result<()> {
        let count = self.validations.fetch_add(1, Ordering::SeqCst) + 1;
        if self.hold_data.load(Ordering::SeqCst) && count == 3 {
            self.entered.notify_one();
            self.resume.notified().await;
        }
        self.gate
            .validate_export_source(s, t, r)
            .await
            .map_err(|_| webdock_plugin_protocol::PluginError::denied())
    }
    async fn archive(
        &self,
        p: ExportJob,
        s: &InvocationScope,
        r: &SourceRef,
    ) -> webdock_plugin_protocol::Result<(SourceRef, Vec<u8>)> {
        *self.proof.lock().unwrap() = Some(p.clone());
        self.gate
            .archive_for_export(p, s, r)
            .await
            .map_err(|_| webdock_plugin_protocol::PluginError::denied())
    }
}
#[tokio::test]
#[ignore = "owns an isolated Core PostgreSQL and real existing repository-protocol runtime"]
async fn exact_plugin_job_claim_owns_repository_stream_and_cancellation_retires_its_lease() {
    let mut f = fixture::JobFixture::start_with_connections(12).await;
    let manifest: Value = serde_json::from_slice(include_bytes!("../manifest.json")).unwrap();
    let raw = manifest.to_string();
    sql(&f.owner,"INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version,runtime,current_generation) VALUES('archives','external','enabled','enabled','0.3.0','ready',1),('hosting','builtin','enabled','enabled','1.0.0','ready',1) ON CONFLICT DO NOTHING",vec![]).await;
    sql(&f.owner,"INSERT INTO webdock_auth.plugin_release(plugin_id,version,manifest_bytes,manifest_sha256,image,registry_url,signer_key_id) VALUES('archives','0.3.0',$1,$2,$3,'https://registry.invalid','fixture')",vec![raw.clone().into(),repository::sha256(raw.as_bytes()).into(),manifest["image"].as_str().unwrap().into()]).await;
    sql(&f.owner,"INSERT INTO webdock_auth.plugin_configuration(plugin_id,approved_capabilities,configuration) VALUES('archives',$1,'{}')",vec![manifest["capabilities"].clone().into()]).await;
    sql(&f.owner,"INSERT INTO webdock_auth.plugin_runtime(plugin_id,generation,version,image,status,configuration_revision,credential_hash,namespace,service_name) VALUES('archives',1,'0.3.0',$1,'ready',0,$2,'fixtures','fixture-plugin')",vec![manifest["image"].as_str().unwrap().into(),"a".repeat(64).into()]).await;
    sql(&f.owner,"INSERT INTO webdock_auth.plugin_feature_policy(plugin_id,feature_id,mode,updated_by) VALUES('archives','exports','everyone','operator')",vec![]).await;
    sql(&f.owner,"INSERT INTO webdock_auth.hosting_cluster(id,name,provider,region,location_evidence,runtime_kind) VALUES('604','Owned authority test','fixture','local','isolated repository test','kubernetes')",vec![]).await;
    sql(&f.owner,"INSERT INTO webdock_auth.hosting_project(project_id,customer_id,provider,cluster_id,namespace,mode) VALUES('124','123','k3s','604','wd-124','selfservice')",vec![]).await;
    let mut pm: Value = serde_json::from_str(include_str!(
        "../../../crates/plugin-protocol/tests/fixtures/project-notes.json"
    ))
    .unwrap();
    pm["name"] = json!("repository-export");
    pm["capabilities"] = json!(["plugin.http.read"]);
    pm["services"]["required"] = json!([]);
    pm["jobs"] = json!([]);
    pm["actions"] = json!([{"name":"repository.resolve","intent":"read","audience":"operator","capabilities":["plugin.http.read"]},{"name":"repository.archive","intent":"read","audience":"operator","capabilities":["plugin.http.read"]}]);
    pm["areas"]["selfService"] = json!({"pages":[]});
    pm["selfService"]["features"] = json!([]);
    pm["api"] = json!([]);
    pm["mcp"] = json!([]);
    let pr = pm.to_string();
    let digest = repository::sha256(pr.as_bytes());
    sql(&f.owner,"INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version,runtime,current_generation) VALUES('repository-export','external','enabled','enabled','1.0.0','ready',1)",vec![]).await;
    sql(&f.owner,"INSERT INTO webdock_auth.plugin_release(plugin_id,version,manifest_bytes,manifest_sha256,image,registry_url,signer_key_id) VALUES('repository-export','1.0.0',$1,$2,$3,'https://registry.invalid','fixture')",vec![pr.into(),digest.clone().into(),pm["image"].as_str().unwrap().into()]).await;
    sql(&f.owner,"INSERT INTO webdock_auth.plugin_configuration(plugin_id,approved_capabilities,configuration) VALUES('repository-export','[\"plugin.http.read\"]','{}')",vec![]).await;
    sql(&f.owner,"INSERT INTO webdock_auth.plugin_runtime(plugin_id,generation,version,image,status,configuration_revision,credential_hash,namespace,service_name) VALUES('repository-export',1,'1.0.0',$1,'ready',0,$2,'fixtures','fixture-plugin')",vec![pm["image"].as_str().unwrap().into(),"a".repeat(64).into()]).await;
    sql(&f.owner,"INSERT INTO webdock_auth.plugin_grant(plugin_id,customer_id,project_id,capabilities) VALUES('repository-export','123','124','[\"plugin.http.read\"]')",vec![]).await;
    f.owner.execute_unprepared("INSERT INTO webdock_auth.git_connection(id,customer_id,installation_id,account_id,account_login,permissions,state,repository_plugin_id) VALUES('601','123','1234','1234','fixture','{}','active','repository-export'); INSERT INTO webdock_auth.git_repository(connection_id,customer_id,repository_id,metadata) VALUES('601','123','123456','{}'); INSERT INTO webdock_auth.git_source(id,customer_id,project_id,connection_id,repository_id,branch,root_directory,recipe,target_id) VALUES('602','123','124','601','123456','main','.','dockerfile','603')").await.unwrap();
    let path = f.directory.path().join("bindings.json");
    let mut bindings: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    bindings["runtimes"]["job-fixture"]["storage"] = json!({"124":{"archives":{"origin":"https://storage.example.test","addresses":["1.1.1.1"],"ca":null,"pathPrefix":"/bucket/"}}});
    for plugin in ["archives", "repository-export"] {
        bindings["runtimes"][plugin] = bindings["runtimes"]["job-fixture"].clone();
    }
    std::fs::write(&path, bindings.to_string()).unwrap();
    f.host = Arc::new(
        Arc::try_unwrap(f.host)
            .ok()
            .unwrap()
            .with_storage_gateway(Arc::new(
                webdock_plugin_host::storage::StorageGateway::new(f.directory.path().into())
                    .unwrap(),
            )),
    );
    let issuer = ed25519_dalek::SigningKey::from_bytes(&[29; 32]);
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[31; 32]);
    let descriptor = repository::Descriptor {
        v: 1,
        plugin_id: "repository-export".into(),
        version: "1.0.0".into(),
        manifest_sha256: digest,
        image: pm["image"].as_str().unwrap().into(),
        protocol: "webdock.repository.v1".into(),
        signer_key_id: "fixture".into(),
    }
    .sign(&publisher)
    .unwrap();
    let repo_path = f.directory.path().join("repository.sock");
    let authority_path = f.directory.path().join("repository-authority.sock");
    let binding_path = f.directory.path().join("repository.json");
    std::fs::write(&binding_path,json!({"runtimes":{"repository-export":{"descriptor":descriptor,"publisherKeyId":"fixture","publisherPublicKeyHex":hex::encode(publisher.verifying_key().as_bytes()),"registryUrl":"https://registry.invalid","transport":{"kind":"unix","path":repo_path}}}}).to_string()).unwrap();
    std::fs::set_permissions(&binding_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let gate = Arc::new(
        webdock_build::RepositoryAdmission::new(
            f.database
                .role_with_connections("webdock_worker_runtime", 12)
                .await,
            "owned-plugin-jobs-rls-secret-32bytes".into(),
            f.host.clone(),
            issuer.clone(),
            Arc::new(webdock_build::FileRepositoryBindings::new(binding_path)),
        )
        .await
        .unwrap(),
    );
    let mut socket = UnixSocket::bind(&authority_path).unwrap();
    let incoming = socket.incoming().unwrap();
    let service = gate.clone().authority_service();
    let _authority = Task(tokio::spawn(async move {
        let _socket = socket;
        tonic::transport::Server::builder()
            .add_service(service)
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    }));
    let provider = Arc::new(Provider {
        pause: AtomicBool::new(false),
        entered: tokio::sync::Notify::new(),
        last: Mutex::new(None),
    });
    let runtime = runtime::Runtime::new(
        descriptor.clone(),
        &publisher.verifying_key(),
        1,
        issuer.verifying_key(),
        Arc::new(
            runtime::RemoteAuthority::new(
                runtime::connect_unix(&authority_path).await.unwrap(),
                "repository-export".into(),
                1,
                repository::sha256(descriptor.as_bytes()),
                vec![runtime::CorePeer::Unix {
                    uid: rustix::process::geteuid().as_raw(),
                }],
            )
            .unwrap(),
        ),
        provider.clone(),
    )
    .unwrap();
    let socket = UnixSocket::bind(&repo_path).unwrap();
    let _runtime = Task(tokio::spawn(async move {
        runtime
            .serve_unix(socket, std::future::pending())
            .await
            .unwrap();
    }));
    let adapter = Arc::new(Adapter {
        gate: gate.clone(),
        proof: Mutex::new(None),
        hold_data: AtomicBool::new(false),
        validations: AtomicUsize::new(0),
        entered: tokio::sync::Notify::new(),
        resume: tokio::sync::Notify::new(),
    });
    let host_path = f.directory.path().join("export.sock");
    let mut socket = UnixSocket::bind(&host_path).unwrap();
    let incoming = socket.incoming().unwrap();
    let service = f.host.clone().export_service(adapter.clone());
    let _export = Task(tokio::spawn(async move {
        let _socket = socket;
        tonic::transport::Server::builder()
            .add_service(service)
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    }));
    let channel = webdock_plugin_runtime::unix_channel(&host_path)
        .await
        .unwrap();
    let mut client =
        webdock_export_protocol::wire::export_sources_client::ExportSourcesClient::new(channel);
    let business = f.database.role("webdock_auth_runtime").await;
    let ids = Arc::new(
        webdock_persistence::IdAllocator::connect(
            f.database.url.as_str(),
            webdock_ids::Identity::platform(0, 2).unwrap(),
        )
        .await
        .unwrap(),
    );
    let hosting = Arc::new(
        webdock_hosting::HostingService::new(
            business.clone(),
            f.auth.clone(),
            f.database.role("webdock_worker_runtime").await,
            "owned-plugin-jobs-rls-secret-32bytes".into(),
            "owned-repository-environment-secret-32bytes".into(),
            ids.clone(),
        )
        .await
        .unwrap(),
    );
    let build_service = webdock_build::BuildService::new(
        business.clone(),
        f.database.role("webdock_worker_runtime").await,
        "owned-plugin-jobs-rls-secret-32bytes".into(),
        ids,
        hosting.clone(),
        "registry.invalid".into(),
    )
    .await
    .unwrap()
    .with_session_authority(f.auth.clone(), f.identity.clone(), None)
    .await
    .unwrap();
    let signer = webdock_identity::RlsSigner::new(
        "owned-plugin-jobs-rls-secret-32bytes",
        "webdock_auth_runtime",
    )
    .unwrap();
    let tx = f.auth.begin().await.unwrap();
    let member = f
        .identity
        .authenticate_session_in(&tx, "member", "member-session")
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    for case in 0..3 {
        let cancelled = case == 1;
        let disconnect = case == 2;
        let actor = if cancelled || disconnect {
            &f.actor
        } else {
            &member
        };
        let resource = uuid::Uuid::new_v4().to_string();
        sql(&f.owner,"INSERT INTO webdock_auth.plugin_resource(plugin_id,resource_id,offering_id,customer_id,project_id,name,created_via,created_by) VALUES('archives',$1,'archive','123','124','Source','operator','operator')",vec![resource.clone().into()]).await;
        let a=f.host.admit_with_authority(actor,serde_json::from_value::<AdmissionInput>(json!({"pluginID":"archives","projectID":"124","action":"archives.create","input":{"request_id":uuid::Uuid::new_v4(),"kind":"project","source_id":"602","retention_seconds":3600},"idempotencyKey":uuid::Uuid::new_v4().to_string()})).unwrap(),true,&webdock_browser_session::StoredAuthority::local_session(actor)).await.unwrap();
        let binding = client
            .bind(webdock_export_protocol::wire::SourceRequest {
                invocation_token: a.token,
                kind: "project".into(),
                source_id: "602".into(),
                reference_json: vec![],
            })
            .await
            .unwrap()
            .into_inner();
        let a=f.host.admit_with_authority(actor,serde_json::from_value::<AdmissionInput>(json!({"pluginID":"archives","projectID":"124","action":"archives.generate","input":{"resource_id":resource},"idempotencyKey":uuid::Uuid::new_v4().to_string()})).unwrap(),true,&webdock_browser_session::StoredAuthority::local_session(actor)).await.unwrap();
        let h = f.host.authenticate(peer(), &a.token).await.unwrap();
        f.host.jobs_enqueue(&h,json!({"name":"archives.produce","payload":{"resource_id":resource},"idempotencyKey":format!("archive-{resource}")})).await.unwrap();
        let lease = f.host.claim_job(Some("archives")).await.unwrap().unwrap();
        provider.pause.store(cancelled, Ordering::SeqCst);
        adapter.validations.store(0, Ordering::SeqCst);
        adapter.hold_data.store(disconnect, Ordering::SeqCst);
        let request = webdock_export_protocol::wire::SourceRequest {
            invocation_token: lease.invocation().token.clone(),
            kind: String::new(),
            source_id: String::new(),
            reference_json: binding.reference_json,
        };
        if !cancelled {
            let mut stream = client.stream(request).await.unwrap().into_inner();
            let first = stream.message().await.unwrap().unwrap();
            let r: SourceRef = serde_json::from_slice(&first.reference_json).unwrap();
            assert_eq!(r.commit, Some("b".repeat(40)));
            if disconnect {
                tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    adapter.entered.notified(),
                )
                .await
                .unwrap();
                let proof = adapter.proof.lock().unwrap().clone().unwrap();
                assert!(
                    f.host
                        .validate_export_job(&proof.snapshot().unwrap())
                        .await
                        .is_ok()
                );
                build_service.execute(&f.actor,json!({"action":"git.connections.disconnect","customerID":"123","connectionID":"601","revision":1})).await.unwrap();
                assert!(
                    f.host
                        .validate_export_job(&proof.snapshot().unwrap())
                        .await
                        .is_ok(),
                    "original invocation/job must remain active during source revocation"
                );
                adapter.resume.notify_one();
                assert!(
                    stream.message().await.is_err(),
                    "held real Core RPC must not release cached bytes after actual Git disconnect"
                );
                f.host
                    .complete_job(&lease, Err(webdock_plugin_protocol::PluginError::denied()))
                    .await
                    .unwrap();
                continue;
            }
            let mut bytes = vec![];
            while let Some(chunk) = stream.message().await.unwrap() {
                bytes.extend(chunk.data);
            }
            assert_eq!(
                webdock_build::artifact::Archive::read(&bytes, 33554432)
                    .unwrap()
                    .files()["README.md"],
                b"owned committed source\n"
            );
            let proof = adapter.proof.lock().unwrap().clone().unwrap();
            assert_eq!(proof.claims().job.as_ref().unwrap().id, lease.id());
            assert!(
                f.host
                    .validate_export_job(&proof.snapshot().unwrap())
                    .await
                    .is_ok()
            );
            f.host
                .complete_job(&lease, Ok(json!({"state":"ready"})))
                .await
                .unwrap();
            assert!(
                f.host
                    .validate_export_job(&proof.snapshot().unwrap())
                    .await
                    .is_err()
            );
            // An issued opaque transfer must recheck retained source authority after
            // an actual projects.update change, even though the Core job succeeded.
            let admitted=f.host.admit_with_authority(actor,serde_json::from_value::<AdmissionInput>(json!({"pluginID":"archives","projectID":"124","action":"archives.download","input":{"resource_id":resource},"idempotencyKey":uuid::Uuid::new_v4().to_string()})).unwrap(),true,&webdock_browser_session::StoredAuthority::local_session(actor)).await.unwrap();
            let handle = f.host.authenticate(peer(), &admitted.token).await.unwrap();
            let plan = webdock_plugin_protocol::storage::StoragePlan {
                version: 1,
                binding: "archives".into(),
                direction: webdock_plugin_protocol::storage::StorageDirection::Download,
                resource_id: resource.clone(),
                resource_epoch: uuid::Uuid::new_v4().to_string(),
                url: "https://storage.example.test/bucket/owned-source".into(),
                headers: Default::default(),
                expires_at: chrono::Utc::now().timestamp() + 120,
                exact_bytes: bytes.len() as u64,
                expected_sha256: Some(repository::sha256(&bytes)),
                attachment: webdock_plugin_protocol::storage::Attachment {
                    filename: "project.tar.gz".into(),
                    content_type: "application/gzip".into(),
                },
            };
            sql(&f.owner,"INSERT INTO webdock_auth.api_token(id,customer_id,session_id,user_id,name,token_hash,can_write,expires_at) VALUES('605','123','member-session','member','Owned readonly source download',$1,false,now()+interval '1 hour')",vec!["d".repeat(64).into()]).await;
            let api_descriptor = f
                .host
                .retain_storage_plan_with_api(
                    &handle,
                    plan.clone(),
                    "c".repeat(64),
                    Some("d".repeat(64)),
                )
                .await
                .unwrap();
            let descriptor = f
                .host
                .retain_storage_plan(&handle, plan, "c".repeat(64))
                .await
                .unwrap();
            let caller = webdock_plugin_host::storage::StorageCaller::browser(
                actor,
                webdock_browser_session::StoredAuthority::local_session(actor),
                true,
            );
            let access = f
                .host
                .storage_access(&descriptor.id, "GET", &caller)
                .await
                .unwrap();
            let validation = webdock_export_protocol::wire::Validation {
                invocation_token: String::new(),
                storage_authorization_json: serde_json::to_vec(&access.authorization).unwrap(),
                reference_json: serde_json::to_vec(&r).unwrap(),
                producer_job_id: lease.id().into(),
            };
            client.validate(validation.clone()).await.unwrap();
            let api_access = f
                .host
                .storage_access(&api_descriptor.id, "GET", &caller)
                .await
                .unwrap();
            let api_validation = webdock_export_protocol::wire::Validation {
                storage_authorization_json: serde_json::to_vec(&api_access.authorization).unwrap(),
                ..validation.clone()
            };
            client.validate(api_validation.clone()).await.unwrap();
            sql(
                &f.owner,
                "DELETE FROM webdock_auth.api_token WHERE id='605'",
                vec![],
            )
            .await;
            assert!(
                client.validate(api_validation).await.is_err(),
                "retained source callback must preserve original API token revocation even for a current browser caller"
            );

            hosting.execute(&f.actor,json!({"action":"projects.update","projectID":"124","mode":"managed","ownImages":false,"revision":1})).await.unwrap();
            assert!(
                client.validate(validation.clone()).await.is_err(),
                "issued transfer must deny selfservice-to-managed transition"
            );
            let scope = InvocationScope {
                plugin_id: "archives".into(),
                customer_id: "123".into(),
                project_id: "124".into(),
                operation_id: "retained-check".into(),
                subject: "member".into(),
                session_id: "member-session".into(),
                generation: 1,
                grant_revision: 0,
            };
            let tx = signer.begin(&business, &member).await.unwrap();
            assert!(gate.validate_export_source(&scope, &tx, &r).await.is_err());
            tx.rollback().await.unwrap();
            let op_scope = InvocationScope {
                subject: "operator".into(),
                session_id: "session".into(),
                ..scope
            };
            let tx = signer.begin(&business, &f.actor).await.unwrap();
            assert!(
                gate.validate_export_source(&op_scope, &tx, &r)
                    .await
                    .is_ok(),
                "qualified native operator may read managed source"
            );
            tx.rollback().await.unwrap();
            sql(
                &f.owner,
                "DELETE FROM webdock_admin.users WHERE auth_subject='operator'",
                vec![],
            )
            .await;
            let tx = signer.begin(&business, &f.actor).await.unwrap();
            assert!(
                gate.validate_export_source(&op_scope, &tx, &r)
                    .await
                    .is_err(),
                "raw operator role without native operator mapping must not bypass managed mode"
            );
            tx.rollback().await.unwrap();
            sql(&f.owner,"INSERT INTO webdock_admin.users(id,name,email,role,auth_subject) VALUES('100','Operator','jobs@example.test','operator','operator')",vec![]).await;
            hosting.execute(&f.actor,json!({"action":"projects.update","projectID":"124","mode":"selfservice","ownImages":false,"revision":2})).await.unwrap();
            sql(
                &f.owner,
                "UPDATE webdock_auth.git_connection SET generation=generation+1 WHERE id='601'",
                vec![],
            )
            .await;
            assert!(
                client.validate(validation).await.is_err(),
                "retained committed-source transfer must deny rotated repository binding"
            );
            let member_scope = InvocationScope {
                subject: "member".into(),
                session_id: "member-session".into(),
                ..op_scope
            };
            let tx = signer.begin(&business, &member).await.unwrap();
            let current = gate
                .export_binding(&member_scope, &tx, "602")
                .await
                .unwrap();
            assert_ne!(
                current.binding, r.binding,
                "fresh source reference observes rotation"
            );
            tx.commit().await.unwrap();
        } else {
            let mut pending = client.clone();
            let call = tokio::spawn(async move { pending.stream(request).await });
            tokio::time::timeout(
                std::time::Duration::from_secs(5),
                provider.entered.notified(),
            )
            .await
            .unwrap();
            let a=f.host.admit_with_authority(actor,serde_json::from_value::<AdmissionInput>(json!({"pluginID":"archives","projectID":"124","action":"archives.delete","input":{"resource_id":resource},"idempotencyKey":uuid::Uuid::new_v4().to_string()})).unwrap(),true,&webdock_browser_session::StoredAuthority::local_session(actor)).await.unwrap();
            let h = f.host.authenticate(peer(), &a.token).await.unwrap();
            f.host
                .jobs_cancel(&h, json!({"id":lease.id()}))
                .await
                .unwrap();
            assert!(
                tokio::time::timeout(std::time::Duration::from_secs(5), call)
                    .await
                    .unwrap()
                    .unwrap()
                    .is_err()
            );
            let claims = provider.last.lock().unwrap().clone().unwrap();
            let mut callback =
                repository::wire::repository_authority_client::RepositoryAuthorityClient::new(
                    runtime::connect_unix(&authority_path).await.unwrap(),
                );
            assert!(
                callback
                    .authorize(repository::wire::LeaseProof {
                        lease_token: claims.sign(&issuer).unwrap()
                    })
                    .await
                    .is_err()
            );
        }
    }
}
