use crate::{
    engine::{Engine, grant},
    types::{Client, GrantContext, SCOPES, valid_url},
    *,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, Statement,
    TransactionTrait,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use subtle::ConstantTimeEq;
use webdock_identity::{IdentityService, Principal, SecretKeys};
use webdock_persistence::IdAllocator;

struct Issuance<'a> {
    client: &'a Client,
    principal: &'a Principal,
    context: &'a GrantContext,
    engine: &'a Engine,
    current: i64,
    ttl: i64,
    refresh_ttl: i64,
    code_id: Option<String>,
}
pub struct OAuthService {
    pub(crate) db: DatabaseConnection,
    identity: Arc<IdentityService>,
    allocator: Arc<IdAllocator>,
    pub(crate) keys: Arc<SecretKeys>,
    pub(crate) config: OAuthConfig,
}
impl OAuthService {
    pub fn new(
        db: DatabaseConnection,
        identity: Arc<IdentityService>,
        allocator: Arc<IdAllocator>,
        keys: Arc<SecretKeys>,
        config: OAuthConfig,
    ) -> Self {
        Self {
            db,
            identity,
            allocator,
            keys,
            config,
        }
    }
    pub(crate) async fn allocate_id(&self) -> Result<String> {
        self.allocator
            .allocate()
            .await
            .map(|id| id.to_string())
            .map_err(|_| Error::Allocation)
    }
    pub fn discovery(&self) -> Value {
        let issuer = self.config.issuer.as_str();
        json!({"issuer":issuer,"authorization_response_iss_parameter_supported":true,"authorization_endpoint":format!("{issuer}/oauth2/authorize"),"token_endpoint":format!("{issuer}/oauth2/token"),"userinfo_endpoint":format!("{issuer}/oauth2/userinfo"),"introspection_endpoint":format!("{issuer}/oauth2/introspect"),"revocation_endpoint":format!("{issuer}/oauth2/revoke"),"end_session_endpoint":format!("{issuer}/oauth2/end-session"),"jwks_uri":format!("{issuer}/jwks"),"response_types_supported":["code"],"grant_types_supported":["authorization_code","refresh_token"],"subject_types_supported":["public"],"id_token_signing_alg_values_supported":["RS256"],"token_endpoint_auth_methods_supported":["client_secret_post","client_secret_basic","none"],"code_challenge_methods_supported":["S256"],"scopes_supported":SCOPES})
    }
    /// Explicit offline connection; never expose this as dynamic client registration.
    pub async fn provision_client(
        &self,
        offline: &DatabaseConnection,
        input: ClientRegistration,
    ) -> Result<RegisteredClient> {
        if input.name.trim().is_empty()
            || input.name.len() > 160
            || input.redirect_uris.is_empty()
            || input.redirect_uris.len() > 20
            || input.scopes.is_empty()
            || input.scopes.iter().any(|s| !SCOPES.contains(&s.as_str()))
        {
            return Err(Error::InvalidRequest);
        }
        for uri in input
            .redirect_uris
            .iter()
            .chain(&input.post_logout_redirect_uris)
        {
            valid_url(uri)?;
        }
        let client_id = self.allocate_id().await?;
        let row_id = self.allocate_id().await?;
        let secret = if input.auth_method == ClientAuthMethod::None {
            None
        } else {
            Some(random_token()?)
        };
        let mut organization = None;
        let mut metadata = json!({});
        let mut binding = None;
        match input.purpose {
            ClientPurpose::McpOperator => {
                metadata = json!({"webdock_mcp":true});
                if input.scopes.iter().any(|s| s.starts_with("hosting:")) {
                    return Err(Error::InvalidScope);
                }
            }
            ClientPurpose::McpHosting => {
                metadata = json!({"webdock_hosting":true});
                if input.scopes.iter().any(|s| s.starts_with("webdock:")) {
                    return Err(Error::InvalidScope);
                }
            }
            ClientPurpose::Portal => {
                binding = Some(self.allocate_id().await?);
            }
            ClientPurpose::Organization { organization_id } => {
                binding = Some(self.allocate_id().await?);
                organization = Some(organization_id);
            }
            ClientPurpose::OperatorOnly { allowed_subjects } => {
                if allowed_subjects.is_empty() || allowed_subjects.iter().any(|id| !valid_id(id)) {
                    return Err(Error::InvalidRequest);
                }
                binding = Some(self.allocate_id().await?);
                metadata = json!({"webdock_operator_only":true,"webdock_allowed_subjects":allowed_subjects});
            }
        }
        if let Some(id) = &binding {
            metadata["webdock_binding"] = json!(id);
        }
        let tx = offline.begin().await?;
        let grant_types = if input.allow_refresh {
            json!(["authorization_code", "refresh_token"])
        } else {
            json!(["authorization_code"])
        };
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.\"oauthClient\"(id,\"clientId\",\"clientSecret\",name,disabled,\"skipConsent\",\"enableEndSession\",scopes,\"redirectUris\",\"postLogoutRedirectUris\",\"grantTypes\",\"responseTypes\",\"tokenEndpointAuthMethod\",\"requirePKCE\",\"subjectType\",metadata,\"createdAt\",\"updatedAt\") VALUES($1,$2,$3,$4,false,$5,true,$6,$7,$8,$9,'[\"code\"]'::jsonb,$10,true,'public',$11,statement_timestamp(),statement_timestamp())",[row_id.into(),client_id.clone().into(),secret.as_ref().map(|s|legacy_token_hash(s)).into(),input.name.clone().into(),input.skip_consent.into(),json!(input.scopes).into(),json!(input.redirect_uris).into(),json!(input.post_logout_redirect_uris).into(),grant_types.into(),input.auth_method.as_str().into(),metadata.clone().into()])).await?;
        if let Some(binding) = &binding {
            tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.app_binding(id,client_id,label,organization_id,enabled) VALUES($1,$2,$3,$4,true)",[binding.clone().into(),client_id.clone().into(),input.name.into(),organization.into()])).await?;
        }
        if metadata.get("webdock_mcp") == Some(&json!(true))
            || metadata.get("webdock_hosting") == Some(&json!(true))
        {
            let resource = self.config.mcp_resource.as_str();
            tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.\"oauthResource\"(id,identifier,name,\"accessTokenTtl\",\"refreshTokenTtl\",\"allowedScopes\",disabled,\"createdAt\",\"updatedAt\") VALUES($1,$2,'Webdock Studio',300,28800,$3,false,statement_timestamp(),statement_timestamp()) ON CONFLICT(identifier) DO NOTHING",[self.allocate_id().await?.into(),resource.into(),json!(["webdock:read","webdock:write","hosting:read","hosting:write","offline_access"]).into()])).await?;
            tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.\"oauthClientResource\"(id,\"clientId\",\"resourceId\",\"createdAt\") VALUES($1,$2,$3,statement_timestamp())",[self.allocate_id().await?.into(),client_id.clone().into(),resource.into()])).await?;
        }
        tx.commit().await?;
        Ok(RegisteredClient {
            client_id,
            client_secret: secret,
            binding_id: binding,
        })
    }
    fn issuer_redirect(&self, redirect: String) -> Result<String> {
        let mut url = url::Url::parse(&redirect).map_err(|_| Error::InvalidRequest)?;
        let query: Vec<(String, String)> = url
            .query_pairs()
            .filter(|(key, _)| key != "iss")
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        url.set_query(None);
        url.query_pairs_mut()
            .extend_pairs(query)
            .append_pair("iss", self.config.issuer.as_str());
        Ok(url.into())
    }
    pub async fn authorize(
        &self,
        cookie: &str,
        mut request: AuthorizationRequest,
    ) -> Result<AuthorizationOutcome> {
        let current = self.identity.authenticate_cookie(cookie).await?;
        let tx = self.db.begin().await?;
        let client = load_client(&tx, &request.client_id).await?;
        if request.scope.is_empty() {
            request.scope = client.scopes.join(" ");
        }
        let mut engine = Engine::new(&client, Utc::now() + chrono::Duration::minutes(5), false)?;
        let pending = engine.validate(&request)?;
        if !client.grants.iter().any(|g| g == "authorization_code") {
            return Err(Error::InvalidGrant);
        }
        self.resource_lifetimes(
            &tx,
            &client,
            &request.resources,
            &scope_vec(&request.scope)?,
        )
        .await?;
        let Some(current) = current else {
            return if request.prompt.as_deref() == Some("none") {
                Err(Error::LoginRequired)
            } else {
                Ok(AuthorizationOutcome::LoginRequired)
            };
        };
        if !current.can_access() {
            return Ok(AuthorizationOutcome::LoginRequired);
        }
        let principal = self
            .live_principal(&tx, &current.user_id, &current.session_id)
            .await?;
        self.current_claims(&tx, &client, &principal).await?;
        let auth_time = session_time(&tx, &principal.session_id).await?;
        let now = now(&tx).await?;
        if request.prompt.as_deref() == Some("login")
            || request
                .max_age
                .is_some_and(|age| now.saturating_sub(auth_time) > age as i64)
        {
            return Ok(AuthorizationOutcome::LoginRequired);
        }
        if request
            .prompt
            .as_deref()
            .is_some_and(|v| !["none", "consent"].contains(&v))
        {
            return Err(Error::InvalidRequest);
        }
        let context = GrantContext {
            subject: principal.user_id.clone(),
            session_id: principal.session_id.clone(),
            request: request.clone(),
            auth_time,
        };
        if !client.skip_consent
            && (request.prompt.as_deref() == Some("consent")
                || !has_consent(&tx, &client.id, &context.subject, &request).await?)
        {
            if request.prompt.as_deref() == Some("none") {
                return Err(Error::ConsentRequired);
            }
            let token = random_token()?;
            store_request(&tx, &token, "consent", &context).await?;
            tx.commit().await?;
            return Ok(AuthorizationOutcome::ConsentRequired(ConsentRequest {
                request_id: token,
                client_id: client.id,
                client_name: client.name,
                scopes: scope_vec(&request.scope)?,
                resources: request.resources,
            }));
        }
        let redirect = engine.authorize(pending, &context.subject)?;
        store_request(&tx, engine.created_code()?, "code", &context).await?;
        tx.commit().await?;
        Ok(AuthorizationOutcome::Redirect(
            self.issuer_redirect(redirect)?,
        ))
    }
    pub async fn consent_details(&self, cookie: &str, request_id: &str) -> Result<ConsentRequest> {
        let current = self
            .identity
            .authenticate_cookie(cookie)
            .await?
            .ok_or(Error::LoginRequired)?;
        let tx = self.db.begin().await?;
        let principal = self
            .live_principal(&tx, &current.user_id, &current.session_id)
            .await?;
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT payload FROM webdock_auth.identity_oauth_request WHERE token_hash=$1 AND kind='consent' AND user_id=$2 AND session_id=$3 AND expires_at>statement_timestamp()",[legacy_token_hash(request_id).into(),principal.user_id.clone().into(),principal.session_id.clone().into()])).await?.ok_or(Error::InvalidGrant)?;
        let context: GrantContext =
            serde_json::from_value(row.try_get("", "payload")?).map_err(|_| Error::InvalidGrant)?;
        let client = load_client(&tx, &context.request.client_id).await?;
        self.current_claims(&tx, &client, &principal).await?;
        let response = ConsentRequest {
            request_id: request_id.into(),
            client_id: client.id,
            client_name: client.name,
            scopes: scope_vec(&context.request.scope)?,
            resources: context.request.resources,
        };
        tx.commit().await?;
        Ok(response)
    }
    pub async fn consent(
        &self,
        cookie: &str,
        request_id: &str,
        approved: bool,
    ) -> Result<AuthorizationOutcome> {
        let current = self
            .identity
            .authenticate_cookie(cookie)
            .await?
            .ok_or(Error::LoginRequired)?;
        let tx = self.db.begin().await?;
        let principal = self
            .live_principal(&tx, &current.user_id, &current.session_id)
            .await?;
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT payload FROM webdock_auth.identity_oauth_request WHERE token_hash=$1 AND kind='consent' AND user_id=$2 AND session_id=$3 AND expires_at>statement_timestamp() FOR UPDATE",[legacy_token_hash(request_id).into(),principal.user_id.clone().into(),principal.session_id.clone().into()])).await?.ok_or(Error::InvalidGrant)?;
        let context: GrantContext =
            serde_json::from_value(row.try_get("", "payload")?).map_err(|_| Error::InvalidGrant)?;
        let client = load_client(&tx, &context.request.client_id).await?;
        self.current_claims(&tx, &client, &principal).await?;
        let mut engine = Engine::new(&client, Utc::now() + chrono::Duration::minutes(5), false)?;
        let pending = engine.validate(&context.request)?;
        self.resource_lifetimes(
            &tx,
            &client,
            &context.request.resources,
            &scope_vec(&context.request.scope)?,
        )
        .await?;
        tx.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM webdock_auth.identity_oauth_request WHERE token_hash=$1",
            [legacy_token_hash(request_id).into()],
        ))
        .await?;
        if !approved {
            let mut redirect = url::Url::parse(&context.request.redirect_uri)
                .map_err(|_| Error::InvalidRequest)?;
            redirect
                .query_pairs_mut()
                .append_pair("error", "access_denied");
            if let Some(state) = &context.request.state {
                redirect.query_pairs_mut().append_pair("state", state);
            }
            tx.commit().await?;
            return Ok(AuthorizationOutcome::Redirect(
                self.issuer_redirect(redirect.into())?,
            ));
        }
        tx.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM webdock_auth.\"oauthConsent\" WHERE \"clientId\"=$1 AND \"userId\"=$2",
            [client.id.clone().into(), context.subject.clone().into()],
        ))
        .await?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.\"oauthConsent\"(id,\"clientId\",\"userId\",scopes,resources,\"createdAt\",\"updatedAt\") VALUES($1,$2,$3,$4,$5,statement_timestamp(),statement_timestamp())",[self.allocate_id().await?.into(),client.id.into(),context.subject.clone().into(),json!(scope_vec(&context.request.scope)?).into(),json!(context.request.resources).into()])).await?;
        let redirect = engine.authorize(pending, &context.subject)?;
        store_request(&tx, engine.created_code()?, "code", &context).await?;
        tx.commit().await?;
        Ok(AuthorizationOutcome::Redirect(
            self.issuer_redirect(redirect)?,
        ))
    }
    pub async fn exchange_code(&self, input: CodeExchange) -> Result<TokenResponse> {
        let tx = self.db.begin().await?;
        let client = load_client(&tx, &input.client.client_id).await?;
        authenticate_client(&client, &input.client)?;
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT payload,expires_at FROM webdock_auth.identity_oauth_request WHERE token_hash=$1 AND kind='code' AND client_id=$2 AND expires_at>statement_timestamp()",[legacy_token_hash(&input.code).into(),client.id.clone().into()])).await?.ok_or(Error::InvalidGrant)?;
        let context: GrantContext =
            serde_json::from_value(row.try_get("", "payload")?).map_err(|_| Error::InvalidGrant)?;
        let principal = self
            .live_principal(&tx, &context.subject, &context.session_id)
            .await?;
        self.current_claims(&tx, &client, &principal).await?;
        // Lock owner/session before the code row, matching logout and account-deletion lock order.
        let locked=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT payload,expires_at FROM webdock_auth.identity_oauth_request WHERE token_hash=$1 AND kind='code' AND client_id=$2 AND expires_at>statement_timestamp() FOR UPDATE",[legacy_token_hash(&input.code).into(),client.id.clone().into()])).await?.ok_or(Error::InvalidGrant)?;
        if locked.try_get::<Value>("", "payload")? != row.try_get::<Value>("", "payload")? {
            return Err(Error::InvalidGrant);
        }
        let row = locked;
        if !client.grants.iter().any(|g| g == "authorization_code") {
            return Err(Error::InvalidGrant);
        }
        let scopes = scope_vec(&context.request.scope)?;
        let (ttl, refresh_ttl) = self
            .resource_lifetimes(&tx, &client, &context.request.resources, &scopes)
            .await?;
        let current = now(&tx).await?;
        let mut engine = Engine::new(
            &client,
            date(current + ttl)?,
            client.grants.iter().any(|g| g == "refresh_token")
                && scopes.iter().any(|s| s == "offline_access"),
        )?;
        engine.load_code(
            &input.code,
            grant(&context, row.try_get("", "expires_at")?)?,
        );
        engine.exchange(&input)?;
        let response = self
            .persist_tokens(
                &tx,
                Issuance {
                    client: &client,
                    principal: &principal,
                    context: &context,
                    engine: &engine,
                    current,
                    ttl,
                    refresh_ttl,
                    code_id: Some(legacy_token_hash(&input.code)),
                },
            )
            .await?;
        tx.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM webdock_auth.identity_oauth_request WHERE token_hash=$1",
            [legacy_token_hash(&input.code).into()],
        ))
        .await?;
        tx.commit().await?;
        Ok(response)
    }
    pub async fn refresh(&self, input: RefreshExchange) -> Result<TokenResponse> {
        let tx = self.db.begin().await?;
        let client = load_client(&tx, &input.client.client_id).await?;
        authenticate_client(&client, &input.client)?;
        if !client.grants.iter().any(|g| g == "refresh_token")
            || client.metadata.get("webdock_operator_only").is_some()
        {
            return Err(Error::InvalidGrant);
        }
        let lookup=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT \"userId\" FROM webdock_auth.\"oauthRefreshToken\" WHERE token=$1 AND \"clientId\"=$2",[legacy_token_hash(&input.refresh_token).into(),client.id.clone().into()])).await?.ok_or(Error::InvalidGrant)?;
        let subject: String = lookup.try_get("", "userId")?;
        // Serialize all refreshes for the owner before examining rotation/replay state.
        tx.query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM webdock_auth.\"user\" WHERE id=$1 FOR UPDATE",
            [subject.clone().into()],
        ))
        .await?
        .ok_or(Error::InvalidGrant)?;
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT * FROM webdock_auth.\"oauthRefreshToken\" WHERE token=$1 AND \"clientId\"=$2 FOR UPDATE",[legacy_token_hash(&input.refresh_token).into(),client.id.clone().into()])).await?.ok_or(Error::InvalidGrant)?;
        if row
            .try_get::<Option<DateTime<Utc>>>("", "rotatedAt")?
            .is_some()
        {
            revoke_family(&tx, &client.id, &subject).await?;
            tx.commit().await?;
            return Err(Error::InvalidGrant);
        }
        if row
            .try_get::<Option<DateTime<Utc>>>("", "revoked")?
            .is_some()
            || row.try_get::<DateTime<Utc>>("", "expiresAt")? <= Utc::now()
        {
            return Err(Error::InvalidGrant);
        }
        let session = row
            .try_get::<Option<String>>("", "sessionId")?
            .ok_or(Error::InvalidGrant)?;
        let principal = self.live_principal(&tx, &subject, &session).await?;
        self.current_claims(&tx, &client, &principal).await?;
        let original_scopes = json_strings(row.try_get("", "scopes")?)?;
        let resources = json_strings(
            row.try_get::<Option<Value>>("", "resources")?
                .unwrap_or(json!([])),
        )?;
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
            .resource_lifetimes(&tx, &client, &resources, &scopes)
            .await?;
        let current = now(&tx).await?;
        let mut request = AuthorizationRequest::code(
            client.id.clone(),
            client.redirects[0].clone(),
            original_scopes.join(" "),
            "x".repeat(43),
        );
        request.resources = resources;
        let auth_time = row
            .try_get::<Option<DateTime<Utc>>>("", "authTime")?
            .map(|d| d.timestamp())
            .unwrap_or(session_time(&tx, &session).await?);
        let mut context = GrantContext {
            subject,
            session_id: session,
            request,
            auth_time,
        };
        let mut engine = Engine::new(&client, date(current + ttl)?, true)?;
        engine.load_refresh(
            &input.refresh_token,
            grant(&context, row.try_get("", "expiresAt")?)?,
        );
        engine.refresh(&input)?;
        context.request.scope = scopes.join(" ");
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.\"oauthRefreshToken\" SET revoked=statement_timestamp(),\"rotatedAt\"=statement_timestamp() WHERE id=$1",[row.try_get::<String>("","id")?.into()])).await?;
        let response = self
            .persist_tokens(
                &tx,
                Issuance {
                    client: &client,
                    principal: &principal,
                    context: &context,
                    engine: &engine,
                    current,
                    ttl,
                    refresh_ttl,
                    code_id: row.try_get("", "authorizationCodeId")?,
                },
            )
            .await?;
        tx.commit().await?;
        Ok(response)
    }
    async fn persist_tokens(
        &self,
        tx: &DatabaseTransaction,
        issuance: Issuance<'_>,
    ) -> Result<TokenResponse> {
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
        let scopes = scope_vec(&context.request.scope)?;
        let refresh_id = if let Some(token) = &tokens.refresh {
            let id = self.allocate_id().await?;
            tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.\"oauthRefreshToken\"(id,token,\"clientId\",\"sessionId\",\"userId\",\"authorizationCodeId\",resources,scopes,\"expiresAt\",\"createdAt\",\"authTime\") VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",[id.clone().into(),legacy_token_hash(token).into(),client.id.clone().into(),principal.session_id.clone().into(),principal.user_id.clone().into(),code_id.clone().into(),json!(context.request.resources).into(),json!(scopes).into(),date(current+refresh_ttl)?.into(),date(current)?.into(),date(context.auth_time)?.into()])).await?;
            Some(id)
        } else {
            None
        };
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.\"oauthAccessToken\"(id,token,\"clientId\",\"sessionId\",\"userId\",\"authorizationCodeId\",\"refreshId\",resources,scopes,\"expiresAt\",\"createdAt\") VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",[self.allocate_id().await?.into(),legacy_token_hash(&tokens.token).into(),client.id.clone().into(),principal.session_id.clone().into(),principal.user_id.clone().into(),code_id.into(),refresh_id.into(),json!(context.request.resources).into(),json!(scopes).into(),date(current+ttl)?.into(),date(current)?.into()])).await?;
        let id_token = if scopes.iter().any(|s| s == "openid") {
            let mut claims = json!({"iss":self.config.issuer.as_str(),"sub":principal.user_id,"aud":client.id,"iat":current,"exp":current+300,"auth_time":context.auth_time,"acr":"0","at_hash":URL_SAFE_NO_PAD.encode(&Sha256::digest(tokens.token.as_bytes())[..16])});
            if let Some(nonce) = &context.request.nonce {
                claims["nonce"] = json!(nonce);
            }
            if client.end_session {
                claims["sid"] = json!(principal.session_id);
            }
            if scopes.iter().any(|s| s == "email") {
                claims["email"] = json!(principal.email);
                claims["email_verified"] = json!(principal.email_verified());
            }
            if scopes.iter().any(|s| s == "profile") {
                claims["name"] = json!(principal.name);
            }
            Some(self.sign_id_token(tx, &claims).await?)
        } else {
            None
        };
        Ok(TokenResponse {
            access_token: tokens.token.clone(),
            token_type: "Bearer".into(),
            expires_in: ttl,
            scope: context.request.scope.clone(),
            refresh_token: tokens.refresh.clone(),
            id_token,
        })
    }
    /// Trusted first-party browser facade only. Never admit an organization/MCP client as a Console.
    pub async fn validate_portal_client(
        &self,
        credentials: &ClientAuthentication,
        redirect_uri: &str,
    ) -> Result<()> {
        self.portal_credentials(credentials)?;
        let tx = self.db.begin().await?;
        self.validate_portal_client_in(&tx, credentials, redirect_uri)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    /// Trusted backend transaction-aware equivalent; never acquires another pool connection.
    pub async fn validate_portal_client_in(
        &self,
        tx: &DatabaseTransaction,
        credentials: &ClientAuthentication,
        redirect_uri: &str,
    ) -> Result<()> {
        self.portal_credentials(credentials)?;
        let client = load_client(tx, &credentials.client_id).await?;
        authenticate_client(&client, credentials)?;
        if !client.redirects.iter().any(|uri| uri == redirect_uri)
            || client.metadata.get("webdock_mcp").is_some()
            || client.metadata.get("webdock_hosting").is_some()
            || client.metadata.get("webdock_operator_only").is_some()
        {
            return Err(Error::InvalidClient);
        }
        let binding = client.metadata["webdock_binding"]
            .as_str()
            .ok_or(Error::InvalidClient)?;
        if tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id FROM webdock_auth.app_binding WHERE id=$1 AND client_id=$2 AND enabled AND organization_id IS NULL",[binding.into(),credentials.client_id.clone().into()])).await?.is_none(){return Err(Error::InvalidClient)}
        for scope in ["openid", "profile", "email"] {
            if !client.scopes.iter().any(|value| value == scope) {
                return Err(Error::InvalidScope);
            }
        }
        Ok(())
    }
    pub async fn introspect(
        &self,
        credentials: &ClientAuthentication,
        token: &str,
    ) -> Result<Value> {
        let tx = self.db.begin().await?;
        let caller = load_client(&tx, &credentials.client_id).await?;
        authenticate_client(&caller, credentials)?;
        let result = self.inspect(&tx, token, Some(&caller.id), true).await?;
        tx.commit().await?;
        Ok(result)
    }
    /// First-party backend only: keeps a read-only preview usable for its explicit exit action.
    /// This is deliberately not exposed by the public introspection endpoint.
    pub async fn introspect_portal(
        &self,
        credentials: &ClientAuthentication,
        token: &str,
    ) -> Result<Value> {
        self.portal_credentials(credentials)?;
        let tx = self.db.begin().await?;
        let result = self.introspect_portal_in(&tx, credentials, token).await?;
        tx.commit().await?;
        Ok(result)
    }
    fn portal_credentials(&self, credentials: &ClientAuthentication) -> Result<()> {
        if self.config.studio_client_id.as_deref() != Some(&credentials.client_id)
            || credentials.method == ClientAuthMethod::None
        {
            return Err(Error::InvalidClient);
        }
        Ok(())
    }
    /// Trusted backend transaction-aware introspection, including the exact first-party preview exception.
    pub async fn introspect_portal_in(
        &self,
        tx: &DatabaseTransaction,
        credentials: &ClientAuthentication,
        token: &str,
    ) -> Result<Value> {
        self.portal_credentials(credentials)?;
        let client = load_client(tx, &credentials.client_id).await?;
        authenticate_client(&client, credentials)?;
        let binding = client.metadata["webdock_binding"]
            .as_str()
            .ok_or(Error::InvalidClient)?;
        if client.metadata.get("webdock_operator_only").is_some()||client.metadata.get("webdock_mcp").is_some()||client.metadata.get("webdock_hosting").is_some()||tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id FROM webdock_auth.app_binding WHERE id=$1 AND client_id=$2 AND enabled AND organization_id IS NULL",[binding.into(),client.id.clone().into()])).await?.is_none(){return Err(Error::InvalidClient)}
        let result = self
            .inspect_scoped(tx, token, Some(&client.id), false, true)
            .await?;
        Ok(result)
    }
    pub async fn introspect_mcp(
        &self,
        credentials: &ClientAuthentication,
        token: &str,
    ) -> Result<Value> {
        let claims = self.introspect(credentials, token).await?;
        if claims["active"] != true
            || claims["token_type"] != "Bearer"
            || !audiences(&claims).contains(&self.config.mcp_resource.as_str().to_owned())
            || !["operator", "hosting-operator", "hosting-customer"]
                .contains(&claims["webdock_role"].as_str().unwrap_or(""))
        {
            return Ok(json!({"active":false}));
        }
        let tx = self.db.begin().await?;
        let issuer = load_client(
            &tx,
            claims["client_id"].as_str().ok_or(Error::AccessDenied)?,
        )
        .await?;
        if issuer.metadata["webdock_mcp"] != true && issuer.metadata["webdock_hosting"] != true {
            return Ok(json!({"active":false}));
        }
        tx.commit().await?;
        Ok(claims)
    }
    /// Identity/scope check only; each tool must still enforce current tenant/resource permissions.
    pub async fn authorize_mcp(
        &self,
        credentials: &ClientAuthentication,
        token: &str,
        required_scope: &str,
    ) -> Result<Value> {
        let (read, write, registry) = match required_scope {
            "webdock:read" => ("webdock:read", false, true),
            "webdock:write" => ("webdock:read", true, true),
            "hosting:read" => ("hosting:read", false, false),
            "hosting:write" => ("hosting:read", true, false),
            _ => return Err(Error::InvalidScope),
        };
        let claims = self.introspect_mcp(credentials, token).await?;
        let role = claims["webdock_role"].as_str().unwrap_or("");
        let scopes = claims["scope"]
            .as_str()
            .unwrap_or("")
            .split_whitespace()
            .collect::<Vec<_>>();
        if claims["active"] != true
            || (registry && role != "operator")
            || (!registry && !["hosting-operator", "hosting-customer"].contains(&role))
            || (!registry && scopes.iter().any(|scope| scope.starts_with("webdock:")))
            || !scopes.contains(&read)
            || (write && !scopes.contains(&required_scope))
        {
            return Err(Error::AccessDenied);
        }
        Ok(claims)
    }
    async fn inspect(
        &self,
        tx: &DatabaseTransaction,
        token: &str,
        caller: Option<&str>,
        allow_refresh: bool,
    ) -> Result<Value> {
        self.inspect_scoped(tx, token, caller, allow_refresh, false)
            .await
    }
    async fn inspect_scoped(
        &self,
        tx: &DatabaseTransaction,
        token: &str,
        caller: Option<&str>,
        allow_refresh: bool,
        allow_preview: bool,
    ) -> Result<Value> {
        if token.len() > 16384 {
            return Ok(json!({"active":false}));
        }
        let access = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT * FROM webdock_auth.\"oauthAccessToken\" WHERE token=$1",
                [legacy_token_hash(token).into()],
            ))
            .await?;
        if access.is_none() && token.matches('.').count() == 2 {
            return self
                .inspect_legacy_jwt(tx, token, caller, allow_preview)
                .await;
        }
        let access = if let Some(row) = access {
            let Some(current) = Self::lock_access_record(tx, row).await? else {
                // A discovered row that disappeared/replaced must never use JWT fallback.
                return Ok(json!({"active":false}));
            };
            Some(current)
        } else {
            None
        };
        self.inspect_token_record(
            tx,
            access,
            allow_refresh.then(|| legacy_token_hash(token)).as_deref(),
            caller,
            allow_preview,
        )
        .await
    }
    /// Discover without a token lock; reload in refresh-compatible ownership order.
    /// Inactive discovery stays inactive, and missing/replaced discovery cannot authorize.
    async fn lock_access_record(
        tx: &DatabaseTransaction,
        row: sea_orm::QueryResult,
    ) -> Result<Option<sea_orm::QueryResult>> {
        if row
            .try_get::<Option<DateTime<Utc>>>("", "revoked")?
            .is_some()
            || row.try_get::<DateTime<Utc>>("", "expiresAt")? <= Utc::now()
        {
            return Ok(Some(row));
        }
        let id: String = row.try_get("", "id")?;
        let hash: String = row.try_get("", "token")?;
        let client: String = row.try_get("", "clientId")?;
        let subject: Option<String> = row.try_get("", "userId")?;
        let session: Option<String> = row.try_get("", "sessionId")?;
        // Refresh takes client/identity before token rows; retain these locks until commit.
        tx.query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM webdock_auth.\"oauthClient\" WHERE \"clientId\"=$1 FOR SHARE",
            [client.clone().into()],
        ))
        .await?;
        if let Some(subject) = &subject {
            tx.query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM webdock_auth.\"user\" WHERE id=$1 FOR SHARE",
                [subject.clone().into()],
            ))
            .await?;
            if let Some(session) = &session {
                tx.query_one_raw(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT id FROM webdock_auth.session WHERE id=$1 AND \"userId\"=$2 FOR SHARE",
                    [session.clone().into(), subject.clone().into()],
                ))
                .await?;
            }
        }
        let Some(current) = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT * FROM webdock_auth.\"oauthAccessToken\" WHERE id=$1 AND token=$2 FOR SHARE",
                [id.clone().into(), hash.clone().into()],
            ))
            .await?
        else {
            return Ok(None);
        };
        if current.try_get::<String>("", "id")? != id
            || current.try_get::<String>("", "token")? != hash
            || current.try_get::<String>("", "clientId")? != client
            || current.try_get::<Option<String>>("", "userId")? != subject
            || current.try_get::<Option<String>>("", "sessionId")? != session
        {
            return Ok(None);
        }
        Ok(Some(current))
    }
    async fn inspect_token_record(
        &self,
        tx: &DatabaseTransaction,
        access: Option<sea_orm::QueryResult>,
        refresh_hash: Option<&str>,
        caller: Option<&str>,
        allow_preview: bool,
    ) -> Result<Value> {
        let (row, is_refresh) = if let Some(row) = access {
            (row, false)
        } else if let Some(token_hash) = refresh_hash {
            let row = tx
                .query_one_raw(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT * FROM webdock_auth.\"oauthRefreshToken\" WHERE token=$1",
                    [token_hash.into()],
                ))
                .await?;
            let Some(row) = row else {
                return Ok(json!({"active":false}));
            };
            (row, true)
        } else {
            return Ok(json!({"active":false}));
        };
        if row
            .try_get::<Option<DateTime<Utc>>>("", "revoked")?
            .is_some()
            || row.try_get::<DateTime<Utc>>("", "expiresAt")? <= Utc::now()
        {
            return Ok(json!({"active":false}));
        }
        let client_id: String = row.try_get("", "clientId")?;
        let client = match load_client(tx, &client_id).await {
            Ok(c) => c,
            Err(Error::InvalidClient) => return Ok(json!({"active":false})),
            Err(e) => return Err(e),
        };
        let resources = json_strings(
            row.try_get::<Option<Value>>("", "resources")?
                .unwrap_or(json!([])),
        )?;
        if let Some(caller) = caller
            && caller != client_id
            && (is_refresh || !linked_any(tx, caller, &client.id, &resources).await?)
        {
            return Ok(json!({"active":false}));
        }
        let (Some(subject), Some(session)) = (
            row.try_get::<Option<String>>("", "userId")?,
            row.try_get::<Option<String>>("", "sessionId")?,
        ) else {
            return Ok(json!({"active":false}));
        };
        let principal = match self
            .inspection_principal(
                tx,
                &subject,
                &session,
                allow_preview && caller == Some(client_id.as_str()),
            )
            .await
        {
            Ok(p) => p,
            Err(Error::AccessDenied) => return Ok(json!({"active":false,"disabled":true})),
            Err(e) => return Err(e),
        };
        let claims = match self
            .inspection_claims(tx, &client, &principal, allow_preview)
            .await
        {
            Ok(c) => c,
            Err(Error::AccessDenied) => return Ok(json!({"active":false,"disabled":true})),
            Err(e) => return Err(e),
        };
        let scopes = json_strings(row.try_get("", "scopes")?)?;
        match self
            .resource_lifetimes(tx, &client, &resources, &scopes)
            .await
        {
            Ok(_) => {}
            Err(Error::AccessDenied | Error::InvalidRequest | Error::InvalidScope) => {
                return Ok(json!({"active":false}));
            }
            Err(error) => return Err(error),
        }
        let mut result = claims;
        result["active"] = json!(true);
        result["iss"] = json!(self.config.issuer.as_str());
        result["sub"] = json!(subject);
        result["sid"] = json!(session);
        result["client_id"] = json!(client_id);
        result["azp"] = result["client_id"].clone();
        result["exp"] = json!(row.try_get::<DateTime<Utc>>("", "expiresAt")?.timestamp());
        result["iat"] = json!(row.try_get::<DateTime<Utc>>("", "createdAt")?.timestamp());
        result["scope"] = json!(scopes.join(" "));
        result["token_type"] = json!(if is_refresh {
            "refresh_token"
        } else {
            "Bearer"
        });
        let mut audience = resources;
        if !audience.is_empty() && scopes.iter().any(|s| s == "openid") {
            audience.push(format!("{}/oauth2/userinfo", self.config.issuer));
        }
        if audience.len() == 1 {
            result["aud"] = json!(audience[0]);
        } else if !audience.is_empty() {
            result["aud"] = json!(audience);
        }
        Ok(result)
    }
    async fn inspect_legacy_jwt(
        &self,
        tx: &DatabaseTransaction,
        token: &str,
        caller: Option<&str>,
        allow_preview: bool,
    ) -> Result<Value> {
        let raw = match self.verify_signed_claims(tx, token, true).await {
            Ok(v) => v,
            Err(Error::InvalidGrant) => return Ok(json!({"active":false})),
            Err(e) => return Err(e),
        };
        let (Some(scope), Some(subject), Some(session), Some(client_id)) = (
            raw["scope"].as_str(),
            raw["sub"].as_str(),
            raw["sid"].as_str(),
            raw["azp"].as_str().or(raw["client_id"].as_str()),
        ) else {
            return Ok(json!({"active":false}));
        };
        if !raw["cnf"].is_null() {
            return Ok(json!({"active":false}));
        }
        let client = match load_client(tx, client_id).await {
            Ok(c) => c,
            Err(Error::InvalidClient) => return Ok(json!({"active":false})),
            Err(e) => return Err(e),
        };
        let principal = match self
            .inspection_principal(
                tx,
                subject,
                session,
                allow_preview && caller == Some(client_id),
            )
            .await
        {
            Ok(p) => p,
            Err(Error::AccessDenied) => return Ok(json!({"active":false})),
            Err(e) => return Err(e),
        };
        let mut claims = match self
            .inspection_claims(tx, &client, &principal, allow_preview)
            .await
        {
            Ok(p) => p,
            Err(Error::AccessDenied) => return Ok(json!({"active":false})),
            Err(e) => return Err(e),
        };
        let resources: Vec<String> = audiences(&raw)
            .into_iter()
            .filter(|a| a != &format!("{}/oauth2/userinfo", self.config.issuer))
            .collect();
        if let Some(caller) = caller
            && caller != client_id
            && !linked_any(tx, caller, &client.id, &resources).await?
        {
            return Ok(json!({"active":false}));
        }
        let scopes = scope_vec(scope)?;
        match self
            .resource_lifetimes(tx, &client, &resources, &scopes)
            .await
        {
            Ok(_) => {}
            Err(Error::AccessDenied | Error::InvalidScope | Error::InvalidRequest) => {
                return Ok(json!({"active":false}));
            }
            Err(e) => return Err(e),
        }
        for field in ["iss", "aud", "sub", "sid", "exp", "iat", "scope"] {
            claims[field] = raw[field].clone();
        }
        claims["client_id"] = json!(client_id);
        claims["azp"] = json!(client_id);
        claims["active"] = json!(true);
        claims["token_type"] = json!("Bearer");
        Ok(claims)
    }
    pub async fn userinfo(&self, token: &str) -> Result<Value> {
        let tx = self.db.begin().await?;
        let claims = self.inspect(&tx, token, None, false).await?;
        if claims["active"] != true
            || !claims["scope"]
                .as_str()
                .is_some_and(|s| s.split_whitespace().any(|s| s == "openid"))
        {
            return Err(Error::AccessDenied);
        }
        let principal = self
            .live_principal(
                &tx,
                claims["sub"].as_str().ok_or(Error::AccessDenied)?,
                claims["sid"].as_str().ok_or(Error::AccessDenied)?,
            )
            .await?;
        let mut user = json!({"sub":claims["sub"]});
        let scopes = claims["scope"].as_str().unwrap_or("");
        if scopes.split_whitespace().any(|s| s == "profile") {
            user["name"] = json!(principal.name);
        }
        if scopes.split_whitespace().any(|s| s == "email") {
            user["email"] = json!(principal.email);
            user["email_verified"] = json!(true);
        }
        tx.commit().await?;
        Ok(user)
    }
    pub async fn end_session(&self, cookie: &str, request: LogoutRequest) -> Result<LogoutOutcome> {
        if request.state.as_ref().is_some_and(|s| s.len() > 2048) {
            return Err(Error::InvalidRequest);
        }
        let current = self.identity.authenticate_cookie(cookie).await?;
        let tx = self.db.begin().await?;
        // Expired ID tokens remain valid logout hints if signed and bound to the current browser session.
        let claims = match self
            .verify_signed_claims(&tx, &request.id_token_hint, false)
            .await
        {
            Ok(c) => c,
            Err(Error::InvalidGrant) => return Ok(LogoutOutcome::ConfirmationRequired),
            Err(e) => return Err(e),
        };
        let client_id = claims["aud"].as_str().ok_or(Error::InvalidRequest)?;
        let client = load_client(&tx, client_id).await?;
        if !client.end_session {
            return Err(Error::AccessDenied);
        }
        let redirect = if let Some(uri) = request.post_logout_redirect_uri {
            if !client.logout_redirects.contains(&uri) {
                return Err(Error::InvalidRequest);
            }
            let mut url = url::Url::parse(&uri).map_err(|_| Error::InvalidRequest)?;
            if let Some(state) = request.state {
                url.query_pairs_mut().append_pair("state", &state);
            }
            Some(url.to_string())
        } else {
            None
        };
        let Some(current) = current else {
            return Ok(LogoutOutcome::ConfirmationRequired);
        };
        if claims["sub"] != current.user_id || claims["sid"] != current.session_id {
            return Ok(LogoutOutcome::ConfirmationRequired);
        }
        tx.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM webdock_auth.session WHERE id=$1 AND \"userId\"=$2",
            [current.session_id.into(), current.user_id.into()],
        ))
        .await?;
        tx.commit().await?;
        Ok(LogoutOutcome::Complete {
            redirect,
            set_cookie: self.identity.clear_cookie(),
        })
    }
    pub async fn revoke(&self, credentials: &ClientAuthentication, token: &str) -> Result<()> {
        let tx = self.db.begin().await?;
        let client = load_client(&tx, &credentials.client_id).await?;
        authenticate_client(&client, credentials)?;
        for table in ["oauthAccessToken", "oauthRefreshToken"] {
            tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,format!("UPDATE webdock_auth.\"{table}\" SET revoked=statement_timestamp() WHERE token=$1 AND \"clientId\"=$2"),[legacy_token_hash(token).into(),client.id.clone().into()])).await?;
        }
        if token.matches('.').count() == 2 {
            match self.verify_signed_claims(&tx, token, true).await {
                Ok(claims)
                    if claims["azp"].as_str().or(claims["client_id"].as_str())
                        == Some(client.id.as_str())
                        && claims["scope"].is_string() =>
                {
                    if let (Some(subject), Some(session), Some(exp), Some(iat)) = (
                        claims["sub"].as_str(),
                        claims["sid"].as_str(),
                        claims["exp"].as_i64(),
                        claims["iat"].as_i64(),
                    ) && let Some(owner)=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT u.id,s.id AS session_id FROM webdock_auth.\"user\" u LEFT JOIN webdock_auth.session s ON s.id=$2 AND s.\"userId\"=u.id WHERE u.id=$1",[subject.into(),session.into()])).await? {
                            let resources:Vec<String>=audiences(&claims).into_iter().filter(|a|a!=&format!("{}/oauth2/userinfo",self.config.issuer)).collect();
                            tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.\"oauthAccessToken\"(id,token,\"clientId\",\"userId\",\"sessionId\",scopes,resources,\"expiresAt\",\"createdAt\",revoked) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,statement_timestamp()) ON CONFLICT(token) DO UPDATE SET revoked=statement_timestamp() WHERE webdock_auth.\"oauthAccessToken\".\"clientId\"=excluded.\"clientId\"",[self.allocate_id().await?.into(),legacy_token_hash(token).into(),client.id.clone().into(),subject.into(),owner.try_get::<Option<String>>("","session_id")?.into(),json!(scope_vec(claims["scope"].as_str().ok_or(Error::InvalidRequest)?)?).into(),json!(resources).into(),date(exp)?.into(),date(iat)?.into()])).await?;
                    }
                }
                Ok(_) | Err(Error::InvalidGrant) => {}
                Err(error) => return Err(error),
            }
        }
        tx.commit().await?;
        Ok(())
    }
    async fn inspection_principal(
        &self,
        tx: &DatabaseTransaction,
        subject: &str,
        session: &str,
        allow_preview: bool,
    ) -> Result<Principal> {
        if !allow_preview {
            return self.live_principal(tx, subject, session).await;
        }
        let principal = self
            .identity
            .authenticate_session_in(tx, subject, session)
            .await?
            .ok_or(Error::AccessDenied)?;
        if !principal.can_access()
            || (principal.in_tenant_preview() && principal.role() != "operator")
        {
            return Err(Error::AccessDenied);
        }
        Ok(principal)
    }
    async fn inspection_claims(
        &self,
        tx: &DatabaseTransaction,
        client: &Client,
        principal: &Principal,
        allow_preview: bool,
    ) -> Result<Value> {
        if principal.in_tenant_preview() {
            if !allow_preview || self.config.studio_client_id.as_deref() != Some(&client.id) {
                return Err(Error::AccessDenied);
            }
            return Ok(
                json!({"disabled":false,"webdock_role":if principal.is_acting_customer() { "admin" } else { "reader" },"webdock_preview":true,"webdock_email":principal.email,"webdock_name":principal.name}),
            );
        }
        self.current_claims(tx, client, principal).await
    }
    async fn live_principal(
        &self,
        tx: &DatabaseTransaction,
        subject: &str,
        session: &str,
    ) -> Result<Principal> {
        let principal = self
            .identity
            .authenticate_session_in(tx, subject, session)
            .await?
            .ok_or(Error::AccessDenied)?;
        if !principal.can_access() || principal.in_tenant_preview() {
            return Err(Error::AccessDenied);
        }
        Ok(principal)
    }
    async fn organization_local(&self, tx: &DatabaseTransaction, org: &str) -> Result<bool> {
        let available:bool=tx.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT to_regprocedure('webdock_rls.local_organization_authority(text)') IS NOT NULL AS present")).await?.ok_or(Error::AccessDenied)?.try_get("","present")?;
        let sql = if available {
            "SELECT webdock_rls.local_organization_authority($1) AS allowed"
        } else {
            "SELECT NOT EXISTS(SELECT 1 FROM webdock_auth.clerk_organization WHERE organization_id=$1) AS allowed"
        };
        Ok(tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                sql,
                [org.into()],
            ))
            .await?
            .ok_or(Error::AccessDenied)?
            .try_get("", "allowed")?)
    }
    async fn current_claims(
        &self,
        tx: &DatabaseTransaction,
        client: &Client,
        user: &Principal,
    ) -> Result<Value> {
        let mut claims =
            json!({"disabled":false,"webdock_email":user.email,"webdock_name":user.name});
        if client.metadata["webdock_hosting"] == true {
            claims = json!({"disabled":false,"webdock_hosting":true});
            claims["webdock_role"] = json!(if user.is_operator() {
                "hosting-operator"
            } else {
                "hosting-customer"
            });
            return Ok(claims);
        }
        if client.metadata["webdock_mcp"] == true {
            if !user.is_operator() {
                return Err(Error::AccessDenied);
            }
            return Ok(json!({"disabled":false,"webdock_role":"operator"}));
        }
        let binding = client.metadata["webdock_binding"]
            .as_str()
            .ok_or(Error::AccessDenied)?;
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT organization_id FROM webdock_auth.app_binding WHERE id=$1 AND client_id=$2 AND enabled",[binding.into(),client.id.clone().into()])).await?.ok_or(Error::AccessDenied)?;
        let organization: Option<String> = row.try_get("", "organization_id")?;
        if client.metadata.get("webdock_operator_only").is_some() {
            let subjects = client.metadata["webdock_allowed_subjects"]
                .as_array()
                .ok_or(Error::AccessDenied)?;
            if client.metadata["webdock_operator_only"] != true
                || organization.is_some()
                || subjects.is_empty()
                || subjects.iter().any(|v| !v.as_str().is_some_and(valid_id))
                || !subjects.iter().any(|v| v == &json!(user.user_id))
                || !user.is_operator()
            {
                return Err(Error::AccessDenied);
            }
        }
        if let Some(org) = organization.as_deref()
            && !self.organization_local(tx, org).await?
        {
            return Err(Error::AccessDenied);
        }
        if user.is_operator() {
            claims["webdock_role"] = json!("operator");
            return Ok(claims);
        }
        if self.config.studio_client_id.as_deref() == Some(&client.id) && organization.is_none() {
            let memberships=tx.query_all_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT m.\"organizationId\" AS org FROM webdock_auth.member m JOIN webdock_auth.tenant_customer t ON t.organization_id=m.\"organizationId\" JOIN webdock_admin.customers c ON c.id=t.customer_id AND c.status='active' WHERE m.\"userId\"=$1",[user.user_id.clone().into()])).await?;
            let mut local = false;
            for row in memberships {
                if self
                    .organization_local(tx, &row.try_get::<String>("", "org")?)
                    .await?
                {
                    local = true;
                    break;
                }
            }
            if !local {
                return Err(Error::AccessDenied);
            }
            claims["webdock_role"] = json!("reader");
            return Ok(claims);
        }
        let organization = organization.ok_or(Error::AccessDenied)?;
        let disabled=tx.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT coalesce((settings->>'cmsEnabled')::boolean,true) AS enabled FROM webdock_auth.platform_settings WHERE id=true")).await?;
        if disabled.is_some_and(|r| r.try_get::<bool>("", "enabled").ok() == Some(false)) {
            return Err(Error::AccessDenied);
        }
        let allowed: bool = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_rls.billing_cms_allowed($1) AS allowed",
                [binding.into()],
            ))
            .await?
            .ok_or(Error::AccessDenied)?
            .try_get("", "allowed")?;
        if !allowed {
            return Err(Error::AccessDenied);
        }
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT CASE WHEN c.status='active' AND m.role IN ('owner','admin') THEN 'admin' ELSE g.role END AS role FROM webdock_auth.member m LEFT JOIN webdock_auth.tenant_customer t ON t.organization_id=m.\"organizationId\" LEFT JOIN webdock_admin.customers c ON c.id=t.customer_id LEFT JOIN webdock_auth.project_grant g ON g.user_id=m.\"userId\" AND g.binding_id=$2 WHERE m.\"userId\"=$1 AND m.\"organizationId\"=$3 AND (t.customer_id IS NULL OR c.status='active') AND (g.id IS NULL OR (g.enabled AND g.organization_id=$3)) AND ((c.status='active' AND m.role IN ('owner','admin')) OR g.enabled) LIMIT 1",[user.user_id.clone().into(),binding.into(),organization.into()])).await?.ok_or(Error::AccessDenied)?;
        let role: String = row.try_get("", "role")?;
        if !["reader", "editor", "admin"].contains(&role.as_str()) {
            return Err(Error::AccessDenied);
        }
        claims["webdock_role"] = json!(role);
        Ok(claims)
    }
    async fn resource_lifetimes(
        &self,
        tx: &DatabaseTransaction,
        client: &Client,
        resources: &[String],
        scopes: &[String],
    ) -> Result<(i64, i64)> {
        if resources.len() > 8 || scopes.iter().any(|s| !client.scopes.contains(s)) {
            return Err(Error::InvalidScope);
        }
        if client.metadata.get("webdock_operator_only").is_some()
            && !scopes.iter().any(|s| s == "openid")
        {
            return Err(Error::AccessDenied);
        }
        lock_resource_grants(tx, resources, &[&client.id]).await?;
        let mut access = 28800;
        let mut refresh = 28800;
        if (client.metadata["webdock_mcp"] == true || client.metadata["webdock_hosting"] == true)
            && !resources.contains(&self.config.mcp_resource.to_string())
        {
            return Err(Error::InvalidRequest);
        }
        for resource in resources {
            if resource == self.config.mcp_resource.as_str() {
                access = access.min(300);
            }
            let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT r.\"accessTokenTtl\",r.\"refreshTokenTtl\",r.\"allowedScopes\" FROM webdock_auth.\"oauthResource\" r JOIN webdock_auth.\"oauthClientResource\" c ON c.\"resourceId\"=r.identifier AND c.\"clientId\"=$1 WHERE r.identifier=$2 AND NOT coalesce(r.disabled,false) AND NOT coalesce(r.\"dpopBoundAccessTokensRequired\",false) FOR SHARE OF r,c",[client.id.clone().into(),resource.clone().into()])).await?.ok_or(Error::AccessDenied)?;
            let allowed = json_strings(
                row.try_get::<Option<Value>>("", "allowedScopes")?
                    .unwrap_or(json!([])),
            )?;
            if scopes.iter().any(|s| {
                !["openid", "profile", "email"].contains(&s.as_str()) && !allowed.contains(s)
            }) {
                return Err(Error::InvalidScope);
            }
            access = access.min(i64::from(
                row.try_get::<Option<i32>>("", "accessTokenTtl")?
                    .unwrap_or(28800),
            ));
            refresh = refresh.min(i64::from(
                row.try_get::<Option<i32>>("", "refreshTokenTtl")?
                    .unwrap_or(28800),
            ));
        }
        if access <= 0 || refresh <= 0 {
            return Err(Error::Configuration);
        }
        Ok((access, refresh))
    }
}
async fn load_client(tx: &DatabaseTransaction, id: &str) -> Result<Client> {
    let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT * FROM webdock_auth.\"oauthClient\" WHERE \"clientId\"=$1 AND NOT coalesce(disabled,false) AND NOT coalesce(\"dpopBoundAccessTokens\",false) AND (\"subjectType\" IS NULL OR \"subjectType\"='public') FOR SHARE",[id.into()])).await?.ok_or(Error::InvalidClient)?;
    let method = match row
        .try_get::<Option<String>>("", "tokenEndpointAuthMethod")?
        .as_deref()
        .unwrap_or("client_secret_post")
    {
        "none" => ClientAuthMethod::None,
        "client_secret_post" => ClientAuthMethod::ClientSecretPost,
        "client_secret_basic" => ClientAuthMethod::ClientSecretBasic,
        _ => return Err(Error::InvalidClient),
    };
    let mut metadata = row
        .try_get::<Option<Value>>("", "metadata")?
        .unwrap_or(json!({}));
    if let Some(text) = metadata.as_str() {
        metadata = serde_json::from_str(text).map_err(|_| Error::InvalidClient)?;
    }
    let redirects = json_strings(row.try_get("", "redirectUris")?)?;
    if redirects.is_empty() {
        return Err(Error::InvalidClient);
    }
    Ok(Client {
        id: id.into(),
        secret: row.try_get("", "clientSecret")?,
        name: row.try_get("", "name")?,
        redirects,
        logout_redirects: json_strings(
            row.try_get::<Option<Value>>("", "postLogoutRedirectUris")?
                .unwrap_or(json!([])),
        )?,
        scopes: json_strings(
            row.try_get::<Option<Value>>("", "scopes")?
                .unwrap_or(json!([])),
        )?,
        grants: json_strings(
            row.try_get::<Option<Value>>("", "grantTypes")?
                .unwrap_or(json!(["authorization_code"])),
        )?,
        method,
        skip_consent: row
            .try_get::<Option<bool>>("", "skipConsent")?
            .unwrap_or(false),
        end_session: row
            .try_get::<Option<bool>>("", "enableEndSession")?
            .unwrap_or(false),
        metadata,
    })
}
fn authenticate_client(client: &Client, credentials: &ClientAuthentication) -> Result<()> {
    if client.id != credentials.client_id
        || client.method != credentials.method
        || credentials
            .client_secret
            .as_ref()
            .is_some_and(|s| s.len() > 4096)
    {
        return Err(Error::InvalidClient);
    }
    let valid = match (&client.secret, &credentials.client_secret) {
        (None, None) => client.method == ClientAuthMethod::None,
        (Some(expected), Some(actual)) => bool::from(
            expected
                .as_bytes()
                .ct_eq(legacy_token_hash(actual).as_bytes()),
        ),
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidClient)
    }
}
fn scope_vec(raw: &str) -> Result<Vec<String>> {
    let scope: oxide_auth::primitives::scope::Scope =
        raw.parse().map_err(|_| Error::InvalidScope)?;
    Ok(scope.iter().map(str::to_owned).collect())
}
fn json_strings(value: Value) -> Result<Vec<String>> {
    match value {
        Value::Array(values) => values
            .into_iter()
            .map(|v| v.as_str().map(str::to_owned).ok_or(Error::InvalidRequest))
            .collect(),
        Value::String(value) => Ok(value.split_whitespace().map(str::to_owned).collect()),
        Value::Null => Ok(vec![]),
        _ => Err(Error::InvalidRequest),
    }
}
fn valid_id(raw: &str) -> bool {
    !raw.starts_with('0')
        && raw.bytes().all(|c| c.is_ascii_digit())
        && raw.parse::<i64>().is_ok_and(|n| n > 0)
}
fn date(seconds: i64) -> Result<DateTime<Utc>> {
    DateTime::from_timestamp(seconds, 0).ok_or(Error::InvalidRequest)
}
async fn now(tx: &DatabaseTransaction) -> Result<i64> {
    Ok(tx
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT floor(extract(epoch FROM statement_timestamp()))::bigint AS now",
        ))
        .await?
        .ok_or(Error::InvalidRequest)?
        .try_get("", "now")?)
}
async fn session_time(tx: &DatabaseTransaction, id: &str) -> Result<i64> {
    Ok(tx
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT \"createdAt\" FROM webdock_auth.session WHERE id=$1",
            [id.into()],
        ))
        .await?
        .ok_or(Error::AccessDenied)?
        .try_get::<DateTime<Utc>>("", "createdAt")?
        .timestamp())
}
async fn store_request(
    tx: &DatabaseTransaction,
    token: &str,
    kind: &str,
    context: &GrantContext,
) -> Result<()> {
    tx.execute_unprepared("DELETE FROM webdock_auth.identity_oauth_request WHERE token_hash IN (SELECT token_hash FROM webdock_auth.identity_oauth_request WHERE expires_at<=statement_timestamp() ORDER BY expires_at LIMIT 100)").await?;
    tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.identity_oauth_request(token_hash,kind,user_id,session_id,client_id,payload,expires_at) VALUES($1,$2,$3,$4,$5,$6,statement_timestamp()+interval '5 minutes')",[legacy_token_hash(token).into(),kind.into(),context.subject.clone().into(),context.session_id.clone().into(),context.request.client_id.clone().into(),serde_json::to_value(context).map_err(|_|Error::InvalidRequest)?.into()])).await?;
    Ok(())
}
async fn has_consent(
    tx: &DatabaseTransaction,
    client: &str,
    user: &str,
    request: &AuthorizationRequest,
) -> Result<bool> {
    let scopes = scope_vec(&request.scope)?;
    for row in tx.query_all_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT scopes,resources FROM webdock_auth.\"oauthConsent\" WHERE \"clientId\"=$1 AND \"userId\"=$2",[client.into(),user.into()])).await?{let permitted=json_strings(row.try_get("","scopes")?)?;let resources=json_strings(row.try_get::<Option<Value>>("","resources")?.unwrap_or(json!([])))?;if scopes.iter().all(|s|permitted.contains(s))&&request.resources.iter().all(|r|resources.contains(r)){return Ok(true)}}
    Ok(false)
}
/// Resource policy precedes grants, using deterministic order for every issuer and
/// introspector. Hold these locks through the caller's transaction, including a
/// native domain's final COMMIT; checking policy again alone leaves a commit gap.
async fn lock_resource_grants(
    tx: &DatabaseTransaction,
    resources: &[String],
    clients: &[&str],
) -> Result<()> {
    let resources = resources
        .iter()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    let clients = clients
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    for resource in &resources {
        tx.query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT identifier FROM webdock_auth.\"oauthResource\" WHERE identifier=$1 FOR SHARE",
            [(*resource).into()],
        ))
        .await?;
    }
    for resource in &resources {
        for client in &clients {
            tx.query_all_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM webdock_auth.\"oauthClientResource\" WHERE \"resourceId\"=$1 AND \"clientId\"=$2 ORDER BY id FOR SHARE",
                [(*resource).into(), (*client).into()],
            )).await?;
        }
    }
    Ok(())
}
async fn linked_any(
    tx: &DatabaseTransaction,
    client: &str,
    issuer: &str,
    resources: &[String],
) -> Result<bool> {
    // Both grant sets are acquired together; otherwise an introspector-first check
    // can invert the issuer/introspector order for a concurrent policy change.
    lock_resource_grants(tx, resources, &[client, issuer]).await?;
    for resource in resources.iter().collect::<std::collections::BTreeSet<_>>() {
        if tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id FROM webdock_auth.\"oauthClientResource\" WHERE \"clientId\"=$1 AND \"resourceId\"=$2 FOR SHARE",[client.into(),resource.clone().into()])).await?.is_some(){return Ok(true)}
    }
    Ok(false)
}
async fn revoke_family(tx: &DatabaseTransaction, client: &str, user: &str) -> Result<()> {
    for table in ["oauthAccessToken", "oauthRefreshToken"] {
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,format!("UPDATE webdock_auth.\"{table}\" SET revoked=statement_timestamp() WHERE \"clientId\"=$1 AND \"userId\"=$2"),[client.into(),user.into()])).await?;
    }
    Ok(())
}
fn audiences(claims: &Value) -> Vec<String> {
    match &claims["aud"] {
        Value::String(s) => vec![s.clone()],
        Value::Array(v) => v
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        _ => vec![],
    }
}

#[path = "mcp_authority.rs"]
mod mcp_authority;
pub use mcp_authority::{McpAuthentication, McpAuthority, McpGuard};
