//! Native MCP record references; caller retains the returned checked transaction.
use super::{McpAuthentication, McpAuthority, hash_record, mcp_deadline, mcp_scope_policy};
use crate::service::{
    authenticate_client, native_client_store,
    native_introspection::{TokenRecord, discover, reload_fenced},
};
use crate::{ClientAuthentication, Error, OAuthService, Result, legacy_token_hash};
use serde_json::{Value, json};
use spitzli_identity::Principal;
use spitzli_persistence::postgres::Transaction;
use std::time::Duration;
const BUDGET: Duration = Duration::from_secs(30);
fn fingerprint(r: &TokenRecord) -> Result<String> {
    hash_record(json!([
        r.id,
        r.hash,
        r.client,
        r.user,
        r.session,
        r.code,
        r.refresh_id,
        r.scopes,
        r.resources,
        r.created,
        r.expires
    ]))
}
impl OAuthService {
    pub(crate) async fn native_authorize_mcp_reference(
        &self,
        credentials: &ClientAuthentication,
        token: &str,
        scope: &str,
    ) -> Result<McpAuthentication> {
        let operation = async {
            let mut connection = self
                .native_mcp_pool()
                .ok_or(Error::Configuration)?
                .acquire()
                .await
                .map_err(|_| Error::Configuration)?;
            let tx = connection
                .transaction()
                .await
                .map_err(|_| Error::Configuration)?;
            let (tx, actor) = self
                .authorize_mcp_reference_native_in(tx, credentials, token, scope)
                .await?;
            tx.commit().await.map_err(|_| Error::Configuration)?;
            Ok(actor)
        };
        tokio::time::timeout(BUDGET, operation)
            .await
            .map_err(|_| Error::Configuration)?
    }
    /// Trusted backend only; keep the returned client/identity/access/resource fences through commit.
    pub async fn authorize_mcp_reference_native_in<'a>(
        &self,
        tx: Transaction<'a>,
        credentials: &ClientAuthentication,
        token: &str,
        scope: &str,
    ) -> Result<(Transaction<'a>, McpAuthentication)> {
        let operation = async move {
            if token.is_empty() || token.len() > 16384 {
                return Err(Error::AccessDenied);
            }
            let caller = native_client_store::load_client(&tx, &credentials.client_id).await?;
            authenticate_client(&caller, credentials)?;
            let hash = legacy_token_hash(token);
            let record = discover(&tx, &hash, false)
                .await?
                .ok_or(Error::AccessDenied)?;
            let record = reload_fenced(&tx, record, false)
                .await?
                .ok_or(Error::AccessDenied)?;
            let authority = McpAuthority {
                v: 1,
                access_id: record.id.clone(),
                token_hash: hash.clone(),
                record_hash: fingerprint(&record)?,
                caller_id: caller.id.clone(),
                issuer_client_id: record.client.clone(),
                subject: record.user.clone().ok_or(Error::AccessDenied)?,
                session_id: record.session.clone().ok_or(Error::AccessDenied)?,
                resource: self.config.mcp_resource.to_string(),
                required_scope: scope.into(),
                write: false,
                role: String::new(),
                issued_at: record.created.timestamp(),
                expires_at: record.expires.timestamp(),
            };
            let (tx, claims) = self
                .inspect_native_record(tx, record, &hash, false, Some(&caller.id), false)
                .await?;
            let write = self.native_mcp_scope(&tx, &claims, scope).await?;
            let (tx, principal) = self
                .identity
                .authenticate_session_native_in(tx, &authority.subject, &authority.session_id)
                .await?;
            let principal = principal.ok_or(Error::AccessDenied)?;
            if !principal.can_access() || principal.in_tenant_preview() {
                return Err(Error::AccessDenied);
            }
            mcp_scope_policy(&claims, self.config.mcp_resource.as_str(), scope)?;
            mcp_deadline(&claims)?;
            let authority = McpAuthority {
                write,
                role: claims["webdock_role"]
                    .as_str()
                    .ok_or(Error::AccessDenied)?
                    .into(),
                ..authority
            };
            Ok((
                tx,
                McpAuthentication {
                    principal,
                    authority,
                },
            ))
        };
        tokio::time::timeout(BUDGET, operation)
            .await
            .map_err(|_| Error::Configuration)?
    }
    async fn native_mcp_scope(
        &self,
        tx: &Transaction<'_>,
        claims: &Value,
        scope: &str,
    ) -> Result<bool> {
        let write = mcp_scope_policy(claims, self.config.mcp_resource.as_str(), scope)?;
        let issuer = native_client_store::load_client(
            tx,
            claims["client_id"].as_str().ok_or(Error::AccessDenied)?,
        )
        .await?;
        if issuer.metadata["webdock_mcp"] != true && issuer.metadata["webdock_hosting"] != true {
            return Err(Error::AccessDenied);
        }
        mcp_deadline(claims)?;
        Ok(write)
    }
    /// Server-owned reference only. No raw identity or cached-role proof is admitted.
    pub async fn revalidate_mcp_authority_native_in<'a>(
        &self,
        tx: Transaction<'a>,
        authority: &McpAuthority,
    ) -> Result<(Transaction<'a>, Principal)> {
        let operation = async move {
            if authority.v != 1
                || authority.resource != self.config.mcp_resource.as_str()
                || authority.token_hash.len() != 43
                || authority.record_hash.len() != 43
                || authority.expires_at <= chrono::Utc::now().timestamp()
            {
                return Err(Error::AccessDenied);
            }
            native_client_store::load_client(&tx, &authority.caller_id).await?;
            let record = discover(&tx, &authority.token_hash, false)
                .await?
                .ok_or(Error::AccessDenied)?;
            let record = reload_fenced(&tx, record, false)
                .await?
                .ok_or(Error::AccessDenied)?;
            if record.id != authority.access_id || fingerprint(&record)? != authority.record_hash {
                return Err(Error::AccessDenied);
            }
            let (tx, claims) = self
                .inspect_native_record(
                    tx,
                    record,
                    &authority.token_hash,
                    false,
                    Some(&authority.caller_id),
                    false,
                )
                .await?;
            let write = self
                .native_mcp_scope(&tx, &claims, &authority.required_scope)
                .await?;
            if claims["sub"] != authority.subject
                || claims["sid"] != authority.session_id
                || claims["client_id"] != authority.issuer_client_id
                || claims["webdock_role"] != authority.role
                || claims["iat"] != authority.issued_at
                || claims["exp"] != authority.expires_at
                || write != authority.write
            {
                return Err(Error::AccessDenied);
            }
            let (tx, principal) = self
                .identity
                .authenticate_session_native_in(tx, &authority.subject, &authority.session_id)
                .await?;
            let principal = principal.ok_or(Error::AccessDenied)?;
            if !principal.can_access() || principal.in_tenant_preview() {
                return Err(Error::AccessDenied);
            }
            mcp_scope_policy(
                &claims,
                self.config.mcp_resource.as_str(),
                &authority.required_scope,
            )?;
            mcp_deadline(&claims)?;
            Ok((tx, principal))
        };
        tokio::time::timeout(BUDGET, operation)
            .await
            .map_err(|_| Error::Configuration)?
    }
}
