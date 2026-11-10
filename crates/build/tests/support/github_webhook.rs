//! Real source consent and installed repository policy; no provider HTTP or Kubernetes calls.
use super::*;
use hmac::{Hmac, Mac};
use spitzli_build::{GithubWebhookVerifier, RepositoryAdmission};
use spitzli_repository_protocol as protocol;
use std::os::unix::fs::PermissionsExt;

struct HeadHandler {
    head: std::sync::Mutex<String>,
    calls: std::sync::atomic::AtomicUsize,
    paused: std::sync::atomic::AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    claims: std::sync::Mutex<Option<protocol::LeaseClaims>>,
}
#[async_trait::async_trait]
impl protocol::runtime::Handler for HeadHandler {
    async fn resolve(
        &self,
        claims: &protocol::LeaseClaims,
        input: Value,
        _: Value,
    ) -> protocol::Result<Value> {
        use std::sync::atomic::Ordering;
        assert_eq!(claims.action, protocol::Action::Resolve);
        assert_eq!(input["reference"], "release");
        assert_eq!(input.as_object().unwrap().len(), 3);
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.claims.lock().unwrap() = Some(claims.clone());
        if self.paused.load(Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.notified().await;
        }
        Ok(json!({"repositoryID":input["repositoryID"],"sha":self.head.lock().unwrap().clone()}))
    }
    async fn archive(
        &self,
        _: &protocol::LeaseClaims,
        _: Value,
        _: Value,
    ) -> protocol::Result<Vec<u8>> {
        panic!("workflow head gate has no archive authority")
    }
}
struct RuntimeTasks(Vec<tokio::task::JoinHandle<()>>);
impl Drop for RuntimeTasks {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}
fn workflow_body(sha: char) -> Value {
    json!({"action":"completed","installation":{"id":1234},"repository":{"id":55,"fork":false},"workflow":{"id":77,"path":".github/workflows/quality.yml"},"workflow_run":{"id":88,"workflow_id":77,"head_branch":"release","head_sha":sha.to_string().repeat(40),"event":"push","status":"completed","conclusion":"success","repository":{"id":55,"fork":false},"head_repository":{"id":55,"fork":false}}})
}

fn body(sha: char, repository: u64) -> Vec<u8> {
    serde_json::to_vec(&json!({"installation":{"id":1234},"repository":{"id":repository},"ref":"refs/heads/main","after":sha.to_string().repeat(40),"deleted":false})).unwrap()
}
fn signature(raw: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(&[19; 32]).unwrap();
    mac.update(raw);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}
fn delivery(number: u32) -> String {
    format!("01111111-2222-4333-8444-{number:012x}")
}

