//! Database-backed repository leases with a real private RPC runtime and callback authority.
use super::browser;
use super::*;
use std::os::unix::fs::PermissionsExt;
use webdock_repository_protocol::{self as protocol, LeaseClaims, runtime};
struct FixtureHandler {
    pause: std::sync::atomic::AtomicBool,
    entered: tokio::sync::Notify,
    resolved: std::sync::Mutex<Option<LeaseClaims>>,
}
#[async_trait::async_trait]
impl runtime::Handler for FixtureHandler {
    async fn resolve(
        &self,
        claims: &LeaseClaims,
        input: Value,
        _: Value,
    ) -> protocol::Result<Value> {
        assert_eq!(
            input.as_object().unwrap().len(),
            3,
            "private browser authority is never provider input"
        );
        *self.resolved.lock().unwrap() = Some(claims.clone());
        Ok(json!({"repositoryID":input["repositoryID"],"sha":"a".repeat(40)}))
    }
    async fn archive(&self, _: &LeaseClaims, _: Value, _: Value) -> protocol::Result<Vec<u8>> {
        if self.pause.load(std::sync::atomic::Ordering::SeqCst) {
            self.entered.notify_one();
            std::future::pending::<()>().await;
        }
        Ok(
            webdock_build::artifact::Archive::from_files(std::collections::BTreeMap::from([(
                "Dockerfile".into(),
                b"FROM scratch\n".to_vec(),
            )]))
            .unwrap()
            .encode()
            .unwrap(),
        )
    }
}
struct Tasks(Vec<tokio::task::JoinHandle<()>>);
impl Drop for Tasks {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}
#[tokio::test]
#[ignore = "owned restricted fixture and native0013 repository authority"]
async fn installed_repository_runtime_rechecks_domain_grants_and_configuration() {
    let fixture = Arc::new(Fixture::new("kubernetes").await);
    let f = fixture.clone();
    let browser_fixture = Arc::new(browser::BrowserFixture::new(&fixture).await);
    let browser = browser_fixture.clone();
    let plugin = format!("repository-{}", f.nonce);
    let connection = id(&f.owner).await;
    let worker_id = id(&f.owner).await;
    let cleanup_plugin = plugin.clone();
    let cleanup_worker = worker_id.clone();
    let outcome=tokio::spawn(async move {
  f.configure("k3s").await;
  let directory=tempfile::tempdir().unwrap();std::fs::set_permissions(directory.path(),std::fs::Permissions::from_mode(0o700)).unwrap();
  let mut manifest:Value=serde_json::from_str(include_str!("../../../plugin-protocol/tests/fixtures/project-notes.json")).unwrap();manifest["name"]=json!(plugin);manifest["jobs"]=json!([]);manifest["services"]["required"]=json!([]);manifest["capabilities"]=json!(["plugin.http.read"]);manifest["actions"]=json!([{"name":"repository.resolve","intent":"read","audience":"operator","capabilities":["plugin.http.read"]},{"name":"repository.archive","intent":"read","audience":"operator","capabilities":["plugin.http.read"]}]);
  let raw=manifest.to_string();let digest=protocol::sha256(raw.as_bytes());let credential=protocol::sha256(b"repository-fixture-credential");
  sql(&f.owner,"INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version,runtime,current_generation) VALUES($1,'external','enabled','enabled','1.0.0','ready',1)",vec![plugin.clone().into()]).await;
  sql(&f.owner,"INSERT INTO webdock_auth.plugin_release(plugin_id,version,manifest_bytes,manifest_sha256,image,registry_url,signer_key_id) VALUES($1,'1.0.0',$2,$3,$4,'https://registry.invalid','fixture')",vec![plugin.clone().into(),raw.into(),digest.clone().into(),manifest["image"].as_str().unwrap().into()]).await;
  sql(&f.owner,"INSERT INTO webdock_auth.plugin_configuration(plugin_id,approved_capabilities,configuration) VALUES($1,$2,'{}')",vec![plugin.clone().into(),manifest["capabilities"].clone().into()]).await;
  sql(&f.owner,"INSERT INTO webdock_auth.plugin_runtime(plugin_id,generation,version,image,status,configuration_revision,credential_hash,namespace,service_name) VALUES($1,1,'1.0.0',$2,'ready',0,$3,'fixtures','fixture-plugin')",vec![plugin.clone().into(),manifest["image"].as_str().unwrap().into(),credential.clone().into()]).await;
  sql(&f.owner,"INSERT INTO webdock_auth.plugin_grant(plugin_id,customer_id,project_id,capabilities) VALUES($1,$2,$3,$4)",vec![plugin.clone().into(),f.cid.clone().into(),f.pid.clone().into(),manifest["capabilities"].clone().into()]).await;
  let uid=rustix::process::geteuid().as_raw();let host_path=directory.path().join("host.json");let runtime_path=directory.path().join("repository.sock");let authority_path=directory.path().join("authority.sock");
  std::fs::write(&host_path,json!({"runtimes":{plugin.clone():{"generation":1,"namespace":"fixtures","service":"fixture-plugin","credentialSha256":credential,"peers":[{"kind":"unix","uid":uid}],"projects":{}}}}).to_string()).unwrap();std::fs::set_permissions(&host_path,std::fs::Permissions::from_mode(0o600)).unwrap();
  let ids=Arc::new(webdock_persistence::IdAllocator::new(db("DATABASE_URL").await,webdock_ids::Identity::platform(0,2).unwrap()));let secret=std::env::var("DATABASE_RLS_SECRET").unwrap();let issuer=ed25519_dalek::SigningKey::from_bytes(&[39;32]);let publisher=ed25519_dalek::SigningKey::from_bytes(&[41;32]);
  let descriptor=protocol::Descriptor{v:1,plugin_id:plugin.clone(),version:"1.0.0".into(),manifest_sha256:digest,image:manifest["image"].as_str().unwrap().into(),protocol:"webdock.repository.v1".into(),signer_key_id:"fixture".into()}.sign(&publisher).unwrap();
  let binding_path=directory.path().join("repository.json");std::fs::write(&binding_path,json!({"runtimes":{plugin.clone():{"descriptor":descriptor,"publisherKeyId":"fixture","publisherPublicKeyHex":hex::encode(publisher.verifying_key().as_bytes()),"registryUrl":"https://registry.invalid","transport":{"kind":"unix","path":runtime_path}}}}).to_string()).unwrap();std::fs::set_permissions(&binding_path,std::fs::Permissions::from_mode(0o600)).unwrap();
  let host=Arc::new(webdock_plugin_host::HostService::new(db("DATABASE_URL").await,db("WEBDOCK_PLUGIN_WORKER_DATABASE_URL").await,secret.clone(),ids.clone(),Arc::new(webdock_plugin_host::FileBindings::new(host_path)),ed25519_dalek::SigningKey::from_bytes(&[37;32])).await.unwrap());
  let gate=Arc::new(webdock_build::RepositoryAdmission::new(db("WEBDOCK_PLUGIN_WORKER_DATABASE_URL").await,secret.clone(),host,issuer.clone(),Arc::new(webdock_build::FileRepositoryBindings::new(binding_path))).await.unwrap().with_session_authority(db("AUTH_DATABASE_URL").await,browser.identity.clone(),Some(browser.browser.clone())).await.unwrap());
  let mut socket=runtime::UnixSocket::bind(&authority_path).unwrap();let incoming=socket.incoming().unwrap();let callback=gate.clone().authority_service();let mut tasks=Tasks(vec![tokio::spawn(async move {let _socket=socket;tonic::transport::Server::builder().add_service(callback).serve_with_incoming(incoming).await.unwrap();})]);
  let authority=runtime::RemoteAuthority::new(runtime::connect_unix(&authority_path).await.unwrap(),plugin.clone(),1,protocol::sha256(descriptor.as_bytes()),vec![runtime::CorePeer::Unix{uid}]).unwrap();
  let handler=Arc::new(FixtureHandler{pause:std::sync::atomic::AtomicBool::new(false),entered:tokio::sync::Notify::new(),resolved:std::sync::Mutex::new(None)});let server=runtime::Runtime::new(descriptor,&publisher.verifying_key(),1,issuer.verifying_key(),Arc::new(authority),handler.clone()).unwrap();let socket=runtime::UnixSocket::bind(&runtime_path).unwrap();tasks.0.push(tokio::spawn(async move{server.serve_unix(socket,std::future::pending()).await.unwrap()}));
  let service=Arc::new(webdock_build::BuildService::new(db("DATABASE_URL").await,db("WEBDOCK_PLUGIN_WORKER_DATABASE_URL").await,secret,ids,f.service.clone(),"registry.invalid".into()).await.unwrap().with_session_authority(db("AUTH_DATABASE_URL").await,browser.identity.clone(),Some(browser.browser.clone())).await.unwrap().with_repository_provider(Arc::new(webdock_build::PluginRepositoryProvider::new(gate))));
  let mut draft=spec();draft.as_object_mut().unwrap().remove("image");draft.as_object_mut().unwrap().remove("template");let app=f.service.execute(&f.principal,json!({"action":"apps.prepare","projectID":f.pid,"name":"repository-target","spec":draft,"subscriptionRevision":1,"idempotencyKey":"repository-target-1"})).await.unwrap();let app=app["appID"].as_str().or_else(||app["id"].as_str()).unwrap();
  sql(&f.owner,"INSERT INTO webdock_auth.git_connection(id,customer_id,installation_id,account_id,account_login,permissions,state,repository_plugin_id) VALUES($1,$2,'1234','1234','fixture','{}','active',$3)",vec![connection.clone().into(),f.cid.clone().into(),plugin.clone().into()]).await;sql(&f.owner,"INSERT INTO webdock_auth.git_repository(connection_id,customer_id,repository_id,metadata) VALUES($1,$2,'123456','{}')",vec![connection.clone().into(),f.cid.clone().into()]).await;
  service.execute(&f.principal,json!({"action":"git.source.configure","projectID":f.pid,"targetID":app,"connectionID":connection,"repositoryID":"123456","branch":"main","rootDirectory":".","recipe":"dockerfile","revision":0,"autoPublish":true})).await.unwrap();
  let request=json!({"action":"git.builds.request","projectID":f.pid,"targetID":app,"idempotencyKey":"repository-build-1"});
  sql(&f.owner,"UPDATE webdock_auth.plugin_grant SET revoked_at=now() WHERE plugin_id=$1",vec![plugin.clone().into()]).await;assert!(service.execute(&f.principal,request.clone()).await.is_err());sql(&f.owner,"UPDATE webdock_auth.plugin_grant SET revoked_at=NULL WHERE plugin_id=$1",vec![plugin.clone().into()]).await;
  let build=service.execute_with_authority(&f.principal,&browser.authority,request,webdock_build::BuildCeiling{read:true,write:true}).await.unwrap();
  let credential="R".repeat(43);sql(&f.owner,"INSERT INTO webdock_auth.git_worker(id,credential_hash,enabled,country,isolation,evidence,capacity) VALUES($1,$2,true,'ZZ','microvm','repository fixture',1)",vec![worker_id.into(),protocol::sha256(credential.as_bytes()).into()]).await;let worker=service.authenticate_worker(&credential).await.unwrap();let claim=service.claim_build(&worker).await.unwrap().unwrap();let build=build["id"].as_str().unwrap().to_owned();let generation=claim["generation"].as_i64().unwrap() as i32;
  sql(&f.owner,"UPDATE webdock_auth.session SET \"expiresAt\"=now()-interval '1 second' WHERE id=$1",vec![f.sid.clone().into()]).await;
  assert!(service.source_archive(&worker,&build,generation).await.unwrap().len()>512,"persisted build authority survives browser expiry");
  handler.pause.store(true,std::sync::atomic::Ordering::SeqCst);let reader=service.clone();let build_copy=build.clone();let pending=tokio::spawn(async move{reader.source_archive(&worker,&build_copy,generation).await});tokio::time::timeout(std::time::Duration::from_secs(5),handler.entered.notified()).await.unwrap();
  sql(&f.owner,"UPDATE webdock_auth.plugin_configuration SET revision=revision+1 WHERE plugin_id=$1",vec![plugin.clone().into()]).await;sql(&f.owner,"UPDATE webdock_auth.plugin_runtime SET configuration_revision=configuration_revision+1 WHERE plugin_id=$1",vec![plugin.clone().into()]).await;
  assert!(tokio::time::timeout(std::time::Duration::from_secs(5),pending).await.unwrap().unwrap().is_err(),"configuration change revokes in-flight source stream");handler.pause.store(false,std::sync::atomic::Ordering::SeqCst);
  let worker=service.authenticate_worker(&credential).await.unwrap();assert!(service.source_archive(&worker,&build,generation).await.is_ok());
  let unsigned=db("WEBDOCK_PLUGIN_WORKER_DATABASE_URL").await;let visible:i64=unsigned.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT count(*) AS n FROM webdock_auth.repository_lease")).await.unwrap().unwrap().try_get("","n").unwrap();assert_eq!(visible,0,"unsigned worker connection cannot read repository leases");
  sql(&f.owner,"UPDATE webdock_auth.plugin_runtime SET credential_hash=repeat('f',64) WHERE plugin_id=$1",vec![plugin.clone().into()]).await;assert!(service.source_archive(&worker,&build,generation).await.is_err());sql(&f.owner,"UPDATE webdock_auth.plugin_runtime SET credential_hash=$2 WHERE plugin_id=$1",vec![plugin.clone().into(),protocol::sha256(b"repository-fixture-credential").into()]).await;

  sql(&f.owner,"UPDATE webdock_auth.\"user\" SET banned=true WHERE id=$1",vec![f.uid.clone().into()]).await;assert!(service.source_archive(&worker,&build,generation).await.is_err());sql(&f.owner,"UPDATE webdock_auth.\"user\" SET banned=false WHERE id=$1",vec![f.uid.clone().into()]).await;
  sql(&f.owner,"UPDATE webdock_auth.session SET \"expiresAt\"=now()+interval '1 hour' WHERE id=$1",vec![f.sid.clone().into()]).await;
  service.execute_with_authority(&f.principal,&browser.authority,json!({"action":"git.builds.request","projectID":f.pid,"targetID":app,"idempotencyKey":"repository-browser-fence-1"}),webdock_build::BuildCeiling{read:true,write:true}).await.unwrap();
  let admitted=handler.resolved.lock().unwrap().clone().unwrap();
  let row=f.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT actor_authority FROM webdock_auth.repository_lease WHERE id=$1::uuid",[admitted.id.clone().into()])).await.unwrap().unwrap().try_get::<Value>("","actor_authority").unwrap();assert_eq!(row["kind"],"first-party-browser");
  // Re-open only this fixture's receipt to exercise a lost-result callback after the browser transaction has committed.
  sql(&f.owner,"UPDATE webdock_auth.repository_lease SET state='active' WHERE id=$1::uuid",vec![admitted.id.clone().into()]).await;
  let token=admitted.sign(&issuer).unwrap();let mut callback=protocol::wire::repository_authority_client::RepositoryAuthorityClient::new(runtime::connect_unix(&authority_path).await.unwrap());
  callback.authorize(protocol::wire::LeaseProof{lease_token:token.clone()}).await.unwrap();browser.revoke(&f.owner).await;
  assert!(callback.authorize(protocol::wire::LeaseProof{lease_token:token}).await.is_err(),"revoked first-party client invalidates persisted repository authority without expiring the local session");
  sql(&f.owner,"UPDATE webdock_auth.git_connection SET repository_plugin_id='other-provider' WHERE id=$1",vec![connection.into()]).await;assert!(service.source_archive(&worker,&build,generation).await.is_err());
 }).await;
    for table in [
        "repository_lease",
        "git_check_outbox",
        "git_release",
        "git_artifact",
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
    sql(
        &fixture.owner,
        "DELETE FROM webdock_auth.git_worker WHERE id=$1",
        vec![cleanup_worker.into()],
    )
    .await;
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
    browser_fixture.cleanup(&fixture.owner).await;
    fixture.cleanup().await;
    outcome.unwrap();
}
