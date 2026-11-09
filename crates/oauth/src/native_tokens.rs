//! Native token issuance/rotation using the existing engine and offline signing keys.
use super::{
    Issuance, OAuthService, authenticate_client, date, json_strings, native_client_store,
    native_grants::{RequestRecord, live_deadline},
    scope_vec,
};
use crate::{
    AuthorizationRequest, CodeExchange, Error, RefreshExchange, Result, TokenResponse,
    engine::{Engine, grant},
    legacy_token_hash,
    types::GrantContext,
};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use std::time::Duration;
use tokio_postgres::types::Type;
use webdock_persistence::postgres::Transaction;
const BUDGET: Duration = Duration::from_secs(30);
struct Issued {
    response: TokenResponse,
    signing_key: Option<String>,
}
async fn code(tx: &Transaction<'_>, hash: &str, client: &str) -> Result<Option<RequestRecord>> {
    let row=tx.query_typed_opt("SELECT payload,user_id,session_id,client_id,expires_at FROM webdock_auth.identity_oauth_request WHERE token_hash=$1 AND kind='code' AND client_id=$2 AND expires_at>statement_timestamp()",&[(&hash,Type::TEXT),(&client,Type::TEXT)]).await.map_err(|_|Error::Configuration)?;
    row.map(|r| -> std::result::Result<_, tokio_postgres::Error> {
        Ok(RequestRecord {
            payload: r.try_get("payload")?,
            user: r.try_get("user_id")?,
            session: r.try_get("session_id")?,
            client: r.try_get("client_id")?,
            expires: r.try_get("expires_at")?,
        })
    })
    .transpose()
    .map_err(|_| Error::Configuration)
}
struct RefreshRecord {
    id: String,
    user: String,
    session: Option<String>,
    code: Option<String>,
    resources: Option<Value>,
    scopes: Value,
    expires: DateTime<Utc>,
    created: DateTime<Utc>,
    auth_time: Option<DateTime<Utc>>,
    rotated: Option<DateTime<Utc>>,
    revoked: Option<DateTime<Utc>>,
    alive: bool,
}
impl RefreshRecord {
    fn same(&self, r: &Self) -> bool {
        self.id == r.id
            && self.user == r.user
            && self.session == r.session
            && self.code == r.code
            && self.resources == r.resources
            && self.scopes == r.scopes
            && self.expires == r.expires
            && self.created == r.created
            && self.auth_time == r.auth_time
    }
}
async fn refresh_record(
    tx: &Transaction<'_>,
    hash: &str,
    client: &str,
) -> Result<Option<RefreshRecord>> {
    let row=tx.query_typed_opt(r#"SELECT id,"userId","sessionId","authorizationCodeId",resources,scopes,"expiresAt","createdAt","authTime","rotatedAt",revoked,(revoked IS NULL AND "expiresAt">statement_timestamp()) AS alive FROM webdock_auth."oauthRefreshToken" WHERE token=$1 AND "clientId"=$2"#,&[(&hash,Type::TEXT),(&client,Type::TEXT)]).await.map_err(|_|Error::Configuration)?;
    row.map(|r| -> std::result::Result<_, tokio_postgres::Error> {
        Ok(RefreshRecord {
            id: r.try_get("id")?,
            user: r.try_get("userId")?,
            session: r.try_get("sessionId")?,
            code: r.try_get("authorizationCodeId")?,
            resources: r.try_get("resources")?,
            scopes: r.try_get("scopes")?,
            expires: r.try_get("expiresAt")?,
            created: r.try_get("createdAt")?,
            auth_time: r.try_get("authTime")?,
            rotated: r.try_get("rotatedAt")?,
            revoked: r.try_get("revoked")?,
            alive: r.try_get("alive")?,
        })
    })
    .transpose()
    .map_err(|_| Error::Configuration)
}
async fn now(tx: &Transaction<'_>) -> Result<i64> {
    tx.query_typed_one(
        "SELECT floor(extract(epoch FROM clock_timestamp()))::bigint AS now",
        &[],
    )
    .await
    .map_err(|_| Error::Configuration)?
    .try_get("now")
    .map_err(|_| Error::Configuration)
}
async fn session_time(tx: &Transaction<'_>, id: &str) -> Result<i64> {
    tx.query_typed_opt(r#"SELECT floor(extract(epoch FROM "createdAt"))::bigint AS auth_time FROM webdock_auth.session WHERE id=$1"#,&[(&id,Type::TEXT)]).await.map_err(|_|Error::Configuration)?.ok_or(Error::AccessDenied)?.try_get("auth_time").map_err(|_|Error::Configuration)
}
async fn key_live(tx: &Transaction<'_>, id: &str) -> Result<()> {
    let row=tx.query_typed_opt(r#"SELECT id FROM webdock_auth.jwks WHERE id=$1 AND(alg='RS256' OR alg IS NULL) AND("expiresAt" IS NULL OR "expiresAt">clock_timestamp())"#,&[(&id,Type::TEXT)]).await.map_err(|_|Error::Configuration)?;
    if row.is_none() {
        return Err(Error::Crypto);
    }
    Ok(())
}
impl OAuthService {
    async fn native_sign(&self, tx: &Transaction<'_>, claims: &Value) -> Result<(String, String)> {
        let row=tx.query_typed_opt(r#"SELECT id FROM webdock_auth.jwks WHERE(alg='RS256' OR alg IS NULL) AND("expiresAt" IS NULL OR "expiresAt">statement_timestamp()) ORDER BY "createdAt" DESC,id DESC LIMIT 1"#,&[]).await.map_err(|_|Error::Configuration)?.ok_or(Error::Crypto)?;
        let id: String = row.try_get("id").map_err(|_| Error::Configuration)?;
        if tx
            .query_typed_opt(
                "SELECT id FROM webdock_auth.jwks WHERE id=$1 FOR SHARE",
                &[(&id, Type::TEXT)],
            )
            .await
            .map_err(|_| Error::Configuration)?
            .is_none()
        {
            return Err(Error::Crypto);
        }
        // Eligibility and encrypted material are fresh after any key-lock wait.
        let row=tx.query_typed_opt(r#"SELECT "privateKey" FROM webdock_auth.jwks WHERE id=$1 AND(alg='RS256' OR alg IS NULL) AND("expiresAt" IS NULL OR "expiresAt">clock_timestamp())"#,&[(&id,Type::TEXT)]).await.map_err(|_|Error::Configuration)?.ok_or(Error::Crypto)?;
        let encrypted: String = row
            .try_get("privateKey")
            .map_err(|_| Error::Configuration)?;
        Ok((
            crate::signing::sign_encrypted(&self.keys, &encrypted, &id, claims)?,
            id,
        ))
    }
    async fn native_persist_tokens(
        &self,
        tx: &Transaction<'_>,
        issuance: Issuance<'_>,
    ) -> Result<Issued> {
        let scopes = scope_vec(&issuance.context.request.scope)?;
        let claims = self.id_token_claims(&issuance, &scopes)?;
        let Issuance {
            client,
            principal,
            context,
            engine,
            current,
            ttl,
            refresh_ttl,
            code_id,
        } = issuance;
        let tokens = engine.tokens.issued.as_ref().ok_or(Error::InvalidGrant)?;
        let created = date(current)?;
        let access_exp = date(current + ttl)?;
        let refresh_exp = date(current + refresh_ttl)?;
        let auth_time = date(context.auth_time)?;
        let resources = json!(context.request.resources);
        let scope_values = json!(scopes);
        let refresh_id = if let Some(token) = &tokens.refresh {
            let id = self.allocate_id().await?;
            let hash = legacy_token_hash(token);
            tx.execute_typed(r#"INSERT INTO webdock_auth."oauthRefreshToken"(id,token,"clientId","sessionId","userId","authorizationCodeId",resources,scopes,"expiresAt","createdAt","authTime") VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)"#,&[(&id,Type::TEXT),(&hash,Type::TEXT),(&client.id,Type::TEXT),(&principal.session_id,Type::TEXT),(&principal.user_id,Type::TEXT),(&code_id,Type::TEXT),(&resources,Type::JSONB),(&scope_values,Type::JSONB),(&refresh_exp,Type::TIMESTAMPTZ),(&created,Type::TIMESTAMPTZ),(&auth_time,Type::TIMESTAMPTZ)]).await.map_err(|_|Error::Configuration)?;
            Some(id)
        } else {
            None
        };
        let id = self.allocate_id().await?;
        let hash = legacy_token_hash(&tokens.token);
        tx.execute_typed(r#"INSERT INTO webdock_auth."oauthAccessToken"(id,token,"clientId","sessionId","userId","authorizationCodeId","refreshId",resources,scopes,"expiresAt","createdAt") VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)"#,&[(&id,Type::TEXT),(&hash,Type::TEXT),(&client.id,Type::TEXT),(&principal.session_id,Type::TEXT),(&principal.user_id,Type::TEXT),(&code_id,Type::TEXT),(&refresh_id,Type::TEXT),(&resources,Type::JSONB),(&scope_values,Type::JSONB),(&access_exp,Type::TIMESTAMPTZ),(&created,Type::TIMESTAMPTZ)]).await.map_err(|_|Error::Configuration)?;
        let (id_token, signing_key) = if let Some(claims) = claims {
            let (token, key) = self.native_sign(tx, &claims).await?;
            (Some(token), Some(key))
        } else {
            (None, None)
        };
        Ok(Issued {
            response: TokenResponse {
                access_token: tokens.token.clone(),
                token_type: "Bearer".into(),
                expires_in: ttl,
                scope: context.request.scope.clone(),
                refresh_token: tokens.refresh.clone(),
                id_token,
            },
            signing_key,
        })
    }
    pub(crate) async fn native_exchange_code(&self, input: CodeExchange) -> Result<TokenResponse> {
        let operation = async {
            let mut connection = self
                .native_tokens_pool()
                .ok_or(Error::Configuration)?
                .acquire()
                .await
                .map_err(|_| Error::Configuration)?;
            let tx = connection
                .transaction()
                .await
                .map_err(|_| Error::Configuration)?;
            let client = native_client_store::load_client(&tx, &input.client.client_id).await?;
            authenticate_client(&client, &input.client)?;
            let hash = legacy_token_hash(&input.code);
            let initial = code(&tx, &hash, &client.id)
                .await?
                .ok_or(Error::InvalidGrant)?;
            let context = initial.context()?;
            let (tx, _) = self
                .grant_actor(tx, &context.subject, &context.session_id, &client)
                .await?;
            if tx.query_typed_opt("SELECT token_hash FROM webdock_auth.identity_oauth_request WHERE token_hash=$1 AND kind='code' AND client_id=$2 AND user_id=$3 AND session_id=$4 FOR UPDATE",&[(&hash,Type::TEXT),(&client.id,Type::TEXT),(&context.subject,Type::TEXT),(&context.session_id,Type::TEXT)]).await.map_err(|_|Error::Configuration)?.is_none(){return Err(Error::InvalidGrant);}
            let record = code(&tx, &hash, &client.id)
                .await?
                .filter(|r| r.same(&initial))
                .ok_or(Error::InvalidGrant)?;
            let context = record.context()?;
            if !client.grants.iter().any(|g| g == "authorization_code") {
                return Err(Error::InvalidGrant);
            }
            let scopes = scope_vec(&context.request.scope)?;
            let (ttl, refresh_ttl) = self
                .native_resources(&tx, &client, &context.request.resources, &scopes)
                .await?;
            let (tx, principal) = self
                .grant_actor(tx, &context.subject, &context.session_id, &client)
                .await?;
            live_deadline(&tx, initial.expires).await?;
            let current = now(&tx).await?;
            let mut engine = Engine::new(
                &client,
                date(current + ttl)?,
                client.grants.iter().any(|g| g == "refresh_token")
                    && scopes.iter().any(|s| s == "offline_access"),
            )?;
            engine.load_code(&input.code, grant(&context, record.expires)?);
            engine.exchange(&input)?;
            let issued = self
                .native_persist_tokens(
                    &tx,
                    Issuance {
                        client: &client,
                        principal: &principal,
                        context: &context,
                        engine: &engine,
                        current,
                        ttl,
                        refresh_ttl,
                        code_id: Some(hash.clone()),
                    },
                )
                .await?;
            if tx.execute_typed("DELETE FROM webdock_auth.identity_oauth_request WHERE token_hash=$1 AND kind='code' AND client_id=$2 AND expires_at>statement_timestamp()",&[(&hash,Type::TEXT),(&client.id,Type::TEXT)]).await.map_err(|_|Error::Configuration)?!=1{return Err(Error::InvalidGrant);}
            let (tx, _) = self
                .grant_actor(tx, &context.subject, &context.session_id, &client)
                .await?;
            live_deadline(&tx, initial.expires).await?;
            if let Some(id) = &issued.signing_key {
                key_live(&tx, id).await?;
            }
            tx.commit().await.map_err(|_| Error::Configuration)?;
            Ok(issued.response)
        };
        tokio::time::timeout(BUDGET, operation)
            .await
            .map_err(|_| Error::Configuration)?
    }
    pub(crate) async fn native_refresh(&self, input: RefreshExchange) -> Result<TokenResponse> {
        let operation = async {
            let mut connection = self
                .native_tokens_pool()
                .ok_or(Error::Configuration)?
                .acquire()
                .await
                .map_err(|_| Error::Configuration)?;
            let tx = connection
                .transaction()
                .await
                .map_err(|_| Error::Configuration)?;
            let client = native_client_store::load_client(&tx, &input.client.client_id).await?;
            authenticate_client(&client, &input.client)?;
            if !client.grants.iter().any(|g| g == "refresh_token")
                || client.metadata.get("webdock_operator_only").is_some()
            {
                return Err(Error::InvalidGrant);
            }
            let hash = legacy_token_hash(&input.refresh_token);
            let initial = refresh_record(&tx, &hash, &client.id)
                .await?
                .ok_or(Error::InvalidGrant)?;
            if initial.rotated.is_none() && (!initial.alive || initial.revoked.is_some()) {
                return Err(Error::InvalidGrant);
            }
            if tx
                .query_typed_opt(
                    r#"SELECT id FROM webdock_auth."user" WHERE id=$1 FOR UPDATE"#,
                    &[(&initial.user, Type::TEXT)],
                )
                .await
                .map_err(|_| Error::Configuration)?
                .is_none()
            {
                return Err(Error::InvalidGrant);
            }
            if let Some(session) = &initial.session {
                tx.query_typed_opt(
                    r#"SELECT id FROM webdock_auth.session WHERE id=$1 AND "userId"=$2 FOR SHARE"#,
                    &[(session, Type::TEXT), (&initial.user, Type::TEXT)],
                )
                .await
                .map_err(|_| Error::Configuration)?;
            }
            if tx.query_typed_opt(r#"SELECT id FROM webdock_auth."oauthRefreshToken" WHERE id=$1 AND token=$2 AND "clientId"=$3 FOR UPDATE"#,&[(&initial.id,Type::TEXT),(&hash,Type::TEXT),(&client.id,Type::TEXT)]).await.map_err(|_|Error::Configuration)?.is_none(){return Err(Error::InvalidGrant);}
            let record = refresh_record(&tx, &hash, &client.id)
                .await?
                .filter(|r| r.same(&initial))
                .ok_or(Error::InvalidGrant)?;
            if record.rotated.is_some() {
                for table in ["oauthAccessToken", "oauthRefreshToken"] {
                    let sql = format!(
                        r#"UPDATE webdock_auth."{table}" SET revoked=statement_timestamp() WHERE "clientId"=$1 AND "userId"=$2"#
                    );
                    tx.execute_typed(
                        &sql,
                        &[(&client.id, Type::TEXT), (&record.user, Type::TEXT)],
                    )
                    .await
                    .map_err(|_| Error::Configuration)?;
                }
                tx.commit().await.map_err(|_| Error::Configuration)?;
                return Err(Error::InvalidGrant);
            }
            if !record.alive || record.revoked.is_some() || !initial.alive {
                return Err(Error::InvalidGrant);
            }
            let session = record.session.as_deref().ok_or(Error::InvalidGrant)?;
            let (tx, _) = self.grant_actor(tx, &record.user, session, &client).await?;
            let original_scopes = json_strings(record.scopes.clone())?;
            let resources = json_strings(record.resources.clone().unwrap_or(json!([])))?;
            let scopes = input
                .scope
                .as_ref()
                .map(|s| scope_vec(s))
                .transpose()?
                .unwrap_or_else(|| original_scopes.clone());
            if scopes.iter().any(|s| !original_scopes.contains(s)) {
                return Err(Error::InvalidScope);
            }
            let (ttl, refresh_ttl) = self
                .native_resources(&tx, &client, &resources, &scopes)
                .await?;
            let (tx, principal) = self.grant_actor(tx, &record.user, session, &client).await?;
            live_deadline(&tx, record.expires).await?;
            let current = now(&tx).await?;
            let mut request = AuthorizationRequest::code(
                client.id.clone(),
                client.redirects[0].clone(),
                original_scopes.join(" "),
                "x".repeat(43),
            );
            request.resources = resources;
            let auth_time = match record.auth_time {
                Some(t) => t.timestamp(),
                None => session_time(&tx, session).await?,
            };
            let mut context = GrantContext {
                subject: record.user.clone(),
                session_id: session.into(),
                request,
                auth_time,
            };
            let mut engine = Engine::new(&client, date(current + ttl)?, true)?;
            engine.load_refresh(&input.refresh_token, grant(&context, record.expires)?);
            engine.refresh(&input)?;
            context.request.scope = scopes.join(" ");
            if tx.execute_typed(r#"UPDATE webdock_auth."oauthRefreshToken" SET revoked=statement_timestamp(),"rotatedAt"=statement_timestamp() WHERE id=$1 AND token=$2 AND "clientId"=$3"#,&[(&record.id,Type::TEXT),(&hash,Type::TEXT),(&client.id,Type::TEXT)]).await.map_err(|_|Error::Configuration)?!=1{return Err(Error::Configuration);}
            let issued = self
                .native_persist_tokens(
                    &tx,
                    Issuance {
                        client: &client,
                        principal: &principal,
                        context: &context,
                        engine: &engine,
                        current,
                        ttl,
                        refresh_ttl,
                        code_id: record.code.clone(),
                    },
                )
                .await?;
            let (tx, _) = self.grant_actor(tx, &record.user, session, &client).await?;
            live_deadline(&tx, record.expires).await?;
            if let Some(id) = &issued.signing_key {
                key_live(&tx, id).await?;
            }
            tx.commit().await.map_err(|_| Error::Configuration)?;
            Ok(issued.response)
        };
        tokio::time::timeout(BUDGET, operation)
            .await
            .map_err(|_| Error::Configuration)?
    }
}