#[tokio::test]
#[ignore = "owned fresh native database via tools/tests/kubernetes-builds.py"]
async fn github_webhook_receipts_bind_live_source_policy_and_deduplicate_pushes() {
    let fixture = Arc::new(Fixture::new("kubernetes").await);
    let f = fixture.clone();
    let plugin = format!("github-{}", f.nonce);
    let cleanup_plugin = plugin.clone();
    let tenant_actor = format!("{}-push-admin", f.nonce);
    let cleanup_actor = tenant_actor.clone();
    let task = async move {
        f.configure("k3s").await;
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut manifest: Value = serde_json::from_str(include_str!(
            "../../../plugin-protocol/tests/fixtures/project-notes.json"
        ))
        .unwrap();
        manifest["name"] = json!(plugin);
        manifest["jobs"] = json!([]);
        manifest["services"]["required"] = json!([]);
        manifest["capabilities"] = json!(["plugin.http.read"]);
        manifest["actions"] = json!([{"name":"repository.resolve","intent":"read","audience":"operator","capabilities":["plugin.http.read"]},{"name":"repository.archive","intent":"read","audience":"operator","capabilities":["plugin.http.read"]}]);
        let raw = manifest.to_string();
        let digest = protocol::sha256(raw.as_bytes());
        let credential = protocol::sha256(b"owned-webhook-repository-credential");
        sql(&f.owner,"INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version,runtime,current_generation) VALUES($1,'external','enabled','enabled','1.0.0','ready',1)",vec![plugin.clone().into()]).await;
        sql(&f.owner,"INSERT INTO webdock_auth.plugin_release(plugin_id,version,manifest_bytes,manifest_sha256,image,registry_url,signer_key_id) VALUES($1,'1.0.0',$2,$3,$4,'https://registry.invalid','fixture')",vec![plugin.clone().into(),raw.into(),digest.clone().into(),manifest["image"].as_str().unwrap().into()]).await;
        sql(&f.owner,"INSERT INTO webdock_auth.plugin_configuration(plugin_id,approved_capabilities,configuration) VALUES($1,$2,'{}')",vec![plugin.clone().into(),manifest["capabilities"].clone().into()]).await;
        sql(&f.owner,"INSERT INTO webdock_auth.plugin_runtime(plugin_id,generation,version,image,status,configuration_revision,credential_hash,namespace,service_name) VALUES($1,1,'1.0.0',$2,'ready',0,$3,'fixtures','fixture-plugin')",vec![plugin.clone().into(),manifest["image"].as_str().unwrap().into(),credential.clone().into()]).await;
        sql(&f.owner,"INSERT INTO webdock_auth.plugin_grant(plugin_id,customer_id,project_id,capabilities) VALUES($1,$2,$3,$4)",vec![plugin.clone().into(),f.cid.clone().into(),f.pid.clone().into(),manifest["capabilities"].clone().into()]).await;
        let host_path = directory.path().join("host.json");
        let binding_path = directory.path().join("repository.json");
        let runtime_path = directory.path().join("repository.sock");
        let authority_path = directory.path().join("authority.sock");
        std::fs::write(&host_path,json!({"runtimes":{plugin.clone():{"generation":1,"namespace":"fixtures","service":"fixture-plugin","credentialSha256":credential,"peers":[{"kind":"unix","uid":rustix::process::geteuid().as_raw()}],"projects":{}}}}).to_string()).unwrap();
        std::fs::set_permissions(&host_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let publisher = ed25519_dalek::SigningKey::from_bytes(&[41; 32]);
        let descriptor = protocol::Descriptor {
            v: 1,
            plugin_id: plugin.clone(),
            version: "1.0.0".into(),
            manifest_sha256: digest,
            image: manifest["image"].as_str().unwrap().into(),
            protocol: "webdock.repository.v1".into(),
            signer_key_id: "fixture".into(),
        }
        .sign(&publisher)
        .unwrap();
        std::fs::write(&binding_path,json!({"runtimes":{plugin.clone():{"descriptor":descriptor,"publisherKeyId":"fixture","publisherPublicKeyHex":hex::encode(publisher.verifying_key().as_bytes()),"registryUrl":"https://registry.invalid","transport":{"kind":"unix","path":runtime_path}}}}).to_string()).unwrap();
        std::fs::set_permissions(&binding_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let ids = Arc::new(spitzli_persistence::IdAllocator::new(
            db("DATABASE_URL").await,
            spitzli_ids::Identity::platform(0, 2).unwrap(),
        ));
        let secret = std::env::var("DATABASE_RLS_SECRET").unwrap();
        let host = Arc::new(
            spitzli_plugin_host::HostService::new(
                db("DATABASE_URL").await,
                db("SPITZLI_PLUGIN_WORKER_DATABASE_URL").await,
                secret.clone(),
                ids.clone(),
                Arc::new(spitzli_plugin_host::FileBindings::new(host_path)),
                ed25519_dalek::SigningKey::from_bytes(&[37; 32]),
            )
            .await
            .unwrap(),
        );
        let gate = Arc::new(
            RepositoryAdmission::new(
                db("SPITZLI_PLUGIN_WORKER_DATABASE_URL").await,
                secret.clone(),
                host,
                ed25519_dalek::SigningKey::from_bytes(&[39; 32]),
                Arc::new(spitzli_build::FileRepositoryBindings::new(binding_path)),
            )
            .await
            .unwrap(),
        );
        let mut socket = protocol::runtime::UnixSocket::bind(&authority_path).unwrap();
        let incoming = socket.incoming().unwrap();
        let callback = gate.clone().authority_service();
        let mut runtime_tasks = RuntimeTasks(vec![tokio::spawn(async move {
            let _socket = socket;
            tonic::transport::Server::builder()
                .add_service(callback)
                .serve_with_incoming(incoming)
                .await
                .unwrap();
        })]);
        let authority = protocol::runtime::RemoteAuthority::new(
            protocol::runtime::connect_unix(&authority_path)
                .await
                .unwrap(),
            plugin.clone(),
            1,
            protocol::sha256(descriptor.as_bytes()),
            vec![protocol::runtime::CorePeer::Unix {
                uid: rustix::process::geteuid().as_raw(),
            }],
        )
        .unwrap();
        let handler = Arc::new(HeadHandler {
            head: std::sync::Mutex::new("a".repeat(40)),
            calls: std::sync::atomic::AtomicUsize::new(0),
            paused: std::sync::atomic::AtomicBool::new(false),
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
            claims: std::sync::Mutex::new(None),
        });
        let server = protocol::runtime::Runtime::new(
            descriptor,
            &publisher.verifying_key(),
            1,
            ed25519_dalek::SigningKey::from_bytes(&[39; 32]).verifying_key(),
            Arc::new(authority),
            handler.clone(),
        )
        .unwrap();
        let socket = protocol::runtime::UnixSocket::bind(&runtime_path).unwrap();
        runtime_tasks.0.push(tokio::spawn(async move {
            server
                .serve_unix(socket, std::future::pending())
                .await
                .unwrap()
        }));
        let service = spitzli_build::BuildService::new(
            db("DATABASE_URL").await,
            db("SPITZLI_PLUGIN_WORKER_DATABASE_URL").await,
            secret,
            ids,
            f.service.clone(),
            "registry.invalid".into(),
        )
        .await
        .unwrap()
        .with_repository_provider(Arc::new(spitzli_build::PluginRepositoryProvider::new(
            gate.clone(),
        )));
        let mut draft = spec();
        draft.as_object_mut().unwrap().remove("image");
        draft.as_object_mut().unwrap().remove("template");
        let app=f.service.execute(&f.principal,json!({"action":"apps.prepare","projectID":f.pid,"name":"webhook-target","spec":draft,"subscriptionRevision":1,"idempotencyKey":"webhook-target-request"})).await.unwrap();
        let target = app["appID"]
            .as_str()
            .or_else(|| app["id"].as_str())
            .unwrap();
        let connection = id(&f.owner).await;
        sql(&f.owner,"INSERT INTO webdock_auth.git_connection(id,customer_id,installation_id,account_id,account_login,permissions,state,repository_plugin_id) VALUES($1,$2,'1234','1234','fixture','{}','active',$3)",vec![connection.clone().into(),f.cid.clone().into(),plugin.clone().into()]).await;
        sql(&f.owner,"INSERT INTO webdock_auth.git_repository(connection_id,customer_id,repository_id,metadata) VALUES($1,$2,'55','{}')",vec![connection.clone().into(),f.cid.clone().into()]).await;
        let source=service.execute(&f.principal,json!({"action":"git.source.configure","projectID":f.pid,"targetID":target,"connectionID":connection,"repositoryID":"55","branch":"main","rootDirectory":".","recipe":"dockerfile","revision":0,"autoPublish":false})).await.unwrap();
        let source = source["id"].as_str().unwrap();
        let verifier = GithubWebhookVerifier::new(vec![19; 32], plugin.clone()).unwrap();
        let raw = body('a', 55);
        assert!(
            verifier
                .verify(&raw, "sha256=invalid", &delivery(1), "push")
                .is_err()
        );
        let verified = |raw: &[u8], number| {
            verifier
                .verify(raw, &signature(raw), &delivery(number), "push")
                .unwrap()
        };
        assert_eq!(
            service
                .ingest_github_webhook(&verified(&raw, 1), &gate)
                .await
                .unwrap()["buildIDs"],
            json!([]),
            "source webhook consent defaults off"
        );
        service
            .configure_github_build_trigger(&f.principal, &f.pid, source, 1, true)
            .await
            .unwrap();
        assert_eq!(
            service
                .ingest_github_webhook(&verified(&raw, 1), &gate)
                .await
                .unwrap()["buildIDs"],
            json!([]),
            "delivery replay preserves its original result"
        );
        let first = service
            .ingest_github_webhook(&verified(&raw, 2), &gate)
            .await
            .unwrap();
        assert_eq!(first["buildIDs"].as_array().unwrap().len(), 1);
        let build = first["buildIDs"][0].as_str().unwrap();
        let replay = service
            .ingest_github_webhook(&verified(&raw, 2), &gate)
            .await
            .unwrap();
        assert_eq!(replay["buildIDs"], first["buildIDs"]);
        assert_eq!(replay["replayed"], true);
        assert_eq!(
            service
                .ingest_github_webhook(&verified(&raw, 3), &gate)
                .await
                .unwrap()["buildIDs"],
            first["buildIDs"],
            "new delivery GUID does not duplicate a logical push"
        );
        let changed = body('b', 55);
        assert_eq!(
            service
                .ingest_github_webhook(&verified(&changed, 2), &gate)
                .await
                .unwrap_err()
                .code,
            "github_webhook_delivery_conflict"
        );
        assert_eq!(
            service
                .ingest_github_webhook(&verified(&body('a', 56), 4), &gate)
                .await
                .unwrap()["buildIDs"],
            json!([]),
            "foreign repository cannot select source"
        );
        sql(
            &f.owner,
            "UPDATE webdock_auth.git_connection SET state='disconnected' WHERE id=$1",
            vec![connection.clone().into()],
        )
        .await;
        assert_eq!(
            service
                .ingest_github_webhook(&verified(&changed, 5), &gate)
                .await
                .unwrap()["buildIDs"],
            json!([])
        );
        sql(
            &f.owner,
            "UPDATE webdock_auth.git_connection SET state='active' WHERE id=$1",
            vec![connection.clone().into()],
        )
        .await;
        sql(
            &f.owner,
            "UPDATE webdock_auth.\"user\" SET banned=true WHERE id=$1",
            vec![f.uid.clone().into()],
        )
        .await;
        assert_eq!(
            service
                .ingest_github_webhook(&verified(&changed, 6), &gate)
                .await
                .unwrap()["buildIDs"],
            json!([]),
            "revoked source actor cannot trigger builds"
        );
        sql(
            &f.owner,
            "UPDATE webdock_auth.\"user\" SET banned=false WHERE id=$1",
            vec![f.uid.clone().into()],
        )
        .await;
        sql(
            &f.owner,
            "UPDATE webdock_auth.plugin_grant SET revoked_at=now() WHERE plugin_id=$1",
            vec![plugin.clone().into()],
        )
        .await;
        assert_eq!(
            service
                .ingest_github_webhook(&verified(&changed, 7), &gate)
                .await
                .unwrap_err()
                .code,
            "repository_plugin_denied"
        );
        sql(
            &f.owner,
            "UPDATE webdock_auth.plugin_grant SET revoked_at=NULL WHERE plugin_id=$1",
            vec![plugin.clone().into()],
        )
        .await;
        let second = service
            .ingest_github_webhook(&verified(&changed, 7), &gate)
            .await
            .unwrap();
        assert_ne!(second["buildIDs"], first["buildIDs"]);
        let row=f.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT status='cancelled' AND failure_code='SUPERSEDED' AND trigger_kind='github-push' AND subject=$2 AS correct FROM webdock_auth.git_build WHERE id=$1",vec![build.into(),f.uid.clone().into()])).await.unwrap().unwrap();
        assert!(row.try_get::<bool>("", "correct").unwrap());
        assert_eq!(
            service
                .ingest_github_webhook(&verified(&raw, 8), &gate)
                .await
                .unwrap()["buildIDs"],
            first["buildIDs"],
            "old commit redelivery cannot supersede newer build"
        );
        let row=f.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT latest_build_id=$2 AND NOT auto_publish AS correct FROM webdock_auth.git_source WHERE id=$1",vec![source.into(),second["buildIDs"][0].as_str().unwrap().into()])).await.unwrap().unwrap();
        assert!(row.try_get::<bool>("", "correct").unwrap());
        let unsigned = db("SPITZLI_PLUGIN_WORKER_DATABASE_URL").await;
        let count = unsigned
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT count(*) AS count FROM webdock_auth.git_webhook_delivery",
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(count.try_get::<i64>("", "count").unwrap(), 0);
        // This is a persisted local admin, with no fabricated runtime Principal or session.
        sql(&f.owner,"INSERT INTO webdock_auth.\"user\"(id,name,email,\"emailVerified\",role,\"mustChangePassword\") VALUES($1,$1,$2,true,'user',false)",vec![tenant_actor.clone().into(),format!("{tenant_actor}@example.invalid").into()]).await;
        sql(&f.owner,"INSERT INTO webdock_auth.member(\"organizationId\",\"userId\",role,\"createdAt\") SELECT organization_id,$2,'admin',now() FROM webdock_auth.tenant_customer WHERE customer_id=$1",vec![f.cid.clone().into(),tenant_actor.clone().into()]).await;
        sql(
            &f.owner,
            "UPDATE webdock_auth.git_source SET webhook_subject=$2 WHERE id=$1",
            vec![source.into(), tenant_actor.clone().into()],
        )
        .await;
        assert_eq!(
            service
                .ingest_github_webhook(&verified(&body('d', 55), 10), &gate)
                .await
                .unwrap()["buildIDs"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        sql(
            &f.owner,
            "DELETE FROM webdock_auth.member WHERE \"userId\"=$1",
            vec![tenant_actor.clone().into()],
        )
        .await;
        assert_eq!(
            service
                .ingest_github_webhook(&verified(&body('e', 55), 11), &gate)
                .await
                .unwrap()["buildIDs"],
            json!([]),
            "current tenant membership is required independently of webhook signature"
        );
        sql(
            &f.owner,
            "UPDATE webdock_auth.git_source SET webhook_subject=$2 WHERE id=$1",
            vec![source.into(), f.uid.clone().into()],
        )
        .await;
        sql(
            &f.owner,
            "UPDATE webdock_auth.git_connection SET generation=generation+1 WHERE id=$1",
            vec![connection.clone().into()],
        )
        .await;
        assert_eq!(
            service
                .ingest_github_webhook(&verified(&raw, 12), &gate)
                .await
                .unwrap()["buildIDs"],
            json!([]),
            "connection rotation invalidates prior trigger consent"
        );
        service
            .configure_github_build_trigger(&f.principal, &f.pid, source, 1, true)
            .await
            .unwrap();
        let rotated = service
            .ingest_github_webhook(&verified(&raw, 13), &gate)
            .await
            .unwrap();
        assert_ne!(
            rotated["buildIDs"], first["buildIDs"],
            "logical key includes connection generation"
        );
        sql(
            &f.owner,
            "UPDATE webdock_auth.git_source SET revision=revision+1 WHERE id=$1",
            vec![source.into()],
        )
        .await;
        assert_eq!(
            service
                .ingest_github_webhook(&verified(&raw, 14), &gate)
                .await
                .unwrap()["buildIDs"],
            json!([]),
            "changed source configuration requires new trigger consent"
        );
        service
            .configure_github_build_trigger(&f.principal, &f.pid, source, 2, true)
            .await
            .unwrap();
        let revised = service
            .ingest_github_webhook(&verified(&raw, 15), &gate)
            .await
            .unwrap();
        assert_ne!(
            revised["buildIDs"], rotated["buildIDs"],
            "logical key includes source revision"
        );
        assert_eq!(
            service
                .ingest_github_webhook_with_admission(&verified(&raw, 2), &gate, false)
                .await
                .unwrap()["buildIDs"],
            first["buildIDs"],
            "Core draining retains exact signed receipts"
        );
        assert_eq!(
            service
                .ingest_github_webhook_with_admission(&verified(&body('e', 55), 100), &gate, false)
                .await
                .unwrap_err()
                .code,
            "build_admission_draining",
            "Core draining cannot admit a fresh delivery even when plugin remains enabled"
        );
        sql(
            &f.owner,
            "UPDATE webdock_auth.plugin_installation SET state='draining' WHERE id='hosting'",
            vec![],
        )
        .await;
        assert_eq!(
            service
                .ingest_github_webhook(&verified(&raw, 2), &gate)
                .await
                .unwrap()["buildIDs"],
            first["buildIDs"],
            "exact acknowledged replay remains available while hosting drains"
        );
        assert_eq!(
            service
                .ingest_github_webhook(&verified(&body('f', 55), 16), &gate)
                .await
                .unwrap_err()
                .code,
            "hosting_unavailable",
            "new push work cannot be admitted while hosting drains"
        );
        sql(
            &f.owner,
            "UPDATE webdock_auth.plugin_installation SET state='enabled' WHERE id='hosting'",
            vec![],
        )
        .await;
        assert_eq!(
            service
                .ingest_github_webhook(&verified(&body('f', 55), 16), &gate)
                .await
                .unwrap()["buildIDs"]
                .as_array()
                .unwrap()
                .len(),
            1,
            "failed admission did not stamp a successful delivery receipt"
        );
        service
            .configure_github_build_trigger(&f.principal, &f.pid, source, 2, false)
            .await
            .unwrap();
        assert_eq!(
            service
                .ingest_github_webhook(&verified(&body('c', 55), 9), &gate)
                .await
                .unwrap()["buildIDs"],
            json!([])
        );
        use std::sync::atomic::Ordering;
        service
            .configure_kubernetes_admission(&f.principal, true, 1, 1)
            .await
            .unwrap();
        service
            .trust_kubernetes_project(&f.principal, &f.cid, &f.pid, true)
            .await
            .unwrap();
        let configure = |revision: i32| json!({"action":"git.source.configure","projectID":f.pid,"targetID":target,"connectionID":connection,"repositoryID":"55","branch":"release","rootDirectory":".","recipe":"dockerfile","executionBackend":"kubernetes","revision":revision,"autoPublish":false});
        let mut quality = configure(2);
        quality["webhookTrigger"] = json!("workflow_run");
        quality["workflowPath"] = json!(".github/workflows/quality.yml");
        let configured = service.execute(&f.principal, quality).await.unwrap();
        assert_eq!(configured["webhookTrigger"], "workflow_run");
        assert_eq!(configured["webhookEnabled"], false);
        let preserved = service.execute(&f.principal, configure(3)).await.unwrap();
        assert_eq!(preserved["webhookTrigger"], "workflow_run");
        assert_eq!(
            preserved["workflowPath"], ".github/workflows/quality.yml",
            "old clients cannot reset the quality workflow path"
        );
        let mut downgrade = configure(4);
        downgrade["executionBackend"] = json!("microvm");
        assert_eq!(
            service
                .execute(&f.principal, downgrade)
                .await
                .unwrap_err()
                .code,
            "invalid_webhook_trigger"
        );
        service
            .configure_github_build_trigger(&f.principal, &f.pid, source, 4, true)
            .await
            .unwrap();
        let service = Arc::new(service);
        let workflow = |value: Value, number: u32| {
            let raw = serde_json::to_vec(&value).unwrap();
            verifier
                .verify(&raw, &signature(&raw), &delivery(number), "workflow_run")
                .unwrap()
        };
        let mut plain: Value = serde_json::from_slice(&body('a', 55)).unwrap();
        plain["ref"] = json!("refs/heads/release");
        let plain = serde_json::to_vec(&plain).unwrap();
        assert_eq!(
            service
                .ingest_github_webhook(&verified(&plain, 200), &gate)
                .await
                .unwrap()["buildIDs"],
            json!([]),
            "push cannot bypass the opt-in workflow gate"
        );
        let first = service
            .ingest_github_webhook(&workflow(workflow_body('a'), 201), &gate)
            .await
            .unwrap();
        assert_eq!(first["buildIDs"].as_array().unwrap().len(), 1);
        let replay = service
            .ingest_github_webhook(&workflow(workflow_body('a'), 201), &gate)
            .await
            .unwrap();
        assert_eq!(replay["buildIDs"], first["buildIDs"]);
        assert_eq!(replay["replayed"], true);
        assert_eq!(
            service
                .ingest_github_webhook(&workflow(workflow_body('a'), 202), &gate)
                .await
                .unwrap()["buildIDs"],
            first["buildIDs"],
            "quality reruns/new GUIDs do not duplicate the same source commit"
        );
        let id = first["buildIDs"][0].as_str().unwrap();
        let row=f.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT execution_backend='kubernetes' AND trigger_kind='github-workflow' AND source_sha=repeat('a',40) AS correct FROM webdock_auth.git_build WHERE id=$1",vec![id.into()])).await.unwrap().unwrap();
        assert!(row.try_get::<bool>("", "correct").unwrap());
        service
            .configure_kubernetes_admission(&f.principal, false, 1, 1)
            .await
            .unwrap();
        assert_eq!(
            service
                .ingest_github_webhook(&workflow(workflow_body('a'), 220), &gate)
                .await
                .unwrap_err()
                .code,
            "kubernetes_project_not_trusted"
        );
        assert_eq!(
            service
                .ingest_github_webhook(&workflow(workflow_body('a'), 201), &gate)
                .await
                .unwrap()["buildIDs"],
            first["buildIDs"],
            "exact receipts remain replayable while native admission is paused"
        );
        service
            .configure_kubernetes_admission(&f.principal, true, 1, 1)
            .await
            .unwrap();
        service
            .trust_kubernetes_project(&f.principal, &f.cid, &f.pid, false)
            .await
            .unwrap();
        assert_eq!(
            service
                .ingest_github_webhook(&workflow(workflow_body('a'), 221), &gate)
                .await
                .unwrap_err()
                .code,
            "kubernetes_project_not_trusted"
        );
        service
            .trust_kubernetes_project(&f.principal, &f.cid, &f.pid, true)
            .await
            .unwrap();
        let before = handler.calls.load(Ordering::SeqCst);
        for (n, pointer, value) in [
            (203, "/workflow_run/conclusion", json!("failure")),
            (
                204,
                "/workflow/path",
                json!(".github/workflows/bootstrap.yml"),
            ),
            (205, "/workflow_run/head_repository/fork", json!(true)),
            (206, "/workflow_run/event", json!("pull_request")),
            (207, "/workflow_run/head_branch", json!("main")),
            (208, "/workflow_run/event", json!("workflow_dispatch")),
        ] {
            let mut event = workflow_body('a');
            *event.pointer_mut(pointer).unwrap() = value;
            assert_eq!(
                service
                    .ingest_github_webhook(&workflow(event, n), &gate)
                    .await
                    .unwrap()["buildIDs"],
                json!([])
            );
        }
        let mut unknown = workflow_body('a');
        for pointer in [
            "/repository/id",
            "/workflow_run/repository/id",
            "/workflow_run/head_repository/id",
        ] {
            *unknown.pointer_mut(pointer).unwrap() = json!(56);
        }
        assert_eq!(
            service
                .ingest_github_webhook(&workflow(unknown, 209), &gate)
                .await
                .unwrap()["buildIDs"],
            json!([])
        );
        assert_eq!(
            handler.calls.load(Ordering::SeqCst),
            before,
            "unrelated/unsafe workflows never reach the repository provider"
        );
        assert_eq!(
            service
                .ingest_github_webhook(&workflow(workflow_body('b'), 210), &gate)
                .await
                .unwrap()["buildIDs"],
            json!([]),
            "old SHA is checked against the real provider branch resolution"
        );
        let raw = serde_json::to_vec(&workflow_body('a')).unwrap();
        assert!(
            verifier
                .verify(&raw, "sha256=invalid", &delivery(211), "workflow_run")
                .is_err()
        );
        *handler.head.lock().unwrap() = "b".repeat(40);
        let newer = service
            .ingest_github_webhook(&workflow(workflow_body('b'), 212), &gate)
            .await
            .unwrap();
        assert_ne!(newer["buildIDs"], first["buildIDs"]);
        let row=f.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT status='cancelled' AND failure_code='SUPERSEDED' AS correct FROM webdock_auth.git_build WHERE id=$1",vec![id.into()])).await.unwrap().unwrap();
        assert!(row.try_get::<bool>("", "correct").unwrap());
        assert_eq!(
            service
                .ingest_github_webhook(&workflow(workflow_body('a'), 224), &gate)
                .await
                .unwrap()["buildIDs"],
            json!([]),
            "an old successful SHA cannot re-enter after the branch advanced"
        );
        assert!(
            f.owner
                .execute_raw(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "UPDATE webdock_auth.git_build SET trigger_kind='github-push' WHERE id=$1",
                    vec![id.into()]
                ))
                .await
                .is_err(),
            "stored trigger proof cannot be relabeled as a legacy push"
        );
        let legacy_id = super::id(&f.owner).await;
        let denied=f.owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.git_build(id,customer_id,project_id,source_id,source_sha,source_revision,connection_generation,environment_revision,subject,idempotency_key,environment_identity,target_revision,trigger_kind) SELECT $1,customer_id,project_id,source_id,source_sha,source_revision,connection_generation,environment_revision,subject,$2,environment_identity,target_revision,'github-push' FROM webdock_auth.git_build WHERE id=$3",vec![legacy_id.into(),format!("legacy-workflow-bypass-{}",f.nonce).into(),id.into()])).await.unwrap_err();
        assert!(
            denied
                .to_string()
                .contains("build source workflow gate changed"),
            "older push-only binaries cannot bypass opted-in workflow sources: {denied}"
        );
        // Loss of actor authority during a genuine provider RPC must prevent admission.
        handler.paused.store(true, Ordering::SeqCst);
        let caller = service.clone();
        let policy = gate.clone();
        let pending_event = workflow(workflow_body('b'), 213);
        let pending =
            tokio::spawn(
                async move { caller.ingest_github_webhook(&pending_event, &policy).await },
            );
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            handler.entered.notified(),
        )
        .await
        .unwrap();
        sql(
            &f.owner,
            "UPDATE webdock_auth.\"user\" SET banned=true WHERE id=$1",
            vec![f.uid.clone().into()],
        )
        .await;
        handler.release.notify_one();
        let rejected = tokio::time::timeout(std::time::Duration::from_secs(5), pending)
            .await
            .unwrap()
            .unwrap();
        assert!(rejected.is_err() || rejected.unwrap()["buildIDs"] == json!([]));
        sql(
            &f.owner,
            "UPDATE webdock_auth.\"user\" SET banned=false WHERE id=$1",
            vec![f.uid.clone().into()],
        )
        .await;
        handler.paused.store(false, Ordering::SeqCst);
        // An exact replay callback survives no new authority: reopen only this owned lease.
        let claims = handler.claims.lock().unwrap().clone().unwrap();
        let issuer = ed25519_dalek::SigningKey::from_bytes(&[39; 32]);
        sql(
            &f.owner,
            "UPDATE webdock_auth.repository_lease SET state='active' WHERE id=$1::uuid",
            vec![claims.id.clone().into()],
        )
        .await;
        let mut callback =
            protocol::wire::repository_authority_client::RepositoryAuthorityClient::new(
                protocol::runtime::connect_unix(&authority_path)
                    .await
                    .unwrap(),
            );
        let token = claims.sign(&issuer).unwrap();
        callback
            .authorize(protocol::wire::LeaseProof {
                lease_token: token.clone(),
            })
            .await
            .unwrap();
        service
            .configure_kubernetes_admission(&f.principal, false, 1, 1)
            .await
            .unwrap();
        assert!(
            callback
                .authorize(protocol::wire::LeaseProof {
                    lease_token: token.clone()
                })
                .await
                .is_err(),
            "paused native admission revokes source-trigger callbacks"
        );
        service
            .configure_kubernetes_admission(&f.principal, true, 1, 1)
            .await
            .unwrap();
        callback
            .authorize(protocol::wire::LeaseProof {
                lease_token: token.clone(),
            })
            .await
            .unwrap();
        let mut archive = claims.clone();
        archive.action = protocol::Action::Archive;
        assert!(
            callback
                .authorize(protocol::wire::LeaseProof {
                    lease_token: archive.sign(&issuer).unwrap()
                })
                .await
                .is_err(),
            "resolve consent cannot authorize archives"
        );
        sql(&f.owner,"UPDATE webdock_auth.repository_lease SET authority_generation=authority_generation+1 WHERE id=$1::uuid",vec![claims.id.clone().into()]).await;
        assert!(
            callback
                .authorize(protocol::wire::LeaseProof {
                    lease_token: token.clone()
                })
                .await
                .is_err(),
            "environment snapshot is part of source trigger authority"
        );
        sql(&f.owner,"UPDATE webdock_auth.repository_lease SET authority_generation=authority_generation-1 WHERE id=$1::uuid",vec![claims.id.clone().into()]).await;
        service
            .configure_github_build_trigger(&f.principal, &f.pid, source, 4, false)
            .await
            .unwrap();
        assert!(
            callback
                .authorize(protocol::wire::LeaseProof {
                    lease_token: token.clone()
                })
                .await
                .is_err(),
            "revoked source consent invalidates in-flight callbacks"
        );
        service.execute(&f.principal, configure(4)).await.unwrap();
        service
            .configure_github_build_trigger(&f.principal, &f.pid, source, 5, true)
            .await
            .unwrap();
        assert!(
            callback
                .authorize(protocol::wire::LeaseProof { lease_token: token })
                .await
                .is_err(),
            "renewed consent cannot revive a lease for an older source revision"
        );
        service
            .ingest_github_webhook(&workflow(workflow_body('b'), 222), &gate)
            .await
            .unwrap();
        let connection_claims = handler.claims.lock().unwrap().clone().unwrap();
        sql(
            &f.owner,
            "UPDATE webdock_auth.repository_lease SET state='active' WHERE id=$1::uuid",
            vec![connection_claims.id.clone().into()],
        )
        .await;
        sql(
            &f.owner,
            "UPDATE webdock_auth.git_connection SET generation=generation+1 WHERE id=$1",
            vec![connection.clone().into()],
        )
        .await;
        service
            .configure_github_build_trigger(&f.principal, &f.pid, source, 5, true)
            .await
            .unwrap();
        assert!(
            callback
                .authorize(protocol::wire::LeaseProof {
                    lease_token: connection_claims.sign(&issuer).unwrap()
                })
                .await
                .is_err(),
            "renewed consent cannot revive an older repository installation generation"
        );
        service
            .ingest_github_webhook(&workflow(workflow_body('b'), 223), &gate)
            .await
            .unwrap();
        let environment_claims = handler.claims.lock().unwrap().clone().unwrap();
        *handler.head.lock().unwrap() = "c".repeat(40);
        let snapshot=f.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT jsonb_build_object('id',s.id,'revision',s.revision,'connection_generation',c.generation,'environment_revision',s.environment_revision) AS value FROM webdock_auth.git_source s JOIN webdock_auth.git_connection c ON c.id=s.connection_id WHERE s.id=$1",vec![source.into()])).await.unwrap().unwrap().try_get::<Value>("","value").unwrap();
        let key = format!(
            "github-workflow-{}",
            protocol::sha256(
                &serde_json::to_vec(&json!([
                    snapshot["id"],
                    snapshot["revision"],
                    snapshot["connection_generation"],
                    snapshot["environment_revision"],
                    "c".repeat(40)
                ]))
                .unwrap()
            )
        );
        service.execute(&f.principal,json!({"action":"git.builds.request","projectID":f.pid,"targetID":target,"idempotencyKey":key})).await.unwrap();
        assert_eq!(
            service
                .ingest_github_webhook(&workflow(workflow_body('c'), 225), &gate)
                .await
                .unwrap_err()
                .code,
            "github_webhook_build_conflict",
            "manual idempotency keys cannot be relabeled as workflow proof"
        );
        sql(
            &f.owner,
            "UPDATE webdock_auth.repository_lease SET state='active' WHERE id=$1::uuid",
            vec![environment_claims.id.clone().into()],
        )
        .await;
        sql(&f.owner,"UPDATE webdock_auth.git_source SET environment_revision=environment_revision+1 WHERE id=$1",vec![source.into()]).await;
        service
            .configure_github_build_trigger(&f.principal, &f.pid, source, 5, true)
            .await
            .unwrap();
        assert!(
            callback
                .authorize(protocol::wire::LeaseProof {
                    lease_token: environment_claims.sign(&issuer).unwrap()
                })
                .await
                .is_err(),
            "renewed consent cannot revive an older source environment"
        );
        service
            .configure_kubernetes_admission(&f.principal, false, 1, 1)
            .await
            .unwrap();
    };
    let outcome = tokio::spawn(task).await;
    sql(
        &fixture.owner,
        "UPDATE webdock_auth.plugin_installation SET state='enabled' WHERE id='hosting'",
        vec![],
    )
    .await;
    sql(
        &fixture.owner,
        "DELETE FROM webdock_auth.git_webhook_delivery WHERE repository_plugin_id=$1",
        vec![cleanup_plugin.clone().into()],
    )
    .await;
    sql(
        &fixture.owner,
        "UPDATE webdock_auth.git_build_admission SET enabled=false WHERE singleton",
        vec![],
    )
    .await;
    sql(
        &fixture.owner,
        "DELETE FROM webdock_auth.git_build_trusted_project WHERE project_id=$1",
        vec![fixture.pid.clone().into()],
    )
    .await;
    for table in [
        "repository_lease",
        "git_check_outbox",
        "git_build",
        "git_source",
        "git_repository",
        "git_connection",
    ] {
        let predicate = if table == "git_check_outbox" {
            "build_id IN (SELECT id FROM webdock_auth.git_build WHERE customer_id=$1)"
        } else {
            "customer_id=$1"
        };
        sql(
            &fixture.owner,
            &format!("DELETE FROM webdock_auth.{table} WHERE {predicate}"),
            vec![fixture.cid.clone().into()],
        )
        .await;
    }
    for table in [
        "plugin_grant",
        "plugin_runtime",
        "plugin_configuration",
        "plugin_release",
    ] {
        sql(
            &fixture.owner,
            &format!("DELETE FROM webdock_auth.{table} WHERE plugin_id=$1"),
            vec![cleanup_plugin.clone().into()],
        )
        .await;
    }
    sql(
        &fixture.owner,
        "DELETE FROM webdock_auth.plugin_installation WHERE id=$1",
        vec![cleanup_plugin.into()],
    )
    .await;
    sql(
        &fixture.owner,
        "DELETE FROM webdock_auth.member WHERE \"userId\"=$1",
        vec![cleanup_actor.clone().into()],
    )
    .await;
    sql(
        &fixture.owner,
        "DELETE FROM webdock_auth.hosting_audit WHERE subject=$1",
        vec![cleanup_actor.clone().into()],
    )
    .await;
    sql(
        &fixture.owner,
        "DELETE FROM webdock_auth.\"user\" WHERE id=$1",
        vec![cleanup_actor.into()],
    )
    .await;
    fixture.cleanup().await;
    outcome.unwrap();
}
