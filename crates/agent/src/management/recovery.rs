//! Enrolled recovery execution and exact protected native callback custody.
use super::*;
use crate::recovery::RecoveryAuthority;
use serde_json::Value;
use wire::recovery::{RecoveryClaim, RecoveryCompletion};
fn bytes(claim: &RecoveryClaim) -> webdock_supervisor::Result<Vec<u8>> {
    claim.validate().map_err(|_| ExecutionError::Expired)?;
    let bytes = serde_json::to_vec(claim).map_err(|_| ExecutionError::Runtime)?;
    if bytes.len() > 131072 {
        return Err(ExecutionError::Invalid("recovery claim"));
    }
    Ok(bytes)
}
impl RemoteAuthority {
    async fn recovery_session(&self) -> Result<Session> {
        let session = self
            .session
            .read()
            .await
            .clone()
            .ok_or(AgentError::Transport)?;
        session.valid()?;
        Ok(session)
    }
    async fn flush_recovery(&self) -> Result<Option<Value>> {
        let Some(delivery) = self.recovery_outbox.lock().await.pending()? else {
            return Ok(None);
        };
        let session = self.recovery_session().await?;
        let mut client = session.recovery_client.clone();
        let value = match delivery.callback.clone().ok_or(AgentError::State)? {
            wire::recovery_delivery::Callback::Checkpoint(checkpoint) => {
                if checkpoint.identity.as_ref() != Some(&session.identity) {
                    return Err(AgentError::RecoveryPending);
                }
                let reply = client
                    .checkpoint_recovery(request(checkpoint))
                    .await
                    .map_err(|_| AgentError::RecoveryPending)?
                    .into_inner();
                if !reply.accepted || reply.plan_json.len() > 320 * 1024 {
                    return Err(AgentError::RecoveryPending);
                }
                Some(serde_json::from_slice(&reply.plan_json).map_err(|_| AgentError::State)?)
            }
            wire::recovery_delivery::Callback::Receipt(receipt) => {
                if receipt.identity.as_ref() != Some(&session.identity) {
                    return Err(AgentError::RecoveryPending);
                }
                let reply = client
                    .complete_recovery(request(receipt))
                    .await
                    .map_err(|_| AgentError::RecoveryPending)?
                    .into_inner();
                if !reply.accepted {
                    return Err(AgentError::RecoveryPending);
                }
                None
            }
        };
        if let Some((claim, checkpoint)) = app_resume_delivery(&delivery)? {
            let plan = value.as_ref().ok_or(AgentError::RecoveryPending)?;
            crate::recovery::app_resume_binding(&claim, &checkpoint, plan)
                .map_err(|_| AgentError::RecoveryPending)?;
            let runtime = self
                .recovery_executor
                .get()
                .ok_or(AgentError::RecoveryPending)?;
            acknowledge_after_finalization(&self.recovery_outbox, &delivery, async {
                runtime
                    .finish_app_resume_ack(&claim, &checkpoint, plan, self)
                    .await
                    .map_err(|_| AgentError::RecoveryPending)
            })
            .await?;
        } else {
            self.recovery_outbox.lock().await.acknowledge(&delivery)?;
        }
        Ok(value)
    }
}
fn app_resume_delivery(
    delivery: &wire::RecoveryDelivery,
) -> Result<Option<(RecoveryClaim, Value)>> {
    let Some(wire::recovery_delivery::Callback::Checkpoint(c)) = &delivery.callback else {
        return Ok(None);
    };
    if c.phase != "checkpoint" {
        return Ok(None);
    }
    let checkpoint: Value =
        serde_json::from_slice(&c.checkpoint_json).map_err(|_| AgentError::State)?;
    if checkpoint["phase"] != "app-resumed" {
        return Ok(None);
    }
    let claim: RecoveryClaim =
        serde_json::from_slice(&c.claim_json).map_err(|_| AgentError::State)?;
    if !crate::recovery::app_backup(&claim) {
        return Ok(None);
    }
    Ok(Some((claim, checkpoint)))
}
async fn acknowledge_after_finalization(
    outbox: &Mutex<crate::outbox::RecoveryOutbox>,
    delivery: &wire::RecoveryDelivery,
    finalization: impl std::future::Future<Output = Result<()>>,
) -> Result<()> {
    if outbox.lock().await.pending()?.as_ref() != Some(delivery) {
        return Err(AgentError::RecoveryPending);
    }
    finalization.await?;
    outbox.lock().await.acknowledge(delivery)
}
#[async_trait::async_trait]
impl RecoveryAuthority for RemoteAuthority {
    async fn guard(&self, claim: &RecoveryClaim) -> webdock_supervisor::Result<()> {
        let body = bytes(claim)?;
        let session = self
            .recovery_session()
            .await
            .map_err(|_| ExecutionError::Denied)?;
        let reply = session
            .recovery_client
            .clone()
            .guard_recovery(request(wire::RecoveryGuardRequest {
                identity: Some(session.identity.clone()),
                claim_json: body,
            }))
            .await
            .map_err(|_| ExecutionError::Denied)?
            .into_inner();
        session.valid().map_err(|_| ExecutionError::Denied)?;
        if !reply.allowed {
            return Err(ExecutionError::Denied);
        }
        Ok(())
    }
    async fn checkpoint(
        &self,
        claim: &RecoveryClaim,
        phase: &str,
        checkpoint: Value,
    ) -> webdock_supervisor::Result<Value> {
        checkpoint_call(self, claim, phase, &checkpoint, async {
            let session = self
                .recovery_session()
                .await
                .map_err(|_| ExecutionError::Denied)?;
            let checkpoint_json =
                serde_json::to_vec(&checkpoint).map_err(|_| ExecutionError::Runtime)?;
            if checkpoint_json.len() > 65536 {
                return Err(ExecutionError::Invalid("recovery checkpoint"));
            }
            let delivery = wire::RecoveryDelivery {
                callback: Some(wire::recovery_delivery::Callback::Checkpoint(
                    wire::RecoveryCheckpointRequest {
                        identity: Some(session.identity),
                        claim_json: bytes(claim)?,
                        phase: phase.into(),
                        checkpoint_json,
                    },
                )),
            };
            self.recovery_outbox
                .lock()
                .await
                .put(&delivery)
                .map_err(|_| ExecutionError::Journal)?;
            self.flush_recovery()
                .await
                .map_err(|_| ExecutionError::Runtime)?
                .ok_or(ExecutionError::Runtime)
        })
        .await
    }
}
async fn checkpoint_call<T>(
    authority: &dyn RecoveryAuthority,
    claim: &RecoveryClaim,
    phase: &str,
    checkpoint: &Value,
    call: impl std::future::Future<Output = webdock_supervisor::Result<T>>,
) -> webdock_supervisor::Result<T> {
    // Exact app metadata phases authorize atomically at the Core checkpoint endpoint.
    // They are not provider-effect guards and never rewrite the original claim.
    if !crate::recovery::app_checkpoint(claim, phase, checkpoint) {
        authority.guard(claim).await?;
    }
    call.await
}
impl Daemon {
    pub(super) async fn process_recovery(&self) -> Result<bool> {
        self.authority.flush_recovery().await?;
        let Some(runtime) = &self.recovery_runtime else {
            return Ok(false);
        };
        let session = self.session().await?;
        let dispatch = session
            .recovery_client
            .clone()
            .poll_recovery(request(wire::PollRequest {
                identity: Some(session.identity.clone()),
            }))
            .await
            .map_err(|error| session.rpc_error(error))?
            .into_inner();
        if dispatch.claim_json.is_empty() {
            return Ok(false);
        }
        if dispatch.claim_json.len() > 131072 {
            return Err(AgentError::Configuration("Invalid recovery dispatch"));
        }
        let claim: RecoveryClaim = serde_json::from_slice(&dispatch.claim_json)
            .map_err(|_| AgentError::Configuration("Invalid recovery claim"))?;
        claim.validate().map_err(|_| ExecutionError::Expired)?;
        let result = runtime.execute(&claim, &*self.authority).await;
        let (state, proof, error_code) = match result {
            Ok(proof) => ("succeeded", proof, None),
            Err(error) => {
                let code = match error {
                    ExecutionError::Ownership | ExecutionError::Fenced | ExecutionError::Denied => {
                        "recovery_authority_or_integrity_denied"
                    }
                    ExecutionError::Expired => "recovery_deadline",
                    _ => "recovery_execution_uncertain",
                };
                (
                    "needs-reconciliation",
                    serde_json::json!({"version":1,"dispatch":"quarantined"}),
                    Some(code.to_owned()),
                )
            }
        };
        let completion = RecoveryCompletion {
            generation: claim.lease.generation,
            token: claim.lease.token.clone(),
            state: state.into(),
            proof,
            error_code,
        };
        let delivery = wire::RecoveryDelivery {
            callback: Some(wire::recovery_delivery::Callback::Receipt(
                wire::RecoveryReceipt {
                    identity: Some(session.identity),
                    claim_json: serde_json::to_vec(&claim).map_err(|_| AgentError::State)?,
                    completion_json: serde_json::to_vec(&completion)
                        .map_err(|_| AgentError::State)?,
                },
            )),
        };
        queue_terminal(&mut *self.authority.recovery_outbox.lock().await, &delivery)?;
        self.authority.flush_recovery().await?;
        Ok(true)
    }
}

