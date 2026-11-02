//! Private write/effect interlock. Core must separately authorize and fence matched recovery.
//! There is deliberately no public activation or quarantine release endpoint.
use crate::store::Store;
use tokio_postgres::Client;
use uuid::Uuid;
type Result<T> = std::result::Result<T, webdock_plugin_kit::Error>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryState {
    pub epoch: Uuid,
    pub state: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryFence {
    pub epoch: Uuid,
    pub operation: Uuid,
    pub generation: i64,
    pub revision: i64,
}
/// Connection drop releases the transaction lock, including cancelled futures/process exit.
pub struct RecoveryGuard {
    _client: Client,
    _permit: tokio::sync::OwnedSemaphorePermit,
}
/// Explicit completion removes durable custody; Drop deliberately leaves uncertainty.
pub struct RecoveryEffect {
    guard: RecoveryGuard,
    token: Uuid,
    epoch: Uuid,
}
impl RecoveryEffect {
    /// Call only after all provider effects and required private outcome writes succeed,
    /// or after proving no provider request started. An error is not that proof.
    pub async fn finish(self) -> Result<()> {
        let n = self
            .guard
            ._client
            .execute(
                "DELETE FROM mail_recovery_effect WHERE token=$1 AND epoch=$2",
                &[&self.token, &self.epoch],
            )
            .await?;
        if n != 1 {
            return Err("recovery effect custody changed".into());
        }
        self.guard._client.batch_execute("COMMIT").await?;
        Ok(())
    }
}
/// Exclusive drain stays held until the caller has finished its consistent snapshot.
pub struct DrainedRecovery {
    _client: Client,
    fence: RecoveryFence,
}
impl DrainedRecovery {
    pub fn fence(&self) -> &RecoveryFence {
        &self.fence
    }
}
impl Store {
    /// Startup may inspect uncertainty, but cannot resolve or replay it.
    pub(crate) async fn recovery_effect_count(
        &self,
    ) -> std::result::Result<i64, crate::store::Error> {
        Ok(self
            .0
            .query_one("SELECT count(*) FROM mail_recovery_effect", &[])
            .await?
            .get(0))
    }
    pub async fn recovery_effect(&self) -> Result<RecoveryEffect> {
        let guard = self.recovery_guard().await?;
        let token = Uuid::new_v4();
        let row = self.0.query_opt("INSERT INTO mail_recovery_effect(token,epoch) SELECT $1,epoch FROM mail_recovery_control WHERE singleton AND state='active' FOR SHARE RETURNING epoch", &[&token]).await?.ok_or("mail recovery fenced")?;
        Ok(RecoveryEffect {
            guard,
            token,
            epoch: row.get(0),
        })
    }
    pub async fn recovery_state(&self) -> Result<RecoveryState> {
        let row = self
            .0
            .query_one(
                "SELECT epoch,state FROM mail_recovery_control WHERE singleton",
                &[],
            )
            .await?;
        Ok(RecoveryState {
            epoch: row.get(0),
            state: row.get(1),
        })
    }
    /// Hold through the complete external effect, including the final SMTP response.
    pub async fn recovery_guard(&self) -> Result<RecoveryGuard> {
        let permit = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.recovery_slots().acquire_owned(),
        )
        .await??;
        let client = self.private_connection().await?;
        client
            .batch_execute("BEGIN; SET LOCAL statement_timeout='10s'")
            .await?;
        let locked: bool = client
            .query_one("SELECT pg_try_advisory_xact_lock_shared(704612,1)", &[])
            .await?
            .get(0);
        if !locked
            || !client
                .query_one(
                    "SELECT state='active' FROM mail_recovery_control WHERE singleton",
                    &[],
                )
                .await?
                .get::<_, bool>(0)
        {
            return Err("mail recovery fenced".into());
        }
        Ok(RecoveryGuard {
            _client: client,
            _permit: permit,
        })
    }
    /// Close durably BEFORE waiting for writers/effects. A timeout/crash leaves it closed.
    /// This is not authority: trusted Core integration must validate its real policy/lease first.
    pub async fn fence_recovery(
        &self,
        epoch: Uuid,
        operation: Uuid,
        generation: i64,
    ) -> Result<DrainedRecovery> {
        if generation <= 0 || operation.is_nil() {
            return Err("invalid recovery fence".into());
        }
        let client = self.private_connection().await?;
        client.batch_execute("SET statement_timeout='5s'").await?;
        let row = client.query_opt("UPDATE mail_recovery_control SET state='fenced',operation=$2,generation=$3 WHERE singleton AND epoch=$1 AND (state='active' AND generation<$3 OR state='fenced' AND operation=$2 AND generation=$3) RETURNING revision", &[&epoch,&operation,&generation]).await?.ok_or("stale recovery fence")?;
        let fence = RecoveryFence {
            epoch,
            operation,
            generation,
            revision: row.get(0),
        };
        client
            .batch_execute(
                "BEGIN; SET LOCAL statement_timeout='5s'; SELECT pg_advisory_xact_lock(704612,1)",
            )
            .await?;
        let valid: bool = client.query_one("SELECT epoch=$1 AND operation=$2 AND generation=$3 AND revision=$4 AND state='fenced' FROM mail_recovery_control WHERE singleton", &[&fence.epoch,&fence.operation,&fence.generation,&fence.revision]).await?.get(0);
        if !valid {
            return Err("stale recovery drain".into());
        }
        let uncertain: bool = client
            .query_one("SELECT EXISTS(SELECT 1 FROM mail_recovery_effect)", &[])
            .await?
            .get(0);
        if uncertain {
            return Err("recovery effects require reconciliation".into());
        }
        Ok(DrainedRecovery {
            _client: client,
            fence,
        })
    }
    /// Source only: caller must recheck current Core intent/lease before this compare-and-set.
    pub async fn resume_recovery(&self, fence: &RecoveryFence) -> Result<()> {
        let n = self.0.execute("UPDATE mail_recovery_control SET state='active',revision=revision+1 WHERE singleton AND state='fenced' AND epoch=$1 AND operation=$2 AND generation=$3 AND revision=$4 AND NOT EXISTS(SELECT 1 FROM mail_recovery_effect)", &[&fence.epoch,&fence.operation,&fence.generation,&fence.revision]).await?;
        if n != 1 {
            return Err("stale recovery resume".into());
        }
        Ok(())
    }
    /// Fresh restored metadata is permanently closed to historical leases and source callbacks.
    pub async fn quarantine_recovery(&self, fence: &RecoveryFence) -> Result<Uuid> {
        let epoch = Uuid::new_v4();
        let n = self.0.execute("UPDATE mail_recovery_control SET state='quarantined',epoch=$5,revision=revision+1 WHERE singleton AND state='fenced' AND epoch=$1 AND operation=$2 AND generation=$3 AND revision=$4", &[&fence.epoch,&fence.operation,&fence.generation,&fence.revision,&epoch]).await?;
        if n != 1 {
            return Err("stale recovery quarantine".into());
        }
        Ok(epoch)
    }
}

#[cfg(test)]
mod tests;
