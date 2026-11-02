//! Server-owned MCP access references. No raw token, synthetic session or portal preview grant.
use super::*;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpAuthority {
    v: u8,
    access_id: String,
    token_hash: String,
    record_hash: String,
    caller_id: String,
    issuer_client_id: String,
    subject: String,
    session_id: String,
    resource: String,
    required_scope: String,
    write: bool,
    role: String,
    issued_at: i64,
    expires_at: i64,
}
impl McpAuthority {
    pub fn registry_scope(&self) -> bool {
        self.required_scope.starts_with("webdock:")
    }
    pub fn can_write(&self) -> bool {
        self.write
    }
    pub fn subject(&self) -> &str {
        &self.subject
    }
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    /// Original Unix expiry; use only with verified authority and a retained guard.
    pub fn expires_at(&self) -> i64 {
        self.expires_at
    }
}
/// Request-owned authentication transaction; retain until the domain commits.
/// Never cache this guard: it holds row locks and a database connection.
pub struct McpGuard {
    service: std::sync::Arc<OAuthService>,
    tx: DatabaseTransaction,
    authority: McpAuthority,
    principal: Principal,
}
impl McpGuard {
    pub fn principal(&self) -> &Principal {
        &self.principal
    }
    pub fn can_write(&self) -> bool {
        self.authority.can_write()
    }
    pub async fn verify(&self) -> Result<()> {
        let current = self
            .service
            .revalidate_mcp_authority_in(&self.tx, &self.authority)
            .await?;
        if current.user_id != self.principal.user_id
            || current.session_id != self.principal.session_id
        {
            return Err(Error::AccessDenied);
        }
        Ok(())
    }
}
pub struct McpAuthentication {
    pub principal: Principal,
    pub authority: McpAuthority,
}
fn record_hash(row: &sea_orm::QueryResult) -> Result<String> {
    let value = json!([
        row.try_get::<String>("", "id")?,
        row.try_get::<String>("", "token")?,
        row.try_get::<String>("", "clientId")?,
        row.try_get::<Option<String>>("", "userId")?,
        row.try_get::<Option<String>>("", "sessionId")?,
        row.try_get::<Option<String>>("", "authorizationCodeId")?,
        row.try_get::<Option<String>>("", "refreshId")?,
        row.try_get::<Value>("", "scopes")?,
        row.try_get::<Option<Value>>("", "resources")?,
        row.try_get::<DateTime<Utc>>("", "createdAt")?,
        row.try_get::<DateTime<Utc>>("", "expiresAt")?
    ]);
    Ok(URL_SAFE_NO_PAD.encode(Sha256::digest(
        serde_json::to_vec(&value).map_err(|_| Error::InvalidRequest)?,
    )))
}
impl OAuthService {
    pub async fn begin_mcp_authority(
        self: &std::sync::Arc<Self>,
        authority: &McpAuthority,
    ) -> Result<McpGuard> {
        let tx = self.db.begin().await?;
        tx.execute_unprepared("SET LOCAL lock_timeout = '5s'; SET LOCAL statement_timeout = '10s'")
            .await?;
        let principal = self.revalidate_mcp_authority_in(&tx, authority).await?;
        Ok(McpGuard {
            service: self.clone(),
            tx,
            authority: authority.clone(),
            principal,
        })
    }
    async fn mcp_scope(
        &self,
        tx: &DatabaseTransaction,
        claims: &Value,
        scope: &str,
    ) -> Result<bool> {
        let (read, write, registry) = match scope {
            "webdock:read" => ("webdock:read", false, true),
            "webdock:write" => ("webdock:read", true, true),
            "hosting:read" => ("hosting:read", false, false),
            "hosting:write" => ("hosting:read", true, false),
            _ => return Err(Error::InvalidScope),
        };
        let role = claims["webdock_role"].as_str().unwrap_or("");
        let scopes = claims["scope"]
            .as_str()
            .unwrap_or("")
            .split_whitespace()
            .collect::<Vec<_>>();
        let write_scope = if registry {
            "webdock:write"
        } else {
            "hosting:write"
        };
        if claims["active"] != true
            || claims["token_type"] != "Bearer"
            || !audiences(claims).contains(&self.config.mcp_resource.as_str().to_owned())
            || (registry && role != "operator")
            || (!registry && !["hosting-operator", "hosting-customer"].contains(&role))
            || (!registry && scopes.iter().any(|scope| scope.starts_with("webdock:")))
            || !scopes.contains(&read)
            || (write && !scopes.contains(&write_scope))
        {
            return Err(Error::AccessDenied);
        }
        let issuer =
            load_client(tx, claims["client_id"].as_str().ok_or(Error::AccessDenied)?).await?;
        if issuer.metadata["webdock_mcp"] != true && issuer.metadata["webdock_hosting"] != true {
            return Err(Error::AccessDenied);
        }
        let iat = claims["iat"].as_i64().ok_or(Error::AccessDenied)?;
        let exp = claims["exp"].as_i64().ok_or(Error::AccessDenied)?;
        if !(1..=300).contains(&exp.saturating_sub(iat))
            || iat > Utc::now().timestamp()
            || exp <= Utc::now().timestamp()
        {
            return Err(Error::AccessDenied);
        }
        Ok(scopes.contains(&write_scope))
    }
    /// Authenticate an access token into an opaque reference backed by an exact retained token row.
    /// Stateless legacy JWTs without an access row cannot become durable native authority references.
    pub async fn authorize_mcp_reference(
        &self,
        credentials: &ClientAuthentication,
        token: &str,
        required_scope: &str,
    ) -> Result<McpAuthentication> {
        if token.is_empty() || token.len() > 16384 {
            return Err(Error::AccessDenied);
        }
        let tx = self.db.begin().await?;
        let caller = load_client(&tx, &credentials.client_id).await?;
        authenticate_client(&caller, credentials)?;
        let hash = legacy_token_hash(token);
        let row = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT * FROM webdock_auth.\"oauthAccessToken\" WHERE token=$1",
                [hash.clone().into()],
            ))
            .await?
            .ok_or(Error::AccessDenied)?;
        let row = Self::lock_access_record(&tx, row)
            .await?
            .ok_or(Error::AccessDenied)?;
        let authority = McpAuthority {
            v: 1,
            access_id: row.try_get("", "id")?,
            token_hash: hash,
            record_hash: record_hash(&row)?,
            caller_id: caller.id.clone(),
            issuer_client_id: row.try_get("", "clientId")?,
            subject: row
                .try_get::<Option<String>>("", "userId")?
                .ok_or(Error::AccessDenied)?,
            session_id: row
                .try_get::<Option<String>>("", "sessionId")?
                .ok_or(Error::AccessDenied)?,
            resource: self.config.mcp_resource.to_string(),
            required_scope: required_scope.into(),
            write: false,
            role: String::new(),
            issued_at: row.try_get::<DateTime<Utc>>("", "createdAt")?.timestamp(),
            expires_at: row.try_get::<DateTime<Utc>>("", "expiresAt")?.timestamp(),
        };
        let claims = self
            .inspect_token_record(&tx, Some(row), None, Some(&caller.id), false)
            .await?;
        let write = self.mcp_scope(&tx, &claims, required_scope).await?;
        let principal = self
            .identity
            .authenticate_session_in(&tx, &authority.subject, &authority.session_id)
            .await?
            .ok_or(Error::AccessDenied)?;
        if principal.in_tenant_preview() {
            return Err(Error::AccessDenied);
        }
        let authority = McpAuthority {
            write,
            role: claims["webdock_role"]
                .as_str()
                .ok_or(Error::AccessDenied)?
                .into(),
            ..authority
        };
        tx.commit().await?;
        Ok(McpAuthentication {
            principal,
            authority,
        })
    }
    /// Backend-only: the caller retains this bootstrap transaction until its domain commit.
    /// Reuses normal access-record inspection with public MCP preview denial and live resource policy.
    pub async fn revalidate_mcp_authority_in(
        &self,
        tx: &DatabaseTransaction,
        authority: &McpAuthority,
    ) -> Result<Principal> {
        if authority.v != 1
            || authority.resource != self.config.mcp_resource.as_str()
            || authority.token_hash.len() != 43
            || authority.record_hash.len() != 43
            || authority.expires_at <= Utc::now().timestamp()
        {
            return Err(Error::AccessDenied);
        }
        let _caller = load_client(tx, &authority.caller_id).await?;
        let row = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT * FROM webdock_auth.\"oauthAccessToken\" WHERE id=$1 AND token=$2",
                [
                    authority.access_id.clone().into(),
                    authority.token_hash.clone().into(),
                ],
            ))
            .await?
            .ok_or(Error::AccessDenied)?;
        let row = Self::lock_access_record(tx, row)
            .await?
            .ok_or(Error::AccessDenied)?;
        if record_hash(&row)? != authority.record_hash {
            return Err(Error::AccessDenied);
        }
        let claims = self
            .inspect_token_record(tx, Some(row), None, Some(&authority.caller_id), false)
            .await?;
        let write = self
            .mcp_scope(tx, &claims, &authority.required_scope)
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
        let principal = self
            .identity
            .authenticate_session_in(tx, &authority.subject, &authority.session_id)
            .await?
            .ok_or(Error::AccessDenied)?;
        if principal.in_tenant_preview() {
            return Err(Error::AccessDenied);
        }
        Ok(principal)
    }
}
