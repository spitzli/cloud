//! Real PostgreSQL, no Core: a disposable local server named by
//! `WEBDOCK_POSTGRES_PLUGIN_TEST_ADMIN_URL` (loopback only) holds plugin storage and shared databases.
//! Dedicated instances go to a stand-in for Core's `plugin.workloads`; they run on the local k3s
//! in the live dev stack.
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio_postgres::NoTls;
use webdock_plugin_protocol::{ErrorCode, InvocationClaims, Manifest};
use webdock_plugin_runtime::ActionHandler;
use webdock_postgres::{
    Handler, Plugin, dedicated, seal::Sealer, shared::SharedServer, store::Store,
};

/// What Core holds for the units: the objects of the last apply and what the agent reported.
#[derive(Default)]
struct Units {
    objects: std::collections::HashMap<String, Value>,
    observed: Value,
    removed: Vec<String>,
}
#[derive(Clone, Default)]
struct Core(Arc<Mutex<Units>>);
#[tonic::async_trait]
impl dedicated::Workloads for Core {
    async fn call(
        &self,
        method: &str,
        input: Value,
    ) -> Result<Value, webdock_plugin_protocol::PluginError> {
        let mut units = self.0.lock().unwrap();
        let unit = input["unit"].as_str().unwrap().to_owned();
        Ok(match method {
            "apply" => {
                units.objects.insert(unit, input["objects"].clone());
                json!({})
            }
            "remove" => {
                units.objects.remove(&unit);
                units.removed.push(unit);
                json!({})
            }
            _ if !units.objects.contains_key(&unit) => json!({"removed": true}),
            _ => json!({"observed": units.observed}),
        })
    }
}
impl Core {
    fn objects(&self, created: &Value) -> Value {
        let id = created["resource"]["id"].as_str().unwrap().parse().unwrap();
        self.0.lock().unwrap().objects[&dedicated::unit(&id)].clone()
    }
}

fn admin_url() -> url::Url {
    let url = url::Url::parse(
        &std::env::var("WEBDOCK_POSTGRES_PLUGIN_TEST_ADMIN_URL").expect("disposable local server"),
    )
    .unwrap();
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    url
}
async fn admin() -> tokio_postgres::Client {
    let (client, connection) = tokio_postgres::connect(admin_url().as_str(), NoTls)
        .await
        .unwrap();
    tokio::spawn(connection);
    client
}
struct Fixture {
    handler: Handler,
    storage: String,
}
async fn fixture(capacity: u32, core: Option<Core>) -> Fixture {
    let storage = format!("plugin_test_{}", uuid::Uuid::new_v4().simple());
    admin()
        .await
        .batch_execute(&format!("CREATE DATABASE {storage}"))
        .await
        .unwrap();
    let mut url = admin_url();
    url.set_path(&storage);
    let base = admin_url();
    let server = SharedServer {
        id: "test".into(),
        label: "Test server".into(),
        host: "127.0.0.1".into(),
        port: base.port().unwrap(),
        admin_url: base.to_string(),
        capacity,
    };
    // Parallel tests harden the same server; concurrent catalog updates may collide once.
    for attempt in 0.. {
        match server.harden().await {
            Ok(()) => break,
            Err(_) if attempt < 5 => {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await
            }
            Err(error) => panic!("{error}"),
        }
    }
    let placement = core.map(|core| {
        dedicated::Placement::new(
            Box::new(core),
            dedicated::DedicatedConfig {
                domain: "db.localhost".into(),
                port: 55442,
                entry_points: vec!["postgres".into()],
                issuer: Some("webdock-dev-ca".into()),
            },
        )
    });
    let plugin = Plugin::new(
        Store::connect(url.as_str()).await.unwrap(),
        Sealer::new(&"42".repeat(32)).unwrap(),
        vec![server],
        placement,
    );
    plugin.resume().await.unwrap();
    Fixture {
        handler: Handler(plugin),
        storage,
    }
}
impl Fixture {
    async fn call(
        &self,
        project: &str,
        action: &str,
        input: Value,
    ) -> Result<Value, webdock_plugin_protocol::PluginError> {
        let claims = InvocationClaims {
            job: None,
            v: 1,
            id: uuid::Uuid::new_v4().to_string(),
            plugin_id: "postgres".into(),
            generation: 1,
            action: action.into(),
            project_id: project.into(),
            customer_id: "101".into(),
            iat: 0,
            exp: 0,
        };
        self.handler.invoke(&claims, "", input, json!({})).await
    }
    async fn ok(&self, project: &str, action: &str, input: Value) -> Value {
        self.call(project, action, input)
            .await
            .unwrap_or_else(|e| panic!("{action}: {e:?}"))
    }
    async fn cleanup(self) {
        drop(self.handler);
        admin()
            .await
            .batch_execute(&format!("DROP DATABASE {} WITH (FORCE)", self.storage))
            .await
            .unwrap();
    }
}
async fn connect(uri: &str) -> Result<tokio_postgres::Client, tokio_postgres::Error> {
    let (client, connection) = tokio_postgres::connect(uri, NoTls).await?;
    tokio::spawn(connection);
    Ok(client)
}
fn id(created: &Value) -> Value {
    json!({"resource_id": created["resource"]["id"]})
}

