//! Tenant-authorized log history and resumable SSE; reauthenticate every poll.
use crate::{
    auth,
    http::{ApiFailure, ServiceState, reject},
};
use axum::{
    Json, Router,
    extract::{Path, Query, RawQuery, State},
    http::{HeaderMap, StatusCode},
    response::{
        IntoResponse, Response,
        sse::{Event, Sse},
    },
    routing::get,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{convert::Infallible, sync::Arc, time::Duration};
use webdock_core::features::OperationKind;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    #[serde(default = "zero")]
    after: String,
}
fn zero() -> String {
    "0".into()
}
fn unavailable() -> ApiFailure {
    reject(
        StatusCode::SERVICE_UNAVAILABLE,
        "unavailable",
        "Build logs are unavailable.",
    )
}
fn cursor(value: Option<&str>) -> Result<String, ApiFailure> {
    let value = value.unwrap_or("0");
    if value != "0" && webdock_build::contracts::id(value).is_err() {
        return Err(reject(
            StatusCode::BAD_REQUEST,
            "invalid_input",
            "Invalid log cursor.",
        ));
    }
    Ok(value.to_owned())
}
pub(crate) fn routes() -> Router<Arc<ServiceState>> {
    Router::new()
        .route(
            "/api/console/hosting/projects/{project}/builds/{build}/logs",
            get(history),
        )
        .route(
            "/api/console/hosting/projects/{project}/builds/{build}/logs/events",
            get(events),
        )
}
async fn page(
    state: &ServiceState,
    headers: &HeaderMap,
    project: &str,
    build: &str,
    after: &str,
) -> Result<Value, ApiFailure> {
    state
        .config
        .features
        .require("hosting", OperationKind::Read)
        .map_err(|_| unavailable())?;
    let actor = auth::resolve_actor(state, headers).await?.ok_or_else(|| {
        reject(
            StatusCode::UNAUTHORIZED,
            "authentication_required",
            "Authentication required.",
        )
    })?;
    state.build.as_ref().ok_or_else(unavailable)?.execute_with_authority(&actor.principal,&actor.authority,
        json!({"action":"git.builds.logs","projectID":project,"buildID":build,"after":after,"limit":25}),
        webdock_build::BuildCeiling{read:true,write:false}).await.map_err(|e|reject(StatusCode::from_u16(e.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),e.code,"Build logs are unavailable."))
}
async fn history(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Path((project, build)): Path<(String, String)>,
    Query(query): Query<Page>,
) -> Response {
    let result = async {
        let after = cursor(Some(&query.after))?;
        Ok::<_, ApiFailure>(
            Json(page(&state, &headers, &project, &build, &after).await?).into_response(),
        )
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}
static STREAMS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(16);
async fn events(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Path((project, build)): Path<(String, String)>,
    RawQuery(query): RawQuery,
) -> Response {
    let result=async {
        if query.is_some_and(|q|!q.is_empty()) {return Err(reject(StatusCode::BAD_REQUEST,"invalid_input","Use Last-Event-ID to resume."));}
        let header=headers.get("last-event-id").map(|v|v.to_str()).transpose().map_err(|_|reject(StatusCode::BAD_REQUEST,"invalid_input","Invalid log cursor."))?;
        let mut after=cursor(header)?;
        let first=page(&state,&headers,&project,&build,&after).await?;
        let permit=STREAMS.try_acquire().map_err(|_|reject(StatusCode::TOO_MANY_REQUESTS,"busy","Too many build log streams."))?;
        let (sender,receiver)=tokio::sync::mpsc::channel::<Result<Event,Infallible>>(2);
        tokio::spawn(async move {
            let _permit=permit;
            let deadline=tokio::time::Instant::now()+Duration::from_secs(60);
            let mut first=Some(first);
            let mut last_state=None;
            loop {
                if sender.is_closed() || tokio::time::Instant::now()>=deadline {break;}
                let value=match first.take() {Some(value)=>Ok(value),None=>page(&state,&headers,&project,&build,&after).await};
                let Ok(value)=value else {let _=sender.try_send(Ok(Event::default().event("unavailable").data("{\"code\":\"authority_or_build_unavailable\"}"))); break;};
                let entries=value["entries"].as_array().map_or(0,Vec::len);
                let state=json!([value["status"],value["phase"]]);
                if entries>0 || last_state.as_ref()!=Some(&state) {
                    let next=value["cursor"].as_str().unwrap_or(&after).to_owned();
                    let Ok(event)=Event::default().event("logs").id(&next).json_data(&value) else {break;};
                    if sender.try_send(Ok(event)).is_err() {break;}
                    after=next; last_state=Some(state);
                }
                if value["terminal"]==true && entries<25 {break;}
                tokio::select! {_=sender.closed()=>break,_=tokio::time::sleep(Duration::from_millis(if entries==25 {0}else{500}))=>{}}
            }
        });
        Ok::<_,ApiFailure>(Sse::new(tokio_stream::wrappers::ReceiverStream::new(receiver)).keep_alive(axum::response::sse::KeepAlive::default()).into_response())
    }.await;
    result.unwrap_or_else(IntoResponse::into_response)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cursors_never_accept_numeric_overflow_or_ambiguous_forms() {
        assert_eq!(cursor(None).ok().as_deref(), Some("0"));
        assert_eq!(cursor(Some("42")).ok().as_deref(), Some("42"));
        for bad in ["", "-1", "01", "9223372036854775808", "1e3", "1\n"] {
            assert!(cursor(Some(bad)).is_err());
        }
    }
}
