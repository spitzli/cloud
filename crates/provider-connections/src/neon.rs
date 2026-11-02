//! Fixed-origin, read-only management API. No ambient credentials or provider error bodies.
use crate::{Error, Result};
use serde::Serialize;
use serde_json::Value;
use std::{collections::HashSet, time::Duration};

const ORIGIN: &str = "https://console.neon.tech/api/v2";
const MAX_BODY: usize = 2 * 1024 * 1024;
pub(crate) fn unavailable() -> Error {
    Error::Unavailable
}
pub(crate) fn invalid() -> Error {
    Error::InvalidInput
}
pub(crate) fn identifier(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 63
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !id.starts_with('-')
        && !id.ends_with('-')
}
fn valid_cursor(cursor: &str) -> bool {
    !cursor.is_empty() && cursor.len() <= 1024 && !cursor.chars().any(char::is_control)
}
// Deliberately no Debug, Serialize, Clone or public secret accessor.
pub(crate) struct ApiKey(String);
impl ApiKey {
    pub(crate) fn new(value: String) -> Result<Self> {
        if value.is_empty() || value.len() > 2048 || !value.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(invalid());
        }
        Ok(Self(value))
    }
    pub(crate) fn value(&self) -> &str {
        &self.0
    }
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Project {
    id: String,
    name: String,
    region_id: String,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Page {
    pub(crate) projects: Vec<Project>,
    pub(crate) next_cursor: Option<String>,
}
fn project_page(value: Value) -> Result<Page> {
    if value
        .get("unavailable_project_ids")
        .is_some_and(|v| v.as_array().is_none_or(|a| !a.is_empty()))
    {
        return Err(unavailable());
    }
    let rows = value["projects"]
        .as_array()
        .filter(|r| r.len() <= 50)
        .ok_or_else(unavailable)?;
    let mut seen = HashSet::new();
    let mut projects = Vec::with_capacity(rows.len());
    for row in rows {
        let id = row["id"]
            .as_str()
            .filter(|id| identifier(id))
            .ok_or_else(unavailable)?;
        let name = row["name"]
            .as_str()
            .filter(|n| !n.is_empty() && n.len() <= 256 && !n.chars().any(char::is_control))
            .ok_or_else(unavailable)?;
        let region = row["region_id"]
            .as_str()
            .filter(|id| identifier(id))
            .ok_or_else(unavailable)?;
        if !seen.insert(id) {
            return Err(unavailable());
        }
        projects.push(Project {
            id: id.into(),
            name: name.into(),
            region_id: region.into(),
        });
    }
    let cursor = match value.get("pagination") {
        None | Some(Value::Null) => None,
        Some(pagination) => match pagination.get("cursor") {
            None | Some(Value::Null) => None,
            Some(Value::String(cursor)) if valid_cursor(cursor) => Some(cursor.clone()),
            _ => return Err(unavailable()),
        },
    };
    Ok(Page {
        projects,
        next_cursor: cursor,
    })
}
pub(crate) struct Client {
    http: reqwest::Client,
    origin: String,
}
impl Client {
    pub(crate) fn new() -> Result<Self> {
        Ok(Self {
            http: Self::http(true)?,
            origin: ORIGIN.into(),
        })
    }
    fn http(https: bool) -> Result<reqwest::Client> {
        reqwest::Client::builder()
            .https_only(https)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| unavailable())
    }
    #[cfg(test)]
    pub(crate) fn test_client(origin: String) -> Self {
        let url = reqwest::Url::parse(&origin).unwrap();
        assert_eq!(url.scheme(), "http");
        assert_eq!(url.host_str(), Some("127.0.0.1"));
        assert!(url.port().is_some());
        Self {
            http: Self::http(false).unwrap(),
            origin,
        }
    }
    pub(crate) async fn projects(
        &self,
        key: &ApiKey,
        org: Option<&str>,
        cursor: Option<&str>,
    ) -> Result<Page> {
        if org.is_some_and(|id| !identifier(id) || id.len() > 60)
            || cursor.is_some_and(|cursor| !valid_cursor(cursor))
        {
            return Err(invalid());
        }
        let mut query = vec![("limit", "50")];
        if let Some(org) = org {
            query.push(("org_id", org));
        }
        if let Some(cursor) = cursor {
            query.push(("cursor", cursor));
        }
        project_page(self.get_json(key, "/projects", &query).await?)
    }
    pub(crate) async fn inventory(
        &self,
        key: &ApiKey,
        org: Option<&str>,
        project: Option<&str>,
        cursor: Option<&str>,
    ) -> Result<Page> {
        let Some(project) = project else {
            return self.projects(key, org, cursor).await;
        };
        if !identifier(project)
            || org.is_some_and(|id| !identifier(id) || id.len() > 60)
            || cursor.is_some()
        {
            return Err(invalid());
        }
        let value = self
            .get_json(key, &format!("/projects/{project}"), &[])
            .await?;
        let row = value
            .get("project")
            .filter(|v| v.is_object())
            .ok_or_else(unavailable)?;
        if row["id"] != project || org.is_some_and(|org| row["org_id"].as_str() != Some(org)) {
            return Err(Error::InvalidScope);
        }
        project_page(serde_json::json!({"projects":[row]}))
    }
    async fn get_json(&self, key: &ApiKey, path: &str, query: &[(&str, &str)]) -> Result<Value> {
        let mut authorization =
            reqwest::header::HeaderValue::from_str(&format!("Bearer {}", key.value()))
                .map_err(|_| invalid())?;
        authorization.set_sensitive(true);
        let mut response = self
            .http
            .get(format!("{}{path}", self.origin))
            .header(reqwest::header::AUTHORIZATION, authorization)
            .header(reqwest::header::ACCEPT, "application/json")
            .query(query)
            .send()
            .await
            .map_err(|_| unavailable())?;
        if [
            reqwest::StatusCode::UNAUTHORIZED,
            reqwest::StatusCode::FORBIDDEN,
        ]
        .contains(&response.status())
        {
            return Err(Error::InvalidCredentials);
        }
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(Error::InvalidScope);
        }
        if response.status() != reqwest::StatusCode::OK
            || response
                .content_length()
                .is_some_and(|len| len > MAX_BODY as u64)
        {
            return Err(unavailable());
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
            if chunk.len() > MAX_BODY - body.len() {
                return Err(unavailable());
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body).map_err(|_| unavailable())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keys_ids_and_provider_json_are_validated() {
        for key in [
            "",
            "contains space",
            "injected\r\nHeader:x",
            &"x".repeat(2049),
        ] {
            assert!(ApiKey::new(key.into()).is_err());
        }
        for id in [
            "../other",
            "x?secret=yes",
            "x/y",
            "",
            "CAPITAL",
            &"a".repeat(64),
        ] {
            assert!(!identifier(id));
        }
        let raw = json!({"projects":[{"id":"snowy-tree-123","name":"Demo","region_id":"aws-eu-central-1","pg_version":17,"password":"hidden","connection_uri":"postgres://secret"}],"pagination":{"cursor":"snowy-tree-123"},"applications":{"secret":"hidden"}});
        let page = project_page(raw).unwrap();
        let output = serde_json::to_string(&page).unwrap();
        assert!(!output.contains("hidden"));
        assert!(!output.contains("postgres://"));
        assert!(project_page(json!({"projects":[{"id":"../bad"}]})).is_err());
        assert!(
            project_page(json!({"projects":[],"unavailable_project_ids":["missing"]})).is_err()
        );
        assert!(
            project_page(json!({"projects":[],"pagination":{"cursor":"invalid\ncursor"}})).is_err()
        );
    }

    #[tokio::test]
    async fn request_uses_only_explicit_key_and_never_follows_redirects() {
        use axum::{Router, http::StatusCode, routing::get};
        let app = Router::new().route(
            "/projects",
            get(|headers: axum::http::HeaderMap| async move {
                assert_eq!(headers["authorization"], "Bearer fixture-key");
                (
                    StatusCode::FOUND,
                    [("location", "http://127.0.0.1:9/stolen")],
                    "fixture-key",
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = Client::test_client(format!("http://{}", listener.local_addr().unwrap()));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let error = client
            .projects(&ApiKey::new("fixture-key".into()).unwrap(), None, None)
            .await
            .unwrap_err();
        assert!(matches!(error, Error::Unavailable));
        assert!(!format!("{error:?}").contains("fixture-key"));
        server.abort();
    }
}

#[cfg(test)]
mod response_limits {
    use super::*;
    use axum::{
        Router,
        http::{StatusCode, header},
        routing::get,
    };
    #[tokio::test]
    async fn provider_errors_malformed_and_oversized_bodies_are_redacted() {
        for (status, body) in [
            (
                StatusCode::UNAUTHORIZED,
                "fixture-key provider diagnostic".into(),
            ),
            (StatusCode::NOT_FOUND, "fixture-key scope diagnostic".into()),
            (
                StatusCode::FORBIDDEN,
                "fixture-key provider diagnostic".into(),
            ),
            (
                StatusCode::TOO_MANY_REQUESTS,
                "fixture-key throttled".into(),
            ),
            (StatusCode::OK, "not json fixture-key".into()),
            (StatusCode::OK, "x".repeat(MAX_BODY + 1)),
        ] {
            let app = Router::new().route(
                "/projects",
                get(move || {
                    let body = body.clone();
                    async move { (status, [(header::CONTENT_TYPE, "application/json")], body) }
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let client = Client::test_client(format!("http://{}", listener.local_addr().unwrap()));
            let server = tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            let error = client
                .projects(&ApiKey::new("fixture-key".into()).unwrap(), None, None)
                .await
                .unwrap_err();
            assert!(!format!("{error:?}").contains("fixture-key"));
            if [StatusCode::UNAUTHORIZED, StatusCode::FORBIDDEN].contains(&status) {
                assert!(matches!(error, Error::InvalidCredentials));
                assert_eq!(error.status(), 400);
            } else if status == StatusCode::NOT_FOUND {
                assert!(matches!(error, Error::InvalidScope));
                assert_eq!(error.status(), 400);
            } else {
                assert!(matches!(error, Error::Unavailable));
            }
            server.abort();
        }
    }
    #[test]
    fn duplicate_projects_and_excess_page_rows_are_rejected() {
        let row =
            serde_json::json!({"id":"snowy-tree-123","name":"Demo","region_id":"aws-eu-central-1"});
        assert!(project_page(serde_json::json!({"projects":[row.clone(),row.clone()]})).is_err());
        assert!(project_page(serde_json::json!({"projects":vec![row;51]})).is_err());
    }
}

#[cfg(test)]
mod opaque_cursors {
    use super::*;
    use axum::{Router, extract::Query, routing::get};
    use std::collections::HashMap;

    #[tokio::test]
    async fn mixed_case_punctuation_cursor_round_trips_without_changing_query_scope() {
        const CURSOR: &str = "AbC+/=?:&org_id=other#fragment";
        let app = Router::new().route(
            "/projects",
            get(|Query(query): Query<HashMap<String, String>>| async move {
                assert_eq!(query.len(), 3);
                assert_eq!(query["limit"], "50");
                assert_eq!(query["org_id"], "org-owned");
                assert_eq!(query["cursor"], CURSOR);
                axum::Json(serde_json::json!({"projects":[],"pagination":{"cursor":CURSOR}}))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = Client::test_client(format!("http://{}", listener.local_addr().unwrap()));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let page = client
            .projects(
                &ApiKey::new("fixture-key".into()).unwrap(),
                Some("org-owned"),
                Some(CURSOR),
            )
            .await
            .unwrap();
        assert_eq!(page.next_cursor.as_deref(), Some(CURSOR));
        for cursor in ["", "bad\ncursor", &"x".repeat(1025)] {
            assert!(
                client
                    .projects(
                        &ApiKey::new("fixture-key".into()).unwrap(),
                        Some("org-owned"),
                        Some(cursor)
                    )
                    .await
                    .is_err()
            );
            assert!(
                project_page(serde_json::json!({"projects":[],"pagination":{"cursor":cursor}}))
                    .is_err()
            );
        }
        server.abort();
    }
}

#[cfg(test)]
mod scoped_projects {
    use super::*;
    use axum::{Router, extract::State, http::StatusCode, routing::get};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    #[tokio::test]
    async fn project_scope_uses_only_exact_get_without_list_or_pagination_fallback() {
        let calls = Arc::new(AtomicUsize::new(0));
        let app=Router::new().route("/projects/fixture-tree-123",get(|State(calls):State<Arc<AtomicUsize>>,headers:axum::http::HeaderMap,uri:axum::http::Uri|async move{
            assert!(uri.query().is_none());
            assert_eq!(headers["authorization"],"Bearer fixture-project-key");
            calls.fetch_add(1,Ordering::SeqCst);
            axum::Json(serde_json::json!({"project":{"id":"fixture-tree-123","name":"Fixture","region_id":"aws-eu-central-1","org_id":"org-fixture","connection_uri":"postgres://private"},"pagination":{"cursor":"never-use"}}))
        })).route("/projects",get(||async{StatusCode::INTERNAL_SERVER_ERROR})).with_state(calls.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = Client::test_client(format!("http://{}", listener.local_addr().unwrap()));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let key = ApiKey::new("fixture-project-key".into()).unwrap();
        let page = client
            .inventory(&key, Some("org-fixture"), Some("fixture-tree-123"), None)
            .await
            .unwrap();
        assert_eq!(page.projects.len(), 1);
        assert!(page.next_cursor.is_none());
        assert!(
            !serde_json::to_string(&page)
                .unwrap()
                .contains("postgres://")
        );
        assert!(matches!(
            client
                .inventory(&key, Some("org-other"), Some("fixture-tree-123"), None)
                .await,
            Err(Error::InvalidScope)
        ));
        for (project, cursor) in [
            ("fixture-tree-123", Some("next-page")),
            ("../other", None),
            ("bad?query=yes", None),
        ] {
            assert!(matches!(
                client.inventory(&key, None, Some(project), cursor).await,
                Err(Error::InvalidInput)
            ));
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "invalid cursor and IDs must not make provider requests"
        );
        server.abort();
    }
    #[tokio::test]
    async fn selected_project_mismatch_and_missing_scope_are_definitive_safe_errors() {
        for (status, response) in [
            (
                StatusCode::OK,
                serde_json::json!({"project":{"id":"another-tree-123","name":"Other","region_id":"aws-eu-central-1"}}),
            ),
            (
                StatusCode::NOT_FOUND,
                serde_json::json!({"message":"fixture-project-key diagnostic"}),
            ),
        ] {
            let app = Router::new().route(
                "/projects/fixture-tree-123",
                get(move || {
                    let response = response.clone();
                    async move { (status, axum::Json(response)) }
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let client = Client::test_client(format!("http://{}", listener.local_addr().unwrap()));
            let server = tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            let error = client
                .inventory(
                    &ApiKey::new("fixture-project-key".into()).unwrap(),
                    None,
                    Some("fixture-tree-123"),
                    None,
                )
                .await
                .unwrap_err();
            assert!(matches!(error, Error::InvalidScope));
            assert_eq!(error.status(), 400);
            assert!(!error.to_string().contains("fixture-project-key"));
            server.abort();
        }
    }
}
