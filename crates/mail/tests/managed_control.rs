//! Task2: fresh restricted roles; no provider or retained fixture access.
mod support;
use hmac::{Hmac, Mac};
use sea_orm::{ConnectionTrait, Database, DbBackend, Statement, TransactionTrait};
use serde_json::{Value, json};
use sha2::Sha256;
use support::sending::{KEY, role, seed, sql};
#[tokio::test]
#[ignore = "fresh exclusively owned local PostgreSQL fixture"]
async fn external_mail_policy_and_managed_lane_are_authoritative() {
    let mut url =
        url::Url::parse(&std::env::var("WEBDOCK_TEST_MIGRATION_DATABASE_URL").unwrap()).unwrap();
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    assert_eq!(url.path(), "/webdock_rls_test");
    let admin = Database::connect(url.as_str()).await.unwrap();
    let name = format!("webdock_task2_{}", uuid::Uuid::new_v4().simple());
    sql(&admin, &format!("CREATE DATABASE {name}"), vec![]).await;
    url.set_path(&format!("/{name}"));
    let owner = Database::connect(url.as_str()).await.unwrap();
    let outcome = std::panic::AssertUnwindSafe(async {
        webdock_persistence::migrations::bootstrap(&owner, &name, KEY)
            .await
            .unwrap();
        seed(&owner).await;
        sql(&owner,"UPDATE webdock_auth.plugin_installation SET kind='external',runtime='ready',current_generation=1 WHERE id='mail'; UPDATE webdock_mail.provider_binding SET plugin_id='mail',revision=revision+1 WHERE customer_id='123'",vec![]).await;
        let mut manifest: Value = serde_json::from_str(include_str!(
            "../../plugin-protocol/tests/fixtures/project-notes.json"
        ))
        .unwrap();
        manifest["name"] = json!("mail");
        manifest["jobs"] = json!([]);
        manifest["actions"] =
            json!([{"name":"mail.send","intent":"write","audience":"customer","capabilities":[]}]);
        manifest["capabilities"] = json!([]);
        manifest["services"] = json!({"required":[],"optional":[]});
        manifest["api"] = json!([]);
        manifest["mcp"] = json!([]);
        manifest["areas"]["selfService"] = json!({"pages":[]});
        for a in manifest["actions"].as_array_mut().unwrap() {
            if a["resource"].is_string() {
                a["input"] = json!({"type":"object","properties":{"resource_id":{"type":"string","maxLength":128}},"required":["resource_id"]});
            }
        }
        let raw = manifest.to_string();
        let image = manifest["image"].as_str().unwrap();
        sql(&owner,"INSERT INTO webdock_auth.plugin_release(plugin_id,version,manifest_bytes,manifest_sha256,image,registry_url,signer_key_id) VALUES('mail','1.0.0',$1,$2,$3,'https://registry.invalid','fixture');",vec![raw.clone().into(),webdock_mail_protocol::sha256(raw.as_bytes()).into(),image.into()]).await;
        sql(&owner,"INSERT INTO webdock_auth.plugin_runtime(plugin_id,generation,version,image,status,configuration_revision,credential_hash,namespace,service_name) VALUES('mail',1,'1.0.0',$1,'ready',0,$2,'fixtures','mail')",vec![image.into(),"a".repeat(64).into()]).await;
        sql(&owner,"INSERT INTO webdock_auth.plugin_configuration(plugin_id,approved_capabilities,configuration) VALUES('mail','[]','{}')",vec![]).await;
        let worker = role("WEBDOCK_PLUGIN_WORKER_DATABASE_URL", &url).await;
        let tx = worker.begin().await.unwrap();
        let raw=json!({"v":1,"aud":"webdock_worker_runtime","exp":chrono::Utc::now().timestamp()+60,"scope":{"kind":"system","purpose":"mail-send-gateway"}}).to_string();
        let mut mac = Hmac::<Sha256>::new_from_slice(KEY.as_bytes()).unwrap();
        mac.update(raw.as_bytes());
        sql(&tx,"SELECT set_config('webdock.rls_context',$1,true),set_config('webdock.rls_signature',$2,true)",vec![raw.into(),hex::encode(mac.finalize().into_bytes()).into()]).await;
        let row = tx
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT webdock_rls.mail_plugin_policy('123',false) AS policy",
            ))
            .await
            .unwrap()
            .unwrap();
        let policy: Option<Value> = row.try_get("", "policy").unwrap();
        assert!(
            policy.is_some(),
            "external Mail must not require a second builtin Mail installation"
        );
        tx.rollback().await.unwrap();
        worker.close().await.unwrap();
    });
    use futures_util::FutureExt;
    let result = outcome.catch_unwind().await;
    owner.close().await.unwrap();
    sql(
        &admin,
        &format!("DROP DATABASE {name} WITH (FORCE)"),
        vec![],
    )
    .await;
    admin.close().await.unwrap();
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}

