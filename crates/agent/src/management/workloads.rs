//! Plugin workloads: poll due units, apply or remove them, report what was observed.
use super::*;

impl Daemon {
    pub(super) async fn process_workloads(&self) -> Result<bool> {
        let Some(executor) = self.workload_runtime.clone() else {
            return Ok(false);
        };
        let session = self.session().await?;
        let batch = session
            .workload_client
            .clone()
            .poll_workloads(request(wire::PollRequest {
                identity: Some(session.identity.clone()),
            }))
            .await
            .map_err(|e| session.rpc_error(e))?
            .into_inner();
        let worked = !batch.workloads.is_empty();
        for workload in batch.workloads {
            let outcome = executor
                .handle(
                    &workload.plugin_id,
                    &workload.unit,
                    &workload.namespace,
                    &workload.objects_json,
                    workload.removing,
                )
                .await;
            if let Some(error) = &outcome.error {
                tracing::warn!(plugin = %workload.plugin_id, unit = %workload.unit, %error, "workload not applied");
            }
            session
                .workload_client
                .clone()
                .report_workload(request(wire::WorkloadReport {
                    identity: Some(session.identity.clone()),
                    plugin_id: workload.plugin_id,
                    unit: workload.unit,
                    revision: workload.revision,
                    observed_json: serde_json::to_vec(&outcome.observed).unwrap_or_default(),
                    error: outcome.error.unwrap_or_default(),
                    removed: outcome.removed,
                }))
                .await
                .map_err(|e| session.rpc_error(e))?;
        }
        Ok(worked)
    }
}
