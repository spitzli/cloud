//! The plugin against a stand-in for Core's `plugin.workloads` calls, with real plugin storage on a
//! disposable local PostgreSQL (`WEBDOCK_HOSTING_PLUGIN_TEST_ADMIN_URL`, loopback). The objects
//! themselves run on the local k3s in the live dev stack.
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio_postgres::NoTls;
use webdock_hosting::{Handler, Plugin, Settings, placement, seal::Sealer, store::Store};
use webdock_plugin_protocol::{ErrorCode, InvocationClaims, Manifest, PluginError};
use webdock_plugin_runtime::ActionHandler;

const IMAGE: &str = "nginx:stable-alpine";

/// What Core holds for each unit: the objects of the last apply and what the agent last reported.
#[derive(Default)]
struct Units {
    objects: std::collections::HashMap<String, Value>,
    revision: i64,
    applied: i64,
    observed: Value,
    error: Option<String>,
    removed: Vec<String>,
    /// Core refuses the next apply with this message (a plan limit, an unverified host).
    refuse: Option<String>,
}
#[derive(Clone, Default)]
struct Core(Arc<Mutex<Units>>);
#[tonic::async_trait]
impl placement::Workloads for Core {
    async fn call(&self, method: &str, input: Value) -> Result<Value, PluginError> {
        let mut units = self.0.lock().unwrap();
        let unit = input["unit"].as_str().unwrap().to_owned();
        Ok(match method {
            "apply" if units.refuse.is_some() => {
                let message = units.refuse.take().unwrap_or_default();
                return Err(PluginError::new(ErrorCode::Conflict, message));
            }
            "apply" => {
                units.revision += 1;
                units.objects.insert(unit, input["objects"].clone());
                json!({"revision": units.revision})
            }
            "remove" => {
                units.objects.remove(&unit);
                units.removed.push(unit);
                json!({})
            }
            _ if !units.objects.contains_key(&unit) => json!({"removed": true}),
            _ => json!({"revision": units.revision, "appliedRevision": units.applied,
                        "observed": units.observed, "error": units.error}),
        })
    }
}
impl Core {
    /// The agent applied the latest revision and saw this.
    fn report(&self, observed: Value, error: Option<&str>) {
        let mut units = self.0.lock().unwrap();
        units.applied = units.revision;
        units.observed = observed;
        units.error = error.map(str::to_owned);
    }
    fn objects(&self, app: &str) -> Value {
        let unit = placement::unit(&app.parse().unwrap());
        self.0.lock().unwrap().objects[&unit].clone()
    }
}
fn config() -> placement::PlacementConfig {
    placement::PlacementConfig {
        domain: "apps.localhost".into(),
        public_url: "https://{host}:55481".into(),
        entry_points: vec!["websecure".into()],
        issuer: Some("webdock-dev-ca".into()),
    }
}

