//! A live outcome is admitted only after every potentially blocking projection/receipt write.
use super::*;
use chrono::{DateTime, Utc};
impl HostingService {
    /// Must be the last database decision before the caller commits. The operation row is
    /// already locked by this transaction. A read avoids triggering another cache write or
    /// acquiring a new lock after checking the authoritative database clock.
    pub(super) async fn admit_completion_commit(
        &self,
        tx: &DatabaseTransaction,
        op: &Value,
        signed_until: Option<u64>,
        browser_guard: Option<&browser_authority::BrowserGuard>,
    ) -> Result<()> {
        if op["status"] != "running" {
            return Err(error(
                409,
                "Completion was not admitted from a running lease.",
            ));
        }
        if !self
            .accepted_authority_with_guard(tx, op, browser_guard)
            .await?
        {
            return Err(error(403, "Operation authority changed."));
        }
        let authority_until = if let Some(guard) = browser_guard {
            Some(guard.verify_expiry().await?)
        } else {
            self.completion_session_expiry(op).await?
        };
        let lease_until = DateTime::parse_from_rfc3339(field(op, "lease_until")?)
            .map_err(|_| error(503, "Stored operation deadline is invalid."))?
            .with_timezone(&Utc);
        let signed_until = signed_until
            .map(|value| {
                i64::try_from(value)
                    .ok()
                    .and_then(|value| DateTime::<Utc>::from_timestamp(value, 0))
                    .ok_or_else(|| error(400, "Invalid signed operation deadline."))
            })
            .transpose()?;
        let allowed=one(tx,
            "SELECT coalesce(generation=$2 AND lease_until=$3 AND lease_until>clock_timestamp() AND ($4::timestamptz IS NULL OR $4>clock_timestamp()) AND ($5::timestamptz IS NULL OR $5>clock_timestamp()),false) AS allowed FROM webdock_auth.hosting_operation WHERE id=$1",
            vec![field(op,"id")?.into(),numeric(op,"generation")?.into(),lease_until.into(),signed_until.into(),authority_until.into()],
        ).await?.ok_or_else(||error(409,"Completion operation changed."))?.try_get::<bool>("","allowed").map_err(db_error)?;
        if !allowed {
            return Err(error(409, "Completion deadline expired or changed."));
        }
        Ok(())
    }
    async fn completion_session_expiry(&self, op: &Value) -> Result<Option<DateTime<Utc>>> {
        // Only an explicitly policy-authorized Git operation may have no session deadline.
        // Other operations retain their recorded initiating or operator-reconciliation session.
        let git = !op["git_release_id"].is_null();
        let subject = if git {
            field(op, "subject")?
        } else {
            op["authority_subject"]
                .as_str()
                .unwrap_or(field(op, "subject")?)
        };
        let session = if git {
            op["actor_session"].as_str()
        } else {
            op["authority_session"]
                .as_str()
                .or_else(|| op["actor_session"].as_str())
        };
        let Some(session) = session.filter(|session| !session.is_empty()) else {
            if git {
                return Ok(None);
            }
            return Err(error(403, "Original operation session is unavailable."));
        };
        let row = one(
            &self.auth,
            "SELECT \"expiresAt\" FROM webdock_auth.session WHERE id=$1 AND \"userId\"=$2",
            vec![session.into(), subject.into()],
        )
        .await?
        .ok_or_else(|| error(403, "Original operation session is unavailable."))?;
        Ok(Some(row.try_get("", "expiresAt").map_err(db_error)?))
    }
}