#[tokio::test]
#[ignore = "fresh exclusively owned local PostgreSQL fixture"]
async fn actual_native_control_action_replay_pool1_lane_and_receipt() {
    let mut url =
        url::Url::parse(&std::env::var("WEBDOCK_TEST_MIGRATION_DATABASE_URL").unwrap()).unwrap();
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    let admin = Database::connect(url.as_str()).await.unwrap();
    let name = format!("webdock_task2_{}", uuid::Uuid::new_v4().simple());
    sql(&admin, &format!("CREATE DATABASE {name}"), vec![]).await;
    url.set_path(&format!("/{name}"));
    let owner = Database::connect(url.as_str()).await.unwrap();
    use futures_util::FutureExt;
    let result = std::panic::AssertUnwindSafe(run_control(&owner, &url, &name))
        .catch_unwind()
        .await;
    owner.close().await.unwrap();
    sql(
        &admin,
        &format!("DROP DATABASE {name} WITH (FORCE)"),
        vec![],
    )
    .await;
    admin.close().await.unwrap();
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}
async fn pool1(key: &str, url: &url::Url) -> sea_orm::DatabaseConnection {
    let mut u = url::Url::parse(&std::env::var(key).unwrap()).unwrap();
    assert_eq!(u.host_str(), url.host_str());
    assert_eq!(u.port(), url.port());
    u.set_path(url.path());
    let mut opts = sea_orm::ConnectOptions::new(u.to_string());
    opts.max_connections(1)
        .min_connections(1)
        .sqlx_logging(false);
    Database::connect(opts).await.unwrap()
}
async fn run_control(owner: &sea_orm::DatabaseConnection, url: &url::Url, name: &str) {
    use std::{os::unix::fs::PermissionsExt, sync::Arc};
    use webdock_mail::{
        MailDependencies,
        managed_control::ManagedControl,
        sending::{Sending, SendingConfig},
        submissions::{QueueCommand, Submissions},
    };
    use webdock_mail_protocol::{native_control::NativeControlClient, outbox::SendLease};
    use webdock_plugin_host::{AdmissionInput, FileBindings, HostService};
    let _legacy_binding_shape = support::sending::bindings();
    webdock_persistence::migrations::bootstrap(owner, name, KEY)
        .await
        .unwrap();
    seed(owner).await;
    sql(owner,"UPDATE webdock_auth.plugin_installation SET kind='external',runtime='ready',current_generation=1 WHERE id='mail'; UPDATE webdock_mail.provider_binding SET plugin_id='mail',revision=revision+1 WHERE customer_id='123'; UPDATE webdock_mail.send_transport SET provider_account_id='reviewed-account' WHERE customer_id='123'",vec![]).await;
    sql(owner,"INSERT INTO webdock_admin.projects(id,name,customer_id) VALUES('234','Owned project','123'); INSERT INTO webdock_auth.\"user\"(id,name,email,\"emailVerified\",role,\"twoFactorEnabled\",\"mustChangePassword\") VALUES('actor','Fixture','actor@example.invalid',true,'operator',true,false); INSERT INTO webdock_auth.session(id,token,\"userId\",\"expiresAt\",\"updatedAt\") VALUES('session','fixture-token','actor',now()+interval '1 hour',now()); INSERT INTO webdock_admin.users(id,name,email,role,auth_subject) VALUES('345','Fixture','actor@example.invalid','operator','actor')",vec![]).await;
    let mut manifest: Value = serde_json::from_str(include_str!(
        "../../plugin-protocol/tests/fixtures/project-notes.json"
    ))
    .unwrap();
    manifest["name"] = json!("mail");
    manifest["jobs"] = json!([]);
    manifest["capabilities"] = json!([]);
    manifest["services"] =
        json!({"required":[{"name":"native-mail-control","version":1}],"optional":[]});
    manifest["actions"] = json!([{"name":"mail.send","intent":"write","audience":"customer","resource":"use","capabilities":[]},{"name":"mail.status","intent":"read","audience":"customer","resource":"use","capabilities":[]},{"name":"mail.cancel","intent":"write","audience":"customer","resource":"use","capabilities":[]},{"name":"mail.history","intent":"read","audience":"customer","resource":"use","capabilities":[]},{"name":"domains.delete","intent":"write","audience":"customer","resource":"delete","capabilities":[]}]);
    manifest["api"] = json!([]);
    manifest["mcp"] = json!([]);
    manifest["areas"]["selfService"] = json!({"pages":[]});
    for a in manifest["actions"].as_array_mut().unwrap() {
        if a["resource"].is_string() {
            a["input"] = json!({"type":"object","properties":{"resource_id":{"type":"string","maxLength":128}},"required":["resource_id"]});
        }
    }
    let raw = manifest.to_string();
    let image = manifest["image"].as_str().unwrap();
    sql(owner,"INSERT INTO webdock_auth.plugin_release(plugin_id,version,manifest_bytes,manifest_sha256,image,registry_url,signer_key_id) VALUES('mail','1.0.0',$1,$2,$3,'https://registry.invalid','fixture')",vec![raw.clone().into(),webdock_mail_protocol::sha256(raw.as_bytes()).into(),image.into()]).await;
    sql(owner,"INSERT INTO webdock_auth.plugin_runtime(plugin_id,generation,version,image,status,configuration_revision,credential_hash,namespace,service_name) VALUES('mail',1,'1.0.0',$1,'ready',0,$2,'fixtures','mail')",vec![image.into(),"a".repeat(64).into()]).await;
    sql(owner,"INSERT INTO webdock_auth.plugin_configuration(plugin_id,approved_capabilities,configuration) VALUES('mail','[]','{}'); INSERT INTO webdock_auth.plugin_grant(plugin_id,customer_id,project_id,capabilities) VALUES('mail','123','234','[]'); INSERT INTO webdock_auth.plugin_resource(plugin_id,resource_id,project_id,customer_id,offering_id,name,created_via,created_by) VALUES('mail','domain-1','234','123','domain','example.test','operator','actor')",vec![]).await;
    let auth = pool1("WEBDOCK_TEST_AUTH_DATABASE_URL", url).await;
    let business = pool1("DATABASE_URL", url).await;
    let worker = pool1("WEBDOCK_PLUGIN_WORKER_DATABASE_URL", url).await;
    let resolver = pool1("WEBDOCK_PLUGIN_WORKER_DATABASE_URL", url).await;
    let cookies =
        webdock_identity::CookiePolicy::new(false, "fixture-cookie-signing-material-atleast32")
            .unwrap();
    let cookie = format!(
        "{}={}",
        cookies.name(),
        cookies.sign("fixture-token").unwrap()
    );
    let identity = Arc::new(webdock_identity::IdentityService::new(
        auth.clone(),
        cookies,
    ));
    let principal = identity
        .authenticate_cookie(&cookie)
        .await
        .unwrap()
        .unwrap();
    let ids = Arc::new(webdock_persistence::IdAllocator::new(
        business.clone(),
        webdock_ids::Identity::platform(0, 2).unwrap(),
    ));
    let (oauth, mcp_actors) =
        fix1_mcp_setup(owner, auth.clone(), identity.clone(), ids.clone(), &cookie).await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = dir.path().join("bindings.json");
    let bindings = json!({"runtimes":{"mail":{"generation":1,"namespace":"fixtures","service":"mail","credentialSha256":"a".repeat(64),"peers":[{"kind":"unix","uid":rustix::process::geteuid().as_raw()}]}}});
    std::fs::write(&path, bindings.to_string()).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let host = Arc::new(
        HostService::new(
            business.clone(),
            worker.clone(),
            KEY.into(),
            ids.clone(),
            Arc::new(
                webdock_plugin_host::ManagedBindings::new(
                    resolver.clone(),
                    KEY.into(),
                    Arc::new(FileBindings::new(path.clone())),
                )
                .await
                .unwrap(),
            ),
            ed25519_dalek::SigningKey::from_bytes(&[33; 32]),
        )
        .await
        .unwrap()
        .with_native_mail_control(true)
        .with_session_authority(auth.clone(), identity.clone(), None)
        .await
        .unwrap()
        .with_mcp_authority(oauth.clone())
        .unwrap(),
    );
    let deps = MailDependencies {
        auth: auth.clone(),
        business: business.clone(),
        worker: worker.clone(),
        identity,
        browser: None,
        ids,
    };
    let sender = Arc::new(
        Sending::new(
            deps.clone(),
            SendingConfig {
                enabled: true,
                rls_secret: KEY.into(),
                signing_key: [78; 32],
                worker_id: "legacy-worker".into(),
                worker_generation: 99,
                worker_credential_sha256: "b".repeat(64),
                incoming: None,
            },
            Arc::new(webdock_control_plane::mail::MailProviderPolicy::new(
                host.clone(),
            )),
        )
        .await
        .unwrap(),
    );
    let queue = Arc::new(Submissions::new(deps, KEY, true).await.unwrap());
    let managed = Arc::new(ManagedControl::new(
        sender.clone(),
        queue.clone(),
        host.clone(),
    ));
    let socket_path = dir.path().join("core.sock");
    let mut socket = webdock_plugin_runtime::UnixSocket::bind(&socket_path).unwrap();
    let incoming = socket.incoming().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let socket_managed = managed.clone();
    let socket_host = host.clone();
    let task = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(socket_host.rpc())
            .add_service(socket_managed.service())
            .add_service(sender.service())
            .serve_with_incoming_shutdown(incoming, async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
        drop(socket);
    });
    let channel = webdock_plugin_runtime::unix_channel(&socket_path)
        .await
        .unwrap();
    let mut control = NativeControlClient::new(channel.clone(), 1).unwrap();
    let native_token = format!("wd_mail_{}", "t".repeat(43));
    let evidence = json!({"id":"domain-fixture","domain":"example.test","spf_verified":true,"dkim_verified":true,"dmarc_verified":true});
    let mut txt_only = evidence.clone();
    txt_only["spf_verified"] = json!("true");
    sql(
        owner,
        "UPDATE webdock_auth.mail_sender_domain SET provider_snapshot=$1 WHERE id='301'",
        vec![txt_only.into()],
    )
    .await;
    let input = json!({"resource_id":"domain-1","from":"sender","to":"target@example.test","subject":"Safe subject","text":"Private body"});
    let admit = |action: &str, input: Value, key: &str| AdmissionInput {
        plugin_id: "mail".into(),
        project_id: "234".into(),
        action: action.into(),
        input,
        idempotency_key: key.into(),
    };
    let invocation = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        host.admit_with_authority(
            &principal,
            admit("mail.send", input.clone(), "stable-send"),
            true,
            &webdock_browser_session::StoredAuthority::LocalSession {},
        ),
    )
    .await
    .expect("original admission must not reacquire worker1")
    .unwrap();
    let submit = |token: &str| json!({"action":"send.admitAction","submission":{"invocationToken":token,"input":input,"payloadRef":uuid::Uuid::new_v4().to_string(),"privatePolicySha256":"c".repeat(64)}});
    let staged = tokio::time::timeout(
        std::time::Duration::from_secs(8),
        control.call(submit(&invocation.token)),
    )
    .await
    .expect("auth1/worker1 lock order")
    .unwrap();
    assert_eq!(staged["state"], "staging");
    let again = host
        .admit_with_authority(
            &principal,
            admit("mail.send", input.clone(), "stable-send"),
            true,
            &webdock_browser_session::StoredAuthority::LocalSession {},
        )
        .await
        .unwrap();
    assert_ne!(invocation.claims.id, again.claims.id);
    for count in [20_000, 30_000] {
        let mut large = input.clone();
        large["text"] = json!("😀".repeat(count));
        let large_admission = host
            .admit_with_authority(
                &principal,
                admit("mail.send", large.clone(), &format!("large-{count}")),
                true,
                &webdock_browser_session::StoredAuthority::LocalSession {},
            )
            .await
            .unwrap();
        let mut host_rpc = webdock_plugin_runtime::wire::plugin_host_client::PluginHostClient::new(
            channel.clone(),
        );
        assert!(
            host_rpc
                .authorize_request(webdock_plugin_runtime::wire::HostPayloadRequest {
                    claims_json: serde_json::to_vec(&large_admission.claims).unwrap(),
                    input_json: serde_json::to_vec(&large).unwrap(),
                    configuration_json: serde_json::to_vec(&large_admission.configuration).unwrap()
                })
                .await
                .unwrap()
                .into_inner()
                .active
        );
        let stored=control.call(json!({"action":"send.admitAction","submission":{"invocationToken":large_admission.token,"input":large,"payloadRef":uuid::Uuid::new_v4().to_string(),"privatePolicySha256":"c".repeat(64)}})).await.unwrap();
        assert_eq!(stored["state"], "staging");
        assert_eq!(control.call(json!({"action":"send.abortPayload","lease":{"id":stored["id"],"customerID":stored["customerID"],"payloadRef":stored["payloadRef"],"uploadToken":stored["uploadToken"]}})).await.unwrap()["aborted"],true);
    }
    let mut over_input = input.clone();
    over_input["text"] = json!("x".repeat(131073));
    assert_eq!(control.call(json!({"action":"send.admitAction","submission":{"invocationToken":again.token,"input":over_input,"payloadRef":uuid::Uuid::new_v4().to_string(),"privatePolicySha256":"c".repeat(64)}})).await.unwrap_err().status,400);
    for bytes in [16384, 16385] {
        assert_eq!(control.call(json!({"action":"send.admitAction","submission":{"invocationToken":"x".repeat(bytes),"input":input,"payloadRef":uuid::Uuid::new_v4().to_string(),"privatePolicySha256":"c".repeat(64)}})).await.unwrap_err().status,403);
    }
    let replay = control.call(submit(&again.token)).await.unwrap();
    assert_eq!(replay["id"], staged["id"]);
    for field in ["customerID", "payloadRef", "uploadToken"] {
        assert_eq!(
            replay[field], staged[field],
            "lost reply staging replay retains exact upload custody: {field}"
        );
    }
    assert!(
        queue
            .execute(QueueCommand::Claim {})
            .await
            .unwrap()
            .is_null()
    );
    let committed=control.call(json!({"action":"send.commitPayload","lease":{"id":staged["id"],"customerID":"123","payloadRef":staged["payloadRef"],"uploadToken":staged["uploadToken"]}})).await.unwrap();
    assert_eq!(committed["queued"], true);
    assert!(
        queue
            .execute(QueueCommand::Claim {})
            .await
            .unwrap()
            .is_null()
    );
    let mut lease: SendLease =
        serde_json::from_value(control.call(json!({"action":"send.claim"})).await.unwrap())
            .unwrap();
    assert_eq!(lease.source_context["executor"], "managed-mail-v1");
    assert_eq!(lease.source_context["subject"], "Safe subject");
    assert!(!lease.source_context.to_string().contains("Private body"));
    let old_claim = lease.clone();
    sql(
        owner,
        "UPDATE webdock_mail.send_job SET lease_until=now()-interval '1 second' WHERE id=$1",
        vec![lease.id.clone().into()],
    )
    .await;
    lease = serde_json::from_value(control.call(json!({"action":"send.claim"})).await.unwrap())
        .unwrap();
    assert_eq!(old_claim.id, lease.id);
    assert_ne!(old_claim.lease_token, lease.lease_token);
    assert!(control.call(json!({"action":"send.begin","lease":old_claim,"digest":"d".repeat(64),"preparation":{"metadataSha256":"e".repeat(64),"mimeSha256":"f".repeat(64),"mimeBytes":100}})).await.unwrap().is_null());
    let mut forged = lease.clone();
    forged.envelope["from"] = json!("forged@other.test");
    assert!(control.call(json!({"action":"send.begin","lease":forged,"digest":"d".repeat(64),"preparation":{"metadataSha256":"e".repeat(64),"mimeSha256":"f".repeat(64),"mimeBytes":100}})).await.is_err());
    sql(owner,"UPDATE webdock_auth.session SET \"expiresAt\"=now()-interval '1 second' WHERE id='session'",vec![]).await;
    let began=control.call(json!({"action":"send.begin","lease":lease,"digest":"d".repeat(64),"preparation":{"metadataSha256":"e".repeat(64),"mimeSha256":"f".repeat(64),"mimeBytes":100}})).await.unwrap();
    assert_eq!(began["kind"], "direct");
    sql(
        owner,
        "UPDATE webdock_auth.session SET \"expiresAt\"=now()+interval '1 hour' WHERE id='session'",
        vec![],
    )
    .await;
    let mut callback =
        webdock_mail_protocol::wire::mail_authority_client::MailAuthorityClient::new(channel);
    let marker: Value = serde_json::from_slice(
        &callback
            .begin_attempt(webdock_mail_protocol::wire::LeaseProof {
                lease_token: began["leaseToken"].as_str().unwrap().into(),
            })
            .await
            .unwrap()
            .into_inner()
            .private_configuration_json,
    )
    .unwrap();
    assert_eq!(marker["kind"], "managed");
    assert_eq!(marker["transport"]["selection"], "project");
    assert_eq!(marker["transport"]["privatePolicySha256"], "c".repeat(64));
    assert!(!marker.to_string().contains("Private body"));
    assert!(
        callback
            .begin_attempt(webdock_mail_protocol::wire::LeaseProof {
                lease_token: began["leaseToken"].as_str().unwrap().into()
            })
            .await
            .is_err()
    );
    let mut forged_finish = lease.clone();
    forged_finish.source_context["payloadStoreID"] = json!("caller-store");
    assert!(
        control
            .call(
                json!({"action":"send.finish","lease":forged_finish,"result":{"state":"unknown"}})
            )
            .await
            .is_err()
    );
    sql(
        owner,
        "UPDATE webdock_mail.send_job SET lease_until=now()-interval '1 second' WHERE id=$1",
        vec![lease.id.clone().into()],
    )
    .await;
    assert!(
        control
            .call(json!({"action":"send.claim"}))
            .await
            .unwrap()
            .is_null()
    );
    assert_eq!(
        control
            .call(json!({"action":"send.finish","lease":lease,"result":{"state":"unknown"}}))
            .await
            .unwrap(),
        true
    );
    assert!(
        control
            .call(json!({"action":"send.claim"}))
            .await
            .unwrap()
            .is_null()
    );
    // The same fixed transition command derives domain deletion from the real original action.
    let transition_input = json!({"resource_id":"domain-1"});
    let delete = host
        .admit_with_authority(
            &principal,
            admit(
                "domains.delete",
                transition_input.clone(),
                "domain-delete-unknown",
            ),
            true,
            &webdock_browser_session::StoredAuthority::LocalSession {},
        )
        .await
        .unwrap();
    assert!(control.call(json!({"action":"send.checkTransition","invocationToken":delete.token,"input":transition_input})).await.is_err());
    let status_input = json!({"resource_id":"domain-1","id":staged["id"]});
    let cancel = host
        .admit_with_authority(
            &principal,
            admit("mail.cancel", status_input.clone(), "cancel"),
            true,
            &webdock_browser_session::StoredAuthority::LocalSession {},
        )
        .await
        .unwrap();
    let state=control.call(json!({"action":"send.actionStatus","invocationToken":cancel.token,"input":status_input})).await.unwrap();
    assert_eq!(state, json!({"id":staged["id"],"state":"unknown"}));
    let history_input = json!({"resource_id":"domain-1"});
    let hist = host
        .admit_with_authority(
            &principal,
            admit("mail.history", history_input.clone(), "history"),
            true,
            &webdock_browser_session::StoredAuthority::LocalSession {},
        )
        .await
        .unwrap();
    let history=control.call(json!({"action":"send.actionStatus","invocationToken":hist.token,"input":history_input})).await.unwrap();
    assert_eq!(history["items"].as_array().unwrap().len(), 3);
    assert_eq!(history["items"][0]["subject"], "Safe subject");
    assert!(history["items"][0].get("payloadRef").is_none());
    assert_eq!(control.call(json!({"action":"send.finish","lease":lease,"result":{"state":"accepted","providerID":"receipt_1"}})).await.unwrap(),true);
    assert_eq!(control.call(json!({"action":"send.finish","lease":lease,"result":{"state":"accepted","providerID":"receipt_1"}})).await.unwrap(),true);
    assert!(
        control
            .call(json!({"action":"send.finish","lease":lease,"result":{"state":"rejected"}}))
            .await
            .is_err()
            || control
                .call(json!({"action":"send.finish","lease":lease,"result":{"state":"rejected"}}))
                .await
                .unwrap()
                == false
    );
    let delete = host
        .admit_with_authority(
            &principal,
            admit(
                "domains.delete",
                json!({"resource_id":"domain-1"}),
                "domain-delete-settled",
            ),
            true,
            &webdock_browser_session::StoredAuthority::LocalSession {},
        )
        .await
        .unwrap();
    assert_eq!(control.call(json!({"action":"send.checkTransition","invocationToken":delete.token,"input":{"resource_id":"domain-1"}})).await.unwrap(),json!({"allowed":true}));
    // Confirmed actor revocation cancels pre-handoff; infrastructure failure must not do this.
    let next = host
        .admit_with_authority(
            &principal,
            admit("mail.send", input.clone(), "revoked-actor"),
            true,
            &webdock_browser_session::StoredAuthority::LocalSession {},
        )
        .await
        .unwrap();
    let staged2 = control.call(submit(&next.token)).await.unwrap();
    control.call(json!({"action":"send.commitPayload","lease":{"id":staged2["id"],"customerID":"123","payloadRef":staged2["payloadRef"],"uploadToken":staged2["uploadToken"]}})).await.unwrap();
    let lease2: SendLease =
        serde_json::from_value(control.call(json!({"action":"send.claim"})).await.unwrap())
            .unwrap();
    sql(owner,"ALTER FUNCTION webdock_rls.managed_mail_source(jsonb) RENAME TO managed_mail_source_fixture_hold",vec![]).await;
    assert_eq!(control.call(json!({"action":"send.begin","lease":lease2,"digest":"d".repeat(64),"preparation":{"metadataSha256":"e".repeat(64),"mimeSha256":"f".repeat(64),"mimeBytes":100}})).await.unwrap_err().status,503);
    sql(owner,"ALTER FUNCTION webdock_rls.managed_mail_source_fixture_hold(jsonb) RENAME TO managed_mail_source",vec![]).await;
    let row = owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT state FROM webdock_mail.send_job WHERE id=$1",
            [lease2.id.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.try_get::<String>("", "state").unwrap(), "leased");
    sql(
        owner,
        "UPDATE webdock_auth.\"user\" SET role='user' WHERE id='actor'",
        vec![],
    )
    .await;
    assert!(control.call(json!({"action":"send.begin","lease":lease2,"digest":"d".repeat(64),"preparation":{"metadataSha256":"e".repeat(64),"mimeSha256":"f".repeat(64),"mimeBytes":100}})).await.unwrap().is_null());
    let row = owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT state FROM webdock_mail.send_job WHERE id=$1",
            [lease2.id.into()],
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.try_get::<String>("", "state").unwrap(), "cancelled");
    sql(
        owner,
        "UPDATE webdock_auth.\"user\" SET role='operator' WHERE id='actor'",
        vec![],
    )
    .await;
    // Native key cancellation retains exact-key scope and never exposes action summaries.
    assert_eq!(
        control
            .call(
                json!({"action":"send.authenticate","token":format!("wd_mail_{}","z".repeat(43))})
            )
            .await
            .unwrap_err()
            .status,
        403
    );
    for field in ["spf_verified", "dkim_verified", "dmarc_verified"] {
        for bad in [json!(false), json!("true"), Value::Null] {
            let mut changed = evidence.clone();
            changed[field] = bad;
            sql(
                owner,
                "UPDATE webdock_auth.mail_sender_domain SET provider_snapshot=$1 WHERE id='301'",
                vec![changed.into()],
            )
            .await;
            assert_eq!(control.call(json!({"action":"send.preflight","token":native_token,"envelope":{"from":"sender@example.test","to":["target@example.test"]}})).await.unwrap_err().status,403);
        }
    }
    for (field, value) in [("id", "wrong-provider"), ("domain", "wrong.example.test")] {
        let mut changed = evidence.clone();
        changed[field] = json!(value);
        sql(
            owner,
            "UPDATE webdock_auth.mail_sender_domain SET provider_snapshot=$1 WHERE id='301'",
            vec![changed.into()],
        )
        .await;
        assert_eq!(control.call(json!({"action":"send.preflight","token":native_token,"envelope":{"from":"sender@example.test","to":["target@example.test"]}})).await.unwrap_err().status,403);
    }
    sql(
        owner,
        "UPDATE webdock_auth.mail_sender_domain SET provider_snapshot=$1 WHERE id='301'",
        vec![evidence.into()],
    )
    .await;

    let native=control.call(json!({"action":"send.admit","submission":{"token":native_token,"idempotencyKey":"native-cancel","requestHash":"a".repeat(64),"payloadRef":uuid::Uuid::new_v4().to_string(),"payloadBytes":100,"envelope":{"from":"sender@example.test","to":["target@example.test"]}}})).await.unwrap();
    assert_eq!(
        control
            .call(
                json!({"action":"send.status","token":native_token,"id":native["id"],"cancel":true})
            )
            .await
            .unwrap()["state"],
        "cancelled"
    );
    assert_eq!(
        control
            .call(
                json!({"action":"send.status","token":native_token,"id":native["id"],"cancel":true})
            )
            .await
            .unwrap()["state"],
        "cancelled"
    );
    assert_eq!(
        control
            .call(
                json!({"action":"send.status","token":native_token,"id":staged["id"],"cancel":true})
            )
            .await
            .unwrap_err()
            .status,
        403
    );
    // Draining permits accepted bookkeeping and claims, but rejects new admission.
    sql(
        owner,
        "UPDATE webdock_auth.plugin_installation SET state='draining' WHERE id='mail'",
        vec![],
    )
    .await;
    assert!(
        control
            .call(json!({"action":"send.claim"}))
            .await
            .unwrap()
            .is_null()
    );
    assert!(control.call(json!({"action":"send.admit","submission":{"token":native_token,"idempotencyKey":"drain-deny","requestHash":"a".repeat(64),"payloadRef":uuid::Uuid::new_v4().to_string(),"payloadBytes":100,"envelope":{"from":"sender@example.test","to":["target@example.test"]}}})).await.is_err());
    sql(
        owner,
        "UPDATE webdock_auth.plugin_installation SET state='disabled' WHERE id='mail'",
        vec![],
    )
    .await;
    assert!(control.call(json!({"action":"send.claim"})).await.is_err());
    assert_eq!(control.call(json!({"action":"send.finish","lease":lease,"result":{"state":"accepted","providerID":"receipt_1"}})).await.unwrap(),true);
    sql(owner,"UPDATE webdock_mail.send_job SET finished_at=now()-interval '2 minutes' WHERE state IN ('accepted','rejected','cancelled')",vec![]).await;
    assert_eq!(
        queue.execute(QueueCommand::Cleanup {}).await.unwrap(),
        json!([])
    );
    let cleanup = control
        .call(json!({"action":"send.cleanup"}))
        .await
        .unwrap();
    assert_eq!(cleanup.as_array().unwrap().len(), 5);
    control
        .call(json!({"action":"send.cleanupDone","items":cleanup}))
        .await
        .unwrap();
    assert_eq!(
        control
            .call(json!({"action":"send.cleanup"}))
            .await
            .unwrap(),
        json!([])
    );
    sql(
        owner,
        "UPDATE webdock_auth.plugin_installation SET state='enabled' WHERE id='mail'",
        vec![],
    )
    .await;
    // A valid hash of a different manifest is not this enrolled release identity.
    let mut wrong_manifest = manifest.clone();
    wrong_manifest["name"] = json!("wrong-mail");
    let wrong_raw = wrong_manifest.to_string();
    sql(owner,"UPDATE webdock_auth.plugin_release SET manifest_bytes=$1,manifest_sha256=$2 WHERE plugin_id='mail'",vec![wrong_raw.clone().into(),webdock_mail_protocol::sha256(wrong_raw.as_bytes()).into()]).await;
    assert_eq!(
        control
            .call(json!({"action":"send.claim"}))
            .await
            .unwrap_err()
            .status,
        403
    );
    sql(owner,"UPDATE webdock_auth.plugin_release SET manifest_bytes=$1,manifest_sha256=$2 WHERE plugin_id='mail'",vec![raw.clone().into(),webdock_mail_protocol::sha256(raw.as_bytes()).into()]).await;
    let stale = NativeControlClient::new(
        webdock_plugin_runtime::unix_channel(&socket_path)
            .await
            .unwrap(),
        2,
    )
    .unwrap();
    assert_eq!(
        stale
            .call(json!({"action":"send.claim"}))
            .await
            .unwrap_err()
            .status,
        403
    );
    // A different actual Unix peer pin invalidates an already-open channel.
    let mut changed = bindings.clone();
    changed["runtimes"]["mail"]["peers"][0]["uid"] = json!(rustix::process::geteuid().as_raw() + 1);
    std::fs::write(&path, changed.to_string()).unwrap();
    assert_eq!(
        control
            .call(json!({"action":"send.claim"}))
            .await
            .unwrap_err()
            .status,
        403
    );
    std::fs::write(&path, bindings.to_string()).unwrap();
    fix1_mcp_retained_jobs(owner, &host, &worker, &control, &mcp_actors).await;
    // Real restricted source predicate: delegated resource-grant revocation retracts accepted intent.
    sql(owner,"INSERT INTO webdock_auth.\"oauthClient\"(id,\"clientId\",name,disabled,\"skipConsent\",scopes,\"redirectUris\",metadata,\"createdAt\",\"updatedAt\") VALUES('mcp-client','mail-mcp','Fixture',false,false,'[\"webdock:read\",\"webdock:write\"]','[\"https://client.invalid/callback\"]','{\"webdock_mcp\":true}',now(),now()); INSERT INTO webdock_auth.\"oauthConsent\"(id,\"clientId\",\"userId\",scopes,resources,\"createdAt\",\"updatedAt\") VALUES('mcp-consent','mail-mcp','actor','[\"webdock:read\",\"webdock:write\"]','[\"https://mcp.invalid/resource\"]',now(),now()); INSERT INTO webdock_auth.\"oauthResource\"(id,identifier,name,\"allowedScopes\",disabled) VALUES('mcp-resource','https://mcp.invalid/resource','Fixture','[\"webdock:read\",\"webdock:write\"]',false); INSERT INTO webdock_auth.\"oauthClientResource\"(id,\"clientId\",\"resourceId\") VALUES('mcp-link','mail-mcp','https://mcp.invalid/resource')",vec![]).await;
    let consent: Value = owner
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT to_jsonb(c) AS c FROM webdock_auth.\"oauthConsent\" c WHERE id='mcp-consent'",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "c")
        .unwrap();
    let client: Value = owner
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT to_jsonb(c) AS c FROM webdock_auth.\"oauthClient\" c WHERE id='mcp-client'",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "c")
        .unwrap();
    let mut delegated = lease.source_context.clone();
    delegated["authorityLineage"] = json!({"source":"mcp","callerID":"mail-mcp","issuerClientID":"mail-mcp","resource":"https://mcp.invalid/resource","writeScope":"webdock:write","consentID":"mcp-consent","consentRevision":consent["updatedAt"],"callerRevision":client["updatedAt"],"issuerRevision":client["updatedAt"],"skipConsent":false,"callerResourceGrantID":"mcp-link","issuerResourceGrantID":"mcp-link"});
    assert!(source_allowed(&host, &worker, &delegated).await);
    sql(
        owner,
        "DELETE FROM webdock_auth.\"oauthClientResource\" WHERE id='mcp-link'",
        vec![],
    )
    .await;
    assert!(!source_allowed(&host, &worker, &delegated).await);
    sql(owner,"INSERT INTO webdock_auth.\"oauthClientResource\"(id,\"clientId\",\"resourceId\") VALUES('mcp-link-2','mail-mcp','https://mcp.invalid/resource')",vec![]).await;
    assert!(
        !source_allowed(&host, &worker, &delegated).await,
        "regrant must not revive the accepted original row references"
    );
    delegated["authorityLineage"]["callerResourceGrantID"] = json!("mcp-link-2");
    delegated["authorityLineage"]["issuerResourceGrantID"] = json!("mcp-link-2");
    assert!(source_allowed(&host, &worker, &delegated).await); // predicate-only new references; actual constructor tested below
    sql(
        owner,
        "UPDATE webdock_auth.\"oauthClient\" SET disabled=true WHERE id='mcp-client'",
        vec![],
    )
    .await;
    assert!(!source_allowed(&host, &worker, &delegated).await);
    sql(owner,"UPDATE webdock_auth.\"oauthClient\" SET disabled=false WHERE id='mcp-client'; DELETE FROM webdock_auth.\"oauthConsent\" WHERE id='mcp-consent'",vec![]).await;
    assert!(!source_allowed(&host, &worker, &delegated).await);
    sql(owner,"INSERT INTO webdock_auth.app_binding(id,client_id,label,enabled) VALUES('browser-binding','mail-mcp','Fixture',true); UPDATE webdock_auth.\"oauthClient\" SET scopes='[\"openid\",\"profile\",\"email\"]',\"redirectUris\"='[\"https://console.invalid/api/sso/callback\"]',metadata='{\"webdock_binding\":\"browser-binding\"}' WHERE id='mcp-client'",vec![]).await;
    let mut browser_source = lease.source_context.clone();
    browser_source["authorityLineage"] = json!({"source":"browser","clientID":"mail-mcp","clientRevision":client["updatedAt"],"bindingID":"browser-binding","appOrigin":"https://console.invalid"});
    assert!(source_allowed(&host, &worker, &browser_source).await);
    sql(
        owner,
        "UPDATE webdock_auth.app_binding SET enabled=false WHERE id='browser-binding'",
        vec![],
    )
    .await;
    assert!(!source_allowed(&host, &worker, &browser_source).await);
    sql(owner,"INSERT INTO webdock_auth.\"user\"(id,name,email,\"emailVerified\",role,\"mustChangePassword\") VALUES('member-actor','Member','member@example.invalid',true,'user',false); INSERT INTO webdock_auth.member(id,\"organizationId\",\"userId\",role,\"createdAt\") VALUES('member-row','124','member-actor','admin',now())",vec![]).await;
    let mut member_source = lease.source_context.clone();
    member_source["actorID"] = json!("member-actor");
    member_source["actorKind"] = json!("member");
    member_source["memberID"] = json!("member-row");
    assert!(source_allowed(&host, &worker, &member_source).await);
    for (role, two_factor) in [("disabled-role", true), ("operator", false)] {
        sql(owner,"UPDATE webdock_auth.\"user\" SET role=$1,\"twoFactorEnabled\"=$2 WHERE id='member-actor'",vec![role.into(),two_factor.into()]).await;
        assert!(
            !source_allowed(&host, &worker, &member_source).await,
            "membership alone cannot bypass current identity eligibility"
        );
    }
    sql(owner,"UPDATE webdock_auth.\"user\" SET role='user',\"twoFactorEnabled\"=false WHERE id='member-actor'",vec![]).await;

    sql(
        owner,
        "UPDATE webdock_auth.member SET role='member' WHERE id='member-row'",
        vec![],
    )
    .await;
    assert!(!source_allowed(&host, &worker, &member_source).await);
    sql(owner,"UPDATE webdock_auth.plugin_resource SET name='changed.example.test' WHERE resource_id='domain-1'",vec![]).await;
    assert!(!source_allowed(&host, &worker, &lease.source_context).await);
    sql(
        owner,
        "UPDATE webdock_auth.plugin_resource SET name='example.test' WHERE resource_id='domain-1'",
        vec![],
    )
    .await;
    exercise_mtls(managed.clone(), &path, &bindings).await;
    control = fix1_terminal_generation(
        owner,
        managed.clone(),
        &path,
        &bindings,
        &socket_path,
        &control,
        &native_token,
    )
    .await;
    let mode_job=control.call(json!({"action":"send.admit","submission":{"token":native_token,"idempotencyKey":"mode-pin","requestHash":"a".repeat(64),"payloadRef":uuid::Uuid::new_v4().to_string(),"payloadBytes":100,"envelope":{"from":"sender@example.test","to":["target@example.test"]}}})).await.unwrap();
    control.call(json!({"action":"send.commitPayload","lease":{"id":mode_job["id"],"customerID":mode_job["customerID"],"payloadRef":mode_job["payloadRef"],"uploadToken":mode_job["uploadToken"]}})).await.unwrap();
    let mode_lease: SendLease =
        serde_json::from_value(control.call(json!({"action":"send.claim"})).await.unwrap())
            .unwrap();
    sql(owner,"UPDATE webdock_mail.send_transport SET mode='byok',byok_revision=1 WHERE customer_id='123'",vec![]).await;
    assert!(control.call(json!({"action":"send.begin","lease":mode_lease,"digest":"d".repeat(64),"preparation":{"metadataSha256":"e".repeat(64),"mimeSha256":"f".repeat(64),"mimeBytes":100}})).await.unwrap().is_null(),"mode/account changes cannot silently reroute an accepted native job");
    let row = owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT state FROM webdock_mail.send_job WHERE id=$1",
            [mode_lease.id.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.try_get::<String>("", "state").unwrap(),
        "preparation_review"
    );
    sql(owner,"UPDATE webdock_mail.send_transport SET mode='managed',byok_revision=NULL WHERE customer_id='123'",vec![]).await;
    assert!(
        control
            .call(json!({"action":"send.claim"}))
            .await
            .unwrap()
            .is_null()
    );
    let revoked_job=control.call(json!({"action":"send.admit","submission":{"token":native_token,"idempotencyKey":"key-revoke","requestHash":"a".repeat(64),"payloadRef":uuid::Uuid::new_v4().to_string(),"payloadBytes":100,"envelope":{"from":"sender@example.test","to":["target@example.test"]}}})).await.unwrap();
    control.call(json!({"action":"send.commitPayload","lease":{"id":revoked_job["id"],"customerID":revoked_job["customerID"],"payloadRef":revoked_job["payloadRef"],"uploadToken":revoked_job["uploadToken"]}})).await.unwrap();
    let revoked_lease: SendLease =
        serde_json::from_value(control.call(json!({"action":"send.claim"})).await.unwrap())
            .unwrap();
    sql(
        owner,
        "UPDATE webdock_mail.api_key SET revoked_at=now() WHERE id='456'",
        vec![],
    )
    .await;
    assert!(control.call(json!({"action":"send.begin","lease":revoked_lease,"digest":"d".repeat(64),"preparation":{"metadataSha256":"e".repeat(64),"mimeSha256":"f".repeat(64),"mimeBytes":100}})).await.unwrap().is_null());
    let row = owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT state FROM webdock_mail.send_job WHERE id=$1",
            [revoked_lease.id.into()],
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.try_get::<String>("", "state").unwrap(), "cancelled");
    fix1_unsupported_versions(owner, &control, &queue).await;
    sql(
        owner,
        "UPDATE webdock_auth.plugin_installation SET schema_high_water=2 WHERE id='mail'",
        vec![],
    )
    .await;
    assert!(
        control.call(json!({"action":"send.claim"})).await.is_err(),
        "schema1-only runtime cannot use custody after schema2 high-water reservation"
    );
    assert!(
        owner
            .execute_unprepared(
                "UPDATE webdock_auth.plugin_installation SET schema_high_water=1 WHERE id='mail'"
            )
            .await
            .is_err()
    );
    stop.send(()).unwrap();
    task.await.unwrap();
    drop(control);
    drop(callback);
    drop(host);
    drop(queue);
    auth.close().await.unwrap();
    business.close().await.unwrap();
    worker.close().await.unwrap();
    resolver.close().await.unwrap();
}

