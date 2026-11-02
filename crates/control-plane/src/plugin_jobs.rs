//! Native durable job dispatch. Database custody ends before contacting a plugin runtime.
use super::PluginEndpoints;
use std::time::Duration;
use webdock_plugin_host::JobLease;
use webdock_plugin_protocol::{ErrorCode, PluginError, Result};
use webdock_plugin_runtime::wire::{DescribeRequest, HealthRequest, RunJobRequest};

fn unavailable() -> PluginError {
    PluginError::new(ErrorCode::Unavailable, "Plugin job runtime unavailable")
}
fn rpc_failure(status: tonic::Status) -> PluginError {
    let code = match status.code() {
        tonic::Code::PermissionDenied | tonic::Code::Unauthenticated => ErrorCode::PermissionDenied,
        tonic::Code::InvalidArgument | tonic::Code::ResourceExhausted => ErrorCode::InvalidInput,
        tonic::Code::Aborted => ErrorCode::Conflict,
        tonic::Code::NotFound => ErrorCode::NotFound,
        tonic::Code::Unimplemented => ErrorCode::CapabilityUnavailable,
        tonic::Code::DeadlineExceeded => ErrorCode::DeadlineExceeded,
        _ => ErrorCode::Unavailable,
    };
    // Runtime exception messages may contain provider credentials.
    PluginError::new(code, "Plugin job attempt failed")
}
impl PluginEndpoints {
    /// One bounded queue pass. A dropped dispatcher leaves its lease for crash recovery.
    pub async fn advance_job(&self) -> Result<bool> {
        let Some(lease) = self.host.claim_job(None).await? else {
            return Ok(false);
        };
        let result = tokio::time::timeout(Duration::from_secs(55), self.execute_job(&lease))
            .await
            .unwrap_or_else(|_| {
                Err(PluginError::new(
                    ErrorCode::DeadlineExceeded,
                    "Plugin job dispatch deadline exceeded",
                ))
            });
        self.host.complete_job(&lease, result).await?;
        Ok(true)
    }

    async fn execute_job(&self, lease: &JobLease) -> Result<serde_json::Value> {
        let admitted = lease.invocation();
        let connection = self
            .connections
            .load_generation(lease.plugin(), lease.generation())
            .await
            .map_err(|_| unavailable())?;
        if connection.generation != lease.generation() {
            return Err(PluginError::denied());
        }
        let (mut client, peer) = connection
            .connect_authenticated()
            .await
            .map_err(|_| unavailable())?;
        let handle = self.host.authenticate(peer, &admitted.token).await?;
        self.host
            .verify_request(&handle, &admitted.input, &admitted.configuration)
            .await?;
        let manifest = client
            .describe(DescribeRequest {})
            .await
            .map_err(rpc_failure)?
            .into_inner();
        let health = client
            .health(HealthRequest {})
            .await
            .map_err(rpc_failure)?
            .into_inner();
        if manifest.manifest_json != lease.manifest().as_bytes()
            || health.plugin_id != lease.plugin()
            || health.version != lease.manifest().version()
            || health.generation != lease.generation()
            || health.protocol_version != 1
        {
            return Err(PluginError::denied());
        }
        self.host.revalidate(&handle).await?;
        let reply = client
            .run_job(RunJobRequest {
                invocation_token: admitted.token.clone(),
                name: admitted.claims.action.clone(),
                input_json: serde_json::to_vec(&admitted.input).map_err(|_| unavailable())?,
                configuration_json: serde_json::to_vec(&admitted.configuration)
                    .map_err(|_| unavailable())?,
            })
            .await
            .map_err(rpc_failure)?
            .into_inner();
        if reply.output_json.len() > 32768 {
            return Err(PluginError::new(
                ErrorCode::InvalidInput,
                "Plugin job result limit",
            ));
        }
        let result = serde_json::from_slice(&reply.output_json)
            .map_err(|_| PluginError::new(ErrorCode::InvalidInput, "Invalid plugin job result"))?;
        if self
            .connections
            .load_generation(lease.plugin(), lease.generation())
            .await
            .map_err(|_| unavailable())?
            != connection
        {
            return Err(PluginError::denied());
        }
        self.host.revalidate(&handle).await?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_failures_have_stable_retry_classification_without_diagnostics() {
        for (status, expected) in [
            (tonic::Code::Unavailable, ErrorCode::Unavailable),
            (tonic::Code::DeadlineExceeded, ErrorCode::DeadlineExceeded),
            (tonic::Code::PermissionDenied, ErrorCode::PermissionDenied),
            (tonic::Code::InvalidArgument, ErrorCode::InvalidInput),
            (tonic::Code::ResourceExhausted, ErrorCode::InvalidInput),
            (tonic::Code::Unimplemented, ErrorCode::CapabilityUnavailable),
            (tonic::Code::Aborted, ErrorCode::Conflict),
            (tonic::Code::NotFound, ErrorCode::NotFound),
        ] {
            let result = rpc_failure(tonic::Status::new(status, "PRIVATE provider diagnostic"));
            assert_eq!(result.code, expected);
            assert_eq!(result.message, "Plugin job attempt failed");
        }
    }
}
