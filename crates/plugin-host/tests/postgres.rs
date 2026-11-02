//! Isolated records in the retained local fixture; no schema reset and no provider I/O.
#[path = "support/controller.rs"]
mod controller;
#[path = "support/mcp.rs"]
mod mcp;
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use webdock_identity::{CookiePolicy, IdentityService};
use webdock_plugin_host::{AdmissionInput, FileBindings, HostService};
use webdock_plugin_runtime::{InvocationAuthority, PeerIdentity};
async fn db(key: &str) -> DatabaseConnection {
    let value = std::env::var(key).expect("explicit local fixture URL");
    let parsed = url::Url::parse(&value).unwrap();
    assert!(
        matches!(parsed.host_str(), Some("localhost" | "127.0.0.1"))
            && parsed.path() == "/webdock_rls_test"
            && parsed.query().is_none()
    );
    Database::connect(value).await.unwrap()
}
async fn sql(db: &DatabaseConnection, text: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        text,
        values,
    ))
    .await
    .unwrap();
}
async fn id(db: &DatabaseConnection) -> String {
    db.query_one_raw(Statement::from_string(
        DbBackend::Postgres,
        "SELECT webdock_admin.next_snowflake() AS id",
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get("", "id")
    .unwrap()
}
fn admission(plugin: &str, project: &str, action: &str, key: &str) -> AdmissionInput {
    serde_json::from_value(json!({"pluginID":plugin,"projectID":project,"action":action,"input":{},"idempotencyKey":key})).unwrap()
}
#[tokio::test]
#[ignore = "requires local retained-schema restricted roles and signed RLS fixture"]
async fn real_admission_data_cas_receipts_authority_revocation_and_admin() {
    let owner = db("WEBDOCK_TEST_MIGRATION_DATABASE_URL").await;
    let nonce = format!(
        "rust-host-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap()
    );
    let plugin = nonce.clone();
    let uid = nonce.clone();
    let sid = format!("{nonce}-session");
    let token = format!("{nonce}-token");
    let cid = id(&owner).await;
    let pid = id(&owner).await;
    let mapped = id(&owner).await;
    sql(&owner,"INSERT INTO webdock_auth.\"user\"(id,name,email,\"emailVerified\",role,\"twoFactorEnabled\",\"mustChangePassword\") VALUES($1,$1,$2,true,'operator',true,false)",vec![uid.clone().into(),format!("{nonce}@example.invalid").into()]).await;
    sql(&owner,"INSERT INTO webdock_auth.session(id,token,\"userId\",\"expiresAt\",\"updatedAt\") VALUES($1,$2,$3,now()+interval '1 hour',now())",vec![sid.clone().into(),token.clone().into(),uid.clone().into()]).await;
    sql(&owner,"INSERT INTO webdock_admin.users(id,name,email,role,auth_subject) VALUES($1,$2,$3,'operator',$2)",vec![mapped.clone().into(),uid.clone().into(),format!("{nonce}@example.invalid").into()]).await;
    sql(
        &owner,
        "INSERT INTO webdock_admin.customers(id,name) VALUES($1,$2)",
        vec![cid.clone().into(), nonce.clone().into()],
    )
    .await;
    sql(
        &owner,
        "INSERT INTO webdock_admin.projects(id,name,customer_id) VALUES($1,$2,$3)",
        vec![pid.clone().into(), nonce.clone().into(), cid.clone().into()],
    )
    .await;
    let mut manifest: Value = serde_json::from_str(include_str!(
        "../../plugin-protocol/tests/fixtures/project-notes.json"
    ))
    .unwrap();
    manifest["name"] = json!(plugin);
    manifest["jobs"] = json!([]);
    manifest["capabilities"] = json!(["plugin.data.read", "plugin.data.write", "plugin.http.read"]);
    manifest["services"]["required"] =
        json!([{"name":"data","version":1},{"name":"http","version":1}]);
    manifest["httpBindings"] = json!(["provider"]);
    manifest["actions"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"provider.read","intent":"read","audience":"operator","capabilities":["plugin.http.read"]}));
    manifest["api"] = json!([{"method":"GET","path":"/notes","action":"notes.read"},{"method":"POST","path":"/notes","action":"notes.write"}]);
    manifest["mcp"] = json!([{"name":"read","description":"Read scoped notes","action":"notes.read"},{"name":"write","description":"Write scoped notes","action":"notes.write"}]);
    for action in manifest["actions"].as_array_mut().unwrap() {
        if action["name"].as_str().unwrap().starts_with("notes.") {
            action["input"] = json!({"type":"object","properties":{"key":{"type":"string"}},"additionalProperties":false});
        }
    }
    let raw = manifest.to_string();
    let digest = hex::encode(Sha256::digest(raw.as_bytes()));
    let credential = hex::encode(Sha256::digest(b"fixture-credential"));
    sql(&owner,"INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version,runtime,current_generation) VALUES($1,'external','enabled','enabled','1.0.0','ready',1)",vec![plugin.clone().into()]).await;
    sql(&owner,"INSERT INTO webdock_auth.plugin_release(plugin_id,version,manifest_bytes,manifest_sha256,image,registry_url,signer_key_id) VALUES($1,'1.0.0',$2,$3,$4,'https://registry.invalid','fixture')",vec![plugin.clone().into(),raw.clone().into(),digest.into(),manifest["image"].as_str().unwrap().into()]).await;
    sql(&owner,"INSERT INTO webdock_auth.plugin_configuration(plugin_id,approved_capabilities,configuration) VALUES($1,$2,'{}')",vec![plugin.clone().into(),manifest["capabilities"].clone().into()]).await;
    sql(&owner,"INSERT INTO webdock_auth.plugin_runtime(plugin_id,generation,version,image,status,configuration_revision,credential_hash,namespace,service_name) VALUES($1,1,'1.0.0',$2,'ready',0,$3,'fixtures','fixture-plugin')",vec![plugin.clone().into(),manifest["image"].as_str().unwrap().into(),credential.clone().into()]).await;
    sql(&owner,"INSERT INTO webdock_auth.plugin_grant(plugin_id,customer_id,project_id,capabilities) VALUES($1,$2,$3,$4)",vec![plugin.clone().into(),cid.clone().into(),pid.clone().into(),manifest["capabilities"].clone().into()]).await;
    let boot_time = chrono::Utc::now();
    let inserted_boot=owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.plugin_bootstrap(singleton,mode,initialized_at) VALUES(true,'fresh',$1) ON CONFLICT DO NOTHING",[boot_time.into()])).await.unwrap().rows_affected()==1;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("bindings.json");
    let config = json!({"runtimes":{plugin.clone():{"generation":1,"namespace":"fixtures","service":"fixture-plugin","credentialSha256":credential,"peers":[{"kind":"unix","uid":rustix::process::geteuid().as_raw()}],"projects":{pid.clone():{"provider":{"origin":"https://provider.invalid","addresses":["127.0.0.1"],"headers":{},"rules":[{"method":"GET","path":"/v1/items","query":["limit"]}],"ca":null}}}}}});
    std::fs::write(&path, config.to_string()).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let cookie_policy =
        CookiePolicy::new(false, "fixture-host-cookie-secret-at-least-32-bytes").unwrap();
    let cookie = format!(
        "{}={}",
        cookie_policy.name(),
        cookie_policy.sign(&token).unwrap()
    );
    let identity = Arc::new(IdentityService::new(
        db("AUTH_DATABASE_URL").await,
        cookie_policy,
    ));
    let principal = identity
        .authenticate_cookie(&cookie)
        .await
        .unwrap()
        .unwrap();
    let mcp_fixture =
        Arc::new(mcp::McpFixture::new(&owner, identity.clone(), &cookie, &nonce).await);
    let mcp = mcp_fixture.clone();
    let host = Arc::new(
        HostService::new(
            db("DATABASE_URL").await,
            db("WEBDOCK_PLUGIN_WORKER_DATABASE_URL").await,
            std::env::var("DATABASE_RLS_SECRET").unwrap(),
            Arc::new(webdock_persistence::IdAllocator::new(
                db("DATABASE_URL").await,
                webdock_ids::Identity::platform(0, 2).unwrap(),
            )),
            Arc::new(FileBindings::new(path.clone())),
            ed25519_dalek::SigningKey::from_bytes(&[19; 32]),
        )
        .await
        .unwrap()
        .with_session_authority(db("AUTH_DATABASE_URL").await, identity.clone(), None)
        .await
        .unwrap()
        .with_mcp_authority(mcp.oauth.clone())
        .unwrap(),
    );
    let outcome = {
        let owner = owner.clone();
        let plugin = plugin.clone();
        let cid = cid.clone();
        let pid = pid.clone();
        let uid = uid.clone();
        async move {
            let admitted = host
                .admit_with_authority(
                    &principal,
                    admission(&plugin, &pid, "notes.write", "operation-1"),
                    true,
                    &webdock_browser_session::StoredAuthority::LocalSession {},
                )
                .await
                .unwrap();
            let authority = host.runtime_authority(plugin.clone(), 1).unwrap();
            authority
                .authorize_peer(&PeerIdentity::Unix {
                    uid: rustix::process::geteuid().as_raw(),
                })
                .await
                .unwrap();
            assert!(
                authority
                    .authorize_peer(&PeerIdentity::Unix { uid: 43 })
                    .await
                    .is_err()
            );
            authority
                .authorize_invocation(
                    &PeerIdentity::Unix {
                        uid: rustix::process::geteuid().as_raw(),
                    },
                    &admitted.claims,
                )
                .await
                .unwrap();
            let handle = host
                .authenticate(
                    PeerIdentity::Unix {
                        uid: rustix::process::geteuid().as_raw(),
                    },
                    &admitted.token,
                )
                .await
                .unwrap();
            host.verify_request(&handle, &json!({}), &admitted.configuration)
                .await
                .unwrap();
            assert!(
                host.verify_request(&handle, &json!({"different":true}), &admitted.configuration)
                    .await
                    .is_err()
            );
            assert!(
                host.verify_request(&handle, &json!({}), &json!({"different":true}))
                    .await
                    .is_err()
            );
            remote_host_roundtrip(
                host.clone(),
                &plugin,
                &raw,
                &admitted,
                identity.clone(),
                &cookie,
                &mcp,
            )
            .await;
            invocation_revoke_race(&owner, &principal, &plugin, &cid, &pid, &path).await;
            let write = json!({"key":"notes.v1","value":{"Z":1,"a":2},"expectedRevision":null,"idempotencyKey":"request-1"});
            let mut changed = admission(&plugin, &pid, "notes.write", "operation-1");
            changed.input = json!({"changed":true});
            assert!(host.admit(&principal, changed).await.is_err());
            assert!(host.data_write(&handle,json!({"key":"receipt-overflow","value":"x".repeat(65520),"expectedRevision":null,"idempotencyKey":"overflow-1"})).await.is_err());
            let remaining:i64=owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT count(*) AS n FROM webdock_auth.plugin_document WHERE plugin_id=$1 AND key='receipt-overflow'",[plugin.clone().into()])).await.unwrap().unwrap().try_get("","n").unwrap();
            assert_eq!(
                remaining, 0,
                "receipt failure rolls document back atomically"
            );
            let first = host.data_write(&handle, write.clone()).await.unwrap();
            assert_eq!(first["revision"], "1");
            assert_eq!(
                host.data_write(&handle, write.clone()).await.unwrap(),
                first
            );
            assert!(host.data_write(&handle,json!({"key":"notes.v1","value":"different","expectedRevision":null,"idempotencyKey":"request-1"})).await.is_err());
            assert!(host.data_write(&handle,json!({"key":"notes.v1","value":"stale","expectedRevision":null,"idempotencyKey":"request-2"})).await.is_err());
            assert_eq!(host.data_write(&handle,json!({"key":"notes.v1","value":"updated","expectedRevision":"1","idempotencyKey":"request-2"})).await.unwrap()["revision"],"2");
            assert!(
                host.data_read(&handle, "notes.v1").await.is_err(),
                "write-only invocation cannot read through read service"
            );
            let read = host
                .admit(
                    &principal,
                    admission(&plugin, &pid, "notes.read", "operation-2"),
                )
                .await
                .unwrap();
            let reader = host
                .authenticate(
                    PeerIdentity::Unix {
                        uid: rustix::process::geteuid().as_raw(),
                    },
                    &read.token,
                )
                .await
                .unwrap();
            assert_eq!(
                host.data_read(&reader, "notes.v1").await.unwrap()["value"],
                "updated"
            );
            assert!(host.data_write(&reader, write.clone()).await.is_err());
            let mut other_project = read.claims.clone();
            other_project.project_id = "1".into();
            assert!(
                authority
                    .authorize_invocation(
                        &PeerIdentity::Unix {
                            uid: rustix::process::geteuid().as_raw()
                        },
                        &other_project
                    )
                    .await
                    .is_err()
            );
            sql(
                &owner,
                "UPDATE webdock_auth.plugin_release SET manifest_bytes='{}' WHERE plugin_id=$1",
                vec![plugin.clone().into()],
            )
            .await;
            assert!(host.revalidate(&reader).await.is_err());
            sql(
                &owner,
                "UPDATE webdock_auth.plugin_release SET manifest_bytes=$2 WHERE plugin_id=$1",
                vec![plugin.clone().into(), raw.clone().into()],
            )
            .await;
            let mut wrong = read.claims.clone();
            wrong.generation = 2;
            assert!(
                authority
                    .authorize_invocation(
                        &PeerIdentity::Unix {
                            uid: rustix::process::geteuid().as_raw()
                        },
                        &wrong
                    )
                    .await
                    .is_err()
            );
            sql(
                &owner,
                "UPDATE webdock_auth.plugin_configuration SET revision=1 WHERE plugin_id=$1",
                vec![plugin.clone().into()],
            )
            .await;
            assert!(host.revalidate(&reader).await.is_err());
            sql(&owner,"UPDATE webdock_auth.plugin_runtime SET configuration_revision=1 WHERE plugin_id=$1",vec![plugin.clone().into()]).await;
            assert!(
                host.revalidate(&reader).await.is_err(),
                "matching new runtime configuration does not revive an old invocation"
            );
            sql(&owner,"UPDATE webdock_auth.plugin_runtime SET configuration_revision=0 WHERE plugin_id=$1",vec![plugin.clone().into()]).await;
            sql(
                &owner,
                "UPDATE webdock_auth.plugin_configuration SET revision=0 WHERE plugin_id=$1",
                vec![plugin.clone().into()],
            )
            .await;
            sql(
                &owner,
                "UPDATE webdock_admin.projects SET status='archived' WHERE id=$1",
                vec![pid.clone().into()],
            )
            .await;
            assert!(host.revalidate(&reader).await.is_err());
            sql(
                &owner,
                "UPDATE webdock_admin.projects SET status='active' WHERE id=$1",
                vec![pid.clone().into()],
            )
            .await;
            #[cfg(feature = "test-transport")]
            local_http_checks(&host, &principal, &plugin, &pid, &owner).await;
            let state = host
                .request_state(&principal, &plugin, "draining", "0", "state-command-1")
                .await
                .unwrap();
            assert_eq!(
                host.request_state(&principal, &plugin, "draining", "0", "state-command-1")
                    .await
                    .unwrap(),
                state
            );
            assert!(
                host.admit(
                    &principal,
                    admission(&plugin, &pid, "notes.read", "new-work")
                )
                .await
                .is_err()
            );
            host.revalidate(&reader).await.unwrap();
            assert!(
                host.request_state(&principal, &plugin, "disabled", "1", "state-command-2")
                    .await
                    .is_err()
            );
            sql(
                &owner,
                "UPDATE webdock_auth.plugin_installation SET state='disabled' WHERE id=$1",
                vec![plugin.clone().into()],
            )
            .await;
            assert!(host.revalidate(&reader).await.is_err());
            sql(
                &owner,
                "UPDATE webdock_auth.plugin_installation SET state='enabled' WHERE id=$1",
                vec![plugin.clone().into()],
            )
            .await;
            // A self-service policy change bumps every grant revision of the plugin.
            sql(
                &owner,
                "UPDATE webdock_auth.plugin_grant SET revision=revision+1 WHERE plugin_id=$1",
                vec![plugin.clone().into()],
            )
            .await;
            assert!(host.revalidate(&reader).await.is_err());
            let fresh = host
                .admit(
                    &principal,
                    admission(&plugin, &pid, "notes.read", "fresh-operation"),
                )
                .await
                .unwrap();
            let fresh = host
                .authenticate(
                    PeerIdentity::Unix {
                        uid: rustix::process::geteuid().as_raw(),
                    },
                    &fresh.token,
                )
                .await
                .unwrap();
            sql(
                &owner,
                "UPDATE webdock_auth.\"user\" SET banned=true WHERE id=$1",
                vec![uid.clone().into()],
            )
            .await;
            assert!(host.data_read(&fresh, "notes.v1").await.is_err());
            sql(
                &owner,
                "UPDATE webdock_auth.\"user\" SET banned=false WHERE id=$1",
                vec![uid.clone().into()],
            )
            .await;
            let mut config = config;
            config["runtimes"][&plugin]["peers"] = json!([]);
            std::fs::write(&path, config.to_string()).unwrap();
            assert!(host.revalidate(&fresh).await.is_err());
            let runtime = db("DATABASE_URL").await;
            let count: i64 = runtime
                .query_one_raw(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT count(*) AS n FROM webdock_auth.plugin_document WHERE plugin_id=$1",
                    [plugin.clone().into()],
                ))
                .await
                .unwrap()
                .unwrap()
                .try_get("", "n")
                .unwrap();
            assert_eq!(count, 0);
            config["runtimes"][&plugin]["peers"] =
                json!([{"kind":"unix","uid":rustix::process::geteuid().as_raw()}]);
            std::fs::write(&path, config.to_string()).unwrap();
            controller::roundtrip(host.clone(), &plugin, &raw, identity, &cookie).await;
        }
    };
    let outcome = tokio::spawn(outcome).await;
    mcp_fixture.cleanup(&owner).await;
    sql(
        &owner,
        "DELETE FROM webdock_mail.send_job WHERE source_id=$1",
        vec![format!("plugin-controller:{plugin}").into()],
    )
    .await;
    for table in [
        "plugin_document_receipt",
        "plugin_document",
        "plugin_invocation",
        "plugin_grant",
        "plugin_runtime",
        "plugin_configuration",
        "plugin_release",
        "plugin_operation",
    ] {
        sql(
            &owner,
            &format!("DELETE FROM webdock_auth.{table} WHERE plugin_id=$1"),
            vec![plugin.clone().into()],
        )
        .await;
    }
    sql(
        &owner,
        "DELETE FROM webdock_auth.plugin_installation WHERE id=$1",
        vec![plugin.clone().into()],
    )
    .await;
    sql(
        &owner,
        "DELETE FROM webdock_auth.access_event WHERE actor_id=$1",
        vec![uid.clone().into()],
    )
    .await;
    sql(
        &owner,
        "DELETE FROM webdock_admin.projects WHERE id=$1",
        vec![pid.into()],
    )
    .await;
    sql(
        &owner,
        "DELETE FROM webdock_auth.tenant_customer WHERE customer_id=$1",
        vec![cid.clone().into()],
    )
    .await;
    sql(
        &owner,
        "DELETE FROM webdock_admin.customers WHERE id=$1",
        vec![cid.into()],
    )
    .await;
    sql(
        &owner,
        "DELETE FROM webdock_auth.organization WHERE name=$1",
        vec![nonce.clone().into()],
    )
    .await;
    sql(
        &owner,
        "DELETE FROM webdock_admin.users WHERE id=$1",
        vec![mapped.into()],
    )
    .await;
    sql(
        &owner,
        "DELETE FROM webdock_auth.session WHERE id=$1",
        vec![sid.into()],
    )
    .await;
    sql(
        &owner,
        "DELETE FROM webdock_auth.\"user\" WHERE id=$1",
        vec![uid.into()],
    )
    .await;
    if inserted_boot {
        sql(
            &owner,
            "DELETE FROM webdock_auth.plugin_bootstrap WHERE initialized_at=$1",
            vec![boot_time.into()],
        )
        .await;
    }
    outcome.unwrap();
}
#[cfg(feature = "test-transport")]
async fn local_http_checks(
    host: &HostService,
    principal: &webdock_identity::Principal,
    plugin: &str,
    project: &str,
    owner: &DatabaseConnection,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use webdock_plugin_host::http::{BoundRequest, HttpResult, HttpTransport};
    struct Local {
        status: u16,
        revoke: Option<(DatabaseConnection, String)>,
    }
    #[async_trait::async_trait]
    impl HttpTransport for Local {
        async fn send(&self, input: BoundRequest) -> webdock_plugin_protocol::Result<HttpResult> {
            assert_eq!(input.url.host_str(), Some("provider.invalid"));
            assert_eq!(
                input.binding.addresses,
                vec!["127.0.0.1".parse::<std::net::IpAddr>().unwrap()]
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let status = self.status;
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut input = [0; 4096];
                assert!(stream.read(&mut input).await.unwrap() > 0);
                let body = "{\"source\":\"local-test\"}";
                stream.write_all(format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
            });
            let result = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap()
                .get(format!("http://{address}/controlled"))
                .send()
                .await
                .unwrap();
            let status = result.status().as_u16();
            let body = serde_json::from_slice(&result.bytes().await.unwrap()).unwrap();
            server.await.unwrap();
            if let Some((owner, plugin)) = &self.revoke {
                sql(
                    owner,
                    "UPDATE webdock_auth.plugin_grant SET revision=revision+1 WHERE plugin_id=$1",
                    vec![plugin.clone().into()],
                )
                .await;
            }
            Ok(HttpResult { status, body })
        }
    }
    let admitted = host
        .admit(
            principal,
            admission(plugin, project, "provider.read", "provider-operation"),
        )
        .await
        .unwrap();
    let handle = host
        .authenticate(
            PeerIdentity::Unix {
                uid: rustix::process::geteuid().as_raw(),
            },
            &admitted.token,
        )
        .await
        .unwrap();
    let input = json!({"binding":"provider","method":"GET","path":"/v1/items?limit=10"});
    assert_eq!(
        host.http_request_with_test_transport(
            &handle,
            input.clone(),
            &Local {
                status: 200,
                revoke: None
            }
        )
        .await
        .unwrap()
        .body["source"],
        "local-test"
    );
    for status in [302, 401, 403, 503] {
        assert!(
            host.http_request_with_test_transport(
                &handle,
                input.clone(),
                &Local {
                    status,
                    revoke: None
                }
            )
            .await
            .is_err()
        );
    }
    for path in [
        "/v1/items?limit=1&limit=2",
        "/v1/items?secret=1",
        "/v1/other",
        "//evil.invalid/v1/items",
    ] {
        assert!(
            host.http_request_with_test_transport(
                &handle,
                json!({"binding":"provider","method":"GET","path":path}),
                &Local {
                    status: 200,
                    revoke: None
                }
            )
            .await
            .is_err()
        );
    }
    assert!(
        host.http_request_with_test_transport(
            &handle,
            input,
            &Local {
                status: 200,
                revoke: Some((owner.clone(), plugin.into()))
            }
        )
        .await
        .is_err(),
        "withhold HTTP result after grant revocation during exchange"
    );
    sql(
        owner,
        "UPDATE webdock_auth.plugin_grant SET revision=0 WHERE plugin_id=$1",
        vec![plugin.into()],
    )
    .await;
}

