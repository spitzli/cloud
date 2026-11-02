//! Reuses the hosting fixture: no duplicate cluster setup, no production resources.
use super::browser;
use super::*;
mod registry;
use webdock_build::{BuildService, RepositoryProvider, RepositoryRef, VerifiedCommit};
struct LocalRepository {
    path: tempfile::TempDir,
}
#[async_trait::async_trait]
impl RepositoryProvider for LocalRepository {
    async fn resolve(
        &self,
        _: &RepositoryRef<'_>,
        reference: &str,
    ) -> webdock_build::Result<VerifiedCommit> {
        let output = std::process::Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .arg("-C")
            .arg(self.path.path())
            .args(["rev-parse", "--verify", &format!("{reference}^{{commit}}")])
            .output()
            .unwrap();
        assert!(output.status.success());
        VerifiedCommit::new(String::from_utf8(output.stdout).unwrap().trim().into())
    }
    async fn archive(&self, _: &RepositoryRef<'_>, commit: &str) -> webdock_build::Result<Vec<u8>> {
        let output = std::process::Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .arg("-C")
            .arg(self.path.path())
            .args(["archive", "--format=tar", commit])
            .output()
            .unwrap();
        assert!(output.status.success());
        Ok(output.stdout)
    }
}
#[tokio::test]
#[ignore = "owned restricted RLS fixture plus native0012 and real VM artifact"]
async fn git_build_publication_preserves_durable_authority_and_atomic_hosting() {
    publication_flow(false).await;
}
#[tokio::test]
#[ignore = "owned fresh database, MinIO, registry and isolated VM fixture"]
async fn git_remote_artifact_cross_worker_publication_and_rollback() {
    publication_flow(true).await;
}
#[tokio::test]
#[ignore = "owned frontend VM, MinIO, fresh native database including coordinated0053"]
async fn git_frontend_remote_outputs_cross_worker_publication_and_rollback() {
    publication_recipe(true, true).await;
}
async fn publication_flow(remote_enabled: bool) {
    publication_recipe(remote_enabled, false).await;
}
async fn publication_recipe(remote_enabled: bool, frontend_enabled: bool) {
    let fixture = Arc::new(Fixture::new("kubernetes").await);
    let browser_fixture = Arc::new(browser::BrowserFixture::new(&fixture).await);
    let browser = browser_fixture.clone();
    let f = fixture.clone();
    let worker_id = id(&f.owner).await;
    let connection = id(&f.owner).await;
    let cleanup_worker = worker_id.clone();
    let publication_worker_id = id(&f.owner).await;
    let cleanup_publication_worker = publication_worker_id.clone();
    let other_project = id(&f.owner).await;
    let cleanup_other_project = other_project.clone();
    let cleanup_connection = connection.clone();
    let outcome=tokio::spawn(async move {
 f.configure("k3s").await;
 let registry=registry::LocalRegistry::start().await;let publisher=webdock_build::publisher::RegistryPublisher::loopback_test(registry.address).unwrap();
 sql(&f.owner,"UPDATE webdock_auth.hosting_agent SET observation=jsonb_set(observation,'{capabilities,registryVersion}','1') WHERE cluster_id=$1",vec![f.cluster.clone().into()]).await;
 let mut draft_spec=spec();draft_spec.as_object_mut().unwrap().remove("image");draft_spec.as_object_mut().unwrap().remove("template");
 let draft=f.service.execute(&f.principal,json!({"action":"apps.prepare","projectID":f.pid,"name":"git-target","spec":draft_spec,"subscriptionRevision":1,"idempotencyKey":"git-target-draft-1"})).await.unwrap();
 let app=draft["appID"].as_str().or_else(||draft["id"].as_str()).unwrap();
 sql(&f.owner,"INSERT INTO webdock_auth.git_connection(id,customer_id,installation_id,account_id,account_login,permissions,state) VALUES($1,$2,$3,$3,'local-fixture','{}','active')",vec![connection.clone().into(),f.cid.clone().into(),f.nonce.clone().into()]).await;
 sql(&f.owner,"INSERT INTO webdock_auth.git_repository(connection_id,customer_id,repository_id,metadata) VALUES($1,$2,'123456','{\"fullName\":\"fixture/local\"}')",vec![connection.clone().into(),f.cid.clone().into()]).await;
 use base64::Engine;let credential=base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(f.nonce.as_bytes()));sql(&f.owner,"INSERT INTO webdock_auth.git_worker(id,credential_hash,enabled,country,isolation,evidence,capacity) VALUES($1,$2,true,'ZZ','microvm','unverified local disposable test worker',1)",vec![worker_id.clone().into(),hex::encode(Sha256::digest(credential.as_bytes())).into()]).await;
 let repository=LocalRepository{path:tempfile::tempdir().unwrap()};
 let recipe=if frontend_enabled {webdock_build::frontend::RECIPE} else {"dockerfile"};
 let vm_root=if frontend_enabled {std::path::PathBuf::from(std::env::var_os("WEBDOCK_FRONTEND_VM_ROOT").expect("new owned frontend VM"))} else {std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.runtime-test/build-vm")}.canonicalize().unwrap();
 std::fs::write(repository.path.path().join("Dockerfile"),"FROM scratch\nCOPY --chmod=755 busybox /bin/busybox\nCOPY --chmod=755 musl /lib/ld-musl-x86_64.so.1\nSHELL [\"/bin/busybox\",\"sh\",\"-c\"]\nARG BUILD_TOKEN\nRUN echo source-commit-built-in-isolated-vm; echo $BUILD_TOKEN\nENTRYPOINT [\"/bin/busybox\",\"true\"]\n").unwrap();
 if frontend_enabled {
  std::fs::create_dir(repository.path.path().join(".webdock")).unwrap();
  std::fs::write(repository.path.path().join(".webdock/frontend.json"),r#"{"version":1,"directory":"app/dist"}"#).unwrap();
  use std::io::Write;
  let mut dockerfile=std::fs::OpenOptions::new().append(true).open(repository.path.path().join("Dockerfile")).unwrap();
  writeln!(dockerfile,"RUN /bin/busybox mkdir -p /app/dist/assets; printf '<p>owned frontend</p>' > /app/dist/index.html; printf 'console.log(42)' > /app/dist/assets/main.js").unwrap();
 }
 std::fs::copy(vm_root.join("rootfs/bin/busybox"),repository.path.path().join("busybox")).unwrap();std::fs::copy(vm_root.join("rootfs/lib/ld-musl-x86_64.so.1"),repository.path.path().join("musl")).unwrap();for args in [vec!["init","-b","main"],vec!["add","--all"],vec!["-c","user.name=Fixture","-c","user.email=fixture@example.invalid","commit","-m","fixture"]]{assert!(std::process::Command::new("git").env("GIT_CONFIG_GLOBAL","/dev/null").env("GIT_CONFIG_NOSYSTEM","1").arg("-C").arg(repository.path.path()).args(args).output().unwrap().status.success());}
 let mut auth_options=sea_orm::ConnectOptions::new(std::env::var("AUTH_DATABASE_URL").unwrap());auth_options.max_connections(1).acquire_timeout(std::time::Duration::from_secs(1));let auth=Database::connect(auth_options).await.unwrap();
 let hosting=Arc::new(HostingService::new(db("DATABASE_URL").await,auth.clone(),db("WEBDOCK_PLUGIN_WORKER_DATABASE_URL").await,std::env::var("DATABASE_RLS_SECRET").unwrap(),std::env::var("BETTER_AUTH_SECRET").unwrap(),Arc::new(webdock_persistence::IdAllocator::new(db("DATABASE_URL").await,webdock_ids::Identity::platform(0,2).unwrap()))).await.unwrap().with_session_authority(browser.identity.clone(),Some(browser.browser.clone())));
 let evidence_repository_path=repository.path.path().to_path_buf();
 let service=Arc::new(BuildService::new(db("DATABASE_URL").await,db("WEBDOCK_PLUGIN_WORKER_DATABASE_URL").await,std::env::var("DATABASE_RLS_SECRET").unwrap(),Arc::new(webdock_persistence::IdAllocator::new(db("DATABASE_URL").await,webdock_ids::Identity::platform(0,2).unwrap())),hosting.clone(),registry.address.to_string()).await.unwrap().with_session_authority(auth,browser.identity.clone(),Some(browser.browser.clone())).await.unwrap().with_repository_provider(Arc::new(repository)));

 let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let lose_ack=Arc::new(std::sync::atomic::AtomicBool::new(true));
 let router=webdock_build::http::worker_router(service.clone()).layer(axum::middleware::from_fn(move|request:axum::extract::Request,next:axum::middleware::Next|{let lost=lose_ack.clone();async move{let complete=request.uri().path()=="/complete";let response=next.run(request).await;if complete&&response.status().is_success()&&lost.swap(false,std::sync::atomic::Ordering::SeqCst){use axum::response::IntoResponse;axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response()}else{response}}}));
 struct ServerGuard(tokio::task::JoinHandle<()>);impl Drop for ServerGuard{fn drop(&mut self){self.0.abort();}}
 let _server=ServerGuard(tokio::spawn(async move{axum::serve(listener,router).await.unwrap()}));let control=webdock_build::worker::ControlClient::loopback_test(&format!("http://{address}"),credential.clone()).unwrap();
 let _source=service.execute(&f.principal,json!({"action":"git.source.configure","projectID":f.pid,"targetID":app,"connectionID":connection,"repositoryID":"123456","branch":"main","rootDirectory":".","recipe":recipe,"revision":0,"autoPublish":true,"buildEnvironment":[{"name":"BUILD_TOKEN","value":"sensitive-fixture-123"}]})).await.unwrap();
 let requested=service.execute(&f.principal,json!({"action":"git.builds.request","projectID":f.pid,"targetID":app,"idempotencyKey":"git-build-request-1"})).await.unwrap();let build=requested["id"].as_str().unwrap();let worker=service.authenticate_worker(&credential).await.unwrap();
 if frontend_enabled {
  assert!(control.post("claim",&json!({})).await.unwrap().is_null(),"old worker excluded before assigning a frontend lease");
  assert!(control.post("claim",&json!({"recipes":["vercel"]})).await.is_err());
 }
 let claim=control.post("claim",&if frontend_enabled {json!({"recipes":["dockerfile","dockerfile-frontend"]})}else{json!({})}).await.unwrap();assert_eq!(claim["buildID"],build);let generation=claim["generation"].as_i64().unwrap() as i32;assert!(service.claim_build(&worker).await.unwrap().is_none());assert!(service.guard_build(&worker,build,generation+1).await.is_err());let archive=control.archive(build,generation).await.unwrap();assert!(webdock_build::artifact::Archive::read(&archive,128*1024*1024).unwrap().files().contains_key("Dockerfile"));
 let original_source_commit=claim["sourceSHA"].as_str().unwrap().to_owned();
 let store_dir=tempfile::tempdir().unwrap();use std::os::unix::fs::PermissionsExt;std::fs::set_permissions(store_dir.path(),std::fs::Permissions::from_mode(0o700)).unwrap();let store=webdock_build::artifact::ArtifactStore::open(store_dir.path().into(),1024*1024*1024).unwrap();

 assert_eq!(std::env::var("WEBDOCK_BUILD_VM_TEST").as_deref(),Ok("1"));
 let hashes=std::fs::read_to_string(vm_root.join("SHA256SUMS")).unwrap();let expected=|name:&str|hashes.lines().find(|l|l.ends_with(&format!("/{name}"))).unwrap().split_whitespace().next().unwrap().to_owned();
 let vm_image=webdock_build::execution::VmImage::new(vm_root.join("base.raw"),expected("base.raw")).unwrap().with_boot(vm_root.join("kernel"),expected("kernel"),vm_root.join("initrd"),expected("initrd")).unwrap();
 let vm_image=if frontend_enabled {vm_image.with_frontend_output()}else{vm_image};
 let machine=vm_image.identity();let execute_image=vm_image.clone();let source_snapshot=archive.clone();let worker_scratch_root=vm_root.clone();
 let vm_control=control.clone();let vm_build=build.to_owned();let handle=tokio::runtime::Handle::current();let job=serde_json::from_value(claim["request"].clone()).unwrap();
 let built=tokio::task::spawn_blocking(move||{let scratch=tempfile::tempdir_in(&vm_root).unwrap();execute_image.execute(&job,&archive,scratch.path(),||handle.block_on(vm_control.guard_build(&vm_build,generation)))}).await.unwrap().unwrap();
 assert!(built.logs.contains("source-commit-built-in-isolated-vm"));assert!(!built.logs.contains("sensitive-fixture-123"));assert!(built.logs.contains("[REDACTED]"));let logs=built.logs;
 let evidence_key=ed25519_dalek::SigningKey::from_bytes(&[9;32]);
 let artifact_bytes=if remote_enabled {webdock_build::evidence::assemble(&built.artifact,&source_snapshot,&webdock_build::evidence::BuildEvidence {customer_id:f.cid.clone(),project_id:f.pid.clone(),build_id:build.into(),generation,source_commit:claim["sourceSHA"].as_str().unwrap().into(),recipe:recipe.into(),root_directory:".".into(),machine},Some(&evidence_key)).unwrap()}else{built.artifact};
 let stored=store.put(&f.cid,build,generation,&artifact_bytes).unwrap();
 let remote = if remote_enabled {
  #[derive(serde::Deserialize)] struct S3Fixture {config: webdock_object_storage::S3Config, credentials: webdock_object_storage::Credentials}
  let config: S3Fixture=serde_json::from_slice(&std::fs::read(std::env::var_os("WEBDOCK_S3_TEST_CONFIG").expect("private owned MinIO fixture")).unwrap()).unwrap();
  Some(webdock_build::remote::RemoteArtifacts::new(webdock_object_storage::Store::loopback_test(config.config,config.credentials).unwrap()))
 } else {None};
 let stored=if let Some(remote)=&remote {remote.persist(&store,&stored).await.unwrap()} else {stored};
 let bindings: Vec<String>=remote.as_ref().map(|r|vec![r.binding().to_owned()]).unwrap_or_default();
 let targets=std::collections::BTreeMap::from([(f.pid.clone(),format!("{}/customers/{}/projects/{}",registry.address,f.cid,f.pid))]);
 let (_store_dir,store)=if remote.is_some() {
  drop(store);drop(store_dir);
  let directory=tempfile::tempdir().unwrap();std::fs::set_permissions(directory.path(),std::fs::Permissions::from_mode(0o700)).unwrap();
  let replacement=webdock_build::artifact::ArtifactStore::open(directory.path().into(),1024*1024*1024).unwrap();
  assert!(replacement.read(&stored).is_err());
  (directory,replacement)
 } else {(store_dir,store)};
 let publication_credential=if remote_enabled {base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(format!("{}-publisher",f.nonce).as_bytes()))} else {credential.clone()};
 let publisher_worker=if remote_enabled {
  sql(&f.owner,"INSERT INTO webdock_auth.git_worker(id,credential_hash,enabled,country,isolation,evidence,capacity) VALUES($1,$2,true,'ZZ','artifact-only','unverified owned separate publisher fixture',1)",vec![publication_worker_id.clone().into(),hex::encode(Sha256::digest(publication_credential.as_bytes())).into()]).await;
  service.authenticate_worker(&publication_credential).await.unwrap()
 } else {service.authenticate_worker(&credential).await.unwrap()};



 let outbox_directory=tempfile::tempdir().unwrap();std::fs::set_permissions(outbox_directory.path(),std::fs::Permissions::from_mode(0o700)).unwrap();let outbox=webdock_build::worker::Outbox::open(outbox_directory.path().into()).unwrap();let mut completion=json!({"buildID":build,"generation":generation,"status":"succeeded","logs":logs,"artifact":{"kind":"oci","digest":stored.digest(),"storageKey":stored.storage_key(),"sizeBytes":stored.size(),"imageDigest":stored.image_digest(),"remoteStorage":stored.remote_storage()}});if !remote_enabled {completion["artifact"].as_object_mut().unwrap().remove("remoteStorage");}outbox.put("complete",completion).unwrap();assert!(outbox.flush(&control).await.is_err(),"lost HTTP acknowledgment after database commit");assert!(outbox.has_pending());drop(outbox);let recovered=webdock_build::worker::Outbox::open(outbox_directory.path().into()).unwrap();let completed=recovered.flush(&control).await.unwrap().unwrap();assert!(!recovered.has_pending());assert_eq!(completed["status"],"succeeded");assert_eq!(completed["release"]["status"],"queued");assert_eq!(service.complete_build(&worker,build,generation,Some(&stored),&logs,None).await.unwrap()["release"]["id"],completed["release"]["id"]);assert!(service.complete_build(&worker,build,generation,Some(&stored),"changed receipt",None).await.is_err());
 if !remote_enabled {
  let expected=hex::encode(Sha256::digest(serde_json::to_vec(&json!({"artifact":[stored.storage_key(),stored.digest(),stored.image_digest(),stored.size()],"logs":logs,"failure":null})).unwrap()));
  let prior=f.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT completion_hash FROM webdock_auth.git_build WHERE id=$1",[build.into()])).await.unwrap().unwrap().try_get::<String>("","completion_hash").unwrap();
  assert_eq!(prior,expected,"previous-version completion hash remains replayable");
 }
 if let Some(remote)=stored.remote_storage() {
  let persisted=f.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT remote_storage FROM webdock_auth.git_artifact WHERE build_id=$1",[build.into()])).await.unwrap().unwrap().try_get::<Value>("","remote_storage").unwrap();
  assert_eq!(persisted,serde_json::to_value(remote).unwrap());
  assert!(f.owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.git_artifact SET remote_storage=jsonb_set(remote_storage,'{binding}',to_jsonb(repeat('0',64))) WHERE build_id=$1",[build.into()])).await.is_err(),"retained backend identity is immutable");
  let mut changed=json!({"buildID":build,"generation":generation,"status":"succeeded","logs":logs,"artifact":{"kind":"oci","digest":stored.digest(),"storageKey":stored.storage_key(),"sizeBytes":stored.size(),"imageDigest":stored.image_digest(),"remoteStorage":remote}});
  changed["artifact"]["remoteStorage"]["binding"]=json!("0".repeat(64));
  assert!(control.post("complete",&changed).await.is_err(),"changed remote receipt cannot replay a completion");
 }
 // The durable source policy survives session expiry, but not account revocation.
 sql(&f.owner,"UPDATE webdock_auth.session SET \"expiresAt\"=now()-interval '1 second' WHERE id=$1",vec![f.sid.clone().into()]).await;
 if remote_enabled {
  sql(&f.owner,"INSERT INTO webdock_admin.projects(id,name,customer_id) VALUES($1,'other-publication-project',$2)",vec![other_project.clone().into(),f.cid.clone().into()]).await;
  let local=tempfile::tempdir().unwrap();let scratch=tempfile::tempdir().unwrap();let state=tempfile::tempdir().unwrap();
  for directory in [&local,&scratch,&state] {std::fs::set_permissions(directory.path(),std::fs::Permissions::from_mode(0o700)).unwrap();}
  let transport_image=tempfile::NamedTempFile::new().unwrap();std::fs::write(transport_image.path(),b"not executed in this publication routing test").unwrap();
  let hash=hex::encode(Sha256::digest(std::fs::read(transport_image.path()).unwrap()));
  let wrong=webdock_build::worker::Worker::new(
    webdock_build::worker::ControlClient::loopback_test(&format!("http://{address}"),publication_credential.clone()).unwrap(),
    webdock_build::execution::VmImage::new(transport_image.path().into(),hash).unwrap(),
    webdock_build::artifact::ArtifactStore::open(local.path().into(),1024*1024*1024).unwrap(),scratch.path().into(),webdock_build::worker::Outbox::open(state.path().into()).unwrap()
  ).unwrap().with_artifact_store(remote.as_ref().unwrap().clone(),false).unwrap()
   .with_publisher(other_project.clone(),format!("{}/customers/{}/projects/{other_project}",registry.address,f.cid),webdock_build::publisher::RegistryPublisher::loopback_test(registry.address).unwrap()).unwrap();
  assert!(matches!(wrong.publish_once().await,Ok(false)),"worker for the other project must not claim or strand this release");
  let mismatched_target=std::collections::BTreeMap::from([(f.pid.clone(),format!("{}/customers/{}/projects/{}",registry.address,f.cid,other_project))]);
  assert!(service.claim_release_with_storage(&publisher_worker,&bindings,&mismatched_target).await.unwrap().is_none(),"right project with wrong registry target is ineligible before lease assignment");
  assert!(service.claim_release(&worker).await.unwrap().is_none(),"old workers must not claim remote artifacts");
  assert!(service.claim_release_with_storage(&publisher_worker,&["0".repeat(64)],&targets).await.unwrap().is_none());
 }
 if remote_enabled {
  let legacy=webdock_build::worker::ControlClient::loopback_test(&format!("http://{address}"),publication_credential.clone()).unwrap();
  assert!(legacy.post("release-claim",&json!({"artifactStoreBindings":bindings,"publicationProjects":targets})).await.unwrap().is_null(),"old publisher must not receive an unsupported evidence format lease");
  assert!(legacy.post("release-claim",&json!({"artifactStoreBindings":bindings,"publicationProjects":targets,"artifactFormats":[9]})).await.is_err(),"unsupported reader format is rejected before leasing");
  if frontend_enabled { assert!(legacy.post("release-claim",&json!({"artifactStoreBindings":bindings,"publicationProjects":targets,"artifactFormats":[0,1]})).await.unwrap().is_null(),"E1-only readers cannot strand format2 releases"); }
 }
 let release=if frontend_enabled {
  let current=webdock_build::worker::ControlClient::loopback_test(&format!("http://{address}"),publication_credential.clone()).unwrap();
  let wire=current.post("release-claim",&json!({"artifactStoreBindings":bindings,"publicationProjects":targets,"artifactFormats":[0,1,2]})).await.unwrap();
  assert!(!wire.is_null(),"compatible publisher claims through normal compiled Core HTTP");
  assert_eq!(wire["artifact"]["digest"],stored.digest());assert_eq!(wire["artifact"]["storageKey"],stored.storage_key());assert_eq!(wire["artifact"]["sizeBytes"],stored.size());assert_eq!(wire["artifact"]["imageDigest"],stored.image_digest());
  json!({"id":wire["releaseID"],"generation":wire["generation"]})
 }else{service.claim_release_with_storage(&publisher_worker,&bindings,&targets).await.unwrap().unwrap()};let rid=release["id"].as_str().unwrap();let rg=release["generation"].as_i64().unwrap() as i32;
 let prepared=service.prepare_publication(&publisher_worker,rid,rg).await.unwrap();let image=format!("{}@{}",prepared["repository"].as_str().unwrap(),prepared["imageDigest"].as_str().unwrap());
 if let Some(remote)=&remote {
  assert_eq!(prepared["remoteStorage"],serde_json::to_value(stored.remote_storage().unwrap()).unwrap());
  assert!(store.read(&stored).is_err(),"publisher starts without producer's local bytes");
  remote.restore(&store,&stored).await.unwrap();
  let bytes=store.read(&stored).unwrap();let envelope=webdock_build::artifact::Archive::read(&bytes,webdock_build::artifact::MAX_OUTPUT).unwrap();
  let evidence=webdock_build::evidence::validate(&envelope,&f.cid,build,generation,Some(&evidence_key.verifying_key())).unwrap().unwrap();
  assert_eq!(evidence.build.project_id,f.pid);assert_eq!(envelope.files()["source.tar"],source_snapshot);
  assert!(!std::str::from_utf8(&envelope.files()["provenance.json"]).unwrap().contains("sensitive-fixture-123"));
  if frontend_enabled {
   assert_eq!(evidence.version,2);assert_eq!(stored.format_version().unwrap(),2);
   let files=webdock_build::frontend::retained(&store,&stored,&evidence.build,Some(&evidence_key.verifying_key())).unwrap();
   assert_eq!(files.files()["index.html"],b"<p>owned frontend</p>");assert_eq!(files.files()["assets/main.js"],b"console.log(42)");
   let mut wrong=evidence.build.clone();wrong.project_id=other_project.clone();assert!(webdock_build::frontend::retained(&store,&stored,&wrong,None).is_err());
  }
 }
 assert!(service.publish_image(&publisher_worker,rid,rg,"untrusted.invalid/other@sha256:0000000000000000000000000000000000000000000000000000000000000000").await.is_err());
 let staged=service.publish_local_artifact(&publisher_worker,rid,rg,&store,&publisher).await.unwrap();assert_eq!(service.publish_image(&publisher_worker,rid,rg,&image).await.unwrap()["id"],staged["id"]);
 let claim=f.service.agent_request(&f.agent,"claim",json!({})).await.unwrap();assert_eq!(claim["id"],staged["id"]);f.service.agent_request(&f.agent,"guard",json!({"id":claim["id"],"generation":claim["generation"]})).await.unwrap();
 sql(&f.owner,"UPDATE webdock_auth.\"user\" SET banned=true WHERE id=$1",vec![f.uid.clone().into()]).await;assert!(f.service.agent_request(&f.agent,"guard",json!({"id":claim["id"],"generation":claim["generation"]})).await.is_err());sql(&f.owner,"UPDATE webdock_auth.\"user\" SET banned=false WHERE id=$1",vec![f.uid.clone().into()]).await;
 f.proof(&claim,"git-fixture-runtime").await;assert_eq!(service.observe_release(&publisher_worker,rid,rg).await.unwrap()["status"],"ready");

 assert_eq!(service.observe_release(&publisher_worker,rid,rg).await.unwrap()["status"],"ready");
 sql(&f.owner,"UPDATE webdock_auth.session SET \"expiresAt\"=now()+interval '1 hour' WHERE id=$1",vec![f.sid.clone().into()]).await;
 let sources=service.execute(&f.principal,json!({"action":"git.sources.list","projectID":f.pid})).await.unwrap();assert_eq!(sources["docs"][0]["buildEnvironmentNames"],json!(["BUILD_TOKEN"]));assert!(!sources.to_string().contains("sensitive-fixture-123"));
 let connections=service.execute(&f.principal,json!({"action":"git.connections.list","customerID":f.cid})).await.unwrap();assert_eq!(connections["docs"][0]["id"],connection);
 let repositories=service.execute(&f.principal,json!({"action":"git.repositories.list","customerID":f.cid,"connectionID":connection})).await.unwrap();assert_eq!(repositories["docs"][0]["fullName"],"fixture/local");
 let builds=service.execute(&f.principal,json!({"action":"git.builds.list","projectID":f.pid})).await.unwrap();assert_eq!(builds["docs"][0]["projectID"],f.pid);assert_eq!(builds["docs"][0]["targetID"],app);
 let retry=service.execute(&f.principal,json!({"action":"git.builds.request","projectID":f.pid,"targetID":app,"idempotencyKey":"cancel-build-request-1"})).await.unwrap();let retry_claim=service.claim_build_compatible(&worker,&[recipe.into()]).await.unwrap().unwrap();assert_eq!(retry_claim["buildID"],retry["id"]);service.execute(&f.principal,json!({"action":"git.builds.cancel","projectID":f.pid,"buildID":retry["id"]})).await.unwrap();assert!(service.guard_build(&worker,retry["id"].as_str().unwrap(),retry_claim["generation"].as_i64().unwrap() as i32).await.is_err());
 let releases=service.execute(&f.principal,json!({"action":"git.releases.list","projectID":f.pid})).await.unwrap();let rollback_command=json!({"action":"git.releases.rollback","projectID":f.pid,"releaseID":rid,"revision":releases["docs"][0]["revision"],"idempotencyKey":"rollback-git-request-1"});let rollback=service.execute_with_authority(&f.principal,&browser.authority,rollback_command.clone(),webdock_build::BuildCeiling{read:true,write:true}).await.unwrap();assert_eq!(service.execute_with_authority(&f.principal,&browser.authority,rollback_command,webdock_build::BuildCeiling{read:true,write:true}).await.unwrap()["id"],rollback["id"]);let rollback_claim=if frontend_enabled {
  let current=webdock_build::worker::ControlClient::loopback_test(&format!("http://{address}"),publication_credential.clone()).unwrap();
  let wire=current.post("release-claim",&json!({"artifactStoreBindings":bindings,"publicationProjects":targets,"artifactFormats":[0,1,2]})).await.unwrap();assert!(!wire.is_null());assert_eq!(wire["artifact"]["digest"],stored.digest());
  json!({"id":wire["releaseID"],"generation":wire["generation"]})
 }else{service.claim_release_with_storage(&publisher_worker,&bindings,&targets).await.unwrap().unwrap()};assert_eq!(rollback_claim["id"],rollback["id"]);
 let rollback_id=rollback["id"].as_str().unwrap();let rollback_generation=rollback_claim["generation"].as_i64().unwrap() as i32;
 let prepared=service.prepare_publication(&publisher_worker,rollback_id,rollback_generation).await.unwrap();let rollback_image=format!("{}@{}",prepared["repository"].as_str().unwrap(),prepared["imageDigest"].as_str().unwrap());
 let operation=service.publish_image(&publisher_worker,rollback_id,rollback_generation,&rollback_image).await.unwrap();
 let persisted=f.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT desired FROM webdock_auth.hosting_operation WHERE id=$1",[operation["id"].as_str().unwrap().into()])).await.unwrap().unwrap().try_get::<Value>("","desired").unwrap();assert_eq!(persisted["gitApprovalAuthority"],serde_json::to_value(&browser.authority).unwrap());
 let native=hosting.claim_runtime(&f.agent).await.unwrap().unwrap();assert_eq!(native.operation_id,operation["id"].as_str().unwrap());hosting.guard_runtime_command(&f.agent,&native).await.unwrap();
 if let Some(remote)=&remote {
  let requested=service.execute(&f.principal,json!({"action":"git.builds.request","projectID":f.pid,"targetID":app,"idempotencyKey":"evidence-worker-build-1"})).await.unwrap();
  let next_id=requested["id"].as_str().unwrap();
  let local=tempfile::tempdir().unwrap();let scratch=tempfile::tempdir_in(&worker_scratch_root).unwrap();let state=tempfile::tempdir().unwrap();
  for directory in [&local,&scratch,&state] {std::fs::set_permissions(directory.path(),std::fs::Permissions::from_mode(0o700)).unwrap();}
  let real_worker=webdock_build::worker::Worker::new(control.clone(),vm_image,webdock_build::artifact::ArtifactStore::open(local.path().into(),1024*1024*1024).unwrap(),scratch.path().into(),webdock_build::worker::Outbox::open(state.path().into()).unwrap()).unwrap().with_artifact_store(remote.clone(),true).unwrap().with_evidence_signer(ed25519_dalek::SigningKey::from_bytes(&[9;32]));
  assert!(real_worker.build_once().await.unwrap(),"actual worker executes the real guest and persists signed source evidence");
  let completed=service.execute(&f.principal,json!({"action":"git.builds.get","projectID":f.pid,"buildID":next_id})).await.unwrap();assert_eq!(completed["status"],"succeeded","actual worker result: {completed}");
  let row=f.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT a.storage_key,a.digest,a.size_bytes,a.image_digest,a.remote_storage,b.generation FROM webdock_auth.git_artifact a JOIN webdock_auth.git_build b ON b.id=a.build_id WHERE b.id=$1",[next_id.into()])).await.unwrap().unwrap();
  let receipt:Value=row.try_get("","remote_storage").unwrap();assert!(receipt.is_object());
  let reader=webdock_build::artifact::ArtifactStore::open(local.path().into(),1024*1024*1024).unwrap();
  let saved=reader.load(row.try_get("","storage_key").unwrap(),row.try_get("","digest").unwrap(),row.try_get::<i64>("","size_bytes").unwrap() as u64,row.try_get("","image_digest").unwrap()).unwrap();
  assert_eq!(receipt["sha256"],saved.digest().strip_prefix("sha256:").unwrap());
  let bytes=reader.read(&saved).unwrap();let envelope=webdock_build::artifact::Archive::read(&bytes,webdock_build::artifact::MAX_OUTPUT).unwrap();
  let proof=webdock_build::evidence::validate(&envelope,&f.cid,next_id,row.try_get("","generation").unwrap(),Some(&evidence_key.verifying_key())).unwrap().unwrap();assert_eq!(proof.build.project_id,f.pid);assert_eq!(envelope.files()["source.tar"],source_snapshot);
  assert!(!std::str::from_utf8(&envelope.files()["provenance.json"]).unwrap().contains("sensitive-fixture-123"));
  // A valid source with an oversized escaped inventory must fail terminally, not retain the oldest lease.
  let component="\u{1}".repeat(230);let inventory=evidence_repository_path.join("inventory").join(&component).join(&component);std::fs::create_dir_all(&inventory).unwrap();
  for index in 0..22_000 {std::fs::write(inventory.join(format!("{index:05}")),b"").unwrap();}
  std::fs::write(evidence_repository_path.join(".dockerignore"),"inventory/\n").unwrap();
  for args in [vec!["add","--all"],vec!["-c","user.name=Fixture","-c","user.email=fixture@example.invalid","commit","-m","inventory boundary"]] {let output=std::process::Command::new("git").env("GIT_CONFIG_GLOBAL","/dev/null").env("GIT_CONFIG_NOSYSTEM","1").arg("-C").arg(&evidence_repository_path).args(args).output().unwrap();assert!(output.status.success());}
  let rejected=service.execute(&f.principal,json!({"action":"git.builds.request","projectID":f.pid,"targetID":app,"idempotencyKey":"evidence-boundary-build-1"})).await.unwrap();
  let following=service.execute(&f.principal,json!({"action":"git.builds.request","projectID":f.pid,"targetID":app,"sourceSHA":original_source_commit,"idempotencyKey":"evidence-after-boundary-1"})).await.unwrap();
  assert!(real_worker.build_once().await.unwrap());
  let failed=service.execute(&f.principal,json!({"action":"git.builds.get","projectID":f.pid,"buildID":rejected["id"]})).await.unwrap();assert_eq!(failed["status"],"failed","boundary result: {failed}");assert!(failed["logs"].as_str().unwrap().contains("artifact_inventory_limit"));
  assert!(real_worker.build_once().await.unwrap(),"next queued build is not starved by deterministic preparation failure");
  let resumed=service.execute(&f.principal,json!({"action":"git.builds.get","projectID":f.pid,"buildID":following["id"]})).await.unwrap();assert_eq!(resumed["status"],"succeeded","following result: {resumed}");
  if frontend_enabled {
   let original=std::process::Command::new("git").env("GIT_CONFIG_GLOBAL","/dev/null").env("GIT_CONFIG_NOSYSTEM","1").arg("-C").arg(&evidence_repository_path).args(["show",&format!("{original_source_commit}:Dockerfile")]).output().unwrap();assert!(original.status.success());
   let mut dockerfile=String::from_utf8(original.stdout).unwrap();dockerfile.push_str("RUN /bin/busybox truncate -s 536870912 /app/dist/oversized\n");std::fs::write(evidence_repository_path.join("Dockerfile"),dockerfile).unwrap();
   for args in [vec!["add","Dockerfile"],vec!["-c","user.name=Fixture","-c","user.email=fixture@example.invalid","commit","-m","frontend output boundary"]] {assert!(std::process::Command::new("git").env("GIT_CONFIG_GLOBAL","/dev/null").env("GIT_CONFIG_NOSYSTEM","1").arg("-C").arg(&evidence_repository_path).args(args).output().unwrap().status.success());}
   let overflow=service.execute(&f.principal,json!({"action":"git.builds.request","projectID":f.pid,"targetID":app,"idempotencyKey":"frontend-output-boundary"})).await.unwrap();
   let following=service.execute(&f.principal,json!({"action":"git.builds.request","projectID":f.pid,"targetID":app,"sourceSHA":original_source_commit,"idempotencyKey":"frontend-after-output-boundary"})).await.unwrap();
   assert!(real_worker.build_once().await.unwrap());let failed=service.execute(&f.principal,json!({"action":"git.builds.get","projectID":f.pid,"buildID":overflow["id"]})).await.unwrap();assert_eq!(failed["status"],"failed");assert!(failed["logs"].as_str().unwrap().contains("frontend_output_limit"),"{failed}");
   assert!(real_worker.build_once().await.unwrap());assert_eq!(service.execute(&f.principal,json!({"action":"git.builds.get","projectID":f.pid,"buildID":following["id"]})).await.unwrap()["status"],"succeeded");
   let oversized=std::fs::File::create(evidence_repository_path.join("source-overflow.bin")).unwrap();oversized.set_len(webdock_build::artifact::MAX_SOURCE+1).unwrap();drop(oversized);
   for args in [vec!["add","source-overflow.bin"],vec!["-c","user.name=Fixture","-c","user.email=fixture@example.invalid","commit","-m","source boundary"]] {assert!(std::process::Command::new("git").env("GIT_CONFIG_GLOBAL","/dev/null").env("GIT_CONFIG_NOSYSTEM","1").arg("-C").arg(&evidence_repository_path).args(args).output().unwrap().status.success());}
   let overflow=service.execute(&f.principal,json!({"action":"git.builds.request","projectID":f.pid,"targetID":app,"idempotencyKey":"frontend-source-boundary"})).await.unwrap();
   let following=service.execute(&f.principal,json!({"action":"git.builds.request","projectID":f.pid,"targetID":app,"sourceSHA":original_source_commit,"idempotencyKey":"frontend-after-source-boundary"})).await.unwrap();
   assert!(real_worker.build_once().await.unwrap());let failed=service.execute(&f.principal,json!({"action":"git.builds.get","projectID":f.pid,"buildID":overflow["id"]})).await.unwrap();assert_eq!(failed["status"],"failed");assert!(failed["logs"].as_str().unwrap().contains("source preparation exceeded"),"{failed}");
   assert!(real_worker.build_once().await.unwrap());assert_eq!(service.execute(&f.principal,json!({"action":"git.builds.get","projectID":f.pid,"buildID":following["id"]})).await.unwrap()["status"],"succeeded");
  }

 }
 browser.revoke(&f.owner).await;
 assert!(service.guard_release(&publisher_worker,rollback_id,rollback_generation).await.is_err());assert!(hosting.guard_runtime_command(&f.agent,&native).await.is_err());assert!(hosting.complete_runtime_failure(&f.agent,&native,"execution_failed").await.is_err());

 sql(&f.owner,"UPDATE webdock_auth.session SET \"expiresAt\"=now()-interval '1 second' WHERE id=$1",vec![f.sid.clone().into()]).await;assert!(service.guard_release(&publisher_worker,rollback["id"].as_str().unwrap(),rollback_claim["generation"].as_i64().unwrap() as i32).await.is_err());
 sql(&f.owner,"UPDATE webdock_auth.git_worker SET enabled=false WHERE id=$1",vec![worker_id.into()]).await;assert!(service.authenticate_worker(&credential).await.is_err());assert_eq!(control.post("claim",&json!({})).await.unwrap_err().code,"worker_revoked");assert!(service.complete_build(&worker,build,generation,Some(&stored),&logs,None).await.is_err());
 }).await;
    for table in [
        "git_check_outbox",
        "git_release",
        "git_artifact",
        "git_build",
        "git_source",
        "git_repository",
        "git_connection",
    ] {
        let predicate = match table {
            "git_check_outbox" => {
                "build_id IN (SELECT id FROM webdock_auth.git_build WHERE customer_id=$1)"
            }
            _ => "customer_id=$1",
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
    sql(
        &fixture.owner,
        "DELETE FROM webdock_auth.git_worker WHERE id=$1",
        vec![cleanup_publication_worker.into()],
    )
    .await;
    sql(
        &fixture.owner,
        "DELETE FROM webdock_admin.projects WHERE id=$1",
        vec![cleanup_other_project.into()],
    )
    .await;
    let _ = cleanup_connection;
    browser_fixture.cleanup(&fixture.owner).await;
    fixture.cleanup().await;
    outcome.unwrap();
}
