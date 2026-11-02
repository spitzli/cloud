//! Actual Rust router + restricted native services. No product runtime or provider is invoked.
#[path = "../../plugin-host/tests/support/catalog_fixture.rs"]
mod signed_catalog;
#[path = "support/plugin_installation.rs"]
mod support;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use support::*;
use tower::ServiceExt;
const CATALOG: &str = "/api/console/plugins/catalog";
const INSTALL: &str = "/api/console/plugins/install";
const RELEASES: &str = "/api/console/plugins/postgres/releases";
const UPDATE: &str = "/api/console/plugins/postgres/update";
const ROLLBACK: &str = "/api/console/plugins/postgres/rollback";

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    cookie: Option<&str>,
    host: &str,
    origin: Option<&str>,
    body: Option<&Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("host", host);
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    if let Some(origin) = origin {
        request = request.header("origin", origin);
    }
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let response = app
        .clone()
        .oneshot(
            request
                .body(body.map_or_else(Body::empty, |v| Body::from(v.to_string())))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    assert_eq!(response.headers()["cache-control"], "no-store");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    for private in [
        "FIXTURE_PRIVATE",
        "private.invalid",
        "worker-only",
        "private-only",
        "postgres://",
        "postgresql://",
        "_authority",
    ] {
        assert!(
            !String::from_utf8_lossy(&bytes).contains(private),
            "public response disclosed {private}"
        );
    }
    (status, body)
}
async fn request(
    app: &Router,
    method: &str,
    path: &str,
    cookie: &str,
    body: Option<&Value>,
) -> (StatusCode, Value) {
    call(
        app,
        method,
        path,
        Some(cookie),
        "localhost:3120",
        Some(ORIGIN),
        body,
    )
    .await
}
fn input(descriptor: &Value) -> Value {
    json!({"catalogID":descriptor["catalogID"],"version":descriptor["version"],"reviewSha256":descriptor["targets"][0]["reviewSha256"],"targetID":"local-eu","configuration":{},"approvedCapabilities":descriptor["capabilities"],"idempotencyKey":"http-install"})
}

