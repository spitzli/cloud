use sea_orm::{
    ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement, TransactionTrait,
};
use serde_json::{Value, json};
use webdock_mail_protocol::sha256;
pub const KEY: &str = "mail-sending-owned-fixture-context-at-least32";
pub const PLUGIN: &str = "mail-policy-fixture";

pub async fn sql(db: &impl ConnectionTrait, text: &str, values: Vec<sea_orm::Value>) {
    if values.is_empty() {
        db.execute_unprepared(text).await.unwrap();
        return;
    }
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        text,
        values,
    ))
    .await
    .unwrap();
}
pub async fn role(key: &str, owner: &url::Url) -> DatabaseConnection {
    let mut url = url::Url::parse(&std::env::var(key).unwrap()).unwrap();
    assert_eq!(url.host_str(), owner.host_str());
    assert_eq!(url.port(), owner.port());
    assert_eq!(url.path(), "/webdock_rls_test");
    assert!(url.query().is_none());
    url.set_path(owner.path());
    Database::connect(url.as_str()).await.unwrap()
}
pub async fn seed(owner: &DatabaseConnection) {
    let tx = owner.begin().await.unwrap();
    sql(&tx,"SELECT set_config('webdock.organization_allocation','{\"customer_id\":\"123\",\"organization_id\":\"124\"}',true)",vec![]).await;
    sql(
        &tx,
        "INSERT INTO webdock_admin.customers(id,name) VALUES('123','Sending fixture')",
        vec![],
    )
    .await;
    tx.commit().await.unwrap();
    sql(owner,"INSERT INTO webdock_auth.platform_settings(id,settings) VALUES(true,'{\"mailEnabled\":true}'); INSERT INTO webdock_auth.tenant_subscription(customer_id,name,description,base,extras,revision,updated_by) VALUES('123','Fixture','Disposable','{\"mailMessages\":100}','{}',1,'fixture'); INSERT INTO webdock_auth.mail_sender_domain(id,customer_id,domain,token,status,provider_id,provider_snapshot) VALUES('301','123','example.test','fixture','ownership_verified','domain-fixture','{\"id\":\"domain-fixture\",\"domain\":\"example.test\",\"spf_verified\":true,\"dkim_verified\":true,\"dmarc_verified\":true}')",vec![]).await;
    sql(owner,"INSERT INTO webdock_mail.api_key(id,customer_id,label,token_hash,key_suffix,created_by) VALUES('456','123','fixture',$1,'tttt','fixture')",vec![sha256(format!("wd_mail_{}","t".repeat(43)).as_bytes()).into()]).await;
    let mut manifest: Value = serde_json::from_str(include_str!(
        "../../../plugin-protocol/tests/fixtures/project-notes.json"
    ))
    .unwrap();
    manifest["name"] = json!(PLUGIN);
    manifest["capabilities"] = json!(["plugin.http.request"]);
    manifest["actions"] = json!([{"name":"mail.send","intent":"write","audience":"operator","capabilities":["plugin.http.request"]}]);
    manifest["services"] = json!({"required":[{"name":"http","version":1}],"optional":[]});
    manifest["jobs"] = json!([]);
    manifest["httpBindings"] = json!(["send"]);
    let raw = manifest.to_string();
    let image = manifest["image"].as_str().unwrap();
    sql(owner,"INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version) VALUES('mail','builtin','enabled','enabled','1.0.0'); INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version,runtime,current_generation) VALUES('mail-policy-fixture','external','enabled','enabled','1.0.0','ready',1); INSERT INTO webdock_mail.send_transport(customer_id,mode,revision,state,region,encrypted_credentials) VALUES('123','managed',1,'ready','eu','fixture-ciphertext'); INSERT INTO webdock_mail.provider_binding(customer_id,plugin_id,revision,transport_revision) VALUES('123','mail-policy-fixture',1,1)",vec![]).await;
    sql(owner,"INSERT INTO webdock_auth.plugin_release(plugin_id,version,manifest_bytes,manifest_sha256,image,registry_url,signer_key_id) VALUES('mail-policy-fixture','1.0.0',$1,$2,$3,'https://registry.invalid','fixture')",vec![raw.clone().into(),sha256(raw.as_bytes()).into(),image.into()]).await;
    sql(owner,"INSERT INTO webdock_auth.plugin_runtime(plugin_id,generation,version,image,status,configuration_revision,credential_hash,namespace,service_name) VALUES('mail-policy-fixture',1,'1.0.0',$1,'ready',0,$2,'fixtures','fixture-mail')",vec![image.into(),"a".repeat(64).into()]).await;
    sql(owner,"INSERT INTO webdock_auth.plugin_configuration(plugin_id,approved_capabilities,configuration) VALUES('mail-policy-fixture','[\"plugin.http.request\"]','{}')",vec![]).await;
}
pub fn bindings() -> Value {
    json!({"runtimes":{PLUGIN:{
        "generation":1,"namespace":"fixtures","service":"fixture-mail",
        "credentialSha256":"a".repeat(64),
        "peers":[{"kind":"unix","uid":rustix::process::geteuid().as_raw()}],
        "customers":{"123":{"send":{
            "origin":"https://api.eu.turbo-smtp.com","addresses":["1.1.1.1"],
            "headers":{"consumerkey":"fixture-provider-key","consumersecret":"fixture-provider-secret"},
            "rules":[{"method":"POST","path":"/api/v2/mail/send","query":[]}],"ca":null
        }}}
    }}})
}