async fn exercise_mtls(
    managed: std::sync::Arc<webdock_mail::managed_control::ManagedControl>,
    path: &std::path::Path,
    bindings: &Value,
) {
    use webdock_mail_protocol::native_control::NativeControlClient;
    let mut parameters = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    parameters.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    parameters.key_usages = vec![
        rcgen::KeyUsagePurpose::KeyCertSign,
        rcgen::KeyUsagePurpose::DigitalSignature,
    ];
    let key = rcgen::KeyPair::generate().unwrap();
    let ca = parameters.self_signed(&key).unwrap();
    let issuer = rcgen::Issuer::new(parameters, key);
    let issue = |server| {
        let mut parameters = rcgen::CertificateParams::new(vec!["localhost".into()]).unwrap();
        parameters.extended_key_usages = vec![if server {
            rcgen::ExtendedKeyUsagePurpose::ServerAuth
        } else {
            rcgen::ExtendedKeyUsagePurpose::ClientAuth
        }];
        let key = rcgen::KeyPair::generate().unwrap();
        (parameters.signed_by(&key, &issuer).unwrap(), key)
    };
    let (server, server_key) = issue(true);
    let (client, client_key) = issue(false);
    let mut enrollment = bindings.clone();
    enrollment["runtimes"]["mail"]["peers"].as_array_mut().unwrap().push(json!({"kind":"mutualTls","certificateSha256":webdock_mail_protocol::sha256(client.der())}));
    std::fs::write(path, enrollment.to_string()).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!(
        "https://127.0.0.1:{}",
        listener.local_addr().unwrap().port()
    );
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let ca_pem = ca.pem();
    let tls = tonic::transport::ServerTlsConfig::new()
        .identity(tonic::transport::Identity::from_pem(
            server.pem(),
            server_key.serialize_pem(),
        ))
        .client_ca_root(tonic::transport::Certificate::from_pem(ca_pem.clone()));
    let task = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .tls_config(tls)
            .unwrap()
            .add_service(managed.service())
            .serve_with_incoming_shutdown(
                tokio_stream::wrappers::TcpListenerStream::new(listener),
                async {
                    let _ = stopped.await;
                },
            )
            .await
            .unwrap();
    });
    let config = |pin: String| webdock_plugin_runtime::PinnedMutualTls {
        certificate: client.pem().into_bytes(),
        private_key: client_key.serialize_pem().into_bytes(),
        authority: ca_pem.clone().into_bytes(),
        server_certificate_sha256: pin,
    };
    let pin = webdock_mail_protocol::sha256(server.der());
    assert!(
        webdock_plugin_runtime::pinned_mtls_channel(
            origin.clone(),
            "localhost".into(),
            config("0".repeat(64))
        )
        .await
        .is_err()
    );
    let channel =
        webdock_plugin_runtime::pinned_mtls_channel(origin, "localhost".into(), config(pin))
            .await
            .unwrap();
    let control = NativeControlClient::new(channel, 1).unwrap();
    assert!(
        control
            .call(json!({"action":"send.claim"}))
            .await
            .unwrap()
            .is_null()
    );
    std::fs::write(path, bindings.to_string()).unwrap();
    assert_eq!(
        control
            .call(json!({"action":"send.claim"}))
            .await
            .unwrap_err()
            .status,
        403
    );
    drop(control);
    stop.send(()).unwrap();
    task.await.unwrap();
}

