//! Native request/code/consent authority. Token exchange and refresh stay explicit.
use super::{OAuthService, json_strings, native_client_store, scope_vec};
use crate::{
    AuthorizationOutcome, AuthorizationRequest, ConsentRequest, Error, Result,
    engine::Engine,
    legacy_token_hash, random_token,
    types::{Client, GrantContext},
};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use spitzli_identity::Principal;
use spitzli_persistence::postgres::Transaction;
use std::time::Duration;
use tokio_postgres::types::Type;
const BUDGET: Duration = Duration::from_secs(30);
pub(super) struct RequestRecord {
    pub(super) payload: Value,
    pub(super) user: String,
    pub(super) session: String,
    pub(super) client: String,
    pub(super) expires: DateTime<Utc>,
}
impl RequestRecord {
    pub(super) fn context(&self) -> Result<GrantContext> {
        let c: GrantContext =
            serde_json::from_value(self.payload.clone()).map_err(|_| Error::InvalidGrant)?;
        if c.subject != self.user
            || c.session_id != self.session
            || c.request.client_id != self.client
        {
            return Err(Error::InvalidGrant);
        }
        Ok(c)
    }
    pub(super) fn same(&self, r: &Self) -> bool {
        self.payload == r.payload
            && self.user == r.user
            && self.session == r.session
            && self.client == r.client
            && self.expires == r.expires
    }
}
async fn request(
    tx: &Transaction<'_>,
    hash: &str,
    user: &str,
    session: &str,
) -> Result<Option<RequestRecord>> {
    let row=tx.query_typed_opt("SELECT payload,user_id,session_id,client_id,expires_at FROM webdock_auth.identity_oauth_request WHERE token_hash=$1 AND kind='consent' AND user_id=$2 AND session_id=$3 AND expires_at>statement_timestamp()",&[(&hash,Type::TEXT),(&user,Type::TEXT),(&session,Type::TEXT)]).await.map_err(|_|Error::Configuration)?;
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
async fn store(tx: &Transaction<'_>, token: &str, kind: &str, c: &GrantContext) -> Result<()> {
    tx.execute_typed("WITH expired AS (SELECT token_hash FROM webdock_auth.identity_oauth_request WHERE expires_at<=statement_timestamp() ORDER BY expires_at,token_hash LIMIT 100 FOR UPDATE SKIP LOCKED) DELETE FROM webdock_auth.identity_oauth_request r USING expired e WHERE r.token_hash=e.token_hash",&[]).await.map_err(|_|Error::Configuration)?;
    let hash = legacy_token_hash(token);
    let payload = serde_json::to_value(c).map_err(|_| Error::InvalidRequest)?;
    if tx.execute_typed("INSERT INTO webdock_auth.identity_oauth_request(token_hash,kind,user_id,session_id,client_id,payload,expires_at) VALUES($1,$2,$3,$4,$5,$6,statement_timestamp()+interval '5 minutes')",&[(&hash,Type::TEXT),(&kind,Type::TEXT),(&c.subject,Type::TEXT),(&c.session_id,Type::TEXT),(&c.request.client_id,Type::TEXT),(&payload,Type::JSONB)]).await.map_err(|_|Error::Configuration)?!=1{return Err(Error::Configuration);}
    Ok(())
}
pub(super) async fn live_deadline(tx: &Transaction<'_>, expires: DateTime<Utc>) -> Result<()> {
    let live: bool = tx
        .query_typed_one(
            "SELECT $1::timestamptz>clock_timestamp() AS live",
            &[(&expires, Type::TIMESTAMPTZ)],
        )
        .await
        .map_err(|_| Error::Configuration)?
        .try_get("live")
        .map_err(|_| Error::Configuration)?;
    if !live {
        return Err(Error::InvalidGrant);
    }
    Ok(())
}
async fn consent_exists(tx: &Transaction<'_>, client: &str, c: &GrantContext) -> Result<bool> {
    let scopes = scope_vec(&c.request.scope)?;
    for r in tx.query_typed(r#"SELECT scopes,resources FROM webdock_auth."oauthConsent" WHERE "clientId"=$1 AND "userId"=$2"#,&[(&client,Type::TEXT),(&c.subject,Type::TEXT)]).await.map_err(|_|Error::Configuration)?{
        let allowed=json_strings(r.try_get("scopes").map_err(|_|Error::Configuration)?)?;
        let resources=json_strings(r.try_get::<_,Option<Value>>("resources").map_err(|_|Error::Configuration)?.unwrap_or(json!([])))?;
        if scopes.iter().all(|s|allowed.contains(s))&&c.request.resources.iter().all(|r|resources.contains(r)){return Ok(true);}
    }
    Ok(false)
}
fn details(id: &str, client: &Client, c: &GrantContext) -> Result<ConsentRequest> {
    Ok(ConsentRequest {
        request_id: id.into(),
        client_id: client.id.clone(),
        client_name: client.name.clone(),
        scopes: scope_vec(&c.request.scope)?,
        resources: c.request.resources.clone(),
    })
}
impl OAuthService {
    pub(super) async fn grant_actor<'a>(
        &self,
        tx: Transaction<'a>,
        user: &str,
        session: &str,
        client: &Client,
    ) -> Result<(Transaction<'a>, Principal)> {
        let (tx, p) = self.native_principal(tx, user, session, false).await?;
        let p = p.ok_or(Error::AccessDenied)?;
        self.native_claims(&tx, client, &p, false).await?;
        Ok((tx, p))
    }
    pub(crate) async fn native_authorize(
        &self,
        cookie: &str,
        mut input: AuthorizationRequest,
    ) -> Result<AuthorizationOutcome> {
        let operation = async {
            let current = self.identity.authenticate_cookie(cookie).await?;
            let mut connection = self
                .native_grants_pool()
                .ok_or(Error::Configuration)?
                .acquire()
                .await
                .map_err(|_| Error::Configuration)?;
            let tx = connection
                .transaction()
                .await
                .map_err(|_| Error::Configuration)?;
            let client = native_client_store::load_client(&tx, &input.client_id).await?;
            if input.scope.is_empty() {
                input.scope = client.scopes.join(" ");
            }
            let mut engine =
                Engine::new(&client, Utc::now() + chrono::Duration::minutes(5), false)?;
            let pending = engine.validate(&input)?;
            if !client.grants.iter().any(|g| g == "authorization_code") {
                return Err(Error::InvalidGrant);
            }
            let scopes = scope_vec(&input.scope)?;
            let Some(current) = current else {
                self.native_resources(&tx, &client, &input.resources, &scopes)
                    .await?;
                return if input.prompt.as_deref() == Some("none") {
                    Err(Error::LoginRequired)
                } else {
                    Ok(AuthorizationOutcome::LoginRequired)
                };
            };
            if !current.can_access() {
                self.native_resources(&tx, &client, &input.resources, &scopes)
                    .await?;
                return Ok(AuthorizationOutcome::LoginRequired);
            }
            let (tx, _) = self
                .grant_actor(tx, &current.user_id, &current.session_id, &client)
                .await?;
            self.native_resources(&tx, &client, &input.resources, &scopes)
                .await?;
            let (tx, principal) = self
                .grant_actor(tx, &current.user_id, &current.session_id, &client)
                .await?;
            let time=tx.query_typed_opt(r#"SELECT floor(extract(epoch FROM "createdAt"))::bigint AS auth_time,floor(extract(epoch FROM statement_timestamp()))::bigint AS now FROM webdock_auth.session WHERE id=$1 AND "userId"=$2"#,&[(&principal.session_id,Type::TEXT),(&principal.user_id,Type::TEXT)]).await.map_err(|_|Error::Configuration)?.ok_or(Error::AccessDenied)?;
            let auth_time: i64 = time
                .try_get("auth_time")
                .map_err(|_| Error::Configuration)?;
            let now: i64 = time.try_get("now").map_err(|_| Error::Configuration)?;
            if input.prompt.as_deref() == Some("login")
                || input
                    .max_age
                    .is_some_and(|age| now.saturating_sub(auth_time) > age as i64)
            {
                return Ok(AuthorizationOutcome::LoginRequired);
            }
            if input
                .prompt
                .as_deref()
                .is_some_and(|p| !["none", "consent"].contains(&p))
            {
                return Err(Error::InvalidRequest);
            }
            let context = GrantContext {
                subject: principal.user_id.clone(),
                session_id: principal.session_id.clone(),
                request: input.clone(),
                auth_time,
            };
            if !client.skip_consent
                && (input.prompt.as_deref() == Some("consent")
                    || !consent_exists(&tx, &client.id, &context).await?)
            {
                if input.prompt.as_deref() == Some("none") {
                    return Err(Error::ConsentRequired);
                }
                let token = random_token()?;
                store(&tx, &token, "consent", &context).await?;
                let (tx, _) = self
                    .grant_actor(tx, &principal.user_id, &principal.session_id, &client)
                    .await?;
                let response = details(&token, &client, &context)?;
                tx.commit().await.map_err(|_| Error::Configuration)?;
                return Ok(AuthorizationOutcome::ConsentRequired(response));
            }
            let redirect = engine.authorize(pending, &context.subject)?;
            store(&tx, engine.created_code()?, "code", &context).await?;
            let (tx, _) = self
                .grant_actor(tx, &principal.user_id, &principal.session_id, &client)
                .await?;
            tx.commit().await.map_err(|_| Error::Configuration)?;
            Ok(AuthorizationOutcome::Redirect(
                self.issuer_redirect(redirect)?,
            ))
        };
        tokio::time::timeout(BUDGET, operation)
            .await
            .map_err(|_| Error::Configuration)?
    }
    pub(crate) async fn native_consent_details(
        &self,
        cookie: &str,
        id: &str,
    ) -> Result<ConsentRequest> {
        match self.native_consent_request(cookie, id, None).await? {
            ConsentResult::Details(d) => Ok(d),
            ConsentResult::Decision(_) => Err(Error::Configuration),
        }
    }
    pub(crate) async fn native_consent(
        &self,
        cookie: &str,
        id: &str,
        approved: bool,
    ) -> Result<AuthorizationOutcome> {
        match self
            .native_consent_request(cookie, id, Some(approved))
            .await?
        {
            ConsentResult::Decision(r) => Ok(r),
            ConsentResult::Details(_) => Err(Error::Configuration),
        }
    }
    async fn native_consent_request(
        &self,
        cookie: &str,
        id: &str,
        decision: Option<bool>,
    ) -> Result<ConsentResult> {
        let operation = async {
            let current = self
                .identity
                .authenticate_cookie(cookie)
                .await?
                .ok_or(Error::LoginRequired)?;
            if !current.can_access() || current.in_tenant_preview() {
                return Err(Error::AccessDenied);
            }
            let mut connection = self
                .native_grants_pool()
                .ok_or(Error::Configuration)?
                .acquire()
                .await
                .map_err(|_| Error::Configuration)?;
            let tx = connection
                .transaction()
                .await
                .map_err(|_| Error::Configuration)?;
            let hash = legacy_token_hash(id);
            let initial = request(&tx, &hash, &current.user_id, &current.session_id)
                .await?
                .ok_or(Error::InvalidGrant)?;
            let context = initial.context()?;
            let client = native_client_store::load_client(&tx, &context.request.client_id).await?;
            if decision.is_some() {
                tx.query_typed_opt(
                    r#"SELECT id FROM webdock_auth."user" WHERE id=$1 FOR UPDATE"#,
                    &[(&current.user_id, Type::TEXT)],
                )
                .await
                .map_err(|_| Error::Configuration)?;
            }
            let (tx, principal) = self
                .grant_actor(tx, &current.user_id, &current.session_id, &client)
                .await?;
            let mode = if decision.is_some() {
                "UPDATE"
            } else {
                "SHARE"
            };
            let sql = format!(
                "SELECT token_hash FROM webdock_auth.identity_oauth_request WHERE token_hash=$1 AND kind='consent' AND user_id=$2 AND session_id=$3 AND client_id=$4 FOR {mode}"
            );
            if tx
                .query_typed_opt(
                    &sql,
                    &[
                        (&hash, Type::TEXT),
                        (&principal.user_id, Type::TEXT),
                        (&principal.session_id, Type::TEXT),
                        (&client.id, Type::TEXT),
                    ],
                )
                .await
                .map_err(|_| Error::Configuration)?
                .is_none()
            {
                return Err(Error::InvalidGrant);
            }
            let fresh = request(&tx, &hash, &principal.user_id, &principal.session_id)
                .await?
                .filter(|r| r.same(&initial))
                .ok_or(Error::InvalidGrant)?;
            let context = fresh.context()?;
            let Some(approved) = decision else {
                let (tx, _) = self
                    .grant_actor(tx, &principal.user_id, &principal.session_id, &client)
                    .await?;
                let response = details(id, &client, &context)?;
                live_deadline(&tx, initial.expires).await?;
                tx.commit().await.map_err(|_| Error::Configuration)?;
                return Ok(ConsentResult::Details(response));
            };
            let mut engine =
                Engine::new(&client, Utc::now() + chrono::Duration::minutes(5), false)?;
            let pending = engine.validate(&context.request)?;
            self.native_resources(
                &tx,
                &client,
                &context.request.resources,
                &scope_vec(&context.request.scope)?,
            )
            .await?;
            let (tx, _) = self
                .grant_actor(tx, &principal.user_id, &principal.session_id, &client)
                .await?;
            if request(&tx, &hash, &principal.user_id, &principal.session_id)
                .await?
                .is_none_or(|r| !r.same(&initial))
            {
                return Err(Error::InvalidGrant);
            }
            if tx.execute_typed("DELETE FROM webdock_auth.identity_oauth_request WHERE token_hash=$1 AND kind='consent' AND user_id=$2 AND session_id=$3 AND client_id=$4 AND expires_at>statement_timestamp()",&[(&hash,Type::TEXT),(&principal.user_id,Type::TEXT),(&principal.session_id,Type::TEXT),(&client.id,Type::TEXT)]).await.map_err(|_|Error::Configuration)?!=1{return Err(Error::InvalidGrant);}
            if !approved {
                let mut redirect = url::Url::parse(&context.request.redirect_uri)
                    .map_err(|_| Error::InvalidRequest)?;
                redirect
                    .query_pairs_mut()
                    .append_pair("error", "access_denied");
                if let Some(state) = &context.request.state {
                    redirect.query_pairs_mut().append_pair("state", state);
                }
                live_deadline(&tx, initial.expires).await?;
                tx.commit().await.map_err(|_| Error::Configuration)?;
                return Ok(ConsentResult::Decision(AuthorizationOutcome::Redirect(
                    self.issuer_redirect(redirect.into())?,
                )));
            }
            tx.execute_typed(
                r#"DELETE FROM webdock_auth."oauthConsent" WHERE "clientId"=$1 AND "userId"=$2"#,
                &[(&client.id, Type::TEXT), (&context.subject, Type::TEXT)],
            )
            .await
            .map_err(|_| Error::Configuration)?;
            let consent_id = self.allocate_id().await?;
            tx.execute_typed(r#"INSERT INTO webdock_auth."oauthConsent"(id,"clientId","userId",scopes,resources,"createdAt","updatedAt") VALUES($1,$2,$3,$4,$5,statement_timestamp(),statement_timestamp())"#,&[(&consent_id,Type::TEXT),(&client.id,Type::TEXT),(&context.subject,Type::TEXT),(&json!(scope_vec(&context.request.scope)?),Type::JSONB),(&json!(context.request.resources),Type::JSONB)]).await.map_err(|_|Error::Configuration)?;
            let redirect = engine.authorize(pending, &context.subject)?;
            store(&tx, engine.created_code()?, "code", &context).await?;
            let (tx, _) = self
                .grant_actor(tx, &principal.user_id, &principal.session_id, &client)
                .await?;
            live_deadline(&tx, initial.expires).await?;
            tx.commit().await.map_err(|_| Error::Configuration)?;
            Ok(ConsentResult::Decision(AuthorizationOutcome::Redirect(
                self.issuer_redirect(redirect)?,
            )))
        };
        tokio::time::timeout(BUDGET, operation)
            .await
            .map_err(|_| Error::Configuration)?
    }
}
enum ConsentResult {
    Details(ConsentRequest),
    Decision(AuthorizationOutcome),
}
