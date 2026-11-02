//! Decode before classifying stale custody; stale does not excuse malformed receipts.
use super::*;
use webdock_hosting::{DeliveryCommand, DeliveryDisposition};
pub(super) enum Completion {
    Mutation(ObservedState),
    Read(Value),
    Failure(String),
}
impl Completion {
    pub(super) fn parse(command: &StoredCommand, input: &wire::Receipt) -> Result<Self, Status> {
        match wire::Outcome::try_from(input.outcome).map_err(|_| invalid())? {
            wire::Outcome::Succeeded => {
                if !input.error_code.is_empty() {
                    return Err(invalid());
                }
                match command {
                    StoredCommand::Mutation(command) => {
                        let Some(wire::receipt::Result::ObservedJson(bytes)) = &input.result else {
                            return Err(invalid());
                        };
                        let observed = parse_observed(bytes, &command.spec)?;
                        Ok(Self::Mutation(observed))
                    }
                    StoredCommand::Read(command) => {
                        let value = match (command.kind, &input.result) {
                            (ReadKind::Logs, Some(wire::receipt::Result::LogsUtf8(bytes))) => {
                                if bytes.len() > 12000 {
                                    return Err(invalid());
                                }
                                let logs = std::str::from_utf8(bytes).map_err(|_| invalid())?;
                                json!({"logs":logs,"truncated":bytes.len()>=11996})
                            }
                            (
                                ReadKind::Inspect,
                                Some(wire::receipt::Result::ObservedJson(bytes)),
                            ) => serde_json::to_value(parse_observed(bytes, &command.spec)?)
                                .map_err(|_| invalid())?,
                            (
                                ReadKind::Metrics,
                                Some(wire::receipt::Result::MetricsJson(bytes)),
                            ) => {
                                if bytes.len() > 65536 {
                                    return Err(invalid());
                                }
                                serde_json::to_value(
                                    serde_json::from_slice::<ResourceMetrics>(bytes)
                                        .map_err(|_| invalid())?,
                                )
                                .map_err(|_| invalid())?
                            }
                            _ => return Err(invalid()),
                        };
                        Ok(Self::Read(value))
                    }
                }
            }
            wire::Outcome::NeedsReconciliation | wire::Outcome::Rejected => {
                let (limit, allow_empty) = match command {
                    StoredCommand::Mutation(_) => (64, true),
                    StoredCommand::Read(_) => (80, false),
                };
                if input.result.is_some()
                    || input.error_code.len() > limit
                    || (!allow_empty && input.error_code.is_empty())
                    || !input
                        .error_code
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
                {
                    return Err(invalid());
                }
                Ok(Self::Failure(input.error_code.clone()))
            }
            wire::Outcome::Unspecified => Err(invalid()),
        }
    }
    pub(super) async fn complete(
        self,
        core: &ControlPlane,
        agent: &AgentIdentity,
        command: &StoredCommand,
        receipt_sha256: &str,
    ) -> Result<(), Status> {
        let result = match (self, command) {
            (Self::Mutation(observed), StoredCommand::Mutation(command)) => {
                core.hosting
                    .complete_runtime_with_receipt(agent, command, &observed, receipt_sha256)
                    .await
            }
            (Self::Read(value), StoredCommand::Read(command)) => {
                core.hosting
                    .complete_read_with_receipt(agent, command, value, receipt_sha256)
                    .await
            }
            (Self::Failure(code), StoredCommand::Mutation(command)) => {
                core.hosting
                    .complete_runtime_failure_with_receipt(agent, command, &code, receipt_sha256)
                    .await
            }
            (Self::Failure(code), StoredCommand::Read(command)) => {
                core.hosting
                    .complete_read_failure_with_receipt(agent, command, &code, receipt_sha256)
                    .await
            }
            _ => return Err(invalid()),
        };
        result.map(|_| ()).map_err(hosting_error)
    }
}
fn parse_observed(bytes: &[u8], spec: &RuntimeSpec) -> Result<ObservedState, Status> {
    if bytes.len() > 65536 {
        return Err(invalid());
    }
    let observed: ObservedState = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    if observed.resource_id != spec.resource_id.to_string() || observed.revision != spec.revision {
        return Err(invalid());
    }
    Ok(observed)
}
impl ControlPlane {
    pub(super) async fn retire_stale_receipt(
        &self,
        agent: &AgentIdentity,
        command: &StoredCommand,
        hash: &str,
    ) -> Result<bool, Status> {
        let command = match command {
            StoredCommand::Mutation(command) => DeliveryCommand::Mutation(command),
            StoredCommand::Read(command) => DeliveryCommand::Read(command),
        };
        self.hosting
            .retire_stale_delivery(agent, command, hash)
            .await
            .map(|disposition| disposition != DeliveryDisposition::Live)
            .map_err(hosting_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failure_decoding_preserves_read_and_mutation_domain_limits() {
        let spec = json!({"version":1,"installation":"test","projectId":"123","resourceId":"456","revision":1,"image":format!("docker.io/library/busybox@sha256:{}","a".repeat(64)),"arguments":[],"cpuMillicores":100,"memoryBytes":33554432,"ephemeralBytes":16777216,"replicas":1,"port":8080,"healthPath":null,"volume":null});
        let mutation:StoredCommand=serde_json::from_value(json!({"operationId":"789","generation":1,"leaseUntil":9999999999u64,"authorization":"fixture-mac","action":"apply","spec":spec})).unwrap();
        let read:StoredCommand=serde_json::from_value(json!({"operationId":"790","generation":1,"leaseUntil":9999999999u64,"authorization":"fixture-mac","kind":"logs","tail":100,"expectedRuntimeId":"owned-runtime","spec":spec})).unwrap();
        mutation.validate().unwrap();
        read.validate().unwrap();
        for (command, length, allowed) in [
            (&read, 80, true),
            (&read, 81, false),
            (&read, 0, false),
            (&mutation, 64, true),
            (&mutation, 65, false),
            (&mutation, 0, true),
        ] {
            let receipt = wire::Receipt {
                outcome: wire::Outcome::NeedsReconciliation as i32,
                error_code: "a".repeat(length),
                ..Default::default()
            };
            assert_eq!(
                Completion::parse(command, &receipt).is_ok(),
                allowed,
                "length={length}, read={}",
                matches!(command, StoredCommand::Read(_))
            );
        }
    }
}