async fn remote_host_roundtrip(
    host: Arc<HostService>,
    plugin: &str,
    manifest: &str,
    admitted: &webdock_plugin_host::AdmittedInvocation,
    identity: Arc<IdentityService>,
    cookie: &str,
    mcp: &mcp::McpFixture,
) {
    use std::os::unix::fs::PermissionsExt;
    use webdock_plugin_protocol::{InvocationClaims, Manifest, PluginError};
    use webdock_plugin_runtime::{
        ActionHandler, CorePeer, HostClient, RemoteAuthority, Runtime, UnixSocket,
    };
    struct Document(HostClient);
    #[tonic::async_trait]
    impl ActionHandler for Document {
        async fn invoke(
            &self,
            claims: &InvocationClaims,
            token: &str,
            input: Value,
            _: Value,
        ) -> Result<Value, PluginError> {
            if claims.action == "notes.read" {
                return self
                    .0
                    .call(token, "data", "read", json!({"key":"rpc.fixture"}))
                    .await;
            }
            self.0.call(token,"data","write",json!({"key":input["key"].as_str().unwrap_or("rpc.fixture"),"value":1,"expectedRevision":null,"idempotencyKey":"rpc-document-1"})).await
        }
    }
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let host_path = directory.path().join("host.sock");
    let plugin_path = directory.path().join("plugin.sock");
    let mut socket = UnixSocket::bind(&host_path).unwrap();
    let incoming = socket.incoming().unwrap();
    let service = host.clone().rpc();
    let host_task = tokio::spawn(async move {
        let _socket = socket;
        tonic::transport::Server::builder()
            .add_service(service)
            .serve_with_incoming(incoming)
            .await
            .unwrap()
    });
    struct Tasks(Vec<tokio::task::JoinHandle<()>>);
    impl Drop for Tasks {
        fn drop(&mut self) {
            for task in &self.0 {
                task.abort();
            }
        }
    }
    let mut tasks = Tasks(vec![host_task]);
    let client = HostClient::connect_unix(&host_path).await.unwrap();
    let authority = RemoteAuthority::new(
        client.clone(),
        plugin.into(),
        1,
        vec![CorePeer::Unix {
            uid: rustix::process::geteuid().as_raw(),
        }],
    )
    .unwrap();
    let runtime = Runtime::new(
        Manifest::parse(manifest.as_bytes()).unwrap(),
        1,
        ed25519_dalek::SigningKey::from_bytes(&[19; 32]).verifying_key(),
        Arc::new(authority),
        Arc::new(Document(client)),
    )
    .unwrap();
    let socket = UnixSocket::bind(&plugin_path).unwrap();
    tasks.0.push(tokio::spawn(async move {
        runtime
            .serve_unix(socket, std::future::pending())
            .await
            .unwrap()
    }));
    let mut client = webdock_plugin_runtime::connect_unix(&plugin_path)
        .await
        .unwrap();
    let request = |input: Value| webdock_plugin_runtime::wire::InvokeRequest {
        invocation_token: admitted.token.clone(),
        action: admitted.claims.action.clone(),
        input_json: serde_json::to_vec(&input).unwrap(),
        configuration_json: serde_json::to_vec(&admitted.configuration).unwrap(),
    };
    let result = client
        .invoke(request(json!({})))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        serde_json::from_slice::<Value>(&result.output_json).unwrap()["revision"],
        "1"
    );
    assert_eq!(
        client
            .invoke(request(json!({})))
            .await
            .unwrap()
            .into_inner()
            .output_json,
        result.output_json
    );
    assert_eq!(
        client
            .invoke(request(json!({"key":"changed"})))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::PermissionDenied
    );
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let connection_file = directory.path().join("connections.json");
    std::fs::write(&connection_file,json!({"runtimes":{plugin:{"generation":1,"transport":{"kind":"unix","path":plugin_path}}}}).to_string()).unwrap();
    std::fs::set_permissions(&connection_file, std::fs::Permissions::from_mode(0o600)).unwrap();
    let secret_path = directory.path().join("mcp.secret");
    std::fs::write(&secret_path, &mcp.secret).unwrap();
    std::fs::set_permissions(&secret_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let mut endpoints = webdock_control_plane::plugins::PluginEndpoints::new(
        host,
        webdock_control_plane::plugin_config::RuntimeConnections::new(connection_file),
        None,
    );
    endpoints.mcp_client = Some(webdock_control_plane::plugin_config::McpClientConfig {
        client_id: mcp.client.clone(),
        secret: secret_path,
    });
    let endpoints = Arc::new(endpoints);
    let app = webdock_control_plane::http::build_app_with_domains(
        webdock_core::config::PlatformConfig::from_map(&std::collections::HashMap::new()).unwrap(),
        db("AUTH_DATABASE_URL").await,
        webdock_control_plane::http::Services {
            identity: Some(identity),
            oauth: Some(mcp.oauth.clone()),
            plugins: Some(endpoints),
            ..Default::default()
        },
    );
    let body = json!({"pluginID":plugin,"projectID":admitted.claims.project_id,"action":"notes.write","input":{"key":"rpc.core"},"idempotencyKey":"core-plugin-operation-1"});
    let command = |body: Value| {
        Request::builder()
            .method("POST")
            .uri("/api/console/plugins/invoke")
            .header("host", "localhost:3120")
            .header("origin", "http://localhost:3120")
            .header("content-type", "application/json")
            .header("cookie", cookie)
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let response = app.clone().oneshot(command(body.clone())).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        serde_json::from_slice::<Value>(&response).unwrap()["revision"],
        "1"
    );
    let replay = app.clone().oneshot(command(body.clone())).await.unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(
        replay.into_body().collect().await.unwrap().to_bytes(),
        response
    );
    let mut changed = body;
    changed["input"]["key"] = json!("different");
    assert_eq!(
        app.clone()
            .oneshot(command(changed))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    let rpc = |token: &str, body: Value| {
        Request::builder()
            .method("POST")
            .uri("/api/mcp")
            .header("host", "localhost:3120")
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("mcp-protocol-version", "2025-11-25")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let listed = app
        .clone()
        .oneshot(rpc(
            &mcp.read,
            json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
        ))
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let listed: Value =
        serde_json::from_slice(&listed.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let tools = listed["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert!(tools[0]["name"].as_str().unwrap().ends_with("__read"));
    let write=app.clone().oneshot(rpc(&mcp.write,json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":format!("plugin__{}__write",plugin.replace('-',"_")),"arguments":{"projectID":admitted.claims.project_id,"input":{"key":"rpc.mcp"},"idempotencyKey":"mcp-fixture-1"}}}))).await.unwrap();
    assert_eq!(write.status(), StatusCode::OK);
    let output: Value =
        serde_json::from_slice(&write.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_ne!(output["result"]["isError"], true, "{output}");
    assert!(
        output["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("revision")
    );
    let denied=app.clone().oneshot(rpc(&mcp.read,json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":format!("plugin__{}__write",plugin.replace('-',"_")),"arguments":{"projectID":admitted.claims.project_id,"input":{},"idempotencyKey":"denied-write-1"}}}))).await.unwrap();
    let denied: Value =
        serde_json::from_slice(&denied.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(denied["result"]["isError"], true);
    let rest = |token: &str, method: &str| {
        Request::builder()
            .method(method)
            .uri(format!(
                "/v1/plugins/{plugin}/projects/{}/api/notes",
                admitted.claims.project_id
            ))
            .header("host", "localhost:3130")
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .header("idempotency-key", "rest-fixture-write-1")
            .body(if method == "GET" {
                Body::empty()
            } else {
                Body::from("{\"key\":\"rpc.rest\"}")
            })
            .unwrap()
    };
    assert_eq!(
        app.clone()
            .oneshot(rest(&mcp.read, "GET"))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        app.clone()
            .oneshot(rest(&mcp.read, "POST"))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.clone()
            .oneshot(rest(&mcp.write, "POST"))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let owner = db("WEBDOCK_TEST_MIGRATION_DATABASE_URL").await;
    sql(
        &owner,
        "UPDATE webdock_auth.\"oauthClient\" SET disabled=true WHERE \"clientId\"=$1",
        vec![mcp.client.clone().into()],
    )
    .await;
    assert_eq!(
        app.clone()
            .oneshot(rpc(
                &mcp.write,
                json!({"jsonrpc":"2.0","id":4,"method":"tools/list"})
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    sql(
        &owner,
        "UPDATE webdock_auth.\"oauthClient\" SET disabled=false WHERE \"clientId\"=$1",
        vec![mcp.client.clone().into()],
    )
    .await;
}

async fn bounded_database(key: &str, name: &str) -> DatabaseConnection {
    let raw = std::env::var(key).unwrap();
    let url = url::Url::parse(&raw).unwrap();
    assert!(
        matches!(url.host_str(), Some("localhost" | "127.0.0.1"))
            && url.path() == "/webdock_rls_test"
            && url.query().is_none()
    );
    let mut options = sea_orm::ConnectOptions::new(raw);
    options
        .max_connections(1)
        .min_connections(1)
        .acquire_timeout(std::time::Duration::from_millis(750));
    let db = Database::connect(options).await.unwrap();
    sql(
        &db,
        "SELECT set_config('application_name',$1,false)",
        vec![name.into()],
    )
    .await;
    db
}
async fn invocation_revoke_race(
    owner: &DatabaseConnection,
    principal: &webdock_identity::Principal,
    plugin: &str,
    customer: &str,
    project: &str,
    path: &std::path::Path,
) {
    use sea_orm::TransactionTrait;
    let suffix = &hex::encode(Sha256::digest(plugin.as_bytes()))[..16];
    let data_name = format!("fixture-fence-data-{suffix}");
    let control_name = format!("fixture-fence-control-{suffix}");
    let host = Arc::new(
        HostService::new(
            bounded_database("DATABASE_URL", &data_name).await,
            bounded_database("WEBDOCK_PLUGIN_WORKER_DATABASE_URL", &control_name).await,
            std::env::var("DATABASE_RLS_SECRET").unwrap(),
            Arc::new(webdock_persistence::IdAllocator::new(
                db("DATABASE_URL").await,
                webdock_ids::Identity::platform(0, 2).unwrap(),
            )),
            Arc::new(FileBindings::new(path.into())),
            ed25519_dalek::SigningKey::from_bytes(&[19; 32]),
        )
        .await
        .unwrap(),
    );
    let admitted = host
        .admit(
            principal,
            admission(plugin, project, "notes.write", "fence-operation-1"),
        )
        .await
        .unwrap();
    let handle = host
        .authenticate(
            PeerIdentity::Unix {
                uid: rustix::process::geteuid().as_raw(),
            },
            &admitted.token,
        )
        .await
        .unwrap();
    let blocker = owner.begin().await.unwrap();
    blocker
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
            [json!([plugin, customer, project, "fence.row"])
                .to_string()
                .into()],
        ))
        .await
        .unwrap();
    let writer_host = host.clone();
    let mut writer = tokio::spawn(async move {
        writer_host.data_write(&handle,json!({"key":"fence.row","value":1,"expectedRevision":null,"idempotencyKey":"fence-request-1"})).await
    });
    struct Abort(Vec<tokio::task::AbortHandle>);
    impl Drop for Abort {
        fn drop(&mut self) {
            for task in &self.0 {
                task.abort();
            }
        }
    }
    let mut cleanup = Abort(vec![writer.abort_handle()]);
    async fn waiting(owner: &DatabaseConnection, name: &str, kind: &str) {
        tokio::time::timeout(std::time::Duration::from_secs(2),async{loop{let blocked:bool=owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT EXISTS(SELECT 1 FROM pg_locks l JOIN pg_stat_activity a ON a.pid=l.pid WHERE a.datname=current_database() AND a.application_name=$1 AND NOT l.granted AND l.locktype=$2) AS blocked",[name.into(),kind.into()])).await.unwrap().unwrap().try_get("","blocked").unwrap();if blocked{break;}tokio::time::sleep(std::time::Duration::from_millis(10)).await;}}).await.expect("fixture must reach its explicit transaction fence");
    }
    waiting(owner, &data_name, "advisory").await;
    let revoker_host = host.clone();
    let token = admitted.token.clone();
    let mut revoker = tokio::spawn(async move { revoker_host.revoke_issued(&token).await });
    cleanup.0.push(revoker.abort_handle());
    waiting(owner, &control_name, "transactionid").await;
    assert!(
        !revoker.is_finished(),
        "revocation waits for the already-admitted effect"
    );
    blocker.rollback().await.unwrap();
    let (write, revoked) = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        tokio::join!(&mut writer, &mut revoker)
    })
    .await
    .expect("single-connection pools must not deadlock on final revalidation");
    assert_eq!(write.unwrap().unwrap()["revision"], "1");
    revoked.unwrap().unwrap();
    assert!(
        host.authenticate(
            PeerIdentity::Unix {
                uid: rustix::process::geteuid().as_raw()
            },
            &admitted.token
        )
        .await
        .is_err()
    );
}
