//! Call only from an owned Foundation/native factory after actual bootstrap/MFA/installation.
//! No SQL, actor/session/Ready seeds, environment credentials or production network requests.
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use webdock_oauth::{
    AuthorizationOutcome, AuthorizationRequest, ClientAuthentication, CodeExchange, OAuthService,
};

/// Nonsecret targets from the actual installation and registered native OAuth client.
pub struct Targets<'a> {
    pub host: &'a str,
    pub callback: &'a str,
    pub resource: &'a str,
    pub customer: &'a str,
    pub project: &'a str,
    pub database: &'a str,
    pub operation: &'a str,
}
async fn token(
    oauth: &OAuthService,
    cookie: &str,
    client: &ClientAuthentication,
    t: &Targets<'_>,
    scope: &str,
) -> String {
    let mut bytes = [0u8; 48];
    getrandom::getrandom(&mut bytes).unwrap();
    let verifier = URL_SAFE_NO_PAD.encode(bytes);
    let mut request = AuthorizationRequest::code(
        client.client_id.clone(),
        t.callback.into(),
        scope.into(),
        URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())),
    );
    request.resources = vec![t.resource.into()];
    let outcome = oauth
        .authorize(cookie, request)
        .await
        .unwrap_or_else(|_| panic!("actual owned OAuth authorization failed"));
    let outcome = match outcome {
        AuthorizationOutcome::ConsentRequired(consent) => {
            eprintln!("database_read phase=actual_oauth_consent_required");
            oauth
                .consent(cookie, &consent.request_id, true)
                .await
                .unwrap_or_else(|_| panic!("actual owned consent failed"))
        }
        other => other,
    };
    let AuthorizationOutcome::Redirect(location) = outcome else {
        panic!("actual authorization redirect required")
    };
    let code = url::Url::parse(&location)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "code")
        .expect("actual code")
        .1
        .into_owned();
    oauth
        .exchange_code(CodeExchange {
            client: client.clone(),
            code,
            redirect_uri: t.callback.into(),
            code_verifier: verifier,
        })
        .await
        .unwrap_or_else(|_| panic!("actual PKCE exchange failed"))
        .access_token
}
async fn request(
    app: &Router,
    t: &Targets<'_>,
    bearer: &str,
    path: &str,
    data: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .uri(path)
        .header("host", t.host)
        .header("authorization", format!("Bearer {bearer}"))
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2025-06-18");
    let body = if let Some(data) = data {
        builder = builder
            .method("POST")
            .header("content-type", "application/json");
        Body::from(data.to_string())
    } else {
        Body::empty()
    };
    let response = app
        .clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    assert_eq!(response.headers()["cache-control"], "no-store");
    let bytes = axum::body::to_bytes(response.into_body(), 262144)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).expect("bounded JSON metadata response"),
    )
}
fn safe(value: &Value) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                assert!(
                    ![
                        "desired",
                        "nativeAuthority",
                        "native_authority",
                        "leaseToken",
                        "lease_until",
                        "writeCredentials",
                        "recoveryCredentials",
                        "write_encrypted",
                        "recovery_encrypted",
                        "credentials_encrypted",
                        "secretAccessKey",
                        "accessKeyId"
                    ]
                    .contains(&key.as_str()),
                    "private metadata field"
                );
                safe(value);
            }
        }
        Value::Array(array) => {
            for value in array {
                safe(value)
            }
        }
        _ => {}
    }
}
async fn read_matrix(app: &Router, t: &Targets<'_>, bearer: &str) -> u64 {
    let reads = [
        (
            "list_database_projects",
            json!({"customerID":t.customer,"limit":1}),
            format!("customers/{}/projects?limit=1", t.customer),
        ),
        (
            "list_managed_databases",
            json!({"projectID":t.project,"limit":1}),
            format!("projects/{}/databases?limit=1", t.project),
        ),
        (
            "get_managed_database",
            json!({"databaseID":t.database}),
            t.database.into(),
        ),
        (
            "get_database_permissions",
            json!({"projectID":t.project}),
            format!("projects/{}/permissions", t.project),
        ),
        (
            "list_database_plans",
            json!({"projectID":t.project,"limit":1}),
            format!("projects/{}/plans?limit=1", t.project),
        ),
        (
            "get_database_backup_history",
            json!({"databaseID":t.database,"limit":1}),
            format!("{}/backups?limit=1", t.database),
        ),
        (
            "get_database_operation",
            json!({"databaseID":t.database,"operationID":t.operation}),
            format!("{}/operations/{}", t.database, t.operation),
        ),
    ];
    for (name, args, path) in reads {
        let (status, rest) = request(
            app,
            t,
            bearer,
            &format!("/api/hosting/databases/{path}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "actual database REST read");
        safe(&rest);
        let (status,rpc)=request(app,t,bearer,"/api/mcp",Some(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":name,"arguments":args}}))).await;
        assert_eq!(status, StatusCode::OK, "actual database MCP read");
        assert_ne!(rpc["result"]["isError"], true, "database MCP denied");
        let metadata: Value = serde_json::from_str(
            rpc["result"]["content"][0]["text"]
                .as_str()
                .expect("actual read content"),
        )
        .unwrap();
        safe(&metadata);
        // Time-dependent observations may refresh between requests; stable identity remains exact.
        for key in ["id", "databaseID", "projectID", "operator", "canWrite"] {
            assert_eq!(metadata.get(key), rest.get(key));
        }
        if name == "get_managed_database" {
            assert_eq!(rest["id"], t.database);
            assert_eq!(rest["backup"]["publicRecoveryAvailable"], false);
            assert_eq!(
                rest["backup"]["disabledReason"],
                "production-qualification-required"
            );
            for field in [
                "rpoSeconds",
                "rtoSeconds",
                "recoverableUntil",
                "lastRestoreTestAt",
            ] {
                assert!(rest["backup"][field].is_null());
            }
        }
    }
    14
}
/// Compiled preparation only until invoked with the genuine fresh factory's private in-memory inputs.
/// This tests token issue/read/revocation; preview/membership/through-commit overlap require the owner fixture.
pub async fn qualify(
    app: &Router,
    oauth: &OAuthService,
    cookie: &str,
    client: &ClientAuthentication,
    t: Targets<'_>,
) -> Value {
    let mut reads = 0;
    for scope in ["openid hosting:read", "openid hosting:read hosting:write"] {
        let bearer = token(oauth, cookie, client, &t, scope).await;
        let (status, discovery) = request(
            app,
            &t,
            &bearer,
            "/api/mcp",
            Some(json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let tools = discovery["result"]["tools"]
            .as_array()
            .expect("real read-token discovery");
        for (_, name) in webdock_databases::DATABASE_READ_TOOLS {
            assert!(tools.iter().any(|tool| tool["name"] == *name));
        }
        reads += read_matrix(app, &t, &bearer).await;
        for action in [
            "databases.backup",
            "databases.backupDestinations.register",
            "databases.credentials",
            "databases.restore",
            "databases.exportMaterial",
        ] {
            let (status,_)=request(app,&t,&bearer,"/api/hosting/commands",Some(json!({"action":action,"databaseID":t.database,"revision":1,"idempotencyKey":"owned-read-boundary-denial"}))).await;
            assert_eq!(
                status,
                StatusCode::BAD_REQUEST,
                "read adapter rejects action before dispatch"
            );
        }
        oauth
            .revoke(client, &bearer)
            .await
            .unwrap_or_else(|_| panic!("actual token revocation failed"));
        assert_eq!(
            request(
                app,
                &t,
                &bearer,
                &format!("/api/hosting/databases/{}", t.database),
                None
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(
                app,
                &t,
                &bearer,
                "/api/mcp",
                Some(json!({"jsonrpc":"2.0","id":3,"method":"tools/list"}))
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
    }
    json!({"actualPkceTokens":2,"metadataReads":reads,"deniedMutationActions":10,"revokedRestAndMcp":4,"secretsReturned":false,"throughCommitRaceQualified":false,"nativeRecoveryQualified":false})
}

/// Only the separately reviewed, fresh pinned Foundation component opts into this lane.
pub fn enabled() -> bool {
    std::env::var("WEBDOCK_DATABASE_READ_COMPONENT_TEST").as_deref() == Ok("1")
}

/// Private connection custody from the runner's one verified fresh PostgreSQL container.
/// No identity, session, grant, plugin or database resource is accepted as fixture input.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OwnedFixture {
    pub container: String,
    pub url: url::Url,
    pub passwords: std::collections::BTreeMap<String, String>,
}
pub fn owned_fixture() -> OwnedFixture {
    use std::{os::unix::fs::MetadataExt, path::PathBuf, process::Command};
    assert!(enabled());
    let path = PathBuf::from(std::env::var_os("WEBDOCK_DATABASE_READ_COMPONENT_INPUT").unwrap());
    assert!(path.is_absolute());
    for ancestor in path.ancestors() {
        assert!(!ancestor.is_symlink(), "component input symlink");
    }
    let private = path.parent().unwrap();
    let candidate = private.parent().unwrap();
    let owner = candidate.file_name().unwrap().to_str().unwrap();
    assert!(
        owner
            .strip_prefix("pg-read-foundation-")
            .is_some_and(|s| s.len() == 12 && s.bytes().all(|b| b.is_ascii_hexdigit()))
    );
    assert_eq!(
        candidate.parent().unwrap().file_name().unwrap(),
        ".runtime-test"
    );
    assert_eq!(private.file_name().unwrap(), "private");
    for directory in [private, candidate] {
        let metadata = directory.metadata().unwrap();
        assert_eq!(metadata.uid(), rustix::process::geteuid().as_raw());
        assert_eq!(metadata.mode() & 0o777, 0o700);
    }
    let pin =
        webdock_control_plane::config::read_private_file(&candidate.join("pin.json")).unwrap();
    assert_eq!(
        hex::encode(Sha256::digest(&pin)),
        std::env::var("WEBDOCK_DATABASE_READ_COMPONENT_PIN").unwrap()
    );
    let pin: Value = serde_json::from_slice(&pin).unwrap();
    assert_eq!(pin["owner"], owner);
    let input = webdock_control_plane::config::read_private_file(&path).unwrap();
    let input: OwnedFixture = serde_json::from_slice(&input).unwrap();
    assert!(input.container.len() == 64 && input.container.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(input.url.host_str(), Some("127.0.0.1"));
    assert_eq!(input.url.path(), "/webdock_operator_test");
    assert_eq!(input.url.username(), "postgres");
    assert!(input.url.password().is_some() && input.url.query().is_none());
    assert_eq!(input.url.port().map(u64::from), pin["port"].as_u64());
    webdock_persistence::roles::validate_passwords(&input.passwords).unwrap();
    let inspected=Command::new("docker").args(["--host","unix:///var/run/docker.sock","inspect","--type","container","--format",r#"{"id":{{json .Id}},"image":{{json .Image}},"name":{{json .Name}},"owner":{{json (index .Config.Labels "webdock.pg-review-owner")}},"mounts":{{json .Mounts}},"ports":{{json .NetworkSettings.Ports}}}"#,&input.container]).output().unwrap();
    assert!(
        inspected.status.success(),
        "owned fixture inspection failed"
    );
    let inspected: Value = serde_json::from_slice(&inspected.stdout).unwrap();
    assert_eq!(inspected["id"], input.container);
    assert_eq!(inspected["image"], pin["image"]);
    assert_eq!(inspected["owner"], owner);
    assert_eq!(inspected["name"], format!("/{owner}"));
    assert!(
        inspected["mounts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["Type"] != "volume")
    );
    assert_eq!(
        inspected["ports"]["5432/tcp"],
        json!([{"HostIp":"127.0.0.1","HostPort":pin["port"].as_u64().unwrap().to_string()}])
    );
    input
}

pub struct Foundation<'a> {
    pub app: &'a Router,
    pub auth: &'a sea_orm::DatabaseConnection,
    pub owner: &'a sea_orm::DatabaseConnection,
    pub identity: &'a std::sync::Arc<webdock_identity::IdentityService>,
    pub oauth: &'a std::sync::Arc<OAuthService>,
    pub browser: &'a std::sync::Arc<webdock_browser_session::BrowserSessions>,
    pub databases: &'a std::sync::Arc<webdock_control_plane::databases::DatabaseEndpoints>,
    pub registry: &'a std::sync::Arc<webdock_registry::RegistryService>,
    pub native: &'a str,
    pub console: &'a str,
    pub private_directory: &'a std::path::Path,
}
async fn console_post(app: &Router, cookie: &str, path: &str, input: Value) -> Value {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("host", "console.example.test")
                .header("origin", "https://console.example.test")
                .header("cookie", cookie)
                .header("content-type", "application/json")
                .body(Body::from(input.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "actual Foundation product command"
    );
    serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 262144)
            .await
            .unwrap(),
    )
    .unwrap()
}
/// Decoder used only for this exact Browser Console producer (databases.rs returns {data}).
pub fn console_database_rows(body: &Value) -> Option<&Vec<Value>> {
    let envelope = body.as_object()?;
    if envelope.len() != 1 {
        return None;
    }
    envelope.get("data")?.as_array()
}
/// Invoked only after the existing fixture completes actual password/TOTP and Console PKCE.
pub async fn foundation(c: Foundation<'_>) {
    use std::{collections::HashMap, fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt};
    use webdock_oauth::{ClientAuthMethod, ClientPurpose, ClientRegistration};
    assert!(enabled());
    console_post(
        c.app,
        c.console,
        "/api/console/plugins/builtins/databases/activate",
        json!({"idempotencyKey":"pg-read-foundation-activate"}),
    )
    .await;
    let customer = console_post(
        c.app,
        c.console,
        "/api/console/registry/customers",
        json!({"name":"Owned bearer metadata qualification","country":"ch"}),
    )
    .await;
    let project = console_post(
        c.app,
        c.console,
        "/api/console/registry/projects",
        json!({"name":"Owned bearer read project","customer":customer["id"]}),
    )
    .await;
    // This is the existing explicit offline configuration service, not SQL grant seeding.
    let registered = c
        .oauth
        .provision_client(
            c.owner,
            ClientRegistration {
                name: "Owned Foundation MCP hosting reader".into(),
                redirect_uris: vec!["https://client.example.test/callback".into()],
                post_logout_redirect_uris: vec![],
                scopes: vec![
                    "openid".into(),
                    "hosting:read".into(),
                    "hosting:write".into(),
                ],
                auth_method: ClientAuthMethod::ClientSecretPost,
                allow_refresh: false,
                skip_consent: false,
                purpose: ClientPurpose::McpHosting,
            },
        )
        .await
        .unwrap_or_else(|_| panic!("owned offline MCP client configuration failed"));
    let secret_path = c.private_directory.join("pg-read-resource-client");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&secret_path)
        .unwrap();
    file.write_all(registered.client_secret.as_ref().unwrap().as_bytes())
        .unwrap();
    file.sync_all().unwrap();
    drop(file);
    let client = ClientAuthentication {
        client_id: registered.client_id.clone(),
        client_secret: registered.client_secret,
        method: ClientAuthMethod::ClientSecretPost,
    };
    let app = webdock_control_plane::http::build_app_with_domains(
        webdock_core::config::PlatformConfig::from_map(&HashMap::from([
            (
                "WEBDOCK_AUTH_ORIGIN".into(),
                "https://auth.example.test".into(),
            ),
            (
                "WEBDOCK_STUDIO_ORIGIN".into(),
                "https://studio.example.test".into(),
            ),
            (
                "WEBDOCK_CONSOLE_ORIGIN".into(),
                "https://console.example.test".into(),
            ),
            (
                "WEBDOCK_FEATURES".into(),
                r#"{"databases":"enabled"}"#.into(),
            ),
        ]))
        .unwrap(),
        c.auth.clone(),
        webdock_control_plane::http::Services {
            identity: Some(c.identity.clone()),
            oauth: Some(c.oauth.clone()),
            browser: Some(c.browser.clone()),
            databases: Some(c.databases.clone()),
            registry: Some(c.registry.clone()),
            resource_client: Some(webdock_control_plane::resource_auth::ResourceClientConfig {
                client_id: registered.client_id,
                secret: secret_path.clone(),
            }),
            ..Default::default()
        },
    );
    // No database/operation exists here. This real Registry project ID is a negative
    // absence probe only, never represented as a product-created database identity.
    let t = Targets {
        host: "studio.example.test",
        callback: "https://client.example.test/callback",
        resource: "https://studio.example.test/api/mcp",
        customer: customer["id"].as_str().unwrap(),
        project: project["id"].as_str().unwrap(),
        database: project["id"].as_str().unwrap(),
        operation: project["id"].as_str().unwrap(),
    };
    let resources = console_post(
        c.app,
        c.console,
        "/api/console/databases",
        json!({"action":"databases.list","projectID":t.project}),
    )
    .await;
    let evidence = if let Some(resource) = console_database_rows(&resources)
        .expect("actual database list")
        .first()
    {
        let database = resource["id"]
            .as_str()
            .expect("product-created database ID");
        let operation = resource["operationID"]
            .as_str()
            .expect("product-created operation ID");
        qualify(
            &app,
            c.oauth,
            c.native,
            &client,
            Targets {
                database,
                operation,
                ..t
            },
        )
        .await
    } else {
        qualify_available(&app, c.oauth, c.native, &client, t).await
    };
    use sea_orm::ConnectionTrait;
    let journal:Value = c.owner.query_one_raw(sea_orm::Statement::from_string(sea_orm::DbBackend::Postgres,
        "SELECT jsonb_object_agg(version::text,jsonb_build_object('checksum',encode(checksum,'hex'),'success',success)) AS value FROM webdock_api_migrations.rust_journal"))
        .await.unwrap().unwrap().try_get("","value").unwrap();
    let mut evidence = evidence;
    evidence["nativeJournalAfter"] = journal;
    std::fs::remove_file(&secret_path).unwrap();
    let output = std::path::PathBuf::from(
        std::env::var_os("WEBDOCK_DATABASE_READ_COMPONENT_INPUT").unwrap(),
    )
    .parent()
    .unwrap()
    .parent()
    .unwrap()
    .join("metadata-evidence.json");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(output)
        .unwrap();
    file.write_all(&serde_json::to_vec_pretty(&evidence).unwrap())
        .unwrap();
    file.sync_all().unwrap();
    eprintln!(
        "database_read phase=genuine_metadata_token_revoke_complete full_object_reads=unqualified"
    );
}

async fn qualify_available(
    app: &Router,
    oauth: &OAuthService,
    cookie: &str,
    client: &ClientAuthentication,
    t: Targets<'_>,
) -> Value {
    let mut read_count = 0;
    let mut absences = Vec::new();
    for scope in ["openid hosting:read", "openid hosting:read hosting:write"] {
        let bearer = token(oauth, cookie, client, &t, scope).await;
        eprintln!("database_read phase=actual_pkce_token_issued");
        let (status, discovery) = request(
            app,
            &t,
            &bearer,
            "/api/mcp",
            Some(json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let tools = discovery["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 7);
        for (_, name) in webdock_databases::DATABASE_READ_TOOLS {
            assert!(tools.iter().any(|v| v["name"] == *name));
        }
        for (tool, args, path) in [
            (
                "list_database_projects",
                json!({"customerID":t.customer,"limit":1}),
                format!("customers/{}/projects?limit=1", t.customer),
            ),
            (
                "list_managed_databases",
                json!({"projectID":t.project,"limit":1}),
                format!("projects/{}/databases?limit=1", t.project),
            ),
            (
                "get_database_permissions",
                json!({"projectID":t.project}),
                format!("projects/{}/permissions", t.project),
            ),
            (
                "list_database_plans",
                json!({"projectID":t.project,"limit":1}),
                format!("projects/{}/plans?limit=1", t.project),
            ),
        ] {
            let (status, rest) = request(
                app,
                &t,
                &bearer,
                &format!("/api/hosting/databases/{path}"),
                None,
            )
            .await;
            assert_eq!(
                status,
                StatusCode::OK,
                "available genuine metadata REST read"
            );
            safe(&rest);
            let (status,rpc)=request(app,&t,&bearer,"/api/mcp",Some(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":tool,"arguments":args}}))).await;
            assert_eq!(status, StatusCode::OK);
            assert_ne!(rpc["result"]["isError"], true);
            let metadata: Value =
                serde_json::from_str(rpc["result"]["content"][0]["text"].as_str().unwrap())
                    .unwrap();
            safe(&metadata);
            assert_eq!(metadata, rest);
            if tool == "list_managed_databases" {
                assert_eq!(rest["items"], json!([]));
                assert!(rest["nextCursor"].is_null());
            }
            if tool == "get_database_permissions" {
                assert_eq!(rest, json!({"operator":true,"canWrite":false}));
            }
            read_count += 2;
        }
        for (tool, args, path) in [
            (
                "get_managed_database",
                json!({"databaseID":t.database}),
                t.database.into(),
            ),
            (
                "get_database_backup_history",
                json!({"databaseID":t.database,"limit":1}),
                format!("{}/backups", t.database),
            ),
            (
                "get_database_operation",
                json!({"databaseID":t.database,"operationID":t.operation}),
                format!("{}/operations/{}", t.database, t.operation),
            ),
        ] {
            let (status, _) = request(
                app,
                &t,
                &bearer,
                &format!("/api/hosting/databases/{path}"),
                None,
            )
            .await;
            assert_eq!(status, StatusCode::NOT_FOUND, "actual missing object probe");
            let (status,rpc)=request(app,&t,&bearer,"/api/mcp",Some(json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":tool,"arguments":args}}))).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(rpc["result"]["isError"], true);
            absences.push(json!({"tool":tool,"restStatus":404,"mcpIsError":true,"qualification":"UNQUALIFIED/no-domain-resource"}));
        }
        for action in [
            "databases.backup",
            "databases.backupDestinations.register",
            "databases.credentials",
            "databases.restore",
            "databases.exportMaterial",
        ] {
            let (status,_)=request(app,&t,&bearer,"/api/hosting/commands",Some(json!({"action":action,"databaseID":t.database,"revision":1,"idempotencyKey":"owned-read-boundary-denial"}))).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
        }
        oauth
            .revoke(client, &bearer)
            .await
            .unwrap_or_else(|_| panic!("actual token revocation failed"));
        assert_eq!(
            request(
                app,
                &t,
                &bearer,
                &format!("/api/hosting/databases/projects/{}/databases", t.project),
                None
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(
                app,
                &t,
                &bearer,
                "/api/mcp",
                Some(json!({"jsonrpc":"2.0","id":4,"method":"tools/list"}))
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
        eprintln!("database_read phase=available_metadata_mutation_denial_revocation_complete");
    }
    json!({"actualPkceTokens":2,"availableMetadataReadFamilies":4,"positiveMetadataReads":read_count,"discoveredReadTools":7,"deniedMutationActions":10,"revokedRestAndMcp":4,"missingObjectProbes":absences,"positiveDatabaseObjectReads":0,"fullSevenObjectRecipeQualified":false,"probeIDSource":"actual Registry project; never database/operation identity","throughCommitRaceQualified":false,"nativeRecoveryQualified":false})
}
