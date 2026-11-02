//! Authenticated binary data adapters; provider URLs and headers never cross this boundary.
use super::*;
use axum::{
    body::Body,
    http::{HeaderValue, Method},
};
use tokio_stream::StreamExt;
use webdock_plugin_host::storage::{StorageAccess, StorageAuthority, StorageCaller};

#[derive(Clone, Copy)]
enum Lane {
    Browser,
    Workspace,
    Mcp,
}
pub(super) fn transport_hash(
    connection: &crate::plugin_config::RuntimeConnection,
) -> Result<String, PluginError> {
    Ok(hex::encode(Sha256::digest(
        serde_json::to_vec(connection).map_err(|_| unavailable_plugin())?,
    )))
}
pub(super) fn routes() -> Router<Arc<ServiceState>> {
    Router::new()
        .route(
            "/api/console/plugins/storage/{id}",
            get(browser).put(browser),
        )
        .route("/v1/api/storage/{id}", get(workspace).put(workspace))
        .route("/v1/plugins/storage/{id}", get(mcp).put(mcp))
        .layer(axum::middleware::map_response(
            |mut response: Response| async move {
                secure_headers(&mut response);
                response
            },
        ))
}
fn secure_headers(response: &mut Response) {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
        .headers_mut()
        .insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
}
struct LiveTransfer {
    state: Arc<ServiceState>,
    headers: HeaderMap,
    id: String,
    method: String,
    lane: Lane,
}
impl LiveTransfer {
    async fn caller(&self) -> Result<StorageCaller, ApiFailure> {
        match self.lane {
            Lane::Browser => {
                if self.headers.contains_key(header::AUTHORIZATION)
                    || self.method == "PUT"
                        && !self
                            .headers
                            .get(header::ORIGIN)
                            .and_then(|v| v.to_str().ok())
                            .is_some_and(|origin| {
                                [
                                    self.state.config.console_origin(),
                                    self.state.config.studio_origin.as_str(),
                                ]
                                .contains(&origin)
                            })
                {
                    return Err(failure(PluginError::denied()));
                }
                let actor = actor(&self.state, &self.headers).await?;
                let can_write = !actor.read_only();
                Ok(StorageCaller::browser(
                    &actor.principal,
                    actor.authority,
                    can_write,
                ))
            }
            Lane::Workspace => {
                let bearer = api::bearer(&self.state, &self.headers).await?;
                Ok(StorageCaller::workspace(
                    &bearer.actor.principal,
                    bearer.actor.authority,
                    bearer.can_write,
                    bearer.customer,
                ))
            }
            Lane::Mcp => {
                let actor = crate::resource_auth::authenticate(
                    &self.state,
                    &self.headers,
                    crate::resource_auth::Resource::Registry,
                    self.method == "PUT",
                )
                .await
                .map_err(|_| failure(PluginError::denied()))?;
                Ok(StorageCaller::mcp(&actor))
            }
        }
    }
    async fn access(&self) -> Result<StorageAccess, PluginError> {
        let caller = self.caller().await.map_err(|_| PluginError::denied())?;
        let endpoint = endpoints(&self.state).map_err(|_| unavailable_plugin())?;
        let access = endpoint
            .host
            .storage_access(&self.id, &self.method, &caller)
            .await?;
        let connection = endpoint
            .connections
            .load_generation(&access.plugin_id, access.generation)
            .await
            .map_err(|_| unavailable_plugin())?;
        if transport_hash(&connection)? != access.transport_sha256 {
            return Err(PluginError::denied());
        }
        let (mut client, _) = connection
            .connect_authenticated()
            .await
            .map_err(|_| PluginError::denied())?;
        let authorization_json =
            serde_json::to_vec(&access.authorization).map_err(|_| PluginError::denied())?;
        let active = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            client.authorize_storage(webdock_plugin_runtime::wire::StorageAuthorizationRequest {
                authorization_json,
            }),
        )
        .await
        .map_err(|_| PluginError::denied())?
        .map_err(|_| PluginError::denied())?
        .into_inner()
        .active;
        if !active {
            return Err(PluginError::denied());
        }
        Ok(access)
    }
}
#[async_trait::async_trait]
impl StorageAuthority for LiveTransfer {
    async fn check(&self) -> Result<(), PluginError> {
        self.access().await.map(|_| ())
    }
}
async fn browser(
    State(state): State<Arc<ServiceState>>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    data(state, id, request, Lane::Browser).await
}
async fn workspace(
    State(state): State<Arc<ServiceState>>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    data(state, id, request, Lane::Workspace).await
}
async fn mcp(
    State(state): State<Arc<ServiceState>>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    data(state, id, request, Lane::Mcp).await
}
async fn data(state: Arc<ServiceState>, id: String, request: Request, lane: Lane) -> Response {
    let result: Result<Response, ApiFailure> = async {
        if request.uri().query().is_some()
            || [
                header::RANGE,
                header::IF_RANGE,
                header::IF_MATCH,
                header::IF_NONE_MATCH,
                header::CONTENT_ENCODING,
            ]
            .iter()
            .any(|h| request.headers().contains_key(h))
        {
            return Err(failure(PluginError::denied()));
        }
        let method = request.method().clone();
        let authority = Arc::new(LiveTransfer {
            state: state.clone(),
            headers: request.headers().clone(),
            id: id.clone(),
            method: method.to_string(),
            lane,
        });
        let access = authority.access().await.map_err(failure)?;
        let endpoint = endpoints(&state)?;
        let gateway = endpoint.host.storage_gateway().map_err(failure)?;
        let target = gateway
            .target(&access.plan, access.binding)
            .map_err(failure)?;
        if method == Method::PUT {
            if request
                .headers()
                .get(header::CONTENT_LENGTH)
                .is_some_and(|h| {
                    h.to_str().ok().and_then(|h| h.parse::<u64>().ok())
                        != Some(access.plan.exact_bytes)
                })
            {
                return Err(failure(PluginError::denied()));
            }
            endpoint
                .host
                .claim_storage_upload(&id, &authority.caller().await?)
                .await
                .map_err(failure)?;
            let chunks = request.into_body().into_data_stream().map(|c| {
                c.map(|c| c.to_vec()).map_err(|_| {
                    PluginError::new(ErrorCode::InvalidInput, "Storage upload interrupted")
                })
            });
            let staged = gateway
                .stage(&access.plan, chunks, authority.clone())
                .await
                .map_err(failure)?;
            let receipt = gateway
                .upload(&access.plan, target, staged)
                .await
                .map_err(|_| failure(webdock_plugin_host::storage::upload_uncertain()))?;
            authority
                .check()
                .await
                .map_err(|_| failure(webdock_plugin_host::storage::upload_uncertain()))?;
            endpoint
                .host
                .finish_storage_upload(
                    &id,
                    &authority
                        .caller()
                        .await
                        .map_err(|_| failure(webdock_plugin_host::storage::upload_uncertain()))?,
                )
                .await
                .map_err(|_| failure(webdock_plugin_host::storage::upload_uncertain()))?;
            Ok(Json(receipt).into_response())
        } else {
            let staged = gateway
                .download(&access.plan, target, authority.clone())
                .await
                .map_err(failure)?;
            let mut response = stream_response(staged, authority);
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_str(&access.plan.attachment.content_type)
                    .map_err(|_| failure(PluginError::denied()))?,
            );
            response.headers_mut().insert(
                header::CONTENT_LENGTH,
                HeaderValue::from(access.plan.exact_bytes),
            );
            response.headers_mut().insert(
                header::CONTENT_DISPOSITION,
                HeaderValue::from_str(&format!(
                    "attachment; filename=\"{}\"",
                    access.plan.attachment.filename
                ))
                .map_err(|_| failure(PluginError::denied()))?,
            );
            Ok(response)
        }
    }
    .await;
    let mut response = result.unwrap_or_else(IntoResponse::into_response);
    secure_headers(&mut response);
    response
}