async fn source_allowed(
    host: &webdock_plugin_host::HostService,
    worker: &sea_orm::DatabaseConnection,
    source: &Value,
) -> bool {
    let tx = worker.begin().await.unwrap();
    let raw=json!({"v":1,"aud":"webdock_worker_runtime","exp":chrono::Utc::now().timestamp()+60,"scope":{"kind":"system","purpose":"mail-send-gateway"}}).to_string();
    let mut mac = Hmac::<Sha256>::new_from_slice(KEY.as_bytes()).unwrap();
    mac.update(raw.as_bytes());
    sql(&tx,"SELECT set_config('webdock.rls_context',$1,true),set_config('webdock.rls_signature',$2,true)",vec![raw.into(),hex::encode(mac.finalize().into_bytes()).into()]).await;
    let allowed = host.mail_source_in(&tx, source).await.unwrap();
    tx.rollback().await.unwrap();
    allowed
}

async fn fix1_mcp_setup(
    owner: &sea_orm::DatabaseConnection,
    auth: sea_orm::DatabaseConnection,
    identity: std::sync::Arc<webdock_identity::IdentityService>,
    ids: std::sync::Arc<webdock_persistence::IdAllocator>,
    cookie: &str,
) -> (
    std::sync::Arc<webdock_oauth::OAuthService>,
    Vec<webdock_oauth::McpAuthentication>,
) {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
    use sha2::Digest;
    use webdock_oauth::{
        AuthorizationOutcome, AuthorizationRequest, ClientAuthMethod, ClientAuthentication,
        ClientPurpose, ClientRegistration, CodeExchange, OAuthConfig, OAuthService,
    };
    let secret = uuid::Uuid::new_v4().to_string();
    let keys = webdock_identity::SecretKeys::new(
        std::collections::BTreeMap::from([(0, secret.clone()), (1, secret.clone())]),
        1,
        Some(secret),
    )
    .unwrap();
    let oauth = std::sync::Arc::new(OAuthService::new(
        auth,
        identity,
        ids,
        std::sync::Arc::new(keys),
        OAuthConfig::new(
            "http://localhost:3121/api/auth",
            "http://localhost:3120/api/mcp",
            None,
        )
        .unwrap(),
    ));
    oauth.provision_signing_key(owner).await.unwrap();
    let register = |name: &str| ClientRegistration {
        name: name.into(),
        redirect_uris: vec!["https://fix1.invalid/callback".into()],
        post_logout_redirect_uris: vec![],
        scopes: vec![
            "openid".into(),
            "webdock:read".into(),
            "webdock:write".into(),
        ],
        auth_method: ClientAuthMethod::ClientSecretPost,
        allow_refresh: false,
        skip_consent: false,
        purpose: ClientPurpose::McpOperator,
    };
    let issuer = oauth
        .provision_client(owner, register("fix1 issuer"))
        .await
        .unwrap();
    let caller = oauth
        .provision_client(owner, register("fix1 caller"))
        .await
        .unwrap();
    let credentials = |client: &webdock_oauth::RegisteredClient| ClientAuthentication {
        client_id: client.client_id.clone(),
        client_secret: client.client_secret.clone(),
        method: ClientAuthMethod::ClientSecretPost,
    };
    let verifier = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ-._~";
    let challenge = B64.encode(sha2::Sha256::digest(verifier.as_bytes()));
    let mut request = AuthorizationRequest::code(
        issuer.client_id.clone(),
        "https://fix1.invalid/callback".into(),
        "openid webdock:read webdock:write".into(),
        challenge,
    );
    request.resources = vec!["http://localhost:3120/api/mcp".into()];
    let outcome = oauth.authorize(cookie, request).await.unwrap();
    let outcome = match outcome {
        AuthorizationOutcome::ConsentRequired(consent) => oauth
            .consent(cookie, &consent.request_id, true)
            .await
            .unwrap(),
        v => v,
    };
    let AuthorizationOutcome::Redirect(location) = outcome else {
        panic!("fixture authorization did not redirect")
    };
    let code = url::Url::parse(&location)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "code")
        .unwrap()
        .1
        .into_owned();
    let token = oauth
        .exchange_code(CodeExchange {
            client: credentials(&issuer),
            code,
            redirect_uri: "https://fix1.invalid/callback".into(),
            code_verifier: verifier.into(),
        })
        .await
        .unwrap();
    let same = oauth
        .authorize_mcp_reference(&credentials(&issuer), &token.access_token, "webdock:write")
        .await
        .unwrap();
    let distinct = oauth
        .authorize_mcp_reference(&credentials(&caller), &token.access_token, "webdock:write")
        .await
        .unwrap();
    (oauth, vec![same, distinct])
}

