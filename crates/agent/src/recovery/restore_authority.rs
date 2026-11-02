//! Live restore effects retain live Core authority; cold capsules use their explicit local deadline.
use super::{RecoveryAuthority, RecoveryClaim};
use crate::storage::StorageGuard;
use webdock_supervisor::Result;
pub(super) async fn before_effect(
    online: Option<(&RecoveryClaim, &dyn RecoveryAuthority)>,
    guard: &StorageGuard,
) -> Result<()> {
    guard.check()?;
    if let Some((claim, authority)) = online {
        authority.guard(claim).await?;
    }
    guard.check()
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    struct Live(AtomicBool);
    #[async_trait::async_trait]
    impl RecoveryAuthority for Live {
        async fn guard(&self, _: &RecoveryClaim) -> Result<()> {
            if self.0.load(Ordering::SeqCst) {
                Ok(())
            } else {
                Err(webdock_supervisor::ExecutionError::Denied)
            }
        }
    }
    #[tokio::test]
    async fn revocation_between_restore_effects_prevents_the_next_database_or_volume_write() {
        let authority = Live(AtomicBool::new(true));
        let claim:RecoveryClaim=serde_json::from_value(serde_json::json!({"spec":{"version":1,"operationID":"123","policyID":"124","policyRevision":1,"clusterID":"125","profile":"mail-installation-v1","action":"restore","source":{},"target":{},"recoverySetID":"126","epoch":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","capacityBytes":67108864},"lease":{"generation":1,"token":"private-unit-token","expiresAt":(chrono::Utc::now()+chrono::Duration::seconds(60))},"checkpoint":null})).unwrap();
        let guard = StorageGuard::new(claim.lease.expires_at, Arc::new(AtomicBool::new(false)));
        let directory = tempfile::tempdir().unwrap();
        before_effect(Some((&claim, &authority)), &guard)
            .await
            .unwrap();
        std::fs::write(directory.path().join("first-acknowledged-effect"), b"owned").unwrap();
        authority.0.store(false, Ordering::SeqCst);
        let check = before_effect(Some((&claim, &authority)), &guard).await;
        if check.is_ok() {
            std::fs::write(directory.path().join("forbidden-next-effect"), b"unowned").unwrap();
        }
        assert!(check.is_err());
        assert!(!directory.path().join("forbidden-next-effect").exists());
        // No source session is minted for a separately verified cold capsule.
        before_effect(None, &guard).await.unwrap();
    }
}