fn admin_url() -> url::Url {
    let url = url::Url::parse(
        &std::env::var("WEBDOCK_HOSTING_PLUGIN_TEST_ADMIN_URL").expect("disposable local server"),
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
    core: Core,
    storage: String,
}
async fn fixture() -> Fixture {
    let storage = format!("hosting_test_{}", uuid::Uuid::new_v4().simple());
    admin()
        .await
        .batch_execute(&format!("CREATE DATABASE {storage}"))
        .await
        .unwrap();
    let mut url = admin_url();
    url.set_path(&storage);
    let core = Core::default();
    let plugin = Plugin::new(
        Store::connect(url.as_str()).await.unwrap(),
        Sealer::new(&"35".repeat(32)).unwrap(),
        placement::Placement::new(Box::new(core.clone()), config()),
        Settings {
            require_digest: false,
        },
    );
    Fixture {
        handler: Handler(plugin),
        core,
        storage,
    }
}
impl Fixture {
    async fn call(&self, project: &str, action: &str, input: Value) -> Result<Value, PluginError> {
        let claims = InvocationClaims {
            job: None,
            v: 1,
            id: uuid::Uuid::new_v4().to_string(),
            plugin_id: "hosting".into(),
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
    async fn code(&self, project: &str, action: &str, input: Value) -> ErrorCode {
        self.call(project, action, input).await.unwrap_err().code
    }
    async fn view(&self, app: &str) -> Value {
        self.ok("101", "apps.view", json!({"resource_id": app}))
            .await
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

#[test]
fn shipped_manifest_is_the_reviewed_contract() {
    webdock_hosting::validate_manifest(
        &Manifest::parse(include_bytes!("../manifest.json")).unwrap(),
    )
    .unwrap();
}

#[test]
fn image_references_and_environment_are_bounded() {
    for good in [
        "nginx:stable-alpine",
        "ghcr.io/org/app:1.2.3",
        "registry.example.com:5000/a/b@sha256:".to_owned().as_str(),
    ] {
        let good = if good.ends_with("sha256:") {
            format!("{good}{}", "a".repeat(64))
        } else {
            good.to_owned()
        };
        assert!(
            webdock_hosting::image_reference(&good, false).is_ok(),
            "{good}"
        );
    }
    for bad in [
        "",
        "Nginx",
        "nginx latest",
        "-x",
        "a//b",
        "nginx@sha256:short",
        "nginx;rm",
    ] {
        assert!(
            webdock_hosting::image_reference(bad, false).is_err(),
            "{bad:?}"
        );
    }
    assert!(
        webdock_hosting::image_reference("nginx:stable-alpine", true).is_err(),
        "production requires a digest"
    );
    let env =
        webdock_hosting::environment("# comment\nA=1\n\nDATABASE_URL=postgres://x=y\n").unwrap();
    assert_eq!(
        env,
        vec![
            ("A".into(), "1".into()),
            ("DATABASE_URL".into(), "postgres://x=y".into())
        ]
    );
    for bad in ["NOVALUE", "1A=x", "A-B=x", "A=1\nA=2", "A=\u{1b}x"] {
        assert!(webdock_hosting::environment(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn the_agents_observation_reads_as_plain_status() {
    let seen = |v: Value| placement::status(&v, true);
    assert_eq!(
        seen(json!({"revision":2,"appliedRevision":1})).status,
        "deploying"
    );
    assert_eq!(
        seen(json!({"revision":2,"appliedRevision":null})).status,
        "deploying"
    );
    assert_eq!(
        seen(json!({"revision":2,"appliedRevision":2,"observed":{"ready":true}})).status,
        "running"
    );
    assert_eq!(
        placement::status(
            &json!({"revision":2,"appliedRevision":2,"observed":{"ready":true}}),
            false
        )
        .status,
        "stopped"
    );
    assert_eq!(seen(json!({"removed":true})).status, "missing");
    let pull = seen(
        json!({"revision":1,"appliedRevision":1,"observed":{"ready":false,"problems":[{"reason":"ImagePullBackOff"}]}}),
    );
    assert_eq!(pull.status, "failed");
    assert!(pull.error.unwrap().contains("image"));
    let refused = seen(json!({"revision":1,"appliedRevision":1,"error":"denied"}));
    assert_eq!(
        (refused.status, refused.error.as_deref()),
        ("failed", Some("The cluster refused the app: denied"))
    );
    assert_eq!(
        seen(json!({"revision":1,"appliedRevision":1,"observed":{"logs":["a","b"]}})).logs,
        ["a", "b"]
    );
}

#[test]
fn hosts_are_dns_labels_under_the_app_domain() {
    let id: uuid::Uuid = "0123abcd-0000-4000-8000-000000000000".parse().unwrap();
    assert_eq!(
        placement::host(&config(), &id, "My Shop!"),
        "my-shop-0123abcd.apps.localhost"
    );
    assert_eq!(
        placement::host(&config(), &id, "äöü"),
        "app-0123abcd.apps.localhost"
    );
    assert!(
        placement::host(&config(), &id, &"x".repeat(63))
            .split('.')
            .next()
            .unwrap()
            .len()
            <= 63
    );
}

#[tokio::test]
#[ignore = "requires a disposable local PostgreSQL server"]
async fn apps_become_cluster_units() {
    let f = fixture().await;
    let app = f
        .ok(
            "101",
            "apps.create",
            json!({"name":"Shop","image":IMAGE,"port":8080}),
        )
        .await["resource"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let unit = placement::unit(&app.parse().unwrap());
    // One unit: env in a Secret, a Deployment with the default memory, a Service and a route.
    let objects = f.core.objects(&app);
    let kinds: Vec<_> = objects
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        [
            "Secret",
            "Deployment",
            "Service",
            "IngressRoute",
            "Certificate"
        ]
    );
    let deployment = &objects[1];
    assert_eq!(deployment["metadata"]["name"], unit);
    assert_eq!(deployment["spec"]["replicas"], 1);
    let container = &deployment["spec"]["template"]["spec"]["containers"][0];
    assert_eq!(
        (
            container["image"].as_str(),
            container["ports"][0]["containerPort"].as_i64()
        ),
        (Some(IMAGE), Some(8080))
    );
    assert_eq!(container["resources"]["limits"]["memory"], "256Mi");
    assert_eq!(objects[2]["spec"]["ports"][0]["targetPort"], 8080);
    let short = &app.replace('-', "")[..8];
    assert_eq!(
        objects[3]["spec"]["routes"][0]["match"],
        format!("Host(`shop-{short}.apps.localhost`)")
    );
    assert_eq!(objects[3]["spec"]["entryPoints"], json!(["websecure"]));
    // HTTPS with a certificate for the app's own host.
    assert_eq!(
        objects[3]["spec"]["tls"],
        json!({"secretName": format!("{unit}-tls")})
    );
    assert_eq!(
        objects[4]["spec"]["dnsNames"],
        json!([format!("shop-{short}.apps.localhost")])
    );
    assert_eq!(objects[4]["spec"]["secretName"], format!("{unit}-tls"));

    // Until the agent reports, the app is deploying and has no address.
    let view = f.view(&app).await;
    assert_eq!(
        (view["status"].as_str(), view["url"].as_str()),
        (Some("deploying"), None)
    );
    f.core
        .report(json!({"ready": true, "logs": ["GET / 200"]}), None);
    let view = f.view(&app).await;
    assert_eq!(view["status"], "running");
    assert_eq!(
        view["url"],
        format!("https://shop-{short}.apps.localhost:55481")
    );
    assert_eq!(
        f.ok("101", "apps.logs", json!({"resource_id": app})).await["lines"],
        json!(["GET / 200"])
    );
    assert_eq!(
        f.code("202", "apps.view", json!({"resource_id": app}))
            .await,
        ErrorCode::NotFound,
        "another project cannot see it"
    );

    // Environment lands in the Secret; values never come back, only the names.
    f.ok(
        "101",
        "apps.env",
        json!({"resource_id": app, "env": "GREETING=hello\nAPI_TOKEN=s3cret"}),
    )
    .await;
    assert_eq!(
        f.core.objects(&app)[0]["stringData"],
        json!({"GREETING":"hello","API_TOKEN":"s3cret"})
    );
    assert_eq!(f.view(&app).await["status"], "deploying");
    f.core.report(json!({"ready": true}), None);
    let view = f.view(&app).await;
    assert_eq!(view["env"], json!(["GREETING", "API_TOKEN"]));
    assert!(!view.to_string().contains("s3cret"));

    // Stop scales to zero, start back to one, restart rolls the pods.
    f.ok(
        "101",
        "apps.power",
        json!({"resource_id": app, "action": "stop"}),
    )
    .await;
    assert_eq!(f.core.objects(&app)[1]["spec"]["replicas"], 0);
    f.core.report(json!({"ready": true}), None);
    assert_eq!(f.view(&app).await["status"], "stopped");
    f.ok(
        "101",
        "apps.power",
        json!({"resource_id": app, "action": "start"}),
    )
    .await;
    assert_eq!(f.core.objects(&app)[1]["spec"]["replicas"], 1);
    let before =
        f.core.objects(&app)[1]["spec"]["template"]["metadata"]["annotations"]["restarted"].clone();
    f.ok(
        "101",
        "apps.power",
        json!({"resource_id": app, "action": "restart"}),
    )
    .await;
    assert_ne!(
        f.core.objects(&app)[1]["spec"]["template"]["metadata"]["annotations"]["restarted"],
        before
    );

    // A broken image fails visibly; resize changes the limit.
    f.ok(
        "101",
        "apps.deploy",
        json!({"resource_id": app, "image": "registry.invalid/none/app:1", "port": 80}),
    )
    .await;
    f.core.report(
        json!({"ready": false, "problems": [{"reason": "ErrImagePull"}]}),
        None,
    );
    let failed = f.view(&app).await;
    assert_eq!(failed["status"], "failed");
    assert!(failed["error"].as_str().unwrap().contains("image"));
    f.ok(
        "101",
        "apps.resize",
        json!({"resource_id": app, "memory_mb": 512}),
    )
    .await;
    assert_eq!(
        f.core.objects(&app)[1]["spec"]["template"]["spec"]["containers"][0]["resources"]["limits"]
            ["memory"],
        "512Mi"
    );
    assert_eq!(f.view(&app).await["memoryMb"], 512);
    // Own domains join the route and the certificate; the platform's domain and junk are refused.
    f.ok(
        "101",
        "apps.domains",
        json!({"resource_id": app, "domains": "Shop.Example.test\n\nwww.shop.example.test.\nshop.example.test"}),
    )
    .await;
    let objects = f.core.objects(&app);
    let own = f.view(&app).await["host"].as_str().unwrap().to_owned();
    assert_eq!(
        objects[3]["spec"]["routes"][0]["match"],
        format!("Host(`{own}`) || Host(`shop.example.test`) || Host(`www.shop.example.test`)")
    );
    assert_eq!(
        objects[4]["spec"]["dnsNames"],
        json!([own, "shop.example.test", "www.shop.example.test"])
    );
    assert_eq!(
        f.view(&app).await["domains"],
        json!(["shop.example.test", "www.shop.example.test"])
    );
    for bad in [
        "x.apps.localhost",
        "*.example.test",
        "localhost",
        "a b.example",
    ] {
        assert_eq!(
            f.code(
                "101",
                "apps.domains",
                json!({"resource_id": app, "domains": bad})
            )
            .await,
            ErrorCode::InvalidInput,
            "{bad}"
        );
    }
    // When Core refuses a change, the app stays as it was and the customer reads why.
    let limit = "Your plan allows 1024 MiB memory; 512 MiB are in use and this needs 2048 MiB.";
    f.core.0.lock().unwrap().refuse = Some(limit.into());
    let refused = f
        .call(
            "101",
            "apps.resize",
            json!({"resource_id": app, "memory_mb": 2048}),
        )
        .await
        .unwrap_err();
    assert_eq!(refused.message, limit);
    assert_eq!(f.view(&app).await["memoryMb"], 512);

    f.ok("101", "apps.delete", json!({"resource_id": app}))
        .await;
    assert_eq!(f.core.0.lock().unwrap().removed, [unit]);
    assert_eq!(
        f.code("101", "apps.view", json!({"resource_id": app}))
            .await,
        ErrorCode::NotFound
    );
    f.ok("101", "apps.delete", json!({"resource_id": app}))
        .await;
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a disposable local PostgreSQL server"]
async fn changes_never_interleave() {
    let f = fixture().await;
    let app = f
        .ok(
            "101",
            "apps.create",
            json!({"name":"Race","image":IMAGE,"port":80}),
        )
        .await["resource"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (env, resize) = tokio::join!(
        f.call("101", "apps.env", json!({"resource_id": app, "env": "A=1"})),
        f.call(
            "101",
            "apps.resize",
            json!({"resource_id": app, "memory_mb": 512})
        )
    );
    assert_eq!(
        [&env, &resize].iter().filter(|r| r.is_ok()).count(),
        1,
        "{env:?} {resize:?}"
    );
    let loser = if env.is_ok() { &resize } else { &env };
    assert_eq!(loser.as_ref().unwrap_err().code, ErrorCode::Conflict);
    // The stored app and the applied objects are exactly what the winning change asked for.
    let view = f.view(&app).await;
    let memory = f.core.objects(&app)[1]["spec"]["template"]["spec"]["containers"][0]["resources"]
        ["limits"]["memory"]
        .clone();
    if env.is_ok() {
        assert_eq!(
            (view["env"].clone(), view["memoryMb"].as_i64(), memory),
            (json!(["A"]), Some(256), json!("256Mi"))
        );
    } else {
        assert_eq!(
            (view["env"].clone(), view["memoryMb"].as_i64(), memory),
            (json!([]), Some(512), json!("512Mi"))
        );
    }
    f.cleanup().await;
}