#[test]
fn shipped_manifest_is_the_reviewed_contract() {
    let manifest = Manifest::parse(include_bytes!("../manifest.json")).unwrap();
    webdock_postgres::validate_manifest(&manifest).unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable local PostgreSQL server"]
async fn shared_database_lifecycle() {
    let f = fixture(10, None).await;
    let created = f
        .ok("101", "databases.create", json!({"name":" Orders "}))
        .await;
    assert_eq!(created["resource"]["name"], "Orders");
    let view = f.ok("101", "databases.view", id(&created)).await;
    assert_eq!(view["state"], "ready");
    assert_eq!(view["offering"], "shared");
    assert_eq!(view["server"], "Test server");
    let secret = f.ok("101", "databases.credentials", id(&created)).await;
    let uri = secret["uri"].as_str().unwrap().to_owned();
    let client = connect(&uri).await.unwrap();
    client
        .batch_execute("CREATE TABLE t(x int); INSERT INTO t VALUES (1)")
        .await
        .unwrap();

    // Another project cannot see it; another customer's role cannot open it.
    assert_eq!(
        f.call("202", "databases.view", id(&created))
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    let other = f
        .ok("202", "databases.create", json!({"name":"Other"}))
        .await;
    let other_uri = f.ok("202", "databases.credentials", id(&other)).await["uri"]
        .as_str()
        .unwrap()
        .to_owned();
    let foreign =
        other_uri.rsplit_once('/').unwrap().0.to_owned() + "/" + view["database"].as_str().unwrap();
    assert!(
        connect(&foreign).await.is_err(),
        "no CONNECT on another customer's database"
    );
    let postgres = other_uri.rsplit_once('/').unwrap().0.to_owned() + "/postgres";
    assert!(
        connect(&postgres).await.is_err(),
        "no CONNECT on the maintenance database"
    );

    // Rotation retires the old password.
    f.ok("101", "databases.rotate", id(&created)).await;
    assert!(connect(&uri).await.is_err());
    let rotated = f.ok("101", "databases.credentials", id(&created)).await["uri"]
        .as_str()
        .unwrap()
        .to_owned();
    connect(&rotated).await.unwrap();

    // The shared delete only deletes shared databases, and removes database and role.
    assert_eq!(
        f.call("101", "instances.delete", id(&created))
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    drop(client);
    f.ok("101", "databases.delete", id(&created)).await;
    assert_eq!(
        f.call("101", "databases.view", id(&created))
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    let gone = admin()
        .await
        .query_one(
            "SELECT count(*) FROM pg_database WHERE datname=$1",
            &[&view["database"].as_str().unwrap()],
        )
        .await
        .unwrap();
    assert_eq!(gone.get::<_, i64>(0), 0);
    let gone = admin()
        .await
        .query_one(
            "SELECT count(*) FROM pg_roles WHERE rolname=$1",
            &[&view["user"].as_str().unwrap()],
        )
        .await
        .unwrap();
    assert_eq!(gone.get::<_, i64>(0), 0);
    f.ok("202", "databases.delete", id(&other)).await;
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a disposable local PostgreSQL server"]
async fn capacity_inputs_and_missing_placement() {
    let f = fixture(1, None).await;
    let first = f.ok("101", "databases.create", json!({"name":"one"})).await;
    assert_eq!(
        f.call("101", "databases.create", json!({"name":"two"}))
            .await
            .unwrap_err()
            .code,
        ErrorCode::Unavailable
    );
    for input in [
        json!({"name":""}),
        json!({"name":"x".repeat(64)}),
        json!({"name":"a","extra":1}),
        json!({}),
    ] {
        assert_eq!(
            f.call("101", "databases.create", input.clone())
                .await
                .unwrap_err()
                .code,
            ErrorCode::InvalidInput,
            "{input}"
        );
    }
    assert_eq!(
        f.call("101", "databases.view", json!({"resource_id":"not-a-uuid"}))
            .await
            .unwrap_err()
            .code,
        ErrorCode::InvalidInput
    );
    assert_eq!(
        f.call(
            "101",
            "instances.create",
            json!({"name":"x","version":"18"})
        )
        .await
        .unwrap_err()
        .code,
        ErrorCode::NotSupported
    );
    f.ok("101", "databases.delete", id(&first)).await;
    f.cleanup().await;
}

#[test]
fn the_agents_observation_reads_as_instance_state() {
    let state = |v: Value| dedicated::state(&v).0;
    assert_eq!(
        state(json!({"revision": 1, "appliedRevision": null})),
        "creating"
    );
    assert_eq!(state(json!({"observed": {"ready": false}})), "creating");
    assert_eq!(
        state(json!({"revision": 3, "appliedRevision": 2, "observed": {"ready": true}})),
        "ready"
    );
    assert_eq!(state(json!({"error": "denied"})), "failed");
    assert_eq!(state(json!({"removed": true})), "failed");
}

#[tokio::test]
#[ignore = "requires a disposable local PostgreSQL server"]
async fn dedicated_instance_lifecycle() {
    let core = Core::default();
    let f = fixture(0, Some(core.clone())).await;
    assert_eq!(
        f.call(
            "101",
            "instances.create",
            json!({"name":"x","version":"16"})
        )
        .await
        .unwrap_err()
        .code,
        ErrorCode::InvalidInput
    );
    let created = f
        .ok(
            "101",
            "instances.create",
            json!({"name":"Analytics","version":"18"}),
        )
        .await;
    let unit = dedicated::unit(&created["resource"]["id"].as_str().unwrap().parse().unwrap());
    // One unit: the role's Secret, a CloudNativePG cluster and a route by TLS server name.
    let objects = core.objects(&created);
    let kinds: Vec<_> = objects
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        ["Secret", "Cluster", "IngressRouteTCP", "Certificate"]
    );
    // The server certificate names the public host, so clients can verify it.
    assert_eq!(
        objects[3]["spec"]["dnsNames"],
        json!([format!("{unit}.db.localhost")])
    );
    assert_eq!(
        objects[3]["spec"]["issuerRef"],
        json!({"name": "webdock-dev-ca", "kind": "ClusterIssuer"})
    );
    assert_eq!(
        objects[1]["spec"]["certificates"],
        json!({"serverTLSSecret": format!("{unit}-tls"), "serverCASecret": format!("{unit}-tls")})
    );
    assert_eq!(objects[0]["metadata"]["labels"]["cnpg.io/reload"], "true");
    let cluster = &objects[1]["spec"];
    assert_eq!(cluster["imageName"], "ghcr.io/cloudnative-pg/postgresql:18");
    assert_eq!(cluster["bootstrap"]["initdb"]["database"], "app");
    let role = objects[0]["stringData"]["username"].clone();
    assert_eq!(cluster["managed"]["roles"][0]["name"], role);
    let host = format!("{unit}.db.localhost");
    assert_eq!(
        objects[2]["spec"]["routes"][0]["match"],
        format!("HostSNI(`{host}`)")
    );
    assert_eq!(
        objects[2]["spec"]["routes"][0]["services"][0]["name"],
        format!("{unit}-rw")
    );
    assert_eq!(objects[2]["spec"]["tls"]["passthrough"], true);

    // Creating until the agent sees the instance ready; credentials only then.
    let view = f.ok("101", "databases.view", id(&created)).await;
    assert_eq!(
        (
            view["state"].as_str(),
            view["host"].as_str(),
            view["port"].as_i64()
        ),
        (Some("creating"), Some(host.as_str()), Some(55442))
    );
    assert_eq!(
        f.call("101", "databases.credentials", id(&created))
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    core.0.lock().unwrap().observed = json!({"ready": true});
    let view = f.ok("101", "databases.view", id(&created)).await;
    assert_eq!(
        (
            view["state"].as_str(),
            view["sslMode"].as_str(),
            view["user"].clone()
        ),
        (Some("ready"), Some("verify-full"), role.clone())
    );
    let secret = f.ok("101", "databases.credentials", id(&created)).await;
    assert_eq!(secret["value"], objects[0]["stringData"]["password"]);
    assert_eq!(
        secret["uri"],
        format!(
            "postgresql://{}:{}@{host}:55442/app?sslmode=verify-full&sslrootcert=system",
            role.as_str().unwrap(),
            secret["value"].as_str().unwrap()
        )
    );

    // Rotation applies a new Secret and stores exactly that password.
    f.ok("101", "databases.rotate", id(&created)).await;
    let rotated = f.ok("101", "databases.credentials", id(&created)).await["value"].clone();
    assert_ne!(rotated, secret["value"]);
    assert_eq!(core.objects(&created)[0]["stringData"]["password"], rotated);

    assert_eq!(
        f.call("101", "databases.delete", id(&created))
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    f.ok("101", "instances.delete", id(&created)).await;
    assert_eq!(core.0.lock().unwrap().removed, [unit]);
    f.ok("101", "instances.delete", id(&created)).await;
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a disposable local PostgreSQL server"]
async fn races_and_repeated_deletes_stay_consistent() {
    let f = fixture(1, None).await;
    // Concurrent creates never exceed a server's capacity.
    let (a, b) = tokio::join!(
        f.call("101", "databases.create", json!({"name":"a"})),
        f.call("101", "databases.create", json!({"name":"b"}))
    );
    assert_eq!(
        [&a, &b].iter().filter(|r| r.is_ok()).count(),
        1,
        "{a:?} {b:?}"
    );
    let created = a.or(b).unwrap();
    // Concurrent rotations leave the stored password working.
    let (x, y) = tokio::join!(
        f.call("101", "databases.rotate", id(&created)),
        f.call("101", "databases.rotate", id(&created))
    );
    x.unwrap();
    y.unwrap();
    let uri = f.ok("101", "databases.credentials", id(&created)).await["uri"]
        .as_str()
        .unwrap()
        .to_owned();
    connect(&uri).await.unwrap();
    // A delete whose result was lost can be repeated.
    f.ok("101", "databases.delete", id(&created)).await;
    f.ok("101", "databases.delete", id(&created)).await;
    f.cleanup().await;
}