fn queue_terminal(
    outbox: &mut crate::outbox::RecoveryOutbox,
    delivery: &wire::RecoveryDelivery,
) -> Result<()> {
    if outbox.pending()?.is_some() {
        return Err(AgentError::RecoveryPending);
    }
    outbox.put(delivery)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn exact_app_metadata_calls_checkpoint_once_without_generic_provider_guard() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Guard(AtomicUsize);
        #[async_trait::async_trait]
        impl RecoveryAuthority for Guard {
            async fn guard(&self, _: &RecoveryClaim) -> webdock_supervisor::Result<()> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Err(ExecutionError::Denied)
            }
        }
        let mut c:RecoveryClaim=serde_json::from_value(serde_json::json!({"spec":{"version":1,"operationID":"123","policyID":"124","policyRevision":1,"clusterID":"125","profile":"ntfy-sqlite-offline-v1","action":"backup","source":{},"target":null,"recoverySetID":null,"epoch":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","capacityBytes":67108864},"lease":{"generation":1,"token":"bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb","expiresAt":"2099-01-01T00:00:00Z"},"checkpoint":null})).unwrap();
        let authority = Guard(AtomicUsize::new(0));
        let original = serde_json::to_vec(&c).unwrap();
        let calls = AtomicUsize::new(0);
        let result = checkpoint_call(
            &authority,
            &c,
            "checkpoint",
            &serde_json::json!({"phase":"app-stopped"}),
            async {
                calls.fetch_add(1, Ordering::SeqCst);
                Err::<(), _>(ExecutionError::Denied)
            },
        )
        .await;
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(authority.0.load(Ordering::SeqCst), 0);
        assert_eq!(serde_json::to_vec(&c).unwrap(), original);
        // Unknown app metadata and Mail retain the ordinary generic guard path.
        assert!(
            checkpoint_call(
                &authority,
                &c,
                "checkpoint",
                &serde_json::json!({"phase":"unknown"}),
                async { Ok(()) }
            )
            .await
            .is_err()
        );
        c.spec.profile = wire::recovery::RecoveryProfile::MailInstallationV1;
        assert!(
            checkpoint_call(
                &authority,
                &c,
                "checkpoint",
                &serde_json::json!({"phase":"app-stopped"}),
                async { Ok(()) }
            )
            .await
            .is_err()
        );
        assert_eq!(authority.0.load(Ordering::SeqCst), 2);
    }
    #[tokio::test]
    async fn lost_resume_ack_replay_finalizes_durable_gate_before_acknowledging_outbox() {
        use crate::recovery::effect_fence::{AppEffectFenceRegistry, AppFenceBinding};
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let binding:AppFenceBinding=serde_json::from_value(serde_json::json!({"appID":"123","projectID":"124","customerID":"125","clusterID":"126","policyID":"127","policyRevision":1,"agentGeneration":1,"sourceRevision":1,"sourceUID":"a".repeat(64),"specSHA256":"b".repeat(64),"environmentSHA256":"c".repeat(64),"sourceOperationID":"128","sourceOperationGeneration":1,"sourceOperationStatus":"succeeded","sourceOperationFingerprint":"d".repeat(64),"operationID":"129","generation":1,"epoch":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","fenceFingerprint":"e".repeat(64)})).unwrap();
        let registry = AppEffectFenceRegistry::open(dir.path().join("apps")).unwrap();
        let gate = registry
            .close_authorized(binding.clone(), async { Ok(()) })
            .await
            .unwrap();
        gate.record_resume_completion(&binding).unwrap();
        let path = dir.path().join("recovery.json");
        let outbox = Mutex::new(crate::outbox::RecoveryOutbox::open(&path).unwrap());
        let delivery = wire::RecoveryDelivery {
            callback: Some(wire::recovery_delivery::Callback::Checkpoint(
                wire::RecoveryCheckpointRequest {
                    identity: None,
                    claim_json: b"private-unit-pending-claim".to_vec(),
                    phase: "checkpoint".into(),
                    checkpoint_json: br#"{"phase":"app-resumed"}"#.to_vec(),
                },
            )),
        };
        outbox.lock().await.put(&delivery).unwrap();
        // No Core reply means no finalization/ACK; the exact durable item survives restart.
        drop(outbox);
        drop(gate);
        drop(registry);
        let outbox = Mutex::new(crate::outbox::RecoveryOutbox::open(&path).unwrap());
        let registry = AppEffectFenceRegistry::open(dir.path().join("apps")).unwrap();
        let local = dir.path().join("apps/123.json");
        let closed = std::fs::read(&local).unwrap();
        assert!(
            acknowledge_after_finalization(&outbox, &delivery, async {
                registry
                    .finish_release_ack(binding.clone(), async { Err(ExecutionError::Fenced) })
                    .await
                    .map_err(|_| AgentError::RecoveryPending)
            })
            .await
            .is_err()
        );
        assert_eq!(
            outbox.lock().await.pending().unwrap(),
            Some(delivery.clone())
        );
        assert_eq!(std::fs::read(&local).unwrap(), closed);
        acknowledge_after_finalization(&outbox, &delivery, async {
            registry
                .finish_release_ack(binding.clone(), async { Ok(()) })
                .await
                .map_err(|_| AgentError::RecoveryPending)
        })
        .await
        .unwrap();
        assert!(outbox.lock().await.pending().unwrap().is_none());
        assert!(
            serde_json::from_slice::<Value>(&std::fs::read(&local).unwrap()).unwrap()["fence"]
                .is_null()
        );
        let released = std::fs::read(&local).unwrap();
        outbox.lock().await.put(&delivery).unwrap();
        acknowledge_after_finalization(&outbox, &delivery, async {
            registry
                .finish_release_ack(binding, async { Err(ExecutionError::Fenced) })
                .await
                .map_err(|_| AgentError::RecoveryPending)
        })
        .await
        .unwrap();
        assert!(outbox.lock().await.pending().unwrap().is_none());
        assert_eq!(std::fs::read(&local).unwrap(), released);
    }
    #[test]
    fn lost_checkpoint_ack_keeps_exact_outbox_and_defers_new_terminal_after_restart() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.path().join("recovery.json");
        let checkpoint = wire::RecoveryDelivery {
            callback: Some(wire::recovery_delivery::Callback::Checkpoint(
                wire::RecoveryCheckpointRequest {
                    identity: None,
                    claim_json: b"private-unit-claim".to_vec(),
                    phase: "fenced".into(),
                    checkpoint_json: b"{}".to_vec(),
                },
            )),
        };
        let terminal = wire::RecoveryDelivery {
            callback: Some(wire::recovery_delivery::Callback::Receipt(
                wire::RecoveryReceipt {
                    identity: None,
                    claim_json: b"private-unit-claim".to_vec(),
                    completion_json: b"{}".to_vec(),
                },
            )),
        };
        {
            let mut outbox = crate::outbox::RecoveryOutbox::open(&path).unwrap();
            outbox.put(&checkpoint).unwrap();
        }
        let mut restarted = crate::outbox::RecoveryOutbox::open(&path).unwrap();
        assert!(
            matches!(
                queue_terminal(&mut restarted, &terminal),
                Err(AgentError::RecoveryPending)
            ),
            "lost checkpoint ACK must not crash daemon with outbox State error"
        );
        assert_eq!(restarted.pending().unwrap(), Some(checkpoint));
    }
}
