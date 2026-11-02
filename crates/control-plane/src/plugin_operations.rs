use super::*;
use axum::{
    extract::Path,
    response::sse::{Event, Sse},
};
use std::{convert::Infallible, time::Duration};
use webdock_plugin_host::{RuntimeDescription, StateProbe};

#[derive(Clone, Copy)]
enum StateProbeBoundary {
    RouteLoad,
    GenerationCheck,
    Connect,
    Describe,
    Health,
    Reload,
    Complete,
}
impl StateProbeBoundary {
    fn token(self) -> &'static str {
        match self {
            Self::RouteLoad => "route_load",
            Self::GenerationCheck => "generation_check",
            Self::Connect => "connect",
            Self::Describe => "describe",
            Self::Health => "health",
            Self::Reload => "reload",
            Self::Complete => "complete",
        }
    }
    fn enabled(value: Option<&std::ffi::OsStr>) -> bool {
        value == Some(std::ffi::OsStr::new("1"))
    }
    fn enter(self) {
        if Self::enabled(std::env::var_os("WEBDOCK_RECOVERY_COMPONENT_TEST").as_deref()) {
            eprintln!("plugin_state_probe stage={}", self.token());
        }
    }
    fn failure_line(self, error: &(dyn std::error::Error + 'static)) -> String {
        let code = error
            .downcast_ref::<tonic::Status>()
            .map(|v| v.code() as i32);
        let errno = error
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::raw_os_error);
        let number = |v: Option<i32>| v.map_or_else(|| "null".to_owned(), |v| v.to_string());
        format!(
            "plugin_state_probe_failure stage={} tonicCode={} ioErrno={}",
            self.token(),
            number(code),
            number(errno)
        )
    }
    fn failed(self, error: &(dyn std::error::Error + 'static)) {
        if Self::enabled(std::env::var_os("WEBDOCK_RECOVERY_COMPONENT_TEST").as_deref()) {
            eprintln!("{}", self.failure_line(error));
        }
    }
}

