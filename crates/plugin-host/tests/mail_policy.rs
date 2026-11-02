use futures_util::FutureExt;
use hmac::{Hmac, Mac};
use sea_orm::{
    ConnectionTrait, Database, DatabaseConnection, DatabaseTransaction, DbBackend, Statement,
    TransactionTrait,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{os::unix::fs::PermissionsExt, sync::Arc};
use webdock_plugin_host::{FileBindings, HostService};
const KEY: &str = "mail-policy-owned-test-secret-at-least-32bytes";
async fn context(db: &DatabaseConnection, purpose: Option<&str>) -> DatabaseTransaction {
    let tx = db.begin().await.unwrap();
    if let Some(purpose) = purpose {
        let raw=json!({"v":1,"aud":"webdock_worker_runtime","exp":chrono::Utc::now().timestamp()+60,"scope":{"kind":"system","purpose":purpose}}).to_string();
        let mut mac = Hmac::<Sha256>::new_from_slice(KEY.as_bytes()).unwrap();
        mac.update(raw.as_bytes());
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT set_config('webdock.rls_context',$1,true),set_config('webdock.rls_signature',$2,true)",[raw.into(),hex::encode(mac.finalize().into_bytes()).into()])).await.unwrap();
    }
    tx
}
async fn policy(tx: &DatabaseTransaction, continuation: bool) -> Option<Value> {
    tx.query_one_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT webdock_rls.mail_plugin_policy('123',$1) AS p",
        [continuation.into()],
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get("", "p")
    .unwrap()
}
async fn role(key: &str, name: &str, owner: &url::Url) -> DatabaseConnection {
    let mut url = url::Url::parse(&std::env::var(key).unwrap()).unwrap();
    assert_eq!(url.host_str(), owner.host_str());
    assert_eq!(url.port(), owner.port());
    assert_eq!(url.path(), "/webdock_rls_test");
    assert!(url.query().is_none());
    url.set_path(&format!("/{name}"));
    Database::connect(url.as_str()).await.unwrap()
}
#[tokio::test]
#[ignore = "creates and removes an isolated owned local database"]
async fn tenant_mail_policy_is_scoped_current_and_locked_through_handoff() {
    let mut url =
        url::Url::parse(&std::env::var("WEBDOCK_TEST_MIGRATION_DATABASE_URL").unwrap()).unwrap();
    assert!(matches!(url.host_str(), Some("localhost" | "127.0.0.1")));
    assert_eq!(url.path(), "/webdock_rls_test");
    assert!(url.query().is_none());
    let admin = Database::connect(url.as_str()).await.unwrap();
    let name = format!("webdock_plugin_mail_{}_test", uuid::Uuid::new_v4().simple());
    admin
        .execute_unprepared(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    url.set_path(&format!("/{name}"));
    let outcome=std::panic::AssertUnwindSafe(async{
  let owner=Database::connect(url.as_str()).await.unwrap();webdock_persistence::migrations::bootstrap(&owner,&name,KEY).await.unwrap();owner.execute_unprepared(include_str!("../migrations/0002_mail_policy.sql")).await.unwrap();
  let seed=owner.begin().await.unwrap();seed.execute_unprepared("SELECT set_config('webdock.organization_allocation','{\"customer_id\":\"123\",\"organization_id\":\"124\"}',true)").await.unwrap();seed.execute_unprepared("INSERT INTO webdock_admin.customers(id,name) VALUES('123','Mail policy fixture')").await.unwrap();seed.commit().await.unwrap();
  let mut manifest:Value=serde_json::from_str(include_str!("../../plugin-protocol/tests/fixtures/project-notes.json")).unwrap();manifest["name"]=json!("mail-policy-fixture");manifest["capabilities"]=json!(["plugin.http.request"]);manifest["actions"]=json!([{"name":"mail.send","intent":"write","audience":"operator","capabilities":["plugin.http.request"]}]);manifest["services"]=json!({"required":[{"name":"http","version":1}],"optional":[]});manifest["jobs"]=json!([]);manifest["httpBindings"]=json!(["relay"]);
  let raw=manifest.to_string();let hash=hex::encode(Sha256::digest(raw.as_bytes()));let image=manifest["image"].as_str().unwrap();
  owner.execute_unprepared("INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version) VALUES('mail','builtin','enabled','enabled','1.0.0') ON CONFLICT(id) DO UPDATE SET state='enabled'; INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version,runtime,current_generation) VALUES('mail-policy-fixture','external','enabled','enabled','1.0.0','ready',1); INSERT INTO webdock_mail.send_transport(customer_id,mode,revision,state,region,encrypted_credentials) VALUES('123','managed',1,'ready','eu','private-fixture-ciphertext'); INSERT INTO webdock_mail.provider_binding(customer_id,plugin_id,revision,transport_revision) VALUES('123','mail-policy-fixture',1,1)").await.unwrap();
  owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.plugin_release(plugin_id,version,manifest_bytes,manifest_sha256,image,registry_url,signer_key_id) VALUES('mail-policy-fixture','1.0.0',$1,$2,$3,'https://registry.invalid','fixture')",[raw.into(),hash.into(),image.into()])).await.unwrap();
  owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.plugin_runtime(plugin_id,generation,version,image,status,configuration_revision,credential_hash,namespace,service_name) VALUES('mail-policy-fixture',1,'1.0.0',$1,'ready',0,$2,'fixtures','fixture-mail')",[image.into(),"a".repeat(64).into()])).await.unwrap();owner.execute_unprepared("INSERT INTO webdock_auth.plugin_configuration(plugin_id,approved_capabilities,configuration) VALUES('mail-policy-fixture','[\"plugin.http.request\"]','{}')").await.unwrap();
  let worker=role("WEBDOCK_PLUGIN_WORKER_DATABASE_URL",&name,&url).await;let business=role("DATABASE_URL",&name,&url).await;
  for purpose in [None,Some("mail-webmail-gateway"),Some("plugin-controller")]{let tx=context(&worker,purpose).await;assert!(policy(&tx,false).await.is_none());tx.rollback().await.unwrap();}
  let file=tempfile::NamedTempFile::new().unwrap();std::fs::set_permissions(file.path(),std::fs::Permissions::from_mode(0o600)).unwrap();let bindings=json!({"runtimes":{"mail-policy-fixture":{"generation":1,"namespace":"fixtures","service":"fixture-mail","credentialSha256":"a".repeat(64),"peers":[{"kind":"unix","uid":rustix::process::geteuid().as_raw()}]}}});std::fs::write(file.path(),bindings.to_string()).unwrap();
  let host=HostService::new(business.clone(),worker.clone(),KEY.into(),Arc::new(webdock_persistence::IdAllocator::new(business.clone(),webdock_ids::Identity::platform(0,2).unwrap())),Arc::new(FileBindings::new(file.path().into())),ed25519_dalek::SigningKey::from_bytes(&[33;32])).await.unwrap();
  let tx=context(&worker,Some("mail-send-gateway")).await;let snapshot=policy(&tx,false).await.unwrap();assert!(snapshot["transport"].get("encrypted_credentials").is_none());assert!(snapshot["transport"].get("pending_token").is_none());let typed=host.mail_policy_in(&tx,"123",false).await.unwrap();assert_eq!(typed.customer_id,"123");assert_eq!(typed.grant_revision,1);assert_eq!(typed.binding_sha256.len(),64);assert!(typed.permits_peer(&webdock_plugin_runtime::PeerIdentity::Unix{uid:rustix::process::geteuid().as_raw()}));
  let blocker=owner.clone();let revoke=tokio::spawn(async move{blocker.execute_unprepared("UPDATE webdock_mail.provider_binding SET revision=revision+1,revoked_at=now() WHERE customer_id='123'").await.unwrap();});tokio::time::sleep(std::time::Duration::from_millis(80)).await;assert!(!revoke.is_finished());tx.commit().await.unwrap();tokio::time::timeout(std::time::Duration::from_secs(2),revoke).await.unwrap().unwrap();
  let tx=context(&worker,Some("mail-send-gateway")).await;assert!(host.mail_policy_in(&tx,"123",true).await.is_err());tx.rollback().await.unwrap();owner.execute_unprepared("UPDATE webdock_mail.provider_binding SET revision=revision+1,revoked_at=NULL WHERE customer_id='123'; UPDATE webdock_auth.plugin_installation SET state='draining' WHERE id='mail-policy-fixture'").await.unwrap();
  let tx=context(&worker,Some("mail-send-gateway")).await;assert!(policy(&tx,false).await.is_none());assert!(host.mail_policy_in(&tx,"123",true).await.is_ok());tx.rollback().await.unwrap();
  owner.execute_unprepared("UPDATE webdock_auth.plugin_installation SET state='enabled' WHERE id='mail-policy-fixture'; UPDATE webdock_mail.send_transport SET revision=2 WHERE customer_id='123'").await.unwrap();let tx=context(&worker,Some("mail-send-gateway")).await;assert!(policy(&tx,true).await.is_none());tx.rollback().await.unwrap();
  owner.execute_unprepared("UPDATE webdock_mail.send_transport SET revision=1 WHERE customer_id='123'; UPDATE webdock_auth.plugin_configuration SET approved_capabilities='[]' WHERE plugin_id='mail-policy-fixture'").await.unwrap();let tx=context(&worker,Some("mail-send-gateway")).await;assert!(policy(&tx,true).await.is_none());tx.rollback().await.unwrap();
  drop(host);worker.close().await.unwrap();business.close().await.unwrap();owner.close().await.unwrap();
 }).catch_unwind().await;
    admin
        .execute_unprepared(&format!("DROP DATABASE {name} WITH (FORCE)"))
        .await
        .unwrap();
    if let Err(error) = outcome {
        std::panic::resume_unwind(error);
    }
}