fn stream_response(
    staged: webdock_plugin_host::storage::StagedTransfer,
    authority: Arc<dyn StorageAuthority>,
) -> Response {
    let (sender, receiver) = tokio::sync::mpsc::channel::<Result<Vec<u8>, PluginError>>(1);
    tokio::spawn(staged.into_stream().deliver(sender));
    let guarded = tokio_stream::wrappers::ReceiverStream::new(receiver).then(move |chunk| {
        let authority = authority.clone();
        async move {
            tokio::time::timeout(std::time::Duration::from_secs(1), authority.check())
                .await
                .map_err(|_| PluginError::denied())??;
            chunk
        }
    });
    Response::new(Body::from_stream(guarded))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn response_headers_are_private_and_do_not_copy_provider_metadata() {
        let mut response = Response::new(axum::body::Body::empty());
        secure_headers(&mut response);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            response.headers()[header::X_CONTENT_TYPE_OPTIONS],
            "nosniff"
        );
        assert!(!response.headers().contains_key(header::LOCATION));
        assert!(!response.headers().contains_key(header::SET_COOKIE));
    }
}

#[cfg(test)]
mod delivery_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use webdock_plugin_host::storage::StorageGateway;
    use webdock_plugin_protocol::storage::StoragePlan;
    struct Live(AtomicBool);
    #[async_trait::async_trait]
    impl StorageAuthority for Live {
        async fn check(&self) -> Result<(), PluginError> {
            if self.0.load(Ordering::SeqCst) {
                Ok(())
            } else {
                Err(PluginError::denied())
            }
        }
    }
    async fn stalled_delivery(cause: &str) {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let gateway = StorageGateway::new(dir.path().into()).unwrap();
        let live = Arc::new(Live(AtomicBool::new(true)));
        let p: StoragePlan = serde_json::from_value(json!({"version":1,"binding":"objects","direction":"download","resourceId":"owned","resourceEpoch":"active-1","url":"https://storage.invalid/private/key","headers":{},"expiresAt":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64+if cause=="deadline" {2} else {300},"exactBytes":262144,"expectedSha256":null,"attachment":{"filename":"file.bin","contentType":"application/octet-stream"}})).unwrap();
        let gateway = &gateway;
        let stage = |authority: Arc<Live>, plan: StoragePlan| async move {
            gateway
                .stage(&plan, tokio_stream::iter([Ok(vec![7; 262144])]), authority)
                .await
        };
        let files = || {
            std::fs::read_dir("/proc/self/fd")
                .unwrap()
                .filter_map(|e| std::fs::read_link(e.ok()?.path()).ok())
                .filter(|p| p.starts_with(dir.path()))
                .count()
        };
        let mut responses = Vec::new();
        for _ in 0..4 {
            responses.push(stream_response(
                stage(live.clone(), p.clone()).await.unwrap(),
                live.clone(),
            ));
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert_eq!(files(), 4, "four unlinked files must actually be held");
        assert!(
            stage(live.clone(), p.clone()).await.is_err(),
            "all four permits held"
        );
        if cause == "revocation" {
            live.0.store(false, Ordering::SeqCst);
        }
        if cause == "disconnect" {
            responses.clear();
        }
        tokio::time::timeout(std::time::Duration::from_secs(4), async {
            while files() != 0 {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("stalled delivery must release anonymous files before body is polled");
        let mut renewed = p.clone();
        renewed.expires_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            + 300;
        let mut held = Vec::new();
        for _ in 0..4 {
            held.push(
                stage(Arc::new(Live(AtomicBool::new(true))), renewed.clone())
                    .await
                    .expect("every permit must be reclaimed"),
            );
        }
        assert_eq!(files(), 4);
        drop(held);
        assert_eq!(files(), 0);
        drop(responses);
    }
    #[tokio::test]
    async fn stalled_consumer_revocation_releases_files_and_all_permits() {
        stalled_delivery("revocation").await;
    }
    #[tokio::test]
    async fn stalled_consumer_deadline_releases_files_and_all_permits() {
        stalled_delivery("deadline").await;
    }
    #[tokio::test]
    async fn disconnected_consumer_releases_files_and_all_permits() {
        stalled_delivery("disconnect").await;
    }
}
