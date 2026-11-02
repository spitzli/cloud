//! Pure HTTP privacy/compatibility checks. Real PKCE/domain qualification needs a fresh owned fixture.
#[path = "support/database_reads.rs"]
pub mod qualification;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use sea_orm::DatabaseConnection;
use std::collections::HashMap;
use tower::ServiceExt;

#[tokio::test]
async fn database_metadata_bearer_routes_never_bridge_a_browser_cookie() {
    let app = webdock_control_plane::http::build_app(
        webdock_core::config::PlatformConfig::from_map(&HashMap::new()).unwrap(),
        DatabaseConnection::default(),
    );
    for path in [
        "/api/hosting/databases/projects/101/databases",
        "/api/hosting/databases/101",
        "/api/hosting/databases/101/backups",
        "/api/hosting/databases/101/operations/202",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header("host", "localhost:3120")
                    .header("cookie", "better-auth.session_token=not-bearer-proof")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert!(
            response.headers()["www-authenticate"]
                .to_str()
                .unwrap()
                .contains("hosting:read")
        );
    }
}

#[tokio::test]
async fn database_bearer_reads_preserve_host_and_origin_boundaries() {
    let app = webdock_control_plane::http::build_app(
        webdock_core::config::PlatformConfig::from_map(&HashMap::new()).unwrap(),
        DatabaseConnection::default(),
    );
    for (host, origin, status) in [
        ("localhost:3130", None, StatusCode::NOT_FOUND),
        ("localhost:3125", None, StatusCode::NOT_FOUND),
        (
            "localhost:3120",
            Some("https://foreign.example.invalid"),
            StatusCode::FORBIDDEN,
        ),
    ] {
        let mut request = Request::builder()
            .uri("/api/hosting/databases/101")
            .header("host", host)
            .header("authorization", "Bearer untrusted");
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
}

#[test]
fn console_database_list_decodes_exact_measured_browser_envelope() {
    use serde_json::json;
    // Actual producer databases.rs217: Json(json!({"data": data}));6395 measured HTTP200.
    assert_eq!(
        qualification::console_database_rows(&json!({"data":[]})),
        Some(&vec![])
    );
    assert_eq!(
        qualification::console_database_rows(&json!({"data":[{"id":"101","operationID":"202"}]}))
            .unwrap()[0]["id"],
        "101"
    );
    for wrong in [
        json!([]),
        json!({"items":[],"nextCursor":null}),
        json!({"data":null}),
        json!({"data":{}}),
        json!({"error":{"code":"denied"}}),
        json!({"data":[],"error":{"code":"denied"}}),
    ] {
        assert!(qualification::console_database_rows(&wrong).is_none());
    }
}

#[test]
fn native_mcp_result_models_match_helper_jsonrpc_content_and_error_paths() {
    use rmcp::model::{CallToolResult, ContentBlock, JsonRpcResponse, JsonRpcVersion2_0};
    use serde_json::{Value, json};
    let metadata = json!({"items":[],"nextCursor":null});
    let response = JsonRpcResponse {
        jsonrpc: JsonRpcVersion2_0,
        id: rmcp::model::NumberOrString::Number(2),
        result: CallToolResult::success(vec![ContentBlock::text(metadata.to_string())]),
    };
    let wire = serde_json::to_value(response).unwrap();
    assert_eq!(wire["jsonrpc"], "2.0");
    assert_eq!(wire["id"], 2);
    assert_eq!(wire["result"]["isError"], false);
    assert_eq!(
        serde_json::from_str::<Value>(wire["result"]["content"][0]["text"].as_str().unwrap())
            .unwrap(),
        metadata
    );
    assert!(wire.get("data").is_none());
    let response = JsonRpcResponse {
        jsonrpc: JsonRpcVersion2_0,
        id: rmcp::model::NumberOrString::Number(3),
        result: CallToolResult::error(vec![ContentBlock::text(
            json!({"error":{"code":"database_error","message":"Database access is unavailable."}})
                .to_string(),
        )]),
    };
    let wire = serde_json::to_value(response).unwrap();
    assert_eq!(wire["result"]["isError"], true);
    let failure: Value =
        serde_json::from_str(wire["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(failure["error"]["code"], "database_error");
    assert!(failure.get("data").is_none());
}

#[tokio::test]
async fn resource_rest_json_carrier_keeps_page_at_root_without_console_envelope() {
    use axum::{Json, response::IntoResponse};
    use serde_json::{Value, json};
    // resource_hosting returns Json(data); service/read.rs page emits exactly these keys.
    let page = json!({"items":[{"id":"101"}],"nextCursor":"101"});
    let response = Json(page.clone()).into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let wire: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 65536)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(wire, page);
    assert!(wire.get("data").is_none());
    assert_eq!(wire.as_object().unwrap().len(), 2);
    assert!(qualification::console_database_rows(&wire).is_none());
}

#[test]
fn oauth_native_token_response_is_typed_and_uses_standard_root_fields() {
    // Pure serialization only: this constant is not an issued token or authority.
    let response = webdock_oauth::TokenResponse {
        access_token: "unissued-wire-test".into(),
        token_type: "Bearer".into(),
        expires_in: 300,
        scope: "openid hosting:read".into(),
        refresh_token: None,
        id_token: None,
    };
    let wire = serde_json::to_value(response).unwrap();
    assert!(wire.get("data").is_none());
    assert_eq!(wire["token_type"], "Bearer");
    assert_eq!(wire["expires_in"], 300);
    let decoded: webdock_oauth::TokenResponse = serde_json::from_value(wire).unwrap();
    assert_eq!(decoded.scope, "openid hosting:read");
    let outcome=webdock_oauth::AuthorizationOutcome::Redirect("https://client.example.test/callback?code=unissued-code&iss=https%3A%2F%2Fauth.example.test%2Fapi%2Fauth".into());
    let webdock_oauth::AuthorizationOutcome::Redirect(location) = outcome else {
        panic!("typed redirect")
    };
    let url = url::Url::parse(&location).unwrap();
    assert_eq!(url.query_pairs().filter(|(k, _)| k == "code").count(), 1);
    // The helper consumes actual typed TokenResponse and Result<()> revoke, never a data wrapper.
}
