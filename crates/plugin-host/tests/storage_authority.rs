//! Fresh owned PostgreSQL fixture; never accesses retained customer services.
#[allow(dead_code, reason = "Shared owned fixture serves jobs as well")]
#[path = "support/jobs_fixture.rs"]
mod fixture;
use sea_orm::{ConnectionTrait, DbBackend, Statement, TransactionTrait};
use serde_json::{Value, json};
use std::sync::Arc;
use webdock_identity::{CookiePolicy, IdentityService};
use webdock_plugin_host::{
    AdmissionInput, FileBindings, HostService,
    storage::{StorageCaller, StorageGateway},
};
use webdock_plugin_protocol::storage::StoragePlan;
use webdock_plugin_runtime::PeerIdentity;
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
async fn setup() -> fixture::JobFixture {
    let mut f = fixture::JobFixture::start_with_connections(4).await;
    f.manifest["capabilities"]
        .as_array_mut()
        .unwrap()
        .extend([json!("plugin.storage.read"), json!("plugin.storage.write")]);
    f.manifest["services"]["required"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"storage","version":1}));
    f.manifest["actions"].as_array_mut().unwrap().extend([
 json!({"name":"download","intent":"read","audience":"customer","feature":"run","resource":"use","storage":"download","capabilities":["plugin.storage.read"],"input":{"type":"object","properties":{"resource_id":{"type":"string","maxLength":128}},"required":["resource_id"],"additionalProperties":false}}),
 json!({"name":"upload","intent":"write","audience":"customer","feature":"run","resource":"use","storage":"upload","capabilities":["plugin.storage.write"],"input":{"type":"object","properties":{"resource_id":{"type":"string","maxLength":128}},"required":["resource_id"],"additionalProperties":false}})]);
    let raw = f.manifest.to_string();
    use sha2::{Digest, Sha256};
    sql(
        &f.owner,
        "UPDATE webdock_auth.plugin_release SET manifest_bytes=$1,manifest_sha256=$2",
        vec![
            raw.clone().into(),
            hex::encode(Sha256::digest(raw.as_bytes())).into(),
        ],
    )
    .await;
    sql(
        &f.owner,
        "UPDATE webdock_auth.plugin_configuration SET approved_capabilities=$1",
        vec![f.manifest["capabilities"].clone().into()],
    )
    .await;
    let path = f.directory.path().join("bindings.json");
    let mut bindings: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    bindings["runtimes"]["job-fixture"]["storage"] = json!({"124":{"objects":{"origin":"https://storage.invalid","addresses":["127.0.0.1"],"ca":null,"pathPrefix":"/private/"}}});
    std::fs::write(path, bindings.to_string()).unwrap();
    f.host = Arc::new(
        Arc::try_unwrap(f.host)
            .ok()
            .unwrap()
            .with_storage_gateway(Arc::new(
                StorageGateway::new(f.directory.path().into()).unwrap(),
            )),
    );
    f
}
fn input(action: &str, resource: &str) -> AdmissionInput {
    serde_json::from_value(json!({"pluginID":"job-fixture","projectID":"124","action":action,"input":{"resource_id":resource},"idempotencyKey":uuid::Uuid::new_v4().to_string()})).unwrap()
}
async fn ticket(f: &fixture::JobFixture) -> String {
    let authority = webdock_browser_session::StoredAuthority::local_session(&f.actor);
    let a = f
        .host
        .admit_with_authority(
            &f.actor,
            input("download", "resource-one"),
            true,
            &authority,
        )
        .await
        .unwrap();
    retain(f, a, None).await
}
async fn retain(
    f: &fixture::JobFixture,
    a: webdock_plugin_host::AdmittedInvocation,
    api_hash: Option<String>,
) -> String {
    let direction = if a.claims.action == "upload" {
        "upload"
    } else {
        "download"
    };
    let h = f
        .host
        .authenticate(
            PeerIdentity::Unix {
                uid: rustix::process::geteuid().as_raw(),
            },
            &a.token,
        )
        .await
        .unwrap();
    let p:StoragePlan=serde_json::from_value(json!({"version":1,"binding":"objects","direction":direction,"resourceEpoch":"active-1","resourceId":"resource-one","url":"https://storage.invalid/private/key?X-Amz-Signature=private","headers":{},"expiresAt":chrono::Utc::now().timestamp()+300,"exactBytes":3,"expectedSha256":"a".repeat(64),"attachment":{"filename":"file.bin","contentType":"application/octet-stream"}})).unwrap();
    let d = f
        .host
        .retain_storage_plan_with_api(&h, p, "c".repeat(64), api_hash)
        .await
        .unwrap();
    f.host.revoke_issued(&a.token).await.unwrap();
    d.id
}
fn caller(f: &fixture::JobFixture) -> StorageCaller {
    StorageCaller::browser(
        &f.actor,
        webdock_browser_session::StoredAuthority::local_session(&f.actor),
        true,
    )
}
#[tokio::test]
#[ignore = "owns a disposable PostgreSQL18 container and private runtime fixture"]
async fn transfer_uses_live_subject_resource_snapshot_and_source_session_after_json_token_ends() {
    let f = setup().await;
    let id = ticket(&f).await;
    assert!(f.host.storage_access(&id, "GET", &caller(&f)).await.is_ok());
    assert!(
        f.host
            .storage_access(&id, "PUT", &caller(&f))
            .await
            .is_err()
    );
    let tx = f.auth.begin().await.unwrap();
    let foreign = f
        .identity
        .authenticate_session_in(&tx, "member", "member-session")
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    let foreign = StorageCaller::browser(
        &foreign,
        webdock_browser_session::StoredAuthority::local_session(&foreign),
        true,
    );
    assert!(f.host.storage_access(&id, "GET", &foreign).await.is_err());
    let foreign_workspace = StorageCaller::workspace(
        &f.actor,
        webdock_browser_session::StoredAuthority::local_session(&f.actor),
        true,
        "999".into(),
    );
    assert!(
        f.host
            .storage_access(&id, "GET", &foreign_workspace)
            .await
            .is_err()
    );
    let path = f.directory.path().join("bindings.json");
    let original = std::fs::read(&path).unwrap();
    let mut rotated: Value = serde_json::from_slice(&original).unwrap();
    rotated["runtimes"]["job-fixture"]["peers"][0]["uid"] =
        json!(rustix::process::geteuid().as_raw() + 1);
    std::fs::write(&path, rotated.to_string()).unwrap();
    assert!(
        f.host
            .storage_access(&id, "GET", &caller(&f))
            .await
            .is_err()
    );
    std::fs::write(path, original).unwrap();
    assert!(
        f.host
            .admit_with_authority(
                &f.actor,
                input("upload", "resource-one"),
                false,
                &webdock_browser_session::StoredAuthority::local_session(&f.actor)
            )
            .await
            .is_err()
    );
    sql(&f.owner,"UPDATE webdock_auth.plugin_invocation SET session_context=jsonb_set(session_context,'{storageTransfer,expiresAt}',to_jsonb((session_context#>>'{storageTransfer,expiresAt}')::bigint+1)) WHERE id=$1::uuid",vec![id.clone().into()]).await;
    assert!(
        f.host
            .storage_access(&id, "GET", &caller(&f))
            .await
            .is_err()
    );
    sql(&f.owner,"UPDATE webdock_auth.plugin_invocation SET session_context=jsonb_set(session_context,'{storageTransfer,expiresAt}',to_jsonb((session_context#>>'{storageTransfer,expiresAt}')::bigint-1)) WHERE id=$1::uuid",vec![id.clone().into()]).await;
    assert!(
        f.host
            .storage_access(&uuid::Uuid::new_v4().to_string(), "GET", &caller(&f))
            .await
            .is_err()
    );
    sql(&f.owner,"UPDATE webdock_auth.plugin_invocation SET created_at=now()-interval '120 seconds',expires_at=now()-interval '60 seconds' WHERE id=$1::uuid",vec![id.clone().into()]).await;
    assert!(f.host.storage_access(&id, "GET", &caller(&f)).await.is_ok());
    sql(&f.owner,"INSERT INTO webdock_admin.projects(id,name,customer_id) VALUES('999','Foreign project','123')",vec![]).await;
    sql(
        &f.owner,
        "UPDATE webdock_auth.plugin_resource SET project_id='999' WHERE resource_id='resource-one'",
        vec![],
    )
    .await;
    assert!(
        f.host
            .storage_access(&id, "GET", &caller(&f))
            .await
            .is_err()
    );
    sql(
        &f.owner,
        "UPDATE webdock_auth.plugin_resource SET project_id='124' WHERE resource_id='resource-one'",
        vec![],
    )
    .await;
    sql(
        &f.owner,
        "UPDATE webdock_auth.plugin_configuration SET revision=revision+1",
        vec![],
    )
    .await;
    assert!(
        f.host
            .storage_access(&id, "GET", &caller(&f))
            .await
            .is_err()
    );
}
#[tokio::test]
#[ignore = "owns a disposable PostgreSQL18 container and private runtime fixture"]
async fn transfer_revokes_on_original_session_loss_and_does_not_store_signed_url_plaintext() {
    let f = setup().await;
    let id = ticket(&f).await;
    let row: Value = f
        .owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT session_context FROM webdock_auth.plugin_invocation WHERE id=$1::uuid",
            [id.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "session_context")
        .unwrap();
    assert!(!row.to_string().contains("Signature"));
    assert!(!row.to_string().contains("storage.invalid"));
    sql(&f.owner,"UPDATE webdock_auth.session SET \"expiresAt\"=now()-interval '1 second' WHERE id='session'",vec![]).await;
    assert!(
        f.host
            .storage_access(&id, "GET", &caller(&f))
            .await
            .is_err()
    );
}