#[async_trait::async_trait]
impl StateProbe for PluginEndpoints {
    async fn describe(
        &self,
        plugin: &str,
        generation: u64,
    ) -> Result<RuntimeDescription, PluginError> {
        StateProbeBoundary::RouteLoad.enter();
        let connection = self
            .connections
            .load_generation(plugin, generation)
            .await
            .map_err(|error| {
                StateProbeBoundary::RouteLoad.failed(error.as_ref());
                unavailable_plugin()
            })?;
        StateProbeBoundary::GenerationCheck.enter();
        if connection.generation != generation {
            return Err(PluginError::denied());
        }
        StateProbeBoundary::Connect.enter();
        let (mut client, peer) = connection.connect_authenticated().await.map_err(|error| {
            StateProbeBoundary::Connect.failed(error.as_ref());
            unavailable_plugin()
        })?;
        StateProbeBoundary::Describe.enter();
        let manifest = client
            .describe(webdock_plugin_runtime::wire::DescribeRequest {})
            .await
            .map_err(|error| {
                StateProbeBoundary::Describe.failed(&error);
                unavailable_plugin()
            })?
            .into_inner()
            .manifest_json;
        StateProbeBoundary::Health.enter();
        let health = client
            .health(webdock_plugin_runtime::wire::HealthRequest {})
            .await
            .map_err(|error| {
                StateProbeBoundary::Health.failed(&error);
                unavailable_plugin()
            })?
            .into_inner();
        StateProbeBoundary::Reload.enter();
        if self
            .connections
            .load_generation(plugin, generation)
            .await
            .map_err(|error| {
                StateProbeBoundary::Reload.failed(error.as_ref());
                unavailable_plugin()
            })?
            != connection
        {
            return Err(PluginError::denied());
        }
        StateProbeBoundary::Complete.enter();
        Ok(RuntimeDescription {
            manifest,
            plugin_id: health.plugin_id,
            version: health.version,
            generation: health.generation,
            protocol_version: health.protocol_version,
            peer,
        })
    }
}
pub(super) fn routes() -> Router<Arc<ServiceState>> {
    Router::new()
        .route("/api/console/plugins/operations/{id}", get(operation))
        .route("/api/console/plugins/operations/{id}/events", get(events))
}
async fn snapshot(
    state: &ServiceState,
    headers: &HeaderMap,
    id: &str,
) -> Result<webdock_plugin_host::OperationSnapshot, ApiFailure> {
    let actor = actor(state, headers).await?;
    endpoints(state)?
        .host
        .operation_with_authority(&actor.principal, &actor.authority, id)
        .await
        .map_err(failure)
}
async fn operation(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    match snapshot(&state, &headers, &id).await {
        Ok(value) => Json(value).into_response(),
        Err(error) => error.into_response(),
    }
}
static STREAMS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(32);
async fn events(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    RawQuery(query): RawQuery,
) -> Response {
    let result: Result<Response, ApiFailure> = async {
        if query.as_ref().is_some_and(|q| !q.is_empty()) {
            return Err(reject(
                StatusCode::BAD_REQUEST,
                "invalid_input",
                "Use Last-Event-ID to resume.",
            ));
        }
        let mut after = match headers.get("last-event-id") {
            Some(value) => value
                .to_str()
                .ok()
                .filter(|v| !v.is_empty() && v.len() <= 19 && v.bytes().all(|c| c.is_ascii_digit()))
                .and_then(|v| v.parse::<u64>().ok())
                .ok_or_else(|| {
                    reject(
                        StatusCode::BAD_REQUEST,
                        "invalid_input",
                        "Invalid event cursor.",
                    )
                })?,
            None => 0,
        };
        let first = snapshot(&state, &headers, &id).await?;
        if after > first.cursor.parse::<u64>().unwrap_or(0) {
            return Err(reject(
                StatusCode::CONFLICT,
                "conflict",
                "Event cursor is ahead of the operation.",
            ));
        }
        let permit = STREAMS.try_acquire().map_err(|_| {
            reject(
                StatusCode::TOO_MANY_REQUESTS,
                "busy",
                "Too many operation streams.",
            )
        })?;
        let (sender, receiver) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(4);
        tokio::spawn(async move {
            let _permit = permit;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
            let mut first = Some(first);
            let mut emitted = false;
            loop {
                if sender.is_closed() || tokio::time::Instant::now() >= deadline {
                    break;
                }
                let value = match first.take() {
                    Some(value) => Ok(value),
                    None => snapshot(&state, &headers, &id).await,
                };
                let Ok(value) = value else {
                    let _ = sender.try_send(Ok(Event::default()
                        .event("unavailable")
                        .data("{\"code\":\"authority_or_operation_unavailable\"}")));
                    break;
                };
                let cursor = value.cursor.parse::<u64>().unwrap_or(0);
                if cursor > after || !emitted && cursor == 0 && after == 0 {
                    let Ok(event) = Event::default()
                        .event("snapshot")
                        .id(value.cursor.clone())
                        .json_data(&value)
                    else {
                        break;
                    };
                    if sender.try_send(Ok(event)).is_err() {
                        break;
                    }
                    after = cursor;
                    emitted = true;
                }
                if matches!(
                    value.status,
                    webdock_plugin_host::OperationStatus::Succeeded
                        | webdock_plugin_host::OperationStatus::Failed
                        | webdock_plugin_host::OperationStatus::Blocked
                ) {
                    break;
                }
                tokio::select! {
                    _ = sender.closed() => break,
                    _ = tokio::time::sleep(Duration::from_millis(500)) => {},
                }
            }
        });
        Ok(Sse::new(tokio_stream::wrappers::ReceiverStream::new(receiver)).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

#[cfg(test)]
mod state_probe_diagnostic_tests {
    use super::StateProbeBoundary;
    #[derive(Debug)]
    struct Poison;
    impl std::fmt::Display for Poison {
        fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            panic!("error Display must not be called")
        }
    }
    impl std::error::Error for Poison {}
    #[test]
    fn exact_flag_and_fixed_numeric_projection_never_format_sensitive_errors() {
        use std::ffi::OsStr;
        assert!(StateProbeBoundary::enabled(Some(OsStr::new("1"))));
        for value in [
            None,
            Some(OsStr::new("0")),
            Some(OsStr::new("true")),
            Some(OsStr::new("01")),
        ] {
            assert!(!StateProbeBoundary::enabled(value));
        }
        assert_eq!(
            StateProbeBoundary::Connect.failure_line(&Poison),
            "plugin_state_probe_failure stage=connect tonicCode=null ioErrno=null"
        );
        assert_eq!(
            StateProbeBoundary::Describe
                .failure_line(&tonic::Status::permission_denied("private-token-poison")),
            "plugin_state_probe_failure stage=describe tonicCode=7 ioErrno=null"
        );
        assert_eq!(
            StateProbeBoundary::RouteLoad.failure_line(&std::io::Error::from_raw_os_error(13)),
            "plugin_state_probe_failure stage=route_load tonicCode=null ioErrno=13"
        );
    }
}
