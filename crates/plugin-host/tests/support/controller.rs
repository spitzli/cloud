use super::*;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use std::os::unix::fs::PermissionsExt;
use tower::ServiceExt;
use webdock_browser_session::StoredAuthority;
use webdock_plugin_host::{RuntimeDescription, StateProbe};
use webdock_plugin_protocol::{InvocationClaims, Manifest, PluginError};
use webdock_plugin_runtime::{
    ActionHandler, CorePeer, HostClient, RemoteAuthority, Runtime, UnixSocket,
};

struct Empty;
#[tonic::async_trait]
impl ActionHandler for Empty {
    async fn invoke(
        &self,
        _: &InvocationClaims,
        _: &str,
        _: Value,
        _: Value,
    ) -> Result<Value, PluginError> {
        Ok(json!({}))
    }
}
struct Mismatch;
#[async_trait::async_trait]
impl StateProbe for Mismatch {
    async fn describe(&self, _: &str, _: u64) -> Result<RuntimeDescription, PluginError> {
        Ok(RuntimeDescription {
            manifest: vec![],
            plugin_id: "wrong".into(),
            version: "0".into(),
            generation: 1,
            protocol_version: 1,
            peer: PeerIdentity::Unix {
                uid: rustix::process::geteuid().as_raw(),
            },
        })
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
async fn requested(
    host: &HostService,
    principal: &webdock_identity::Principal,
    plugin: &str,
    desired: &str,
    key: &str,
) -> String {
    let installed = host
        .installed_with_authority(principal, &StoredAuthority::LocalSession {})
        .await
        .unwrap();
    let revision = installed
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == plugin)
        .unwrap()["revision"]
        .as_str()
        .unwrap();
    host.request_state_with_authority(
        principal,
        &StoredAuthority::LocalSession {},
        plugin,
        desired,
        revision,
        key,
    )
    .await
    .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned()
}
pub async fn roundtrip(
    host: Arc<HostService>,
    plugin: &str,
    manifest: &str,
    identity: Arc<IdentityService>,
    cookie: &str,
) {
    let owner = db("WEBDOCK_TEST_MIGRATION_DATABASE_URL").await;
    let principal = identity.authenticate_cookie(cookie).await.unwrap().unwrap();
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let host_path = directory.path().join("host.sock");
    let plugin_path = directory.path().join("plugin.sock");
    let mut socket = UnixSocket::bind(&host_path).unwrap();
    let incoming = socket.incoming().unwrap();
    let rpc = host.clone().rpc();
    let host_task = tokio::spawn(async move {
        let _socket = socket;
        tonic::transport::Server::builder()
            .add_service(rpc)
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });
    let client = HostClient::connect_unix(&host_path).await.unwrap();
    let runtime = Runtime::new(
        Manifest::parse(manifest.as_bytes()).unwrap(),
        1,
        ed25519_dalek::SigningKey::from_bytes(&[19; 32]).verifying_key(),
        Arc::new(
            RemoteAuthority::new(
                client,
                plugin.into(),
                1,
                vec![CorePeer::Unix {
                    uid: rustix::process::geteuid().as_raw(),
                }],
            )
            .unwrap(),
        ),
        Arc::new(Empty),
    )
    .unwrap();
    let socket = UnixSocket::bind(&plugin_path).unwrap();
    let plugin_task = tokio::spawn(async move {
        runtime
            .serve_unix(socket, std::future::pending())
            .await
            .unwrap();
    });
    let _tasks = Tasks(vec![host_task, plugin_task]);
    let path = directory.path().join("connections.json");
    std::fs::write(&path,json!({"runtimes":{plugin:{"generation":1,"transport":{"kind":"unix","path":plugin_path}}}}).to_string()).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let endpoints = Arc::new(webdock_control_plane::plugins::PluginEndpoints::new(
        host.clone(),
        webdock_control_plane::plugin_config::RuntimeConnections::new(path),
        None,
    ));
    // Old pending state commands without persisted authority cannot be upgraded into native privilege.
    let legacy = host
        .claim_state_operation(Some(plugin))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        host.advance_state_operation(&legacy, endpoints.as_ref())
            .await
            .unwrap()
            .status
            .as_str(),
        "failed"
    );
    let operation = requested(&host, &principal, plugin, "enabled", "controller-enable-1").await;
    let mut attempts = Vec::new();
    for _ in 0..8 {
        let host = host.clone();
        let plugin = plugin.to_owned();
        attempts.push(tokio::spawn(async move {
            host.claim_state_operation(Some(&plugin)).await.unwrap()
        }));
    }
    let mut leases = Vec::new();
    for task in attempts {
        if let Some(lease) = task.await.unwrap() {
            leases.push(lease);
        }
    }
    assert_eq!(leases.len(), 1);
    let old = leases.pop().unwrap();
    assert_eq!(old.id(), operation);
    sql(&owner,"UPDATE webdock_auth.plugin_operation SET lease_until=now()-interval '1 second' WHERE id=$1",vec![operation.clone().into()]).await;
    let fresh = host
        .claim_state_operation(Some(plugin))
        .await
        .unwrap()
        .unwrap();
    assert!(
        host.advance_state_operation(&old, endpoints.as_ref())
            .await
            .is_err()
    );
    let enabled = host
        .advance_state_operation(&fresh, endpoints.as_ref())
        .await
        .unwrap();
    assert_eq!(enabled.status.as_str(), "succeeded");
    assert!(
        host.advance_state_operation(&fresh, endpoints.as_ref())
            .await
            .is_err()
    );
    let authority = StoredAuthority::LocalSession {};
    assert_eq!(
        host.operation_with_authority(&principal, &authority, &operation)
            .await
            .unwrap()
            .cursor,
        enabled.cursor
    );
    requested(&host, &principal, plugin, "enabled", "controller-mismatch").await;
    let lease = host
        .claim_state_operation(Some(plugin))
        .await
        .unwrap()
        .unwrap();
    let mismatch = host
        .advance_state_operation(&lease, &Mismatch)
        .await
        .unwrap();
    assert_eq!(mismatch.status.as_str(), "failed");
    assert_eq!(mismatch.condition.as_deref(), Some("runtime_mismatch"));
    let app = webdock_control_plane::http::build_app_with_domains(
        webdock_core::config::PlatformConfig::from_map(&std::collections::HashMap::new()).unwrap(),
        db("AUTH_DATABASE_URL").await,
        webdock_control_plane::http::Services {
            identity: Some(identity),
            plugins: Some(endpoints.clone()),
            ..Default::default()
        },
    );
    let request = |id: &str, cursor: Option<&str>| {
        let mut builder = Request::builder()
            .uri(format!("/api/console/plugins/operations/{id}/events"))
            .header("host", "localhost:3120")
            .header("cookie", cookie);
        if let Some(cursor) = cursor {
            builder = builder.header("last-event-id", cursor);
        }
        builder.body(Body::empty()).unwrap()
    };
    let response = app
        .clone()
        .oneshot(request(&operation, None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("event: snapshot"));
    assert!(text.contains("\"status\":\"succeeded\""));
    assert!(!text.contains("authority"));
    let response = app
        .clone()
        .oneshot(request(&operation, Some(&enabled.cursor)))
        .await
        .unwrap();
    assert!(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .is_empty()
    );
    assert_eq!(
        app.clone()
            .oneshot(request(&operation, Some("99999")))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    let disabled = requested(&host, &principal, plugin, "disabled", "controller-disable").await;
    let lease = host
        .claim_state_operation(Some(plugin))
        .await
        .unwrap()
        .unwrap();
    let waiting = host
        .advance_state_operation(&lease, endpoints.as_ref())
        .await
        .unwrap();
    assert_eq!(waiting.status.as_str(), "pending");
    assert_eq!(waiting.condition.as_deref(), Some("active_continuations"));
    sql(
        &owner,
        "UPDATE webdock_auth.plugin_invocation SET revoked_at=now() WHERE plugin_id=$1",
        vec![plugin.into()],
    )
    .await;
    let customer: String = owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT customer_id FROM webdock_auth.plugin_grant WHERE plugin_id=$1",
            [plugin.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "customer_id")
        .unwrap();
    let mail_id = id(&owner).await;
    sql(&owner,"INSERT INTO webdock_mail.send_job(id,customer_id,source_id,idempotency_key,request_hash,payload_ref,payload_bytes,route,envelope,state,created_at) VALUES($1,$2,$3,$1,$4,$5::uuid,100,$6,'{}','queued',now()-interval '2 hours')",
        vec![mail_id.clone().into(),customer.into(),format!("plugin-controller:{plugin}").into(),"a".repeat(64).into(),uuid::Uuid::new_v4().to_string().into(),json!({"kind":"direct","pluginID":plugin,"generation":1}).into()]).await;
    for state in [
        "staging",
        "queued",
        "leased",
        "preparation_review",
        "handoff",
        "unknown",
    ] {
        sql(
            &owner,
            "UPDATE webdock_mail.send_job SET state=$2 WHERE id=$1",
            vec![mail_id.clone().into(), state.into()],
        )
        .await;
        sql(
            &owner,
            "UPDATE webdock_auth.plugin_operation SET available_at=now() WHERE id=$1",
            vec![disabled.clone().into()],
        )
        .await;
        let lease = host
            .claim_state_operation(Some(plugin))
            .await
            .unwrap()
            .unwrap();
        let unscoped = db("WEBDOCK_PLUGIN_WORKER_DATABASE_URL").await;
        let visible:Option<bool>=unscoped.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT webdock_rls.plugin_state_continuations($1,'00000000-0000-4000-8000-000000000000'::uuid) AS active",[lease.id().into()])).await.unwrap().unwrap().try_get("","active").unwrap();
        assert!(visible.is_none());
        assert_eq!(
            host.advance_state_operation(&lease, endpoints.as_ref())
                .await
                .unwrap()
                .status
                .as_str(),
            "pending",
            "accepted {state} must retain its provider even with no current invocation"
        );
    }
    sql(
        &owner,
        "UPDATE webdock_mail.send_job SET state='accepted' WHERE id=$1",
        vec![mail_id.clone().into()],
    )
    .await;
    sql(
        &owner,
        "UPDATE webdock_auth.plugin_operation SET available_at=now() WHERE id=$1",
        vec![disabled.clone().into()],
    )
    .await;
    let lease = host
        .claim_state_operation(Some(plugin))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        host.advance_state_operation(&lease, endpoints.as_ref())
            .await
            .unwrap()
            .status
            .as_str(),
        "succeeded"
    );
    let row=owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT state,(SELECT count(*) FROM webdock_auth.plugin_document WHERE plugin_id=$1) AS documents,(SELECT count(*) FROM webdock_auth.plugin_runtime WHERE plugin_id=$1) AS runtimes FROM webdock_auth.plugin_installation WHERE id=$1",[plugin.into()])).await.unwrap().unwrap();
    assert_eq!(row.try_get::<String>("", "state").unwrap(), "disabled");
    assert!(row.try_get::<i64>("", "documents").unwrap() > 0);
    assert_eq!(row.try_get::<i64>("", "runtimes").unwrap(), 1);
    let revoked = requested(&host, &principal, plugin, "enabled", "controller-revoked").await;
    let response = app.clone().oneshot(request(&revoked, None)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body();
    assert!(stream.frame().await.unwrap().unwrap().into_data().is_ok());
    sql(
        &owner,
        "UPDATE webdock_auth.\"user\" SET banned=true WHERE id=$1",
        vec![principal.user_id.clone().into()],
    )
    .await;
    let lease = host
        .claim_state_operation(Some(plugin))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        host.advance_state_operation(&lease, endpoints.as_ref())
            .await
            .unwrap()
            .condition
            .as_deref(),
        Some("authority_revoked")
    );
    let ended = tokio::time::timeout(std::time::Duration::from_secs(3), stream.collect())
        .await
        .unwrap()
        .unwrap()
        .to_bytes();
    assert!(
        std::str::from_utf8(&ended)
            .unwrap()
            .contains("event: unavailable")
    );
    assert_ne!(
        app.clone()
            .oneshot(request(&operation, None))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    sql(
        &owner,
        "UPDATE webdock_auth.\"user\" SET banned=false WHERE id=$1",
        vec![principal.user_id.clone().into()],
    )
    .await;
    requested(&host, &principal, plugin, "enabled", "controller-resume").await;
    let lease = host
        .claim_state_operation(Some(plugin))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        host.advance_state_operation(&lease, endpoints.as_ref())
            .await
            .unwrap()
            .status
            .as_str(),
        "succeeded"
    );
}
