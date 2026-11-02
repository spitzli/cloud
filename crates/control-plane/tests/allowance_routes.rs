//! The native allowance path must be registered and fail closed without its service.
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use std::collections::HashMap;
use tower::ServiceExt;
#[tokio::test]
async fn native_operator_allowance_path_exists_and_fails_closed() {
    let app = webdock_control_plane::http::build_app(
        webdock_core::config::PlatformConfig::from_map(&HashMap::new()).unwrap(),
        sea_orm::DatabaseConnection::default(),
    );
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/console/access/customers/123/allowances")
                .header("host", "localhost:3120")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}