async fn fix1_mcp_retained_jobs(
    owner: &sea_orm::DatabaseConnection,
    host: &webdock_plugin_host::HostService,
    worker: &sea_orm::DatabaseConnection,
    control: &webdock_mail_protocol::native_control::NativeControlClient,
    actors: &[webdock_oauth::McpAuthentication],
) {
    use webdock_mail_protocol::outbox::SendLease;
    for (case, actor, side) in [
        ("same", &actors[0], "caller"),
        ("caller", &actors[1], "caller"),
        ("issuer", &actors[1], "issuer"),
    ] {
        let input = json!({"resource_id":"domain-1","from":"sender","to":"target@example.test","subject":"MCP durable intent","text":"Private fixture body"});
        let admit = |key: &str| webdock_plugin_host::AdmissionInput {
            plugin_id: "mail".into(),
            project_id: "234".into(),
            action: "mail.send".into(),
            input: input.clone(),
            idempotency_key: key.into(),
        };
        let invocation = host
            .admit_with_mcp_authority(
                &actor.principal,
                admit(&format!("fix1-{case}-old")),
                true,
                &actor.authority,
            )
            .await
            .unwrap();
        let staged=control.call(json!({"action":"send.admitAction","submission":{"invocationToken":invocation.token,"input":input,"payloadRef":uuid::Uuid::new_v4().to_string(),"privatePolicySha256":"c".repeat(64)}})).await.unwrap();
        control.call(json!({"action":"send.commitPayload","lease":{"id":staged["id"],"customerID":staged["customerID"],"payloadRef":staged["payloadRef"],"uploadToken":staged["uploadToken"]}})).await.unwrap();
        let lease: SendLease =
            serde_json::from_value(control.call(json!({"action":"send.claim"})).await.unwrap())
                .unwrap();
        let context: &Value = &lease.source_context["authorityLineage"];
        assert_eq!(context["source"], "mcp");
        let mut originals = std::collections::BTreeMap::new();
        for kind in ["caller", "issuer"] {
            let client_field = if kind == "caller" {
                "callerID"
            } else {
                "issuerClientID"
            };
            let grant_field = if kind == "caller" {
                "callerResourceGrantID"
            } else {
                "issuerResourceGrantID"
            };
            let id:String=owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id FROM webdock_auth.\"oauthClientResource\" WHERE \"clientId\"=$1 AND \"resourceId\"=$2",[context[client_field].as_str().unwrap().into(),context["resource"].as_str().unwrap().into()])).await.unwrap().unwrap().try_get("","id").unwrap();
            assert_eq!(
                context[grant_field], id,
                "constructor must capture exact original grant row"
            );
            originals.insert(kind, id);
        }
        assert_eq!(originals["caller"] == originals["issuer"], case == "same");
        let id = &originals[side];
        sql(
            owner,
            "DELETE FROM webdock_auth.\"oauthClientResource\" WHERE id=$1",
            vec![id.clone().into()],
        )
        .await;
        let replacement = uuid::Uuid::new_v4().to_string();
        let client_field = if side == "caller" {
            "callerID"
        } else {
            "issuerClientID"
        };
        sql(owner,"INSERT INTO webdock_auth.\"oauthClientResource\"(id,\"clientId\",\"resourceId\") VALUES($1,$2,$3)",vec![replacement.clone().into(),context[client_field].as_str().unwrap().into(),context["resource"].as_str().unwrap().into()]).await;
        assert!(
            !source_allowed(host, worker, &lease.source_context).await,
            "delete/regrant must remain revoked"
        );
        assert!(control.call(json!({"action":"send.begin","lease":lease,"digest":"d".repeat(64),"preparation":{"metadataSha256":"e".repeat(64),"mimeSha256":"f".repeat(64),"mimeBytes":100}})).await.unwrap().is_null());
        let row=owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT j.state,b.state AS budget,(SELECT count(*)::bigint FROM webdock_mail.provider_lease l WHERE l.job_id=j.id) AS attempts FROM webdock_mail.send_job j JOIN webdock_mail.budget_reservation b ON b.reference_id=j.id WHERE j.id=$1",[lease.id.clone().into()])).await.unwrap().unwrap();
        assert_eq!(row.try_get::<String>("", "state").unwrap(), "cancelled");
        assert_eq!(row.try_get::<String>("", "budget").unwrap(), "released");
        assert_eq!(row.try_get::<i64>("", "attempts").unwrap(), 0);
        let fresh = host
            .admit_with_mcp_authority(
                &actor.principal,
                admit(&format!("fix1-{case}-fresh")),
                true,
                &actor.authority,
            )
            .await
            .unwrap();
        let staged=control.call(json!({"action":"send.admitAction","submission":{"invocationToken":fresh.token,"input":input,"payloadRef":uuid::Uuid::new_v4().to_string(),"privatePolicySha256":"c".repeat(64)}})).await.unwrap();
        let source: Value = owner
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT source_context FROM webdock_mail.send_job WHERE id=$1",
                [staged["id"].as_str().unwrap().into()],
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get("", "source_context")
            .unwrap();
        let grant_field = if side == "caller" {
            "callerResourceGrantID"
        } else {
            "issuerResourceGrantID"
        };
        assert_eq!(source["authorityLineage"][grant_field], replacement);
        assert!(source_allowed(host, worker, &source).await);
        control.call(json!({"action":"send.abortPayload","lease":{"id":staged["id"],"customerID":staged["customerID"],"payloadRef":staged["payloadRef"],"uploadToken":staged["uploadToken"]}})).await.unwrap();
    }
}

