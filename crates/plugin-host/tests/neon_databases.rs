//! Exclusively owned PostgreSQL + fake Neon. No ambient provider credentials or target DSN.
#[allow(dead_code, reason = "Shared owned PostgreSQL fixture")]
#[path = "support/jobs_fixture.rs"]
mod fixture;
use sea_orm::{ConnectionTrait, DbBackend, Statement, TransactionTrait};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use webdock_identity::{CookiePolicy, IdentityService};
use webdock_plugin_host::{AdmissionInput, FileBindings, HostService};
use webdock_plugin_runtime::PeerIdentity;
const SECRET: &str = "owned-plugin-jobs-rls-secret-32bytes";
async fn sql(db: &impl ConnectionTrait, text: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        text,
        values,
    ))
    .await
    .unwrap();
}
async fn retained_metadata(f: &fixture::JobFixture) -> Value {
    f.owner.query_one_raw(Statement::from_string(DbBackend::Postgres,
        "SELECT jsonb_build_object('connections',(SELECT jsonb_agg(to_jsonb(c) ORDER BY customer_id) FROM webdock_auth.neon_connection c),'operator',(SELECT to_jsonb(u) FROM webdock_auth.\"user\" u WHERE id='operator')) AS value"))
        .await.unwrap().unwrap().try_get("", "value").unwrap()
}
fn peer() -> PeerIdentity {
    PeerIdentity::Unix {
        uid: rustix::process::geteuid().as_raw(),
    }
}
fn input(action: &str, body: Value, key: &str) -> AdmissionInput {
    serde_json::from_value(json!({"pluginID":"neon","projectID":"124","action":action,"input":body,"idempotencyKey":key})).unwrap()
}
async fn handle(
    f: &fixture::JobFixture,
    host: &HostService,
    action: &str,
    body: Value,
    key: &str,
) -> webdock_plugin_host::InvocationHandle {
    let admitted = host
        .admit_with_authority(
            &f.actor,
            input(action, body, key),
            true,
            &webdock_browser_session::StoredAuthority::local_session(&f.actor),
        )
        .await
        .unwrap();
    host.authenticate(peer(), &admitted.token).await.unwrap()
}
#[derive(Default)]
struct Gate {
    requests: AtomicUsize,
    pause: AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
async fn provider(
    axum::extract::State(gate): axum::extract::State<Arc<Gate>>,
    headers: axum::http::HeaderMap,
    uri: axum::http::Uri,
) -> axum::Json<Value> {
    assert_eq!(headers["authorization"], "Bearer fixture-key");
    gate.requests.fetch_add(1, Ordering::SeqCst);
    if gate.pause.load(Ordering::SeqCst) {
        gate.entered.notify_one();
        gate.release.notified().await;
    }
    let body = match uri.path() {
        "/projects" => {
            json!({"projects":[{"id":"p-one","name":"Synthetic","region_id":"aws-eu-central-1"}]})
        }
        "/projects/p-one" => {
            json!({"project":{"id":"p-one","name":"Synthetic","region_id":"aws-eu-central-1"}})
        }
        "/projects/p-one/branches/b-one" => {
            json!({"branch":{"id":"b-one","project_id":"p-one","name":"main","current_state":"ready"}})
        }
        "/projects/p-one/endpoints/ep-one" => {
            json!({"endpoint":{"id":"ep-one","branch_id":"b-one","host":"ep-one.eu.neon.tech","type":"read_write","current_state":"active"}})
        }
        "/projects/p-one/branches" => {
            json!({"branches":[{"id":"b-one","project_id":"p-one","name":"main","current_state":"ready"}]})
        }
        "/projects/p-one/branches/b-one/databases" => {
            json!({"databases":[{"id":1,"name":"dbname","branch_id":"b-one","owner_name":"app"}]})
        }
        "/projects/p-one/branches/b-one/roles" => {
            json!({"roles":[{"name":"app","branch_id":"b-one","password":"fixture-password"}]})
        }
        "/projects/p-one/endpoints" => {
            json!({"endpoints":[{"id":"ep-one","branch_id":"b-one","host":"ep-one.eu.neon.tech","type":"read_write","current_state":"active"}]})
        }
        "/projects/p-one/connection_uri" => {
            let query = url::form_urlencoded::parse(uri.query().unwrap().as_bytes())
                .into_owned()
                .collect::<std::collections::BTreeMap<_, _>>();
            assert_eq!(query.len(), 5);
            assert_eq!(query["branch_id"], "b-one");
            assert_eq!(query["endpoint_id"], "ep-one");
            assert_eq!(query["database_name"], "dbname");
            assert_eq!(query["role_name"], "app");
            assert_eq!(query["pooled"], "true");
            json!({"uri":"postgresql://app:fixture-password@ep-one-pooler.eu.neon.tech/dbname?sslmode=require"})
        }
        _ => panic!("unapproved fake-provider path"),
    };
    axum::Json(body)
}
#[tokio::test]
#[ignore = "requires owned PostgreSQL18 (WEBDOCK_PLUGIN_TEST_LOCAL_POSTGRES=1 on qualified EU runner)"]
async fn atomic_attach_lost_replies_uri_authority_detach_and_key_race() {
    let mut f = fixture::JobFixture::start_with_connections(4).await;
    sql(&f.owner, "INSERT INTO webdock_auth.plan(id,name,limits,is_default,updated_by) VALUES('9999','BYOK test','{\"neon.database\":0,\"postgres.shared\":2,\"memory\":512}',true,'operator')", vec![]).await;
    sql(&f.owner, "INSERT INTO webdock_auth.customer_plan(customer_id,plan_id,overrides,updated_by) VALUES('123','9999','{\"neon.database\":1,\"memory\":256,\"storage\":null}','operator')", vec![]).await;
    f.owner.execute_unprepared("INSERT INTO webdock_ids.generator(slot,layout) VALUES(3,'2026-01-01:41:6:4:12') ON CONFLICT DO NOTHING").await.unwrap();
    f.owner
        .execute_unprepared(include_str!(
            "../../../tools/operations/neon-byok-plan-cleanup.sql"
        ))
        .await
        .unwrap();
    let cleaned=f.owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT jsonb_build_object('limits',p.limits,'overrides',c.overrides,'planActor',p.updated_by,'customerActor',c.updated_by) AS value FROM webdock_auth.plan p JOIN webdock_auth.customer_plan c ON c.plan_id=p.id WHERE p.id='9999'")).await.unwrap().unwrap().try_get::<Value>("","value").unwrap();
    assert_eq!(cleaned["limits"], json!({"postgres.shared":2,"memory":512}));
    assert_eq!(cleaned["overrides"], json!({"memory":256,"storage":null}));
    assert_eq!(cleaned["planActor"], "system:neon-byok-policy");
    assert_eq!(cleaned["customerActor"], "system:neon-byok-policy");
    // Reconstruct accepted 0074 only inside this fresh, exclusively owned fixture.
    // 0075 is additive; none of these new tables has customer data yet. This is
    // fixture preparation, never a supported production schema rollback.
    f.owner.execute_unprepared("DO $$ BEGIN IF EXISTS(SELECT 1 FROM webdock_auth.neon_database_receipt) OR EXISTS(SELECT 1 FROM webdock_auth.neon_database_attachment) THEN RAISE EXCEPTION 'Fresh owned fixture required'; END IF; END $$; DROP FUNCTION webdock_rls.commit_neon_database(uuid,uuid,bigint,bigint,jsonb); DROP FUNCTION webdock_rls.reclaim_neon_attachment_reservation(uuid); DROP TABLE webdock_auth.neon_database_receipt; DROP TABLE webdock_auth.neon_database_attachment; DROP FUNCTION webdock_auth.neon_database_receipt_immutable(); DELETE FROM webdock_api_migrations.rust_journal WHERE version=75").await.unwrap();
    let gate = Arc::new(Gate::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let app = axum::Router::new()
        .route("/{*path}", axum::routing::get(provider))
        .with_state(gate.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    f.manifest["name"] = json!("neon");
    f.manifest["jobs"] = json!([]);
    f.manifest["capabilities"] = json!(["plugin.http.read", "plugin.http.request"]);
    f.manifest["services"]["required"] = json!([{"name":"http","version":1}]);
    f.manifest["httpBindings"] = json!(["neon"]);
    f.manifest["offerings"] =
        json!([{"id":"database","label":"Database","description":"An existing Neon database"}]);
    f.manifest["selfService"]["features"] =
        json!([{"id":"connect-databases","label":"Connect databases","offering":"database"}]);
    let resource_schema = json!({"type":"object","properties":{"resource_id":{"type":"string","maxLength":128},"pooled":{"type":"boolean"}},"required":["resource_id"],"additionalProperties":false});
    f.manifest["actions"] = json!([
        {"name":"databases.discover","intent":"read","audience":"customer","feature":"connect-databases","capabilities":["plugin.http.read"]},
        {"name":"databases.attach","intent":"write","audience":"customer","feature":"connect-databases","resource":"create","offering":"database","capabilities":["plugin.http.read","plugin.http.request"]},
        {"name":"databases.view","intent":"read","audience":"customer","resource":"use","capabilities":["plugin.http.read"],"input":resource_schema},
        {"name":"databases.branches","intent":"read","audience":"customer","resource":"use","capabilities":["plugin.http.read"],"input":resource_schema},
        {"name":"databases.credentials","intent":"write","audience":"customer","resource":"use","capabilities":["plugin.http.read","plugin.http.request"],"input":resource_schema},
        {"name":"databases.detach","intent":"write","audience":"customer","resource":"delete","capabilities":["plugin.http.request"],"input":resource_schema}
    ]);
    let raw = f.manifest.to_string();
    use sha2::{Digest, Sha256};
    sql(&f.owner,"INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version,runtime,current_generation) SELECT 'neon',kind,state,desired_state,version,runtime,current_generation FROM webdock_auth.plugin_installation WHERE id='job-fixture'",vec![]).await;
    sql(&f.owner,"INSERT INTO webdock_auth.plugin_release(plugin_id,version,manifest_bytes,manifest_sha256,image,registry_url,signer_key_id) SELECT 'neon',version,$1,$2,image,registry_url,signer_key_id FROM webdock_auth.plugin_release WHERE plugin_id='job-fixture'",vec![raw.clone().into(),hex::encode(Sha256::digest(raw.as_bytes())).into()]).await;
    sql(&f.owner,"INSERT INTO webdock_auth.plugin_configuration(plugin_id,approved_capabilities,configuration) VALUES('neon',$1,'{}')",vec![f.manifest["capabilities"].clone().into()]).await;
    sql(&f.owner,"INSERT INTO webdock_auth.plugin_runtime(plugin_id,generation,version,image,status,configuration_revision,credential_hash,namespace,service_name) SELECT 'neon',generation,version,image,status,configuration_revision,credential_hash,namespace,service_name FROM webdock_auth.plugin_runtime WHERE plugin_id='job-fixture'",vec![]).await;
    sql(&f.owner,"INSERT INTO webdock_auth.plugin_feature_policy(plugin_id,feature_id,mode,updated_by) VALUES('neon','connect-databases','everyone','operator')",vec![]).await;
    let path = f.directory.path().join("bindings.json");
    let mut bindings: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    bindings["runtimes"]["neon"] = bindings["runtimes"]["job-fixture"].clone();
    std::fs::write(&path, bindings.to_string()).unwrap();
    let business = f
        .database
        .role_with_connections("webdock_auth_runtime", 4)
        .await;
    let service = webdock_provider_connections::Connections::new(
        business.clone(),
        f.auth.clone(),
        f.identity.clone(),
        None,
        SECRET,
        SECRET,
    )
    .unwrap()
    .with_test_provider(origin);
    let connection = uuid::Uuid::new_v4().to_string();
    let cipher = webdock_secrets::SecretCipher::new(SECRET).unwrap();
    let encrypted = cipher
        .seal(
            &"fixture-key",
            &format!("native-neon-api-key-v1:123:{connection}:1"),
        )
        .unwrap();
    sql(&f.owner,"INSERT INTO webdock_auth.neon_connection(customer_id,id,revision,status,encrypted_key) VALUES('123',$1::uuid,1,'connected',$2)",vec![connection.clone().into(),encrypted.into()]).await;
    let retained = retained_metadata(&f).await;
    let database_name = f.database.url.path().trim_start_matches('/');
    webdock_persistence::migrations::upgrade(&f.owner, database_name)
        .await
        .unwrap();
    assert!(webdock_persistence::migrations::native_schema_ready(&f.owner).await);
    assert_eq!(
        retained_metadata(&f).await,
        retained,
        "upgrade retains the existing encrypted connection and operator account and MFA flag"
    );
    let service = Arc::new(service);
    let host = Arc::new(
        HostService::new(
            business.clone(),
            f.database
                .role_with_connections("webdock_worker_runtime", 4)
                .await,
            SECRET.into(),
            Arc::new(
                webdock_persistence::IdAllocator::connect(
                    f.database.url.as_str(),
                    webdock_ids::Identity::platform(0, 3).unwrap(),
                )
                .await
                .unwrap(),
            ),
            Arc::new(FileBindings::new(path)),
            ed25519_dalek::SigningKey::from_bytes(&[19; 32]),
        )
        .await
        .unwrap()
        .with_session_authority(f.auth.clone(), f.identity.clone(), None)
        .await
        .unwrap()
        .with_named_http(service.clone()),
    );
    let authority = webdock_browser_session::StoredAuthority::local_session(&f.actor);
    host.save_plan(
        &f.actor,
        Some(&authority),
        Some("9999"),
        webdock_plugin_host::PlanInput {
            name: "BYOK test".into(),
            description: "".into(),
            is_default: true,
            limits: json!({"neon.database":0,"postgres.shared":2,"memory":512}),
        },
    )
    .await
    .unwrap();
    host.set_customer_plan(
        &f.actor,
        Some(&authority),
        "123",
        webdock_plugin_host::CustomerPlanInput {
            plan_id: Some("9999".into()),
            overrides: json!({"neon.database":0,"postgres.shared":1}),
        },
    )
    .await
    .unwrap();
    let effective = f
        .owner
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT webdock_auth.limits_of('123') AS value",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<Value>("", "value")
        .unwrap();
    assert_eq!(
        effective["neon.database"],
        Value::Null,
        "BYOK Neon has no Spitzli quantity quota"
    );
    assert_eq!(effective["postgres.shared"], 1);
    assert_eq!(effective["memory"], 512);
    let plans = host
        .plans(
            &f.actor,
            Some(&webdock_browser_session::StoredAuthority::local_session(
                &f.actor,
            )),
        )
        .await
        .unwrap();
    assert!(
        plans["meters"]
            .as_array()
            .unwrap()
            .iter()
            .all(|meter| meter["plugin"] != "neon")
    );
    assert!(
        host.admit(
            &f.actor,
            input("databases.discover", json!({}), "missing-browser-proof")
        )
        .await
        .is_err()
    );
    let endpoints = webdock_control_plane::plugins::PluginEndpoints::new(
        host.clone(),
        webdock_control_plane::plugin_config::RuntimeConnections::new(
            f.directory.path().join("no-route-needed.json"),
        ),
        None,
    );
    let actor_tx = f.auth.begin().await.unwrap();
    let current_actor = f
        .identity
        .authenticate_session_in(&actor_tx, &f.actor.user_id, &f.actor.session_id)
        .await
        .unwrap()
        .unwrap();
    actor_tx.commit().await.unwrap();
    let actor = webdock_control_plane::auth::AuthenticatedActor {
        principal: current_actor,
        authority: webdock_browser_session::StoredAuthority::local_session(&f.actor),
    };
    assert!(
        endpoints
            .invoke_api(
                &actor,
                input("databases.discover", json!({}), "api-token-unsupported"),
                true,
                "fixture-api-hash".into()
            )
            .await
            .is_err()
    );
    let tuple = json!({"name":"Synthetic existing database","neon_project_id":"p-one","branch_id":"b-one","database_name":"dbname","role_name":"app","endpoint_id":"ep-one"});
    let initial = host
        .admit_with_authority(
            &f.actor,
            input("databases.attach", tuple.clone(), "attach-once"),
            true,
            &webdock_browser_session::StoredAuthority::local_session(&f.actor),
        )
        .await
        .unwrap();
    let attach = host.authenticate(peer(), &initial.token).await.unwrap();
    // Force the final receipt insertion to fail; binding/index/reservation must all roll back.
    sql(&f.owner,"CREATE FUNCTION webdock_auth.fixture_fail_receipt() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic failure'; END $$",vec![]).await;
    sql(&f.owner,"CREATE TRIGGER fixture_fail_receipt BEFORE INSERT ON webdock_auth.neon_database_receipt FOR EACH ROW EXECUTE FUNCTION webdock_auth.fixture_fail_receipt()",vec![]).await;
    let request =
        json!({"binding":"neon","method":"POST","path":"/_spitzli/databases/attach","body":tuple});
    assert!(host.http_request(&attach, request.clone()).await.is_err());
    let counts=f.owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT (SELECT count(*) FROM webdock_auth.neon_database_attachment) AS bindings,(SELECT count(*) FROM webdock_auth.plugin_resource WHERE plugin_id='neon') AS resources")).await.unwrap().unwrap();
    assert_eq!(counts.try_get::<i64>("", "bindings").unwrap(), 0);
    assert_eq!(counts.try_get::<i64>("", "resources").unwrap(), 0);
    sql(
        &f.owner,
        "DROP TRIGGER fixture_fail_receipt ON webdock_auth.neon_database_receipt",
        vec![],
    )
    .await;
    assert!(
        host.admit_with_authority(
            &f.actor,
            input("databases.attach", tuple.clone(), "attach-once"),
            true,
            &webdock_browser_session::StoredAuthority::local_session(&f.actor)
        )
        .await
        .is_err(),
        "a live uncertain invocation cannot be reclaimed"
    );
    // Production always revokes issued tokens after an invocation attempt, even on callback error.
    host.revoke_issued(&initial.token).await.unwrap();
    let fresh = host
        .admit_with_authority(
            &f.actor,
            input("databases.attach", tuple.clone(), "attach-once"),
            true,
            &webdock_browser_session::StoredAuthority::local_session(&f.actor),
        )
        .await
        .unwrap();
    assert!(fresh.replay_result.is_none());
    let fresh_handle = host.authenticate(peer(), &fresh.token).await.unwrap();
    let (stale, result) = tokio::join!(
        host.http_request(&attach, request.clone()),
        host.http_request(&fresh_handle, request)
    );
    assert!(
        stale.is_err(),
        "reclaimed old handle cannot commit concurrently"
    );
    let result = result.unwrap().body;
    let attach = fresh_handle;
    let counts=f.owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT (SELECT count(*) FROM webdock_auth.neon_database_attachment) AS bindings,(SELECT count(*) FROM webdock_auth.plugin_resource WHERE plugin_id='neon') AS resources, (webdock_auth.allocated_of('123')->>'neon.database')::bigint AS allocated")).await.unwrap().unwrap();
    assert_eq!(counts.try_get::<i64>("", "bindings").unwrap(), 1);
    assert_eq!(counts.try_get::<i64>("", "resources").unwrap(), 1);
    assert_eq!(
        counts.try_get::<i64>("", "allocated").unwrap(),
        1,
        "dead local reservation consumes no extra quota"
    );
    let resource = result["resource"]["id"].as_str().unwrap();
    host.record_resource(&attach, &result).await.unwrap();
    host.record_resource(&attach, &result).await.unwrap();
    let retry = host
        .admit_with_authority(
            &f.actor,
            input("databases.attach", tuple.clone(), "attach-once"),
            true,
            &webdock_browser_session::StoredAuthority::local_session(&f.actor),
        )
        .await
        .unwrap();
    assert_eq!(
        retry.replay_result,
        Some(result.clone()),
        "lost reply replays Core receipt"
    );
    let view = handle(
        &f,
        &host,
        "databases.view",
        json!({"resource_id":resource}),
        "view-one",
    )
    .await;
    assert!(host.http_request(&view,json!({"binding":"neon","method":"POST","path":"/_spitzli/databases/credentials","body":{"resource_id":resource}})).await.is_err());
    gate.requests.store(0, Ordering::SeqCst);
    let current = host
        .http_request(
            &view,
            json!({"binding":"neon","method":"GET","path":"/_spitzli/databases/current"}),
        )
        .await
        .unwrap();
    assert_eq!(current.body["status"], "active");
    assert_eq!(
        gate.requests.load(Ordering::SeqCst),
        2,
        "one scoped project validation and one endpoint read are sufficient"
    );
    let discover_roles = handle(
        &f,
        &host,
        "databases.discover",
        json!({}),
        "discover-safe-roles",
    )
    .await;
    let roles=host.http_request(&discover_roles,json!({"binding":"neon","method":"GET","path":"/projects/p-one/branches/b-one/roles","query":{"limit":"100"}})).await.unwrap();
    assert!(!roles.body.to_string().contains("fixture-password"));
    assert!(
        host.http_request(
            &discover_roles,
            json!({"binding":"neon","method":"GET","path":"/projects/p-one/connection_uri"})
        )
        .await
        .is_err()
    );
    let credentials = handle(
        &f,
        &host,
        "databases.credentials",
        json!({"resource_id":resource}),
        "reveal-one",
    )
    .await;
    let revealed=host.http_request(&credentials,json!({"binding":"neon","method":"POST","path":"/_spitzli/databases/credentials","body":{"resource_id":resource}})).await.unwrap();
    assert!(
        revealed.body["value"]
            .as_str()
            .unwrap()
            .contains("fixture-password")
    );
    let serialized=f.owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT coalesce(jsonb_agg(to_jsonb(r)),'[]')::text AS value FROM webdock_auth.neon_database_receipt r")).await.unwrap().unwrap().try_get::<String>("","value").unwrap();
    assert!(!serialized.contains("fixture-password"));
    assert!(!serialized.contains("postgresql://"));
    let detach_body = json!({"resource_id":resource});
    let detach = handle(
        &f,
        &host,
        "databases.detach",
        detach_body.clone(),
        "detach-once",
    )
    .await;
    let detached=host.http_request(&detach,json!({"binding":"neon","method":"POST","path":"/_spitzli/databases/detach","body":detach_body})).await.unwrap().body;
    host.record_resource(&detach, &detached).await.unwrap();
    let retry = host
        .admit_with_authority(
            &f.actor,
            input("databases.detach", detach_body.clone(), "detach-once"),
            true,
            &webdock_browser_session::StoredAuthority::local_session(&f.actor),
        )
        .await
        .unwrap();
    assert_eq!(retry.replay_result, Some(detached));
    assert!(
        host.admit_with_authority(
            &f.actor,
            input(
                "databases.detach",
                json!({"resource_id":"another"}),
                "detach-once"
            ),
            true,
            &webdock_browser_session::StoredAuthority::local_session(&f.actor)
        )
        .await
        .is_err()
    );
    // No signed actor means forced RLS reveals no attachment or receipt, even with runtime SELECT.
    let rows=business.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT (SELECT count(*) FROM webdock_auth.neon_database_attachment) AS bindings,(SELECT count(*) FROM webdock_auth.neon_database_receipt) AS receipts")).await.unwrap().unwrap();
    assert_eq!(rows.try_get::<i64>("", "bindings").unwrap(), 0);
    assert_eq!(rows.try_get::<i64>("", "receipts").unwrap(), 0);
    // A key rotation during provider IO discards an otherwise successful response.
    let discover = handle(&f, &host, "databases.discover", json!({}), "discover-race").await;
    gate.pause.store(true, Ordering::SeqCst);
    let racing_host = host.clone();
    let pending = tokio::spawn(async move {
        racing_host
            .http_request(
                &discover,
                json!({"binding":"neon","method":"GET","path":"/projects"}),
            )
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), gate.entered.notified())
        .await
        .unwrap();
    let encrypted = cipher
        .seal(
            &"fixture-key",
            &format!("native-neon-api-key-v1:123:{connection}:2"),
        )
        .unwrap();
    sql(&f.owner,"UPDATE webdock_auth.neon_connection SET revision=2,encrypted_key=$1 WHERE customer_id='123'",vec![encrypted.into()]).await;
    gate.pause.store(false, Ordering::SeqCst);
    gate.release.notify_one();
    assert!(pending.await.unwrap().is_err());
    // A retained tuple detached while provider IO is pending cannot reveal a credential.
    let attach_two = handle(
        &f,
        &host,
        "databases.attach",
        tuple.clone(),
        "attach-race-two",
    )
    .await;
    let second=host.http_request(&attach_two,json!({"binding":"neon","method":"POST","path":"/_spitzli/databases/attach","body":tuple})).await.unwrap().body;
    let second_id = second["resource"]["id"].as_str().unwrap();
    let reveal_body = json!({"resource_id":second_id});
    let reveal = handle(
        &f,
        &host,
        "databases.credentials",
        reveal_body.clone(),
        "reveal-detach-race",
    )
    .await;
    let detach = handle(
        &f,
        &host,
        "databases.detach",
        reveal_body.clone(),
        "detach-during-reveal",
    )
    .await;
    gate.pause.store(true, Ordering::SeqCst);
    let racing_host = host.clone();
    let reveal_input = reveal_body.clone();
    let pending = tokio::spawn(async move {
        racing_host.http_request(&reveal,json!({"binding":"neon","method":"POST","path":"/_spitzli/databases/credentials","body":reveal_input})).await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), gate.entered.notified())
        .await
        .unwrap();
    host.http_request(&detach,json!({"binding":"neon","method":"POST","path":"/_spitzli/databases/detach","body":reveal_body})).await.unwrap();
    gate.pause.store(false, Ordering::SeqCst);
    gate.release.notify_one();
    assert!(pending.await.unwrap().is_err());
    // Membership authority is checked again after IO, before any data reaches the plugin.
    let tx = f.auth.begin().await.unwrap();
    let member = f
        .identity
        .authenticate_session_in(&tx, "member", "member-session")
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    // A real second tenant remains invisible even to the first tenant's admin.
    let seed = f.owner.begin().await.unwrap();
    seed.execute_unprepared("SELECT set_config('webdock.organization_allocation','{\"customer_id\":\"223\",\"organization_id\":\"225\"}',true); INSERT INTO webdock_admin.customers(id,name) VALUES('223','Other tenant'); INSERT INTO webdock_admin.projects(id,name,customer_id) VALUES('224','Other project','223')").await.unwrap();
    seed.commit().await.unwrap();
    let foreign_connection = uuid::Uuid::new_v4().to_string();
    let foreign_key = cipher
        .seal(
            &"foreign-fixture-key",
            &format!("native-neon-api-key-v1:223:{foreign_connection}:1"),
        )
        .unwrap();
    sql(&f.owner,"INSERT INTO webdock_auth.neon_connection(customer_id,id,revision,status,encrypted_key) VALUES('223',$1::uuid,1,'connected',$2)",vec![foreign_connection.clone().into(),foreign_key.into()]).await;
    sql(&f.owner,"INSERT INTO webdock_auth.neon_database_attachment(resource_id,attach_invocation_id,customer_id,project_id,connection_id,connection_revision,name,neon_project_id,branch_id,database_name,role_name,endpoint_id) VALUES('foreign-neon-resource',$1::uuid,'223','224',$2::uuid,1,'Other database','p-one','b-one','dbname','app','ep-one')",vec![uuid::Uuid::new_v4().to_string().into(),foreign_connection.into()]).await;
    sql(&f.owner,"INSERT INTO webdock_auth.plugin_resource(plugin_id,resource_id,offering_id,customer_id,project_id,name,created_via,created_by) VALUES('neon','foreign-neon-resource','database','223','224','Other database','operator','operator')",vec![]).await;
    sql(&f.owner,"INSERT INTO webdock_auth.neon_database_receipt(customer_id,project_id,actor_id,operation_id,action,request_hash,resource_id,result) VALUES('223','224','member',repeat('c',64),'databases.attach',repeat('d',64),'foreign-neon-resource','{}')",vec![]).await;
    let scoped = webdock_identity::RlsSigner::new(SECRET, "webdock_auth_runtime")
        .unwrap()
        .begin(&business, &member)
        .await
        .unwrap();
    let foreign = scoped.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT (SELECT count(*) FROM webdock_auth.neon_database_attachment WHERE customer_id='223') AS bindings,(SELECT count(*) FROM webdock_auth.neon_database_receipt WHERE customer_id='223') AS receipts,(SELECT count(*) FROM webdock_auth.neon_connection WHERE customer_id='223') AS connections,(SELECT count(*) FROM webdock_auth.neon_database_attachment WHERE customer_id='123') AS own_bindings,(SELECT count(*) FROM webdock_auth.neon_connection WHERE customer_id='123') AS own_connections")).await.unwrap().unwrap();
    for column in ["bindings", "receipts", "connections"] {
        assert_eq!(foreign.try_get::<i64>("", column).unwrap(), 0);
    }
    assert!(foreign.try_get::<i64>("", "own_bindings").unwrap() > 0);
    assert_eq!(foreign.try_get::<i64>("", "own_connections").unwrap(), 1);
    scoped.commit().await.unwrap();
    for project in ["124", "224"] {
        let forged = AdmissionInput {
            project_id: project.into(),
            ..input(
                "databases.view",
                json!({"resource_id":"foreign-neon-resource"}),
                "foreign-resource-denied",
            )
        };
        let denied = host
            .admit_with_authority(
                &member,
                forged,
                true,
                &webdock_browser_session::StoredAuthority::local_session(&member),
            )
            .await
            .err()
            .expect("foreign tenant resource must not produce an invocation");
        assert!(matches!(
            denied.code,
            webdock_plugin_host::ErrorCode::PermissionDenied
                | webdock_plugin_host::ErrorCode::NotFound
        ));
    }
    let admitted = host
        .admit_with_authority(
            &member,
            input("databases.discover", json!({}), "membership-race"),
            true,
            &webdock_browser_session::StoredAuthority::local_session(&member),
        )
        .await
        .unwrap();
    let discover = host.authenticate(peer(), &admitted.token).await.unwrap();
    gate.pause.store(true, Ordering::SeqCst);
    let racing_host = host.clone();
    let pending = tokio::spawn(async move {
        racing_host
            .http_request(
                &discover,
                json!({"binding":"neon","method":"GET","path":"/projects"}),
            )
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), gate.entered.notified())
        .await
        .unwrap();
    sql(
        &f.owner,
        "UPDATE webdock_auth.member SET role='member' WHERE id='member-binding'",
        vec![],
    )
    .await;
    gate.pause.store(false, Ordering::SeqCst);
    gate.release.notify_one();
    assert!(pending.await.unwrap().is_err());
    assert!(
        host.admit_with_authority(
            &member,
            input("databases.discover", json!({}), "member-discovery-denied"),
            true,
            &webdock_browser_session::StoredAuthority::local_session(&member)
        )
        .await
        .is_err()
    );
    let metadata = service
        .execute(
            &member,
            &webdock_browser_session::StoredAuthority::local_session(&member),
            webdock_provider_connections::Command::Get {
                customer: "123".into(),
            },
        )
        .await;
    assert!(
        matches!(
            metadata,
            Err(webdock_provider_connections::Error::Forbidden)
        ),
        "ordinary membership must not broaden the existing provider-connection permission"
    );
    // Session revocation is a separate fence from provider/key and membership revisions.
    let discover = handle(&f, &host, "databases.discover", json!({}), "session-race").await;
    gate.pause.store(true, Ordering::SeqCst);
    let racing_host = host.clone();
    let pending = tokio::spawn(async move {
        racing_host
            .http_request(
                &discover,
                json!({"binding":"neon","method":"GET","path":"/projects"}),
            )
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), gate.entered.notified())
        .await
        .unwrap();
    sql(&f.owner,"UPDATE webdock_auth.session SET \"expiresAt\"=now()-interval '1 second' WHERE id='session'",vec![]).await;
    gate.pause.store(false, Ordering::SeqCst);
    gate.release.notify_one();
    assert!(pending.await.unwrap().is_err());
    let retained = retained_metadata(&f).await;
    webdock_persistence::migrations::upgrade(&f.owner, database_name)
        .await
        .unwrap();
    assert!(webdock_persistence::migrations::native_schema_ready(&f.owner).await);
    assert_eq!(
        retained_metadata(&f).await,
        retained,
        "repeated migration retains current connection and operator data"
    );
    server.abort();
}