// Removing a route's transport/strict DTO checks must make these actual router calls fail.
async fn release_transport_guards(app: &Router, cookie: &str, update: &Value, rollback: &Value) {
    for (method, path, body) in [
        ("GET", RELEASES, None),
        ("POST", UPDATE, Some(update)),
        ("POST", ROLLBACK, Some(rollback)),
    ] {
        assert_eq!(
            call(
                app,
                method,
                path,
                None,
                "localhost:3120",
                Some(ORIGIN),
                body
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED,
            "unauthenticated {method} {path}"
        );
        assert_eq!(
            call(
                app,
                method,
                path,
                Some(cookie),
                "localhost:3130",
                Some(ORIGIN),
                body
            )
            .await
            .0,
            StatusCode::NOT_FOUND,
            "wrong Host {method} {path}"
        );
    }
    for (path, body) in [(UPDATE, update), (ROLLBACK, rollback)] {
        for origin in [None, Some("https://attacker.invalid")] {
            assert_eq!(
                call(
                    app,
                    "POST",
                    path,
                    Some(cookie),
                    "localhost:3120",
                    origin,
                    Some(body)
                )
                .await
                .0,
                StatusCode::FORBIDDEN,
                "mutation Origin {path}"
            );
        }
        for field in [
            "_authority",
            "image",
            "manifestRaw",
            "keys",
            "recipe",
            "endpoint",
            "targetPolicy",
            "targetID",
            "configuration",
            "files",
            "runtime",
        ] {
            let mut injected = body.clone();
            injected[field] = json!({"value":"FIXTURE_PRIVATE_INJECTED"});
            assert_eq!(
                request(app, "POST", path, cookie, Some(&injected)).await.0,
                StatusCode::BAD_REQUEST,
                "private or server-owned field {field} in {path}"
            );
        }
        let mut malformed = body.clone();
        malformed["expectedRevision"] = json!(1);
        assert_eq!(
            request(app, "POST", path, cookie, Some(&malformed)).await.0,
            StatusCode::BAD_REQUEST,
            "decimal revision must remain a string"
        );
        assert_eq!(
            request(app, "POST", path, cookie, Some(&json!({}))).await.0,
            StatusCode::BAD_REQUEST,
            "required reviewed intent {path}"
        );
        assert_eq!(
            request(app, "POST", path, cookie, Some(body)).await.0,
            StatusCode::CONFLICT,
            "uninstalled release change {path}"
        );
    }
    assert_eq!(
        request(
            app,
            "GET",
            "/api/console/plugins/missing-plugin/releases",
            cookie,
            None
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
        "unknown plugin exposes no history"
    );
    for (suffix, body) in [("update", update), ("rollback", rollback)] {
        assert_eq!(
            request(
                app,
                "POST",
                &format!("/api/console/plugins/missing-plugin/{suffix}"),
                cookie,
                Some(body)
            )
            .await
            .0,
            StatusCode::CONFLICT,
            "unknown plugin cannot allocate a lifecycle operation"
        );
    }
}

#[tokio::test]
#[ignore = "owns a fresh disposable PostgreSQL container; no shared fixture or provider"]
async fn restricted_http_installation_guards_private_data_and_idempotent_retry() {
    let mut catalog = signed_catalog::catalog();
    catalog["targets"][0]["policy"]["privateKey"] = json!("FIXTURE_PRIVATE_KEY_NEVER_PUBLIC");
    catalog["releases"][0]["recipe"]["privateConfiguration"] = json!({"storageUrl":"postgres://fixture:FIXTURE_PRIVATE_PASSWORD@private.invalid/database"});
    let fixture = Fixture::new(catalog).await;
    let initial_projects = count(
        &fixture.owner,
        "SELECT count(*)::bigint n FROM webdock_admin.projects",
    )
    .await;
    assert_eq!(
        call(
            &fixture.local,
            "GET",
            CATALOG,
            None,
            "localhost:3120",
            None,
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let (status, rows) = request(
        &fixture.local,
        "GET",
        CATALOG,
        &fixture.operator_cookie,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        rows[0]["targets"],
        json!([]),
        "unenrolled target must not be offered"
    );
    let reviewed = serde_json::to_value(
        webdock_plugin_host::Catalog::from_bytes(&serde_json::to_vec(&fixture.catalog).unwrap())
            .unwrap()
            .descriptors(),
    )
    .unwrap();
    let cached_command = input(&reviewed[0]);
    assert_eq!(
        request(
            &fixture.local,
            "POST",
            INSTALL,
            &fixture.operator_cookie,
            Some(&cached_command)
        )
        .await
        .0,
        StatusCode::BAD_REQUEST,
        "cached review cannot admit an unenrolled target"
    );
    assert_eq!(
        count(
            &fixture.owner,
            "SELECT count(*)::bigint n FROM webdock_auth.plugin_installation"
        )
        .await,
        0
    );
    fixture.enroll().await;
    let (status, rows) = request(
        &fixture.local,
        "GET",
        CATALOG,
        &fixture.operator_cookie,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rows[0]["pluginID"], "postgres");
    assert!(rows[0].get("recipe").is_none());
    assert!(rows[0]["targets"][0].get("policy").is_none());
    let public = rows.to_string();
    assert!(!public.contains(fixture.catalog["keys"][0]["key"]["x"].as_str().unwrap()));
    assert!(!public.contains(fixture.catalog["releases"][0]["jws"].as_str().unwrap()));
    let command = input(&rows[0]);
    let update = json!({"version":rows[0]["version"],"reviewSha256":rows[0]["targets"][0]["reviewSha256"],"expectedRevision":"0","approvedCapabilities":rows[0]["capabilities"],"idempotencyKey":"http-update"});
    let rollback = json!({"releaseGeneration":"1","reviewSha256":rows[0]["targets"][0]["reviewSha256"],"expectedRevision":"0","approvedCapabilities":rows[0]["capabilities"],"idempotencyKey":"http-rollback"});

    for (invalidate, restore) in [
        (
            "UPDATE webdock_auth.hosting_agent SET revoked=true",
            "UPDATE webdock_auth.hosting_agent SET revoked=false",
        ),
        (
            "UPDATE webdock_auth.agent_transport_enrollment SET revoked=true",
            "UPDATE webdock_auth.agent_transport_enrollment SET revoked=false",
        ),
        (
            "UPDATE webdock_auth.agent_transport_enrollment SET generation=2",
            "UPDATE webdock_auth.agent_transport_enrollment SET generation=1",
        ),
        (
            "UPDATE webdock_auth.agent_transport_enrollment SET credential_hash=repeat('b',64)",
            "UPDATE webdock_auth.agent_transport_enrollment SET credential_hash=repeat('a',64)",
        ),
    ] {
        sql(&fixture.owner, invalidate, vec![]).await;
        assert_eq!(
            request(
                &fixture.local,
                "GET",
                CATALOG,
                &fixture.operator_cookie,
                None
            )
            .await
            .1[0]["targets"],
            json!([])
        );
        assert_eq!(
            request(
                &fixture.local,
                "POST",
                INSTALL,
                &fixture.operator_cookie,
                Some(&command)
            )
            .await
            .0,
            StatusCode::BAD_REQUEST,
            "invalid enrollment: {invalidate}"
        );
        sql(&fixture.owner, restore, vec![]).await;
    }
    for (host, origin, status) in [
        ("localhost:3130", Some(ORIGIN), StatusCode::NOT_FOUND),
        ("localhost:3120", None, StatusCode::FORBIDDEN),
        (
            "localhost:3120",
            Some("https://attacker.invalid"),
            StatusCode::FORBIDDEN,
        ),
    ] {
        assert_eq!(
            call(
                &fixture.local,
                "POST",
                INSTALL,
                Some(&fixture.operator_cookie),
                host,
                origin,
                Some(&command)
            )
            .await
            .0,
            status
        );
    }
    for (header, value) in [
        ("host", "localhost:3125"),
        ("x-forwarded-host", "localhost:3120"),
    ] {
        let response = fixture
            .local
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(INSTALL)
                    .header("host", "localhost:3120")
                    .header("origin", ORIGIN)
                    .header("cookie", &fixture.operator_cookie)
                    .header("content-type", "application/json")
                    .header(header, value)
                    .body(Body::from(command.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "ambiguous transport header {header}"
        );
    }
    for field in [
        "_authority",
        "image",
        "manifestRaw",
        "keys",
        "recipe",
        "endpoint",
        "targetPolicy",
        "files",
        "runtime",
    ] {
        let mut tampered = command.clone();
        tampered[field] = json!({"value":"FIXTURE_PRIVATE_INJECTED"});
        assert_eq!(
            request(
                &fixture.local,
                "POST",
                INSTALL,
                &fixture.operator_cookie,
                Some(&tampered)
            )
            .await
            .0,
            StatusCode::BAD_REQUEST,
            "injected {field}"
        );
    }
    for field in ["version", "targetID", "catalogID"] {
        let mut changed = command.clone();
        changed[field] = json!("unreviewed");
        assert_eq!(
            request(
                &fixture.local,
                "POST",
                INSTALL,
                &fixture.operator_cookie,
                Some(&changed)
            )
            .await
            .0,
            StatusCode::BAD_REQUEST,
            "changed {field}"
        );
    }
    let mut stale = command.clone();
    stale["reviewSha256"] = json!("0".repeat(64));
    assert_eq!(
        request(
            &fixture.local,
            "POST",
            INSTALL,
            &fixture.operator_cookie,
            Some(&stale)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let mut invalid = command.clone();
    invalid["configuration"] = json!({"arbitrary":true});
    assert_eq!(
        request(
            &fixture.local,
            "POST",
            INSTALL,
            &fixture.operator_cookie,
            Some(&invalid)
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    invalid = command.clone();
    invalid["approvedCapabilities"] = json!([]);
    assert_eq!(
        request(
            &fixture.local,
            "POST",
            INSTALL,
            &fixture.operator_cookie,
            Some(&invalid)
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        count(
            &fixture.owner,
            "SELECT count(*)::bigint n FROM webdock_auth.plugin_installation"
        )
        .await,
        0
    );
    let portal = fixture.first_party_cookie(&fixture.operator_cookie).await;
    release_transport_guards(&fixture.local, &fixture.operator_cookie, &update, &rollback).await;
    release_transport_guards(&fixture.first_party, &portal, &update, &rollback).await;
    for (method, path, body) in [
        ("GET", RELEASES, None),
        ("POST", UPDATE, Some(&update)),
        ("POST", ROLLBACK, Some(&rollback)),
    ] {
        assert_eq!(
            request(
                &fixture.first_party,
                method,
                path,
                &fixture.operator_cookie,
                body
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED,
            "first-party {path} cannot fall back to native cookie"
        );
    }
    assert_eq!(
        count(
            &fixture.owner,
            "SELECT count(*)::bigint n FROM webdock_auth.plugin_operation"
        )
        .await,
        0,
        "denied HTTP requests never admit work"
    );

    assert_eq!(
        request(&fixture.first_party, "GET", CATALOG, &portal, None)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &fixture.first_party,
            "GET",
            CATALOG,
            &fixture.operator_cookie,
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED,
        "a configured first-party surface never falls back to the account cookie"
    );
    let (status, accepted) = request(
        &fixture.local,
        "POST",
        INSTALL,
        &fixture.operator_cookie,
        Some(&command),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(accepted["kind"], "install");
    for (app, cookie) in [
        (&fixture.local, &fixture.operator_cookie),
        (&fixture.first_party, &portal),
    ] {
        let (status, history) = request(app, "GET", RELEASES, cookie, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(history["pluginID"], "postgres");
        assert_eq!(history["currentGeneration"], Value::Null);
        assert_eq!(history["candidateGeneration"], "1");
        assert_eq!(history["schemaHighWater"], 1);
        assert_eq!(history["targetID"], "local-eu");
        assert_eq!(history["releases"].as_array().unwrap().len(), 1);
        assert_eq!(history["releases"][0]["generation"], "1");
        assert_eq!(history["releases"][0]["version"], rows[0]["version"]);
        assert_eq!(history["releases"][0]["rollbackEligible"], false);
        assert_eq!(
            history["releases"][0]["rollbackBlocker"],
            "release_not_promoted"
        );
    }
    let id = accepted["id"].as_str().unwrap();
    let operation = format!("/api/console/plugins/operations/{id}");
    let retry = format!("{operation}/retry");
    assert_eq!(
        request(
            &fixture.first_party,
            "POST",
            INSTALL,
            &portal,
            Some(&command)
        )
        .await
        .1["id"],
        accepted["id"],
        "the same intent replays across native and first-party operator authority"
    );
    assert_eq!(
        request(
            &fixture.local,
            "GET",
            &operation,
            &fixture.operator_cookie,
            None
        )
        .await
        .1["kind"],
        "install"
    );
    let retry_body = json!({"idempotencyKey":"http-retry"});
    assert_eq!(
        call(
            &fixture.first_party,
            "POST",
            &retry,
            Some(&portal),
            "localhost:3120",
            None,
            Some(&retry_body)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &fixture.local,
            "POST",
            &retry,
            &fixture.operator_cookie,
            Some(&retry_body)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    for (method, path, body) in [
        ("GET", CATALOG, None),
        ("POST", INSTALL, Some(&command)),
        ("GET", operation.as_str(), None),
        ("POST", retry.as_str(), Some(&retry_body)),
        ("GET", RELEASES, None),
        ("POST", UPDATE, Some(&update)),
        ("POST", ROLLBACK, Some(&rollback)),
    ] {
        assert_eq!(
            request(&fixture.local, method, path, &fixture.customer_cookie, body)
                .await
                .0,
            StatusCode::FORBIDDEN,
            "customer {method} {path}"
        );
    }
    // Existing first-party sessions remain usable inside an exact-session scope; new OAuth
    // grants are deliberately denied while scoped, so mint this cookie before entering it.
    let scoped = fixture.first_party_cookie(&fixture.scope_cookie).await;
    for (mode, acting) in [("read_only", false), ("owner", true)] {
        fixture.scope(mode).await;
        let (_, session) = request(
            &fixture.first_party,
            "GET",
            "/api/console/session",
            &scoped,
            None,
        )
        .await;
        assert_eq!(session["data"]["user"]["preview"], true);
        assert_eq!(session["data"]["user"]["acting"], acting);
        for (method, path, body) in [
            ("GET", CATALOG, None),
            ("POST", INSTALL, Some(&command)),
            ("GET", operation.as_str(), None),
            ("POST", retry.as_str(), Some(&retry_body)),
            ("GET", RELEASES, None),
            ("POST", UPDATE, Some(&update)),
            ("POST", ROLLBACK, Some(&rollback)),
        ] {
            assert_eq!(
                request(&fixture.first_party, method, path, &scoped, body)
                    .await
                    .0,
                StatusCode::FORBIDDEN,
                "{mode} {method} {path}"
            );
        }
        for (method, path, body) in [
            ("GET", RELEASES, None),
            ("POST", UPDATE, Some(&update)),
            ("POST", ROLLBACK, Some(&rollback)),
        ] {
            assert_eq!(
                request(&fixture.local, method, path, &fixture.scope_cookie, body)
                    .await
                    .0,
                StatusCode::FORBIDDEN,
                "native {mode} {method} {path}"
            );
        }
    }
    let claim = fixture
        .host
        .claim_installation(None)
        .await
        .unwrap()
        .unwrap();
    fixture
        .host
        .fail_installation(&claim, "fixture_failure", true)
        .await
        .unwrap();
    sql(
        &fixture.owner,
        "UPDATE webdock_auth.agent_transport_enrollment SET revoked=true",
        vec![],
    )
    .await;
    assert_eq!(
        request(
            &fixture.first_party,
            "POST",
            &retry,
            &portal,
            Some(&retry_body)
        )
        .await
        .0,
        StatusCode::BAD_REQUEST,
        "explicit retry must recheck current enrollment"
    );
    sql(
        &fixture.owner,
        "UPDATE webdock_auth.agent_transport_enrollment SET revoked=false",
        vec![],
    )
    .await;
    let (status, retried) = request(
        &fixture.first_party,
        "POST",
        &retry,
        &portal,
        Some(&retry_body),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(retried["id"], accepted["id"]);
    assert_eq!(retried["kind"], "install");
    let (status, replay) = request(
        &fixture.first_party,
        "POST",
        &retry,
        &portal,
        Some(&retry_body),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["id"], retried["id"]);
    let different = format!(
        "/api/console/plugins/operations/{}/retry",
        retried["id"].as_str().unwrap()
    );
    assert_eq!(
        request(
            &fixture.first_party,
            "POST",
            &different,
            &portal,
            Some(&retry_body)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    sql(
        &fixture.owner,
        "UPDATE webdock_auth.\"user\" SET role='user' WHERE id='operator'",
        vec![],
    )
    .await;
    for (app, cookie, expected) in [
        (
            &fixture.local,
            &fixture.operator_cookie,
            StatusCode::FORBIDDEN,
        ),
        (&fixture.first_party, &portal, StatusCode::UNAUTHORIZED),
    ] {
        for (method, path, body) in [
            ("GET", RELEASES, None),
            ("POST", UPDATE, Some(&update)),
            ("POST", ROLLBACK, Some(&rollback)),
        ] {
            assert_eq!(
                request(app, method, path, cookie, body).await.0,
                expected,
                "current operator role is rechecked for {path}"
            );
        }
    }
    sql(
        &fixture.owner,
        "UPDATE webdock_auth.\"user\" SET role='operator' WHERE id='operator'",
        vec![],
    )
    .await;
    assert_eq!(
        count(
            &fixture.owner,
            "SELECT count(*)::bigint n FROM webdock_auth.plugin_runtime"
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &fixture.owner,
            "SELECT count(*)::bigint n FROM webdock_admin.projects"
        )
        .await,
        initial_projects,
        "installation must not create its own customer project"
    );
}
