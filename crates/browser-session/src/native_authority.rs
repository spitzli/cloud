//! Native browser proof, sharing claim/ceiling policy with retained callers.
use super::{BrowserSessions, ReferenceRecord, string};
use crate::{BrowserAuthentication, BrowserAuthority, Error, Result};
use serde_json::Value;
use std::time::Duration;
use tokio_postgres::types::Type;
use webdock_identity::Principal;
use webdock_persistence::postgres::Transaction;
const BUDGET: Duration = Duration::from_secs(30);
async fn record(
    tx: &Transaction<'_>,
    reference: &str,
    origin: &str,
    client: &str,
) -> Result<Option<ReferenceRecord>> {
    let row=tx.query_typed_opt("SELECT user_id,session_id,payload,expires_at FROM webdock_auth.identity_browser_session WHERE token_hash=$1 AND app_origin=$2 AND client_id=$3 AND expires_at>statement_timestamp()",&[(&reference,Type::TEXT),(&origin,Type::TEXT),(&client,Type::TEXT)]).await.map_err(|_|Error::Configuration)?;
    row.map(
        |row| -> std::result::Result<ReferenceRecord, tokio_postgres::Error> {
            Ok(ReferenceRecord {
                reference: reference.into(),
                origin: origin.into(),
                user: row.try_get("user_id")?,
                session: row.try_get("session_id")?,
                payload: row.try_get("payload")?,
                expires: row.try_get("expires_at")?,
            })
        },
    )
    .transpose()
    .map_err(|_| Error::Configuration)
}
impl BrowserSessions {
    pub(super) async fn native_reference(
        &self,
        origin: &str,
        reference: &str,
        ceiling: Option<&BrowserAuthority>,
    ) -> Result<Option<BrowserAuthentication>> {
        tokio::time::timeout(BUDGET, async {
            let mut connection = self
                .native_store
                .as_ref()
                .ok_or(Error::Configuration)?
                .acquire()
                .await
                .map_err(|_| Error::Configuration)?;
            let tx = connection
                .transaction()
                .await
                .map_err(|_| Error::Configuration)?;
            let (tx, value) = self
                .native_reference_in(tx, origin, reference, ceiling)
                .await?;
            tx.commit().await.map_err(|_| Error::Configuration)?;
            Ok(value)
        })
        .await
        .map_err(|_| Error::Configuration)?
    }
    pub(super) async fn native_revalidate(
        &self,
        authority: &BrowserAuthority,
    ) -> Result<BrowserAuthentication> {
        tokio::time::timeout(BUDGET, async {
            let mut connection = self
                .native_store
                .as_ref()
                .ok_or(Error::Configuration)?
                .acquire()
                .await
                .map_err(|_| Error::Configuration)?;
            let tx = connection
                .transaction()
                .await
                .map_err(|_| Error::Configuration)?;
            let (tx, value) = self.revalidate_native_in(tx, authority).await?;
            tx.commit().await.map_err(|_| Error::Configuration)?;
            Ok(value)
        })
        .await
        .map_err(|_| Error::Configuration)?
    }
    /// Trusted server-owned proof; retain returned transaction through domain completion.
    pub async fn revalidate_native_in<'a>(
        &self,
        tx: Transaction<'a>,
        authority: &BrowserAuthority,
    ) -> Result<(Transaction<'a>, BrowserAuthentication)> {
        tokio::time::timeout(BUDGET, async move {
            if authority.client_id != self.config.client_id
                || authority.expires_at <= chrono::Utc::now().timestamp()
            {
                return Err(Error::Unauthorized);
            }
            let (tx, current) = self
                .native_reference_in(
                    tx,
                    &authority.app_origin,
                    &authority.reference,
                    Some(authority),
                )
                .await?;
            let mut current = current.ok_or(Error::Unauthorized)?;
            current.authority.expires_at = current.authority.expires_at.min(authority.expires_at);
            if current.authority.expires_at <= chrono::Utc::now().timestamp() {
                return Err(Error::Unauthorized);
            }
            current.principal.expires_at =
                chrono::DateTime::from_timestamp(current.authority.expires_at, 0)
                    .ok_or(Error::Unauthorized)?;
            Ok((tx, current))
        })
        .await
        .map_err(|_| Error::Configuration)?
    }
    async fn native_reference_in<'a>(
        &self,
        tx: Transaction<'a>,
        origin: &str,
        reference: &str,
        ceiling: Option<&BrowserAuthority>,
    ) -> Result<(Transaction<'a>, Option<BrowserAuthentication>)> {
        let app = self.config.app(origin)?;
        if reference.len() != 64 || !reference.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::Unauthorized);
        }
        let tx = self
            .oauth
            .validate_portal_client_native_in(
                tx,
                &self.config.credentials(),
                app.join("/api/sso/callback")
                    .map_err(|_| Error::Configuration)?
                    .as_str(),
            )
            .await?;
        let Some(discovered) = record(&tx, reference, origin, &self.config.client_id).await? else {
            return Ok((tx, None));
        };
        let stored = self.reference_tokens(&discovered)?;
        // OAuth locks client→canonical identity→token before the sidecar fence.
        let (tx, claims) = self
            .oauth
            .introspect_portal_native_in(tx, &self.config.credentials(), &stored.access_token)
            .await?;
        let (tx, _) = self
            .native_principal_in(tx, &claims, Some(&discovered.user))
            .await?;
        if tx.query_typed_opt("SELECT token_hash FROM webdock_auth.identity_browser_session WHERE token_hash=$1 AND app_origin=$2 AND client_id=$3 FOR SHARE",&[(&reference,Type::TEXT),(&origin,Type::TEXT),(&self.config.client_id,Type::TEXT)]).await.map_err(|_|Error::Configuration)?.is_none(){return Ok((tx,None));}
        let Some(mut current) = record(&tx, reference, origin, &self.config.client_id)
            .await?
            .filter(|r| r.same_authority(&discovered))
        else {
            return Ok((tx, None));
        };
        current.expires = current.expires.min(discovered.expires);
        // The sidecar lock may wait: current OAuth/provider/canonical proof must be read again.
        let (tx, claims) = self
            .oauth
            .introspect_portal_native_in(tx, &self.config.credentials(), &stored.access_token)
            .await?;
        let (tx, principal) = self
            .native_principal_in(tx, &claims, Some(&current.user))
            .await?;
        let Some(fresh) = record(&tx, reference, origin, &self.config.client_id)
            .await?
            .filter(|r| r.same_authority(&current))
        else {
            return Ok((tx, None));
        };
        current.expires = current.expires.min(fresh.expires);
        let result =
            self.reference_authentication(&current, &stored, &claims, principal, ceiling)?;
        Ok((tx, Some(result)))
    }
    pub(super) async fn native_principal(
        &self,
        claims: &Value,
        expected: Option<&str>,
    ) -> Result<Principal> {
        tokio::time::timeout(BUDGET, async {
            let mut connection = self
                .native_store
                .as_ref()
                .ok_or(Error::Configuration)?
                .acquire()
                .await
                .map_err(|_| Error::Configuration)?;
            let tx = connection
                .transaction()
                .await
                .map_err(|_| Error::Configuration)?;
            let (tx, principal) = self.native_principal_in(tx, claims, expected).await?;
            tx.commit().await.map_err(|_| Error::Configuration)?;
            Ok(principal)
        })
        .await
        .map_err(|_| Error::Configuration)?
    }
    async fn native_principal_in<'a>(
        &self,
        tx: Transaction<'a>,
        claims: &Value,
        expected: Option<&str>,
    ) -> Result<(Transaction<'a>, Principal)> {
        let subject = self.claim_subject(claims, expected)?;
        let (tx, principal) = self
            .identity
            .authenticate_session_native_in(tx, subject, string(claims, "sid")?)
            .await?;
        self.claim_subject(claims, expected)?;
        Ok((
            tx,
            self.checked_principal(claims, principal.ok_or(Error::Unauthorized)?)?,
        ))
    }
}
