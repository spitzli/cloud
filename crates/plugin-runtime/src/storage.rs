//! Private producer revalidation. Runs on the existing authenticated Runtime listener.
use super::*;
use webdock_plugin_protocol::storage::{StorageAuthorization, StorageDirection};
pub(crate) async fn authorize(
    runtime: &Runtime,
    request: Request<wire::StorageAuthorizationRequest>,
) -> Result<Response<wire::StorageAuthorizationResponse>, Status> {
    let _permit = runtime
        .permits
        .try_acquire()
        .map_err(|_| Status::unavailable("Storage authorization capacity"))?;
    let peer = runtime.peer(&request).await?;
    let body = request.into_inner().authorization_json;
    if body.len() > 8192 {
        return Err(status(PluginError::denied()));
    }
    let authorization: StorageAuthorization =
        serde_json::from_slice(&body).map_err(|_| status(PluginError::denied()))?;
    authorization
        .validate(runtime.manifest.id(), runtime.generation, now()?)
        .map_err(status)?;
    let action = runtime
        .manifest
        .action(&authorization.action)
        .ok_or_else(|| status(PluginError::denied()))?;
    if action["storage"] != authorization.direction.as_str()
        || action["resource"] != "use"
        || authorization.direction == StorageDirection::Upload && action["intent"] != "write"
    {
        return Err(status(PluginError::denied()));
    }
    tokio::time::timeout(
        Duration::from_secs(1),
        runtime.handler.authorize_storage(&authorization),
    )
    .await
    .map_err(|_| Status::deadline_exceeded("Storage authorization deadline"))?
    .map_err(status)?;
    runtime.check_peer(&peer).await?;
    authorization
        .validate(runtime.manifest.id(), runtime.generation, now()?)
        .map_err(status)?;
    Ok(Response::new(wire::StorageAuthorizationResponse {
        active: true,
    }))
}
