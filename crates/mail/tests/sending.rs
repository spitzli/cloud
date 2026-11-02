//! Real restricted roles, installed HostService policy, and the authenticated durable callback.
use futures_util::FutureExt;
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
use serde_json::{Value, json};
use std::{os::unix::fs::PermissionsExt, path::Path, sync::Arc};
use webdock_mail::{
    MailDependencies,
    outbox::{PayloadLease, Receipt, SendLease},
    sending::{Admission, Preparation, SendCommand, Sending, SendingConfig},
    submissions::{QueueCommand, Submissions},
};
use webdock_mail_protocol::{
    sha256,
    wire::{LeaseProof, mail_authority_client::MailAuthorityClient},
};
use webdock_plugin_host::{FileBindings, HostService};
use webdock_plugin_runtime::UnixSocket;
mod support;
use support::sending::{KEY, PLUGIN, bindings, role, seed, sql};

fn configuration() -> SendingConfig {
    SendingConfig {
        enabled: true,
        rls_secret: KEY.into(),
        signing_key: [78; 32],
        worker_id: "mail-worker".into(),
        worker_generation: 1,
        worker_credential_sha256: "b".repeat(64),
        incoming: None,
    }
}
fn admission(token: &str, key: &str) -> Admission {
    serde_json::from_value(json!({
        "token":token,"idempotencyKey":key,"requestHash":"d".repeat(64),
        "payloadRef":uuid::Uuid::new_v4().to_string(),"payloadBytes":100,
        "envelope":{"from":"sender@example.test","to":["target@example.test"]}
    }))
    .unwrap()
}
fn begin(lease: SendLease) -> SendCommand {
    SendCommand::Begin {
        lease: Box::new(lease),
        digest: "e".repeat(64),
        preparation: Preparation {
            metadata_sha256: "f".repeat(64),
            mime_sha256: "a".repeat(64),
            mime_bytes: 100,
        },
    }
}
async fn claim(sending: &Sending, queue: &Submissions, token: &str, key: &str) -> SendLease {
    let admitted = sending
        .execute(SendCommand::Admit {
            submission: admission(token, key),
        })
        .await
        .unwrap();
    assert_eq!(admitted["state"], "staging");
    queue
        .execute(QueueCommand::CommitPayload {
            lease: PayloadLease {
                id: admitted["id"].as_str().unwrap().into(),
                customer_id: "123".into(),
                payload_ref: admitted["payloadRef"].as_str().unwrap().into(),
                upload_token: admitted["uploadToken"].as_str().unwrap().into(),
            },
        })
        .await
        .unwrap();
    serde_json::from_value(queue.execute(QueueCommand::Claim {}).await.unwrap()).unwrap()
}
async fn grant(
    sending: &Sending,
    queue: &Submissions,
    token: &str,
    key: &str,
) -> (SendLease, String) {
    let lease = claim(sending, queue, token, key).await;
    let authorized = sending.execute(begin(lease.clone())).await.unwrap();
    assert_eq!(authorized["kind"], "direct");
    assert!(!authorized.to_string().contains("consumersecret"));
    assert!(!authorized.to_string().contains("fixture-provider-secret"));
    (lease, authorized["leaseToken"].as_str().unwrap().into())
}
fn proof(token: &str) -> LeaseProof {
    LeaseProof {
        lease_token: token.into(),
    }
}
async fn callback_client(
    sending: Arc<Sending>,
    path: &Path,
) -> (
    MailAuthorityClient<tonic::transport::Channel>,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let mut socket = UnixSocket::bind(path).unwrap();
    let incoming = socket.incoming().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(sending.service())
            .serve_with_incoming_shutdown(incoming, async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
        drop(socket);
    });
    let channel = webdock_plugin_runtime::unix_channel(path).await.unwrap();
    (MailAuthorityClient::new(channel), stop, server)
}
async fn deny_plain_callback(sending: Arc<Sending>, token: &str) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(sending.service())
            .serve_with_incoming_shutdown(
                tokio_stream::wrappers::TcpListenerStream::new(listener),
                async {
                    let _ = stopped.await;
                },
            )
            .await
            .unwrap();
    });
    let mut client = MailAuthorityClient::connect(format!("http://{address}"))
        .await
        .unwrap();
    assert!(client.guard(proof(token)).await.is_err());
    assert!(client.begin_attempt(proof(token)).await.is_err());
    drop(client);
    stop.send(()).unwrap();
    server.await.unwrap();
}
async fn state(owner: &DatabaseConnection, id: &str) -> String {
    owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT state FROM webdock_mail.send_job WHERE id=$1",
            [id.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "state")
        .unwrap()
}
#[tokio::test]
#[ignore = "creates and removes one isolated owned local sending database"]
async fn installed_policy_and_authenticated_callbacks_fence_every_external_attempt() {
    let mut url =
        url::Url::parse(&std::env::var("WEBDOCK_TEST_MIGRATION_DATABASE_URL").unwrap()).unwrap();
    assert!(matches!(url.host_str(), Some("localhost" | "127.0.0.1")));
    assert_eq!(url.path(), "/webdock_rls_test");
    assert!(url.query().is_none());
    let admin = Database::connect(url.as_str()).await.unwrap();
    let name = format!(
        "webdock_mail_sending_{}_test",
        uuid::Uuid::new_v4().simple()
    );
    admin
        .execute_unprepared(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    url.set_path(&format!("/{name}"));
    let result = std::panic::AssertUnwindSafe(run(&url, &name))
        .catch_unwind()
        .await;
    admin
        .execute_unprepared(&format!("DROP DATABASE {name} WITH (FORCE)"))
        .await
        .unwrap();
    admin.close().await.unwrap();
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}
async fn run(url: &url::Url, name: &str) {
    let owner = Database::connect(url.as_str()).await.unwrap();
    webdock_persistence::migrations::bootstrap(&owner, name, KEY)
        .await
        .unwrap();
    seed(&owner).await;
    let auth = role("AUTH_DATABASE_URL", url).await;
    let business = role("DATABASE_URL", url).await;
    let worker = role("WEBDOCK_PLUGIN_WORKER_DATABASE_URL", url).await;
    let ids = Arc::new(webdock_persistence::IdAllocator::new(
        business.clone(),
        webdock_ids::Identity::platform(0, 2).unwrap(),
    ));
    let identity = Arc::new(webdock_identity::IdentityService::new(
        auth.clone(),
        webdock_identity::CookiePolicy::new(false, "fixture-session-key-at-least32-characters")
            .unwrap(),
    ));
    let dependencies = MailDependencies {
        auth: auth.clone(),
        business: business.clone(),
        worker: worker.clone(),
        identity,
        browser: None,
        ids: ids.clone(),
    };
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let binding_path = dir.path().join("bindings.json");
    std::fs::write(&binding_path, bindings().to_string()).unwrap();
    std::fs::set_permissions(&binding_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let host = Arc::new(
        HostService::new(
            business.clone(),
            worker.clone(),
            KEY.into(),
            ids,
            Arc::new(FileBindings::new(binding_path.clone())),
            ed25519_dalek::SigningKey::from_bytes(&[33; 32]),
        )
        .await
        .unwrap(),
    );
    let policy = Arc::new(webdock_control_plane::mail::MailProviderPolicy::new(host));
    let sending = Arc::new(
        Sending::new(dependencies.clone(), configuration(), policy.clone())
            .await
            .unwrap(),
    );
    let queue = Submissions::new(dependencies.clone(), KEY, true)
        .await
        .unwrap();
    let (mut client, stop, server) =
        callback_client(sending.clone(), &dir.path().join("authority.sock")).await;
    let token = format!("wd_mail_{}", "t".repeat(43));
    assert_eq!(
        sending
            .execute(SendCommand::Authenticate {
                token: token.clone()
            })
            .await
            .unwrap()["customerID"],
        "123"
    );
    let lease = claim(&sending, &queue, &token, "first").await;
    let duplicate = sending
        .execute(SendCommand::Admit {
            submission: admission(&token, "first"),
        })
        .await
        .unwrap();
    assert_eq!(duplicate["id"], lease.id);
    assert_eq!(duplicate["created"], false);
    let mut different = admission(&token, "first");
    different.request_hash = "c".repeat(64);
    assert!(
        sending
            .execute(SendCommand::Admit {
                submission: different
            })
            .await
            .is_err()
    );
    let mut forged = lease.clone();
    forged.envelope["to"] = json!(["attacker@example.test"]);
    assert!(sending.execute(begin(forged)).await.is_err());
    let authorized = sending.execute(begin(lease.clone())).await.unwrap();
    assert!(!authorized.to_string().contains("fixture-provider-secret"));
    let granted_token = authorized["leaseToken"].as_str().unwrap();
    deny_plain_callback(sending.clone(), granted_token).await;
    assert!(
        sending
            .execute(begin(lease.clone()))
            .await
            .unwrap()
            .is_null()
    );
    assert!(
        client
            .guard(proof(granted_token))
            .await
            .unwrap()
            .into_inner()
            .active
    );
    let mut changed = bindings();
    changed["runtimes"][PLUGIN]["peers"][0]["uid"] = json!(rustix::process::geteuid().as_raw() + 1);
    std::fs::write(&binding_path, changed.to_string()).unwrap();
    assert!(client.guard(proof(granted_token)).await.is_err());
    std::fs::write(&binding_path, bindings().to_string()).unwrap();
    changed = bindings();
    changed["runtimes"][PLUGIN]["customers"]["123"]["send"]["headers"]["consumersecret"] =
        json!("changed-secret");
    std::fs::write(&binding_path, changed.to_string()).unwrap();
    assert!(client.begin_attempt(proof(granted_token)).await.is_err());
    std::fs::write(&binding_path, bindings().to_string()).unwrap();
    let mut calls = tokio::task::JoinSet::new();
    for _ in 0..4 {
        let mut client = client.clone();
        let request = proof(granted_token);
        calls.spawn(async move { client.begin_attempt(request).await });
    }
    let mut admitted = Vec::new();
    while let Some(result) = calls.join_next().await {
        if let Ok(response) = result.unwrap() {
            admitted.push(response.into_inner());
        }
    }
    assert_eq!(admitted.len(), 1);
    let response = admitted.pop().unwrap();
    assert!(response.admitted);
    let private: Value = serde_json::from_slice(&response.private_configuration_json).unwrap();
    assert_eq!(
        private["binding"]["headers"]["consumersecret"],
        "fixture-provider-secret"
    );
    assert!(client.begin_attempt(proof(granted_token)).await.is_err());
    assert!(client.guard(proof(granted_token)).await.is_err());
    // A new service instance cannot reissue a grant after a lost acknowledgement.
    let restarted = Arc::new(
        Sending::new(dependencies.clone(), configuration(), policy.clone())
            .await
            .unwrap(),
    );
    assert!(
        restarted
            .execute(begin(lease.clone()))
            .await
            .unwrap()
            .is_null()
    );
    let (mut replacement, stop_replacement, replacement_server) =
        callback_client(restarted.clone(), &dir.path().join("restarted.sock")).await;
    assert!(replacement.guard(proof(granted_token)).await.is_err());
    assert!(
        replacement
            .begin_attempt(proof(granted_token))
            .await
            .is_err()
    );
    drop(replacement);
    stop_replacement.send(()).unwrap();
    replacement_server.await.unwrap();
    queue
        .execute(QueueCommand::Finish {
            lease: lease.clone(),
            result: Receipt::Unknown {},
        })
        .await
        .unwrap();
    assert_eq!(state(&owner, &lease.id).await, "unknown");
    assert!(
        queue
            .execute(QueueCommand::Claim {})
            .await
            .unwrap()
            .is_null()
    );
    let receipt = Receipt::Accepted {
        provider_id: "provider_1".into(),
    };
    for _ in 0..2 {
        queue
            .execute(QueueCommand::Finish {
                lease: lease.clone(),
                result: receipt.clone(),
            })
            .await
            .unwrap();
    }
    let usage = owner
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT accepted,reserved FROM webdock_mail.month_usage WHERE customer_id='123'",
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(usage.try_get::<i64>("", "accepted").unwrap(), 1);
    assert_eq!(usage.try_get::<i64>("", "reserved").unwrap(), 0);
    draining_and_legacy_routes(&owner, &sending, &queue, &token).await;
    let (revoked, revoked_token) = grant(&sending, &queue, &token, "revocation").await;
    sql(&owner,"UPDATE webdock_mail.provider_binding SET revision=revision+1,revoked_at=now() WHERE customer_id='123'",vec![]).await;
    assert!(client.guard(proof(&revoked_token)).await.is_err());
    assert!(client.begin_attempt(proof(&revoked_token)).await.is_err());
    assert_eq!(state(&owner, &revoked.id).await, "handoff");
    sql(&owner,"UPDATE webdock_mail.provider_binding SET revision=revision+1,revoked_at=NULL WHERE customer_id='123'",vec![]).await;
    let (_, source_token) = grant(&sending, &queue, &token, "source-after-handoff").await;
    let cancelled = claim(&sending, &queue, &token, "source-before-handoff").await;
    sql(
        &owner,
        "UPDATE webdock_mail.api_key SET revoked_at=now() WHERE id='456'",
        vec![],
    )
    .await;
    assert!(client.guard(proof(&source_token)).await.is_err());
    assert!(client.begin_attempt(proof(&source_token)).await.is_err());
    assert!(
        sending
            .execute(begin(cancelled.clone()))
            .await
            .unwrap()
            .is_null()
    );
    assert_eq!(state(&owner, &cancelled.id).await, "cancelled");
    assert!(
        sending
            .execute(SendCommand::Admit {
                submission: admission(&token, "revoked-key")
            })
            .await
            .is_err()
    );
    incoming_webmail(&owner, dependencies.clone(), policy, &queue).await;
    drop(client);
    stop.send(()).unwrap();
    server.await.unwrap();
    drop(queue);
    drop(restarted);
    drop(sending);
    drop(dependencies);
    auth.close().await.unwrap();
    business.close().await.unwrap();
    worker.close().await.unwrap();
    owner.close().await.unwrap();
}

async fn draining_and_legacy_routes(
    owner: &DatabaseConnection,
    sending: &Sending,
    queue: &Submissions,
    token: &str,
) {
    let draining = claim(sending, queue, token, "continue-during-drain").await;
    sql(
        owner,
        "UPDATE webdock_auth.plugin_installation SET state='draining' WHERE id IN ('mail','mail-policy-fixture'); UPDATE webdock_auth.plugin_runtime SET status='draining' WHERE plugin_id='mail-policy-fixture'",
        vec![],
    )
    .await;
    assert!(
        sending
            .execute(SendCommand::Admit {
                submission: admission(token, "new-during-drain"),
            })
            .await
            .is_err()
    );
    assert_eq!(
        sending.execute(begin(draining.clone())).await.unwrap()["kind"],
        "direct"
    );
    assert_eq!(state(owner, &draining.id).await, "handoff");
    sql(
        owner,
        "UPDATE webdock_auth.plugin_installation SET state='enabled' WHERE id IN ('mail','mail-policy-fixture'); UPDATE webdock_auth.plugin_runtime SET status='ready' WHERE plugin_id='mail-policy-fixture'",
        vec![],
    )
    .await;
    queue
        .execute(QueueCommand::Finish {
            lease: draining,
            result: Receipt::Rejected {},
        })
        .await
        .unwrap();

    let mut legacy = claim(sending, queue, token, "legacy-route-review").await;
    legacy.route = json!({"kind":"direct","mode":"managed","transportRevision":"1"});
    sql(
        owner,
        "UPDATE webdock_mail.send_job SET route=$2 WHERE id=$1",
        vec![legacy.id.clone().into(), legacy.route.clone().into()],
    )
    .await;
    assert!(
        sending
            .execute(begin(legacy.clone()))
            .await
            .unwrap()
            .is_null()
    );
    assert_eq!(state(owner, &legacy.id).await, "preparation_review");
    let retained = owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT j.payload_ref::text,r.state AS reservation_state,(SELECT count(*) FROM webdock_mail.provider_lease l WHERE l.job_id=j.id) AS provider_leases FROM webdock_mail.send_job j JOIN webdock_mail.budget_reservation r ON r.reference_id=j.id WHERE j.id=$1",
            [legacy.id.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        retained.try_get::<String>("", "payload_ref").unwrap(),
        legacy.payload_ref
    );
    assert_eq!(
        retained.try_get::<String>("", "reservation_state").unwrap(),
        "reserved"
    );
    assert_eq!(retained.try_get::<i64>("", "provider_leases").unwrap(), 0);
    assert!(
        queue
            .execute(QueueCommand::Claim {})
            .await
            .unwrap()
            .is_null()
    );
}

async fn webmail_command(sending: &Sending, command: Value) -> webdock_mail::Result<Value> {
    sending
        .execute_webmail(serde_json::from_value(command).unwrap())
        .await
}
async fn queued_draft(
    sending: &Sending,
    queue: &Submissions,
    token: &str,
    email: &str,
) -> SendLease {
    let request = uuid::Uuid::new_v4();
    let prepared = webmail_command(
        sending,
        json!({"action":"send.webmail.prepare","submission":{
            "token":token,"requestID":request,"emailID":email,"identityID":"identity1"
        }}),
    )
    .await
    .unwrap();
    assert_eq!(prepared["existing"], false);
    let staged=webmail_command(sending,json!({"action":"send.webmail.stage","submission":{
        "token":token,"requestID":request,"emailID":email,"identityID":"identity1",
        "operationID":prepared["operationID"],"leaseToken":prepared["leaseToken"],
        "payloadRef":uuid::Uuid::new_v4().to_string(),"payloadBytes":100,"requestHash":"d".repeat(64),
        "envelope":{"from":"alice@example.test","to":["target@example.test"]}
    }})).await.unwrap();
    queue
        .execute(QueueCommand::CommitPayload {
            lease: PayloadLease {
                id: staged["id"].as_str().unwrap().into(),
                customer_id: "123".into(),
                payload_ref: staged["payloadRef"].as_str().unwrap().into(),
                upload_token: staged["uploadToken"].as_str().unwrap().into(),
            },
        })
        .await
        .unwrap();
    let job: SendLease =
        serde_json::from_value(queue.execute(QueueCommand::Claim {}).await.unwrap()).unwrap();
    assert_eq!(job.id, staged["id"]);
    assert!(matches!(
        job.payload_format,
        webdock_mail::outbox::PayloadFormat::Mime
    ));
    job
}
async fn incoming_webmail(
    owner: &DatabaseConnection,
    dependencies: MailDependencies,
    policy: Arc<webdock_control_plane::mail::MailProviderPolicy>,
    queue: &Submissions,
) {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use webdock_mail::{
        secrets::MailCipher,
        webmail::{WebmailConfig, WebmailService},
    };
    sql(owner,"INSERT INTO webdock_auth.\"user\"(id,name,email,\"emailVerified\",role,\"twoFactorEnabled\",\"mustChangePassword\") VALUES('alice','Alice','alice@example.invalid',true,'user',false,false); INSERT INTO webdock_auth.session(id,token,\"userId\",\"expiresAt\",\"updatedAt\") VALUES('alice-session','alice-token','alice',now()+interval '1 hour',now()); INSERT INTO webdock_auth.member(id,\"organizationId\",\"userId\",role,\"createdAt\") VALUES('member-alice','124','alice','member',now()); INSERT INTO webdock_mail.service(customer_id,instance_key,host_id,desired_enabled,state,revision,applied_revision) VALUES('123','local-fixture','200',true,'ready',1,1); INSERT INTO webdock_mail.domain(id,customer_id,sender_domain_id,name,provider_id,state) VALUES('302','123','301','example.test','domain1','ready'); INSERT INTO webdock_mail.mailbox(id,customer_id,domain_id,local_part,name,quota_bytes,provider_id,state) VALUES('303','123','302','alice','Alice mailbox',1048576,'account-a','ready')",vec![]).await;
    let key = STANDARD.encode([29u8; 32]);
    let cipher = MailCipher::new(&key).unwrap();
    let identity = cipher
        .seal(
            &json!({"username":"native-alice","password":"retained-fixture-password"}),
            "identity:123:304",
        )
        .unwrap();
    sql(owner,"INSERT INTO webdock_mail.identity(id,customer_id,subject,provider_id,encrypted_credentials,state) VALUES('304','123','alice','principal1',$1,'ready')",vec![identity.into()]).await;
    sql(owner,"INSERT INTO webdock_mail.mailbox_grant(customer_id,mailbox_id,subject,enabled,revision,applied_revision,member_id) VALUES('123','303','alice',true,1,1,'member-alice')",vec![]).await;
    let hook = format!("wd_hook_123_{}", "h".repeat(43));
    let submission=cipher.seal(&json!({"username":"wd-submit@mail-123.example.test","password":"fixture-smtp-password-32-characters","hookToken":hook}),"submission:123").unwrap();
    sql(owner,"INSERT INTO webdock_mail.instance(customer_id,encrypted_credentials,verified_at,smtp_certificate_sha256,smtp_certificate_revision,encrypted_submission_credentials,submission_hook_token_hash,submission_transport_revision,submission_ready_revision,submission_state) VALUES('123','fixture',now(),$1,1,$2,$3,1,1,'ready')",vec!["a".repeat(64).into(),submission.into(),sha256(hook.as_bytes()).into()]).await;
    let service = Arc::new(
        WebmailService::new(
            dependencies.clone(),
            WebmailConfig {
                enabled: true,
                gateway_origin: "https://mail.example.test".into(),
                managed_cluster: "200".into(),
                rls_secret: KEY.into(),
                encryption_key: key.clone(),
            },
        )
        .await
        .unwrap(),
    );
    let cookies =
        webdock_identity::CookiePolicy::new(false, "fixture-session-key-at-least32-characters")
            .unwrap();
    let cookie = format!(
        "{}={}",
        cookies.name(),
        cookies.sign("alice-token").unwrap()
    );
    let alice = dependencies
        .identity
        .authenticate_cookie(&cookie)
        .await
        .unwrap()
        .unwrap();
    let launch = service
        .launch(
            &alice,
            &webdock_browser_session::StoredAuthority::LocalSession {},
            "123",
            "303",
        )
        .await
        .unwrap();
    let scope = service.exchange(&launch.code).await.unwrap();
    let mut config = configuration();
    config.incoming = Some(webdock_mail::sending::IncomingConfig {
        managed_cluster: "200".into(),
        hostname_suffix: "example.test".into(),
        encryption_key: key,
    });
    let sending = Sending::new(dependencies, config, policy)
        .await
        .unwrap()
        .with_webmail(service.clone());
    let banned = queued_draft(&sending, queue, &scope.token, "draft-banned").await;
    let valid = queued_draft(&sending, queue, &scope.token, "draft-valid").await;
    let revoked = queued_draft(&sending, queue, &scope.token, "draft-revoked").await;
    sql(
        owner,
        "UPDATE webdock_auth.\"user\" SET banned=true WHERE id='alice'",
        vec![],
    )
    .await;
    assert!(
        sending
            .execute(begin(banned.clone()))
            .await
            .unwrap()
            .is_null()
    );
    assert_eq!(state(owner, &banned.id).await, "cancelled");
    sql(
        owner,
        "UPDATE webdock_auth.\"user\" SET banned=false WHERE id='alice'",
        vec![],
    )
    .await;
    service.revoke(&scope.token).await.unwrap();
    sql(
        owner,
        "DELETE FROM webdock_auth.session WHERE id='alice-session'",
        vec![],
    )
    .await;
    assert!(service.inspect(&scope.token).await.is_err());
    let authorized = sending.execute(begin(valid.clone())).await.unwrap();
    assert_eq!(authorized["kind"], "stalwart");
    assert_eq!(authorized["submissionID"], valid.id);
    assert_eq!(authorized["username"], "wd-submit@mail-123.example.test");
    assert_eq!(authorized["proof"].as_str().unwrap().len(), 43);
    let policy_input = json!({"customerID":"123","token":hook,"id":valid.id,"proof":authorized["proof"],
        "digest":"e".repeat(64),"username":authorized["username"],"queueID":"a123", "envelope":valid.envelope});
    // An already handed-off SMTP queue retains its proof when the sender loses its reply.
    sql(
        owner,
        "UPDATE webdock_mail.send_job SET lease_until=now()-interval '1 second' WHERE id=$1",
        vec![valid.id.clone().into()],
    )
    .await;
    for field in ["proof", "valid", "valid", "queueID", "digest"] {
        let mut input = policy_input.clone();
        match field {
            "proof" => input["proof"] = json!("x".repeat(43)),
            "queueID" => input["queueID"] = json!("b456"),
            "digest" => input["digest"] = json!("b".repeat(64)),
            "valid" => {}
            _ => unreachable!(),
        }
        let result = sending
            .execute(
                serde_json::from_value(json!({"action":"send.policy","submission":input})).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(result["accept"], field == "valid", "{field}");
    }
    queue
        .execute(QueueCommand::Finish {
            lease: valid.clone(),
            result: Receipt::Accepted {
                provider_id: valid.id.clone(),
            },
        })
        .await
        .unwrap();
    sql(
        owner,
        "UPDATE webdock_auth.plugin_installation SET state='draining' WHERE id='mail'",
        vec![],
    )
    .await;
    let access=webmail_command(&sending,json!({"action":"send.webmail.filingAccess","id":valid.id,"customerID":"123","leaseToken":valid.lease_token})).await.unwrap();
    assert_eq!(access["emailID"], "draft-valid");
    assert_eq!(access["complete"], false);
    assert!(
        access["access"]["authorization"]
            .as_str()
            .unwrap()
            .starts_with("Basic ")
    );
    for (filed, expected) in [(false, false), (true, true), (false, true)] {
        let result=webmail_command(&sending,json!({"action":"send.webmail.filed","id":valid.id,"customerID":"123","leaseToken":valid.lease_token,"filed":filed})).await.unwrap();
        assert_eq!(result["filed"], expected);
        assert_eq!(state(owner, &valid.id).await, "accepted");
    }
    assert!(
        queue
            .execute(QueueCommand::Claim {})
            .await
            .unwrap()
            .is_null()
    );
    sql(
        owner,
        "UPDATE webdock_auth.plugin_installation SET state='enabled' WHERE id='mail'",
        vec![],
    )
    .await;
    sql(owner,"UPDATE webdock_mail.mailbox_grant SET enabled=false,revision=2,applied_revision=2 WHERE mailbox_id='303'",vec![]).await;
    assert!(
        sending
            .execute(begin(revoked.clone()))
            .await
            .unwrap()
            .is_null()
    );
    assert_eq!(state(owner, &revoked.id).await, "cancelled");
}