async fn fix1_terminal_generation(
    owner: &sea_orm::DatabaseConnection,
    managed: std::sync::Arc<webdock_mail::managed_control::ManagedControl>,
    path: &std::path::Path,
    bindings: &Value,
    socket: &std::path::Path,
    old: &webdock_mail_protocol::native_control::NativeControlClient,
    token: &str,
) -> webdock_mail_protocol::native_control::NativeControlClient {
    use std::{os::unix::fs::PermissionsExt, sync::Arc};
    use tower::ServiceExt;
    use webdock_mail_protocol::{native_control::NativeControlClient, outbox::SendLease};
    let private = tempfile::tempdir().unwrap();
    std::fs::set_permissions(private.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = Arc::new(
        webdock_mail_runtime::FilesystemPayloadStore::open(
            private.path(),
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        )
        .unwrap(),
    );
    let mut terminal = Vec::new();
    for state in ["accepted", "rejected", "cancelled"] {
        let staged=old.call(json!({"action":"send.admit","submission":{"token":token,"idempotencyKey":format!("fix1-terminal-{state}"),"requestHash":"a".repeat(64),"payloadRef":uuid::Uuid::new_v4().to_string(),"payloadBytes":100,"envelope":{"from":"sender@example.test","to":["target@example.test"]}}})).await.unwrap();
        let id = staged["id"].as_str().unwrap().to_owned();
        let reference = staged["payloadRef"].as_str().unwrap().to_owned();
        store
            .put("123", b"retained fixture bytes", Some(&reference))
            .unwrap();
        if state == "cancelled" {
            assert_eq!(
                old.call(json!({"action":"send.status","token":token,"id":id,"cancel":true}))
                    .await
                    .unwrap()["state"],
                state
            );
        } else {
            old.call(json!({"action":"send.commitPayload","lease":{"id":id,"customerID":"123","payloadRef":reference,"uploadToken":staged["uploadToken"]}})).await.unwrap();
            let lease: SendLease =
                serde_json::from_value(old.call(json!({"action":"send.claim"})).await.unwrap())
                    .unwrap();
            assert_eq!(lease.id, id);
            old.call(json!({"action":"send.begin","lease":lease,"digest":"d".repeat(64),"preparation":{"metadataSha256":"e".repeat(64),"mimeSha256":"f".repeat(64),"mimeBytes":100}})).await.unwrap();
            let receipt = if state == "accepted" {
                json!({"state":"accepted","providerID":"fixture_terminal"})
            } else {
                json!({"state":"rejected"})
            };
            assert_eq!(
                old.call(json!({"action":"send.finish","lease":lease,"result":receipt}))
                    .await
                    .unwrap(),
                true
            );
        }
        terminal.push((id, reference, state));
    }
    let staged=old.call(json!({"action":"send.admit","submission":{"token":token,"idempotencyKey":"fix1-old-nonterminal","requestHash":"a".repeat(64),"payloadRef":uuid::Uuid::new_v4().to_string(),"payloadBytes":100,"envelope":{"from":"sender@example.test","to":["target@example.test"]}}})).await.unwrap();
    old.call(json!({"action":"send.commitPayload","lease":{"id":staged["id"],"customerID":"123","payloadRef":staged["payloadRef"],"uploadToken":staged["uploadToken"]}})).await.unwrap();
    let nonterminal: SendLease =
        serde_json::from_value(old.call(json!({"action":"send.claim"})).await.unwrap()).unwrap();
    let mut ca_params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![
        rcgen::KeyUsagePurpose::KeyCertSign,
        rcgen::KeyUsagePurpose::DigitalSignature,
    ];
    let ca_key = rcgen::KeyPair::generate().unwrap();
    let ca = ca_params.self_signed(&ca_key).unwrap();
    let issuer = rcgen::Issuer::new(ca_params, ca_key);
    let issue = |server| {
        let mut p = rcgen::CertificateParams::new(vec!["localhost".into()]).unwrap();
        p.extended_key_usages = vec![if server {
            rcgen::ExtendedKeyUsagePurpose::ServerAuth
        } else {
            rcgen::ExtendedKeyUsagePurpose::ClientAuth
        }];
        let k = rcgen::KeyPair::generate().unwrap();
        (p.signed_by(&k, &issuer).unwrap(), k)
    };
    let (server, server_key) = issue(true);
    let (peer1, key1) = issue(false);
    let (peer2, key2) = issue(false);
    let mut enrolled1 = bindings.clone();
    enrolled1["runtimes"]["mail"]["peers"].as_array_mut().unwrap().push(json!({"kind":"mutualTls","certificateSha256":webdock_mail_protocol::sha256(peer1.der())}));
    std::fs::write(path, enrolled1.to_string()).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!(
        "https://127.0.0.1:{}",
        listener.local_addr().unwrap().port()
    );
    let tls = tonic::transport::ServerTlsConfig::new()
        .identity(tonic::transport::Identity::from_pem(
            server.pem(),
            server_key.serialize_pem(),
        ))
        .client_ca_root(tonic::transport::Certificate::from_pem(ca.pem()));
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .tls_config(tls)
            .unwrap()
            .add_service(managed.service())
            .serve_with_incoming_shutdown(
                tokio_stream::wrappers::TcpListenerStream::new(listener),
                async {
                    let _ = stopped.await;
                },
            )
            .await
            .unwrap();
    });
    let config =
        |cert: &rcgen::Certificate, key: &rcgen::KeyPair| webdock_plugin_runtime::PinnedMutualTls {
            certificate: cert.pem().into_bytes(),
            private_key: key.serialize_pem().into_bytes(),
            authority: ca.pem().into_bytes(),
            server_certificate_sha256: webdock_mail_protocol::sha256(server.der()),
        };
    let old_channel = webdock_plugin_runtime::pinned_mtls_channel(
        origin.clone(),
        "localhost".into(),
        config(&peer1, &key1),
    )
    .await
    .unwrap();
    let tls_old = NativeControlClient::new(old_channel.clone(), 1).unwrap();
    assert_eq!(
        tls_old
            .call(json!({"action":"send.status","token":token,"id":terminal[0].0}))
            .await
            .unwrap()["state"],
        "accepted"
    );
    sql(owner,"INSERT INTO webdock_auth.plugin_runtime(plugin_id,generation,version,image,status,configuration_revision,credential_hash,namespace,service_name) SELECT plugin_id,2,version,image,'ready',configuration_revision,$1,namespace,service_name FROM webdock_auth.plugin_runtime WHERE plugin_id='mail' AND generation=1",vec!["f".repeat(64).into()]).await;
    sql(
        owner,
        "UPDATE webdock_auth.plugin_installation SET current_generation=2 WHERE id='mail'",
        vec![],
    )
    .await;
    let mut enrolled2 = bindings.clone();
    enrolled2["runtimes"]["mail"]["generation"] = json!(2);
    enrolled2["runtimes"]["mail"]["credentialSha256"] = json!("f".repeat(64));
    enrolled2["runtimes"]["mail"]["peers"].as_array_mut().unwrap().push(json!({"kind":"mutualTls","certificateSha256":webdock_mail_protocol::sha256(peer2.der())}));
    std::fs::write(path, enrolled2.to_string()).unwrap();
    let current = NativeControlClient::new(
        webdock_plugin_runtime::unix_channel(socket).await.unwrap(),
        2,
    )
    .unwrap();
    let router =
        webdock_mail_runtime::send_api::SendApi::new(Arc::new(current.clone()), store.clone())
            .router();
    for (id, reference, state) in &terminal {
        for method in [axum::http::Method::GET, axum::http::Method::DELETE] {
            let response = router
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .method(method)
                        .uri(format!("/v1/emails/{id}"))
                        .header("authorization", format!("Bearer {token}"))
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                axum::http::StatusCode::OK,
                "current peer must read terminal old-generation row"
            );
            let body: Value = serde_json::from_slice(
                &axum::body::to_bytes(response.into_body(), 4096)
                    .await
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(body, json!({"id":id,"state":state}));
        }
        if *state == "cancelled" {
            assert!(store.get("123", reference).is_err());
        } else {
            assert_eq!(
                store.get("123", reference).unwrap(),
                b"retained fixture bytes"
            );
        }
    }
    assert_eq!(
        old.call(json!({"action":"send.status","token":token,"id":terminal[0].0}))
            .await
            .unwrap_err()
            .status,
        403
    );
    let spoof = NativeControlClient::new(old_channel, 2).unwrap();
    assert_eq!(
        spoof
            .call(json!({"action":"send.status","token":token,"id":terminal[0].0}))
            .await
            .unwrap_err()
            .status,
        403,
        "old actual certificate cannot impersonate the current generation"
    );
    let new_channel = webdock_plugin_runtime::pinned_mtls_channel(
        origin,
        "localhost".into(),
        config(&peer2, &key2),
    )
    .await
    .unwrap();
    let tls_current = NativeControlClient::new(new_channel, 2).unwrap();
    assert_eq!(
        tls_current
            .call(json!({"action":"send.status","token":token,"id":terminal[0].0,"cancel":true}))
            .await
            .unwrap()["state"],
        "accepted"
    );
    let wrong = format!("wd_mail_{}", "w".repeat(43));
    sql(owner,"INSERT INTO webdock_mail.api_key(id,customer_id,label,token_hash,key_suffix,created_by) VALUES('457','123','Wrong origin',$1,'wwww','fixture')",vec![webdock_mail_protocol::sha256(wrong.as_bytes()).into()]).await;
    assert_eq!(
        current
            .call(json!({"action":"send.status","token":wrong,"id":terminal[0].0,"cancel":true}))
            .await
            .unwrap_err()
            .status,
        403
    );
    for (id, context) in [
        (
            "990001",
            json!({"executor":"managed-mail-v1","payloadStoreID":"other-store","payloadStoreVersion":1,"executorGeneration":1}),
        ),
        (
            "990002",
            json!({"executor":"managed-mail-v1","payloadStoreID":"mail:platform:v1","payloadStoreVersion":2,"executorGeneration":1}),
        ),
    ] {
        let reference = uuid::Uuid::new_v4().to_string();
        store
            .put("123", b"unsupported store bytes", Some(&reference))
            .unwrap();
        sql(owner,"INSERT INTO webdock_mail.send_job(id,customer_id,source_id,idempotency_key,request_hash,payload_ref,payload_bytes,route,envelope,state,source_context,finished_at) VALUES($1,'123','key:456',$1,$2,$3::uuid,100,'{\"kind\":\"direct\",\"pluginID\":\"mail\"}','{\"from\":\"sender@example.test\",\"to\":[\"target@example.test\"]}','accepted',$4,now())",vec![id.into(),"a".repeat(64).into(),reference.clone().into(),context.into()]).await;
        for method in [axum::http::Method::GET, axum::http::Method::DELETE] {
            let response = router
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .method(method)
                        .uri(format!("/v1/emails/{id}"))
                        .header("authorization", format!("Bearer {token}"))
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert!(response.status().is_client_error() || response.status().is_server_error());
        }
        assert_eq!(
            store.get("123", &reference).unwrap(),
            b"unsupported store bytes"
        );
    }
    for cancel in [false, true] {
        assert!(current.call(json!({"action":"send.status","token":token,"id":nonterminal.id,"cancel":cancel})).await.is_err());
    }
    assert!(current.call(json!({"action":"send.begin","lease":nonterminal,"digest":"d".repeat(64),"preparation":{"metadataSha256":"e".repeat(64),"mimeSha256":"f".repeat(64),"mimeBytes":100}})).await.is_err());
    assert!(
        current
            .call(json!({"action":"send.claim"}))
            .await
            .unwrap()
            .is_null()
    );
    stop.send(()).unwrap();
    task.await.unwrap();
    current
}

