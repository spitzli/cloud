//! Plugin workloads for enrolled agents: due units of the agent's own cluster, and their reports.
use super::*;

/// Units whose revision is not applied yet, or whose observation is older than this, are due.
const REFRESH_SECONDS: i32 = 15;
const BATCH: i64 = 16;

#[tonic::async_trait]
impl wire::workload_control_server::WorkloadControl for ControlPlane {
    async fn poll_workloads(
        &self,
        request: Request<wire::PollRequest>,
    ) -> Result<Response<wire::WorkloadBatch>, Status> {
        let agent = self
            .authorize(&request, request.get_ref().identity.as_ref())
            .await?;
        let tx = self
            .hosting
            .begin_authenticated_agent(&agent)
            .await
            .map_err(hosting_error)?;
        let rows = tx
            .query_all_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT plugin_id,unit,namespace,revision,objects::text AS objects,removing FROM webdock_auth.plugin_workload \
                 WHERE cluster_id=$1 AND (applied_revision<revision OR observed_at IS NULL OR observed_at<statement_timestamp()-make_interval(secs=>$2)) \
                 ORDER BY observed_at NULLS FIRST LIMIT $3",
                [agent.cluster_id().into(), REFRESH_SECONDS.into(), BATCH.into()],
            ))
            .await
            .map_err(unavailable)?;
        let mut workloads = Vec::with_capacity(rows.len());
        for row in rows {
            let objects: String = row.try_get("", "objects").map_err(unavailable)?;
            workloads.push(wire::Workload {
                plugin_id: row.try_get("", "plugin_id").map_err(unavailable)?,
                unit: row.try_get("", "unit").map_err(unavailable)?,
                namespace: row.try_get("", "namespace").map_err(unavailable)?,
                revision: u64::try_from(row.try_get::<i64>("", "revision").map_err(unavailable)?)
                    .map_err(|_| invalid())?,
                objects_json: objects.into_bytes(),
                removing: row.try_get("", "removing").map_err(unavailable)?,
            });
        }
        tx.commit().await.map_err(unavailable)?;
        Ok(Response::new(wire::WorkloadBatch { workloads }))
    }
    async fn report_workload(
        &self,
        request: Request<wire::WorkloadReport>,
    ) -> Result<Response<wire::Ack>, Status> {
        let input = request.get_ref();
        let agent = self.authorize(&request, input.identity.as_ref()).await?;
        if input.observed_json.len() > 65536 || input.error.len() > 500 {
            return Err(invalid());
        }
        let observed: Option<serde_json::Value> = if input.removed {
            None
        } else {
            Some(serde_json::from_slice(&input.observed_json).map_err(|_| invalid())?)
        };
        let revision = i64::try_from(input.revision).map_err(|_| invalid())?;
        let error = (!input.error.is_empty()).then(|| input.error.clone());
        let tx = self
            .hosting
            .begin_authenticated_agent(&agent)
            .await
            .map_err(hosting_error)?;
        let accepted: bool = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_rls.report_plugin_workload($1,$2,$3,$4,$5,$6) AS accepted",
                [
                    input.plugin_id.clone().into(),
                    input.unit.clone().into(),
                    revision.into(),
                    observed.into(),
                    error.into(),
                    input.removed.into(),
                ],
            ))
            .await
            .map_err(unavailable)?
            .ok_or_else(invalid)?
            .try_get("", "accepted")
            .map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)?;
        Ok(Response::new(wire::Ack { accepted }))
    }
}
impl ControlPlane {
    pub fn workload_service(self) -> wire::workload_control_server::WorkloadControlServer<Self> {
        wire::workload_control_server::WorkloadControlServer::new(self)
            .max_decoding_message_size(wire::MAX_MESSAGE_BYTES)
            .max_encoding_message_size(wire::MAX_MESSAGE_BYTES)
    }
}