struct PrivateProducer {
    active: std::sync::atomic::AtomicBool,
    plan: StoragePlan,
}
#[async_trait::async_trait]
impl webdock_plugin_runtime::ActionHandler for PrivateProducer {
    async fn invoke(
        &self,
        _: &webdock_plugin_protocol::InvocationClaims,
        _: &str,
        _: Value,
        _: Value,
    ) -> webdock_plugin_protocol::Result<Value> {
        Ok(serde_json::to_value(&self.plan).unwrap())
    }
    async fn authorize_storage(
        &self,
        authorization: &webdock_plugin_protocol::storage::StorageAuthorization,
    ) -> webdock_plugin_protocol::Result<()> {
        use std::sync::atomic::Ordering;
        if self.active.load(Ordering::SeqCst)
            && authorization.resource_id == "resource-one"
            && authorization.resource_epoch == "active-1"
            && authorization.project_id == "124"
        {
            Ok(())
        } else {
            Err(webdock_plugin_protocol::PluginError::denied())
        }
    }
}
struct ServerTask(tokio::task::JoinHandle<()>);
impl Drop for ServerTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}
#[tokio::test]
#[ignore = "owns a disposable PostgreSQL18 container and private runtime fixture"]
async fn core_intercepts_private_plan_and_private_runtime_deactivation_denies_consumption_and_delivery()
 {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use std::sync::atomic::Ordering;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tower::ServiceExt;
    use webdock_control_plane::{
        http::{Services, build_app_with_domains},
        plugin_config::RuntimeConnections,
        plugins::PluginEndpoints,
    };
    use webdock_plugin_protocol::Manifest;
    use webdock_plugin_runtime::{Runtime, UnixSocket};
    let mut f = setup().await;
    // This controlled provider serves only this fixture's exact private bytes.
    let bytes = vec![29u8; 3 * 1024 * 1024 + 9];
    use sha2::{Digest, Sha256};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let provider = ServerTask(tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut input = [0; 8192];
        let _ = socket.read(&mut input).await.unwrap();
        socket
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    bytes.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        socket.write_all(&bytes).await.unwrap();
    }));
    let binding_path = f.directory.path().join("bindings.json");
    let mut bindings: Value =
        serde_json::from_slice(&std::fs::read(&binding_path).unwrap()).unwrap();
    bindings["runtimes"]["job-fixture"]["storage"]["124"]["objects"]["origin"] = json!(origin);
    std::fs::write(binding_path, bindings.to_string()).unwrap();
    f.host = Arc::new(
        Arc::try_unwrap(f.host)
            .ok()
            .unwrap()
            .with_storage_gateway(Arc::new(
                StorageGateway::loopback_test(f.directory.path().into()).unwrap(),
            )),
    );
    let bytes = vec![29u8; 3 * 1024 * 1024 + 9];
    let plan:StoragePlan=serde_json::from_value(json!({"version":1,"binding":"objects","resourceEpoch":"active-1","resourceId":"resource-one","direction":"download","url":format!("{origin}/private/object?X-Amz-Signature=private"),"headers":{},"expiresAt":chrono::Utc::now().timestamp()+300,"exactBytes":bytes.len(),"expectedSha256":hex::encode(Sha256::digest(&bytes)),"attachment":{"filename":"file.bin","contentType":"application/octet-stream"}})).unwrap();
    let handler = Arc::new(PrivateProducer {
        active: std::sync::atomic::AtomicBool::new(true),
        plan,
    });
    let runtime = Runtime::new(
        Manifest::parse(f.manifest.to_string().as_bytes()).unwrap(),
        1,
        ed25519_dalek::SigningKey::from_bytes(&[19; 32]).verifying_key(),
        Arc::new(f.host.runtime_authority("job-fixture".into(), 1).unwrap()),
        handler.clone(),
    )
    .unwrap();
    let socket_path = f.directory.path().join("runtime.sock");
    let socket = UnixSocket::bind(&socket_path).unwrap();
    let _runtime = ServerTask(tokio::spawn(async move {
        runtime
            .serve_unix(socket, std::future::pending())
            .await
            .unwrap();
    }));
    let connection_path = f.directory.path().join("connections.json");
    std::fs::write(&connection_path,json!({"runtimes":{"job-fixture":{"generation":1,"transport":{"kind":"unix","path":socket_path}}}}).to_string()).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&connection_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let endpoint = Arc::new(PluginEndpoints::new(
        f.host.clone(),
        RuntimeConnections::new(connection_path.clone()),
        None,
    ));
    let tx = f.auth.begin().await.unwrap();
    let principal = f
        .identity
        .authenticate_session_in(&tx, "operator", "session")
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    let actor = webdock_control_plane::auth::AuthenticatedActor {
        authority: webdock_browser_session::StoredAuthority::local_session(&principal),
        principal,
    };
    let descriptor = endpoint
        .invoke(&actor, input("download", "resource-one"))
        .await
        .unwrap();
    let public = descriptor.to_string();
    assert!(!public.contains("X-Amz"));
    assert!(!public.contains(&origin));
    assert!(descriptor["id"].is_string());
    let app = build_app_with_domains(
        webdock_core::config::PlatformConfig::from_map(&Default::default()).unwrap(),
        f.auth.clone(),
        Services {
            identity: Some(f.identity.clone()),
            plugins: Some(endpoint),
            ..Default::default()
        },
    );
    let cookie = format!(
        "webdock-auth.session_token={}",
        CookiePolicy::new(false, "owned-plugin-jobs-cookie-key-32bytes")
            .unwrap()
            .sign("owned-token")
            .unwrap()
    );
    let request = |authenticated: bool| {
        let mut r = Request::builder()
            .uri(descriptor["path"].as_str().unwrap())
            .header("host", "localhost:3120");
        if authenticated {
            r = r.header("cookie", &cookie);
        }
        r.body(Body::empty()).unwrap()
    };
    assert_eq!(
        app.clone().oneshot(request(false)).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    handler.active.store(false, Ordering::SeqCst);
    assert_eq!(
        app.clone().oneshot(request(true)).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    handler.active.store(true, Ordering::SeqCst);
    let response = app.clone().oneshot(request(true)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(
        response.headers()["content-disposition"],
        "attachment; filename=\"file.bin\""
    );
    let mut body = response.into_body();
    let first = body.frame().await.unwrap().unwrap().into_data().unwrap();
    assert!(first.len() <= 65536);
    // Keep genuine browser + private producer authority alive past BOTH ordinary 60s rows.
    tokio::time::sleep(std::time::Duration::from_secs(61)).await;
    assert!(
        body.frame().await.unwrap().is_ok(),
        "live binary transfer survives JSON row expiry"
    );
    handler.active.store(false, Ordering::SeqCst);
    assert!(
        body.frame().await.unwrap().is_err(),
        "queued private bytes must recheck producer authority when consumed"
    );
    drop(body);
    drop(provider);
}

#[tokio::test]
#[ignore = "owns a disposable PostgreSQL18 container and private runtime fixture"]
async fn original_workspace_token_revocation_is_checked_even_for_a_live_browser_caller() {
    let f = setup().await;
    let hash = "d".repeat(64);
    sql(&f.owner,"INSERT INTO webdock_auth.api_token(id,customer_id,session_id,user_id,name,token_hash,can_write,expires_at) VALUES('444','123','session','operator','Owned test',$1,true,now()+interval '1 hour')",vec![hash.clone().into()]).await;
    let authority = webdock_browser_session::StoredAuthority::local_session(&f.actor);
    let a = f
        .host
        .admit_with_authority(
            &f.actor,
            input("download", "resource-one"),
            true,
            &authority,
        )
        .await
        .unwrap();
    let id = retain(&f, a, Some(hash)).await;
    assert!(f.host.storage_access(&id, "GET", &caller(&f)).await.is_ok());
    sql(
        &f.owner,
        "DELETE FROM webdock_auth.api_token WHERE id='444'",
        vec![],
    )
    .await;
    assert!(
        f.host
            .storage_access(&id, "GET", &caller(&f))
            .await
            .is_err()
    );
}
#[tokio::test]
#[ignore = "owns a disposable PostgreSQL18 container and private runtime fixture"]
async fn original_mcp_grant_is_rechecked_after_ticket_issuance_instead_of_becoming_link_authority()
{
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use sha2::{Digest, Sha256};
    use webdock_oauth::{
        AuthorizationOutcome, AuthorizationRequest, ClientAuthMethod, ClientAuthentication,
        ClientPurpose, ClientRegistration, CodeExchange, OAuthConfig, OAuthService,
    };
    let mut f = setup().await;
    let secret = "owned-storage-oauth-key-material-32bytes".to_owned();
    let keys = Arc::new(
        webdock_identity::SecretKeys::new(
            std::collections::BTreeMap::from([(0, secret.clone())]),
            0,
            Some(secret),
        )
        .unwrap(),
    );
    let oauth = Arc::new(OAuthService::new(
        f.auth.clone(),
        f.identity.clone(),
        Arc::new(
            webdock_persistence::IdAllocator::connect(
                f.database.url.as_str(),
                webdock_ids::Identity::platform(0, 2).unwrap(),
            )
            .await
            .unwrap(),
        ),
        keys,
        OAuthConfig::new(
            "http://localhost:3125/api/auth",
            "http://localhost:3120/api/mcp",
            None,
        )
        .unwrap(),
    ));
    oauth.provision_signing_key(&f.owner).await.unwrap();
    let client = oauth
        .provision_client(
            &f.owner,
            ClientRegistration {
                name: "Owned storage MCP".into(),
                redirect_uris: vec!["https://client.invalid/callback".into()],
                post_logout_redirect_uris: vec!["https://client.invalid/logout".into()],
                scopes: vec![
                    "openid".into(),
                    "webdock:read".into(),
                    "webdock:write".into(),
                ],
                auth_method: ClientAuthMethod::ClientSecretPost,
                allow_refresh: false,
                skip_consent: false,
                purpose: ClientPurpose::McpOperator,
            },
        )
        .await
        .unwrap();
    let credentials = || ClientAuthentication {
        client_id: client.client_id.clone(),
        client_secret: client.client_secret.clone(),
        method: ClientAuthMethod::ClientSecretPost,
    };
    let cookie = format!(
        "webdock-auth.session_token={}",
        CookiePolicy::new(false, "owned-plugin-jobs-cookie-key-32bytes")
            .unwrap()
            .sign("owned-token")
            .unwrap()
    );
    let verifier = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ-._~";
    let mut request = AuthorizationRequest::code(
        client.client_id.clone(),
        "https://client.invalid/callback".into(),
        "openid webdock:read".into(),
        URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())),
    );
    request.resources = vec!["http://localhost:3120/api/mcp".into()];
    let result = oauth.authorize(&cookie, request).await.unwrap();
    let result = match result {
        AuthorizationOutcome::ConsentRequired(c) => {
            oauth.consent(&cookie, &c.request_id, true).await.unwrap()
        }
        r => r,
    };
    let AuthorizationOutcome::Redirect(location) = result else {
        panic!("owned OAuth redirect");
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
            client: credentials(),
            code,
            redirect_uri: "https://client.invalid/callback".into(),
            code_verifier: verifier.into(),
        })
        .await
        .unwrap();
    let actor = oauth
        .authorize_mcp_reference(&credentials(), &token.access_token, "webdock:read")
        .await
        .unwrap();
    f.host = Arc::new(
        Arc::try_unwrap(f.host)
            .ok()
            .unwrap()
            .with_mcp_authority(oauth.clone())
            .unwrap(),
    );
    assert!(
        f.host
            .admit_with_mcp_authority(
                &actor.principal,
                input("upload", "resource-one"),
                actor.authority.can_write(),
                &actor.authority
            )
            .await
            .is_err()
    );
    let a = f
        .host
        .admit_with_mcp_authority(
            &actor.principal,
            input("download", "resource-one"),
            false,
            &actor.authority,
        )
        .await
        .unwrap();
    let id = retain(&f, a, None).await;
    assert!(f.host.storage_access(&id, "GET", &caller(&f)).await.is_ok());
    sql(
        &f.owner,
        "DELETE FROM webdock_auth.\"oauthAccessToken\"",
        vec![],
    )
    .await;
    assert!(
        f.host
            .storage_access(&id, "GET", &caller(&f))
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "owns a disposable PostgreSQL18 container and private runtime fixture"]
async fn storage_only_ledger_budget_accepts_sealed_peer_overhead_but_preserves_ordinary_limit() {
    let f = setup().await;
    let id = ticket(&f).await;
    // A 12KiB plan plus a large leaf seals above the ordinary authority ceiling.
    let result=f.owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,
        "UPDATE webdock_auth.plugin_invocation SET session_context=jsonb_set(session_context,'{storageTransfer,sealed}',to_jsonb($2::text)) WHERE id=$1::uuid",[id.clone().into(),format!("v1.{}", "a".repeat(24000)).into()])).await;
    assert!(
        result.is_ok(),
        "documented private envelope must fit storage-only ledger"
    );
    let denied=f.owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,
        "UPDATE webdock_auth.plugin_invocation SET session_context=(session_context-'storageTransfer') || jsonb_build_object('padding',$2::text) WHERE id=$1::uuid",[id.into(),"a".repeat(17000).into()])).await;
    assert!(
        denied.is_err(),
        "ordinary invocation context must retain 16KiB ceiling"
    );
}

