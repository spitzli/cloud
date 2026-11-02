//! Live notifications contain revisions only; row reads retain native authorization and RLS.
use crate::{
    auth,
    http::{ApiFailure, ServiceState, reject},
};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

#[derive(Clone)]
pub struct Realtime {
    pub url: String,
    socket: PathBuf,
    client: reqwest::Client,
}
impl Realtime {
    pub fn from_map(env: &HashMap<String, String>) -> Result<Option<Self>, &'static str> {
        let (Some(url), Some(socket)) = (
            env.get("NEON_REALTIME_URL"),
            env.get("WEBDOCK_REALTIME_SOCKET"),
        ) else {
            if env.contains_key("NEON_REALTIME_URL") || env.contains_key("WEBDOCK_REALTIME_SOCKET")
            {
                return Err("Complete private realtime configuration required");
            }
            return Ok(None);
        };
        let parsed = url::Url::parse(url).map_err(|_| "Invalid realtime URL")?;
        if parsed.scheme() != "wss"
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.path() != "/"
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || parsed.port().is_some()
            || !parsed.host_str().is_some_and(|h| h.ends_with(".neon.tech"))
        {
            return Err("Verified Neon realtime origin required");
        }
        let socket = PathBuf::from(socket);
        if !socket.is_absolute()
            || socket.components().any(|c| {
                matches!(
                    c,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
        {
            return Err("Absolute private realtime socket required");
        }
        let client = reqwest::Client::builder()
            .unix_socket(socket.clone())
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(2))
            .build()
            .map_err(|_| "Private realtime client unavailable")?;
        Ok(Some(Self {
            url: parsed.to_string(),
            socket,
            client,
        }))
    }
    async fn seal(&self, topic: Topic) -> Result<Capability, ApiFailure> {
        let unavailable = || {
            reject(
                StatusCode::SERVICE_UNAVAILABLE,
                "realtime_unavailable",
                "Live updates are temporarily unavailable.",
            )
        };
        let socket = std::fs::symlink_metadata(&self.socket).map_err(|_| unavailable())?;
        let parent = self.socket.parent().ok_or_else(unavailable)?;
        let dir = std::fs::symlink_metadata(parent).map_err(|_| unavailable())?;
        let uid = rustix::process::geteuid().as_raw();
        if !socket.file_type().is_socket()
            || socket.uid() != uid
            || socket.mode() & 0o077 != 0
            || !dir.is_dir()
            || dir.uid() != uid
            || dir.mode() & 0o077 != 0
            || std::fs::canonicalize(parent).ok().as_deref() != Some(parent)
        {
            return Err(unavailable());
        }
        let mut response = self
            .client
            .post("http://private/seal")
            .json(&Input { topic })
            .send()
            .await
            .map_err(|_| unavailable())?;
        if response.status() != StatusCode::OK {
            return Err(unavailable());
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
            if body.len() + chunk.len() > 16384 {
                return Err(unavailable());
            }
            body.extend_from_slice(&chunk)
        }
        let result: Capability = serde_json::from_slice(&body).map_err(|_| unavailable())?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| unavailable())?
            .as_millis() as u64;
        if result.capability.is_empty()
            || result.capability.len() > 12000
            || result.query_fingerprint.is_empty()
            || result.query_fingerprint.len() > 256
            || result.expires_at <= now
            || result.expires_at > now + 61000
        {
            return Err(unavailable());
        }
        Ok(result)
    }
}
#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Topic {
    Workspace,
    OperatorCustomers,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Input {
    topic: Topic,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Capability {
    capability: String,
    query_fingerprint: String,
    expires_at: u64,
}
fn permit(actor: &auth::AuthenticatedActor, topic: Topic) -> bool {
    actor.principal.can_access()
        && !actor.read_only()
        && !actor.principal.in_tenant_preview()
        && !actor.principal.is_acting_customer()
        && match topic {
            Topic::Workspace => true,
            Topic::OperatorCustomers => actor.principal.is_operator(),
        }
}
pub fn routes() -> Router<Arc<ServiceState>> {
    Router::new().route("/api/console/realtime/capability", post(capability))
}
async fn capability(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Json(input): Json<Input>,
) -> Response {
    let result = async {
        let realtime = state.realtime.as_ref().ok_or_else(|| {
            reject(
                StatusCode::SERVICE_UNAVAILABLE,
                "realtime_unavailable",
                "Live updates are not configured.",
            )
        })?;
        let denied = || {
            reject(
                StatusCode::FORBIDDEN,
                "permission_denied",
                "A current full account session is required.",
            )
        };
        let actor = auth::resolve_actor(&state, &headers)
            .await?
            .ok_or_else(denied)?;
        if !permit(&actor, input.topic) {
            return Err(denied());
        }
        let sealed = realtime.seal(input.topic).await?;
        let current = auth::resolve_actor(&state, &headers)
            .await?
            .ok_or_else(denied)?;
        if current.principal.user_id != actor.principal.user_id
            || current.principal.session_id != actor.principal.session_id
            || !permit(&current, input.topic)
        {
            return Err(denied());
        }
        Ok::<_, ApiFailure>(Json(sealed))
    }
    .await;
    let mut response = match result {
        Ok(value) => value.into_response(),
        Err(error) => error.into_response(),
    };
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn realtime_configuration_requires_a_complete_private_pair() {
        assert!(Realtime::from_map(&HashMap::new()).unwrap().is_none());
        let mut env = HashMap::from([
            (
                "NEON_REALTIME_URL".into(),
                "wss://ep-test.rt.eu.neon.tech/".into(),
            ),
            (
                "WEBDOCK_REALTIME_SOCKET".into(),
                "/run/webdock-live/sdk.sock".into(),
            ),
        ]);
        assert!(Realtime::from_map(&env).unwrap().is_some());
        for bad in [
            "wss://other.invalid/",
            "https://ep-test.neon.tech/",
            "wss://a.neon.tech/?secret=x",
            "wss://user@a.neon.tech/",
        ] {
            env.insert("NEON_REALTIME_URL".into(), bad.into());
            assert!(Realtime::from_map(&env).is_err())
        }
        env.remove("NEON_REALTIME_URL");
        assert!(Realtime::from_map(&env).is_err());
    }
}