async fn fix1_unsupported_versions(
    owner: &sea_orm::DatabaseConnection,
    control: &webdock_mail_protocol::native_control::NativeControlClient,
    legacy: &webdock_mail::submissions::Submissions,
) {
    use std::{os::unix::fs::PermissionsExt, sync::Arc};
    let private = tempfile::tempdir().unwrap();
    std::fs::set_permissions(private.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = Arc::new(
        webdock_mail_runtime::FilesystemPayloadStore::open(
            private.path(),
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        )
        .unwrap(),
    );
    let versions = [
        Some(json!(2)),
        Some(json!("1")),
        Some(Value::Null),
        None,
        Some(json!(1.0)),
    ];
    let mut references = Vec::new();
    let mut n = 0;
    for state in ["accepted", "queued", "leased", "handoff", "staging"] {
        for version in &versions {
            n += 1;
            let id = (980000 + n).to_string();
            let reference = uuid::Uuid::new_v4().to_string();
            let token = uuid::Uuid::new_v4().to_string();
            let mut context = json!({"executor":"managed-mail-v1","payloadStoreID":"mail:platform:v1","executorGeneration":2});
            if let Some(value) = version {
                context["payloadStoreVersion"] = value.clone();
            }
            store
                .put(
                    "123",
                    b"unsupported-version private bytes",
                    Some(&reference),
                )
                .unwrap();
            references.push((id.clone(), reference));
            sql(owner,"INSERT INTO webdock_mail.send_job(id,customer_id,source_id,idempotency_key,request_hash,payload_ref,payload_bytes,route,envelope,state,source_context,lease_token,lease_until,finished_at) VALUES($1,'123','key:456',$1,$2,$3::uuid,100,'{\"kind\":\"direct\",\"pluginID\":\"mail\"}','{\"from\":\"sender@example.test\",\"to\":[\"target@example.test\"]}',$4,$5,$6::uuid,now()-interval '2 minutes',CASE WHEN $4='accepted' THEN now()-interval '2 minutes' END)",vec![id.clone().into(),"a".repeat(64).into(),references.last().unwrap().1.clone().into(),state.into(),context.into(),token.into()]).await;
            sql(owner,"INSERT INTO webdock_mail.budget_reservation(reference_id,customer_id,units,period_start,send_path,state) VALUES($1,'123',1,date_trunc('month',now() AT TIME ZONE 'UTC')::date,'{\"kind\":\"direct\"}',$2)",vec![id.into(),if state=="accepted"{"accepted".into()}else{"reserved".into()}]).await;
        }
    }
    sql(owner,"UPDATE webdock_mail.month_usage SET reserved=reserved+20,accepted=accepted+5 WHERE customer_id='123' AND period_start=date_trunc('month',now() AT TIME ZONE 'UTC')::date",vec![]).await;
    let before = unsupported_snapshot(owner).await;
    assert!(
        legacy
            .execute(webdock_mail::submissions::QueueCommand::Claim {})
            .await
            .unwrap()
            .is_null()
    );
    assert_eq!(
        legacy
            .execute(webdock_mail::submissions::QueueCommand::Cleanup {})
            .await
            .unwrap(),
        json!([])
    );
    assert_eq!(
        unsupported_snapshot(owner).await,
        before,
        "legacy never adopts any managed marker version"
    );
    let selected = control
        .call(json!({"action":"send.cleanup"}))
        .await
        .unwrap();
    assert!(
        selected
            .as_array()
            .unwrap()
            .iter()
            .all(|item| !references.iter().any(|(id, _)| item["id"] == *id)),
        "unsupported version must be excluded before listing a private reference"
    );
    let cleaned = webdock_mail_runtime::dispatch::cleanup_payloads(control, store.clone()).await;
    for (_, reference) in &references {
        assert_eq!(
            store.get("123", reference).unwrap(),
            b"unsupported-version private bytes",
            "unsupported private bytes must survive cleanup"
        );
    }
    assert!(cleaned.is_ok());
    assert!(
        control
            .call(json!({"action":"send.claim"}))
            .await
            .unwrap()
            .is_null()
    );
    assert_eq!(
        unsupported_snapshot(owner).await,
        before,
        "queued/expired leased/handoff/staging versions must remain byte-for-byte unchanged"
    );
    let selected = control
        .call(json!({"action":"send.cleanup"}))
        .await
        .unwrap();
    assert!(
        selected
            .as_array()
            .unwrap()
            .iter()
            .all(|item| !references.iter().any(|(id, _)| item["id"] == *id))
    );
    for (_, reference) in references {
        assert_eq!(
            store.get("123", &reference).unwrap(),
            b"unsupported-version private bytes"
        );
    }
}
async fn unsupported_snapshot(owner: &sea_orm::DatabaseConnection) -> Value {
    owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT jsonb_build_object('jobs',(SELECT jsonb_agg(to_jsonb(j) ORDER BY j.id) FROM webdock_mail.send_job j WHERE j.id::bigint BETWEEN 980001 AND 980025),'budgets',(SELECT jsonb_agg(to_jsonb(b) ORDER BY b.reference_id) FROM webdock_mail.budget_reservation b WHERE b.reference_id::bigint BETWEEN 980001 AND 980025),'usage',(SELECT to_jsonb(u) FROM webdock_mail.month_usage u WHERE u.customer_id='123' AND u.period_start=date_trunc('month',now() AT TIME ZONE 'UTC')::date)) AS snapshot")).await.unwrap().unwrap().try_get("","snapshot").unwrap()
}