#[tokio::test]
#[ignore = "owns a disposable PostgreSQL18 container and private runtime fixture"]
async fn upload_claim_is_atomic_and_cancellation_never_makes_a_ticket_reusable() {
    let f = setup().await;
    let admitted = f
        .host
        .admit_with_authority(
            &f.actor,
            input("upload", "resource-one"),
            true,
            &webdock_browser_session::StoredAuthority::local_session(&f.actor),
        )
        .await
        .unwrap();
    let id = retain(&f, admitted, None).await;
    let caller = caller(&f);
    let (a, b) = tokio::join!(
        f.host.claim_storage_upload(&id, &caller),
        f.host.claim_storage_upload(&id, &caller)
    );
    assert_ne!(
        a.is_ok(),
        b.is_ok(),
        "exactly one concurrent consumer may claim"
    );
    let access = f.host.storage_access(&id, "PUT", &caller).await.unwrap();
    struct Live;
    #[async_trait::async_trait]
    impl webdock_plugin_host::storage::StorageAuthority for Live {
        async fn check(&self) -> webdock_plugin_protocol::Result<()> {
            Ok(())
        }
    }
    let gateway = f.host.storage_gateway().unwrap().clone();
    let task = tokio::spawn(async move {
        gateway
            .stage(
                &access.plan,
                futures_util::stream::pending::<webdock_plugin_protocol::Result<Vec<u8>>>(),
                Arc::new(Live),
            )
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    task.abort();
    assert!(task.await.err().unwrap().is_cancelled());
    let files = || {
        std::fs::read_dir("/proc/self/fd")
            .unwrap()
            .filter_map(|entry| std::fs::read_link(entry.ok()?.path()).ok())
            .filter(|path| {
                path.starts_with(f.directory.path())
                    && path
                        .file_name()
                        .is_some_and(|name| name.to_string_lossy().starts_with("transfer-"))
            })
            .count()
    };
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while files() != 0 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("cancelled claimed upload must release its anonymous staging file");
    let mut fresh = f
        .host
        .storage_access(&id, "PUT", &caller)
        .await
        .unwrap()
        .plan;
    fresh.expected_sha256 =
        Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into());
    let mut held = Vec::new();
    for _ in 0..4 {
        held.push(
            f.host
                .storage_gateway()
                .unwrap()
                .stage(
                    &fresh,
                    futures_util::stream::iter([Ok(b"abc".to_vec())]),
                    Arc::new(Live),
                )
                .await
                .expect("cancellation must return all permits"),
        );
    }
    drop(held);
    assert!(f.host.claim_storage_upload(&id, &caller).await.is_err());
    let state: Value = f
        .owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT session_context FROM webdock_auth.plugin_invocation WHERE id=$1::uuid",
            [id.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "session_context")
        .unwrap();
    assert_eq!(state["storageTransfer"]["state"], "consuming");
}
