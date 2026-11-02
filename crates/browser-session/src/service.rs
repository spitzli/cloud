use crate::*;
use cookie::{Cookie, CookieJar, Key, SameSite};
use openidconnect::{
    AccessToken, AccessTokenHash, ClientId, CsrfToken, IssuerUrl, Nonce, PkceCodeChallenge,
    PkceCodeVerifier,
    core::{CoreIdToken, CoreIdTokenVerifier, CoreJsonWebKeySet, CoreJwsSigningAlgorithm},
};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, Statement,
    TransactionTrait,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{str::FromStr, sync::Arc};
use subtle::ConstantTimeEq;
use webdock_identity::{IdentityService, Principal, SecretKeys};
use webdock_oauth::{CodeExchange, OAuthService};

pub struct BrowserSessions {
    db: DatabaseConnection,
    identity: Arc<IdentityService>,
    oauth: Arc<OAuthService>,
    keys: Arc<SecretKeys>,
    cookie_key: Key,
    pub config: BrowserConfig,
    legacy: Option<LegacyCookies>,
}
pub struct BrowserRedirect {
    pub location: String,
    pub set_cookies: Vec<String>,
}
impl BrowserSessions {
    pub fn new(
        db: DatabaseConnection,
        identity: Arc<IdentityService>,
        oauth: Arc<OAuthService>,
        keys: Arc<SecretKeys>,
        cookie_secret: &str,
        config: BrowserConfig,
    ) -> Result<Self> {
        if cookie_secret.len() < 32 {
            return Err(Error::Configuration);
        }
        Ok(Self {
            db,
            identity,
            oauth,
            keys,
            cookie_key: Key::derive_from(cookie_secret.as_bytes()),
            config,
            legacy: None,
        })
    }
    pub fn with_legacy(mut self, legacy: LegacyCookies) -> Self {
        self.legacy = Some(legacy);
        self
    }
    fn secure(&self) -> bool {
        self.config.app_origin.scheme() == "https"
    }
    pub fn cookie_name(&self) -> &'static str {
        if self.secure() {
            "__Host-webdock-console.session"
        } else {
            "webdock-console.session"
        }
    }
    fn flow_name(&self) -> &'static str {
        if self.secure() {
            "__Host-webdock-console.flow"
        } else {
            "webdock-console.flow"
        }
    }
    fn old_name(&self, flow: bool) -> &'static str {
        match (self.secure(), flow) {
            (true, true) => "__Host-webdock-sso-flow",
            (true, false) => "__Host-webdock-sso",
            (false, true) => "webdock-sso-flow",
            (false, false) => "webdock-sso",
        }
    }
    fn cookie(&self, name: &str, value: String, age: i64) -> String {
        let mut jar = CookieJar::new();
        jar.signed_mut(&self.cookie_key).add(
            Cookie::build((name.to_owned(), value))
                .http_only(true)
                .secure(self.secure())
                .same_site(SameSite::Lax)
                .path("/")
                .max_age(cookie::time::Duration::seconds(age))
                .build(),
        );
        jar.delta().next().expect("signed cookie").to_string()
    }
    fn read(&self, header: &str, name: &str) -> Result<Option<String>> {
        let Some(value) = unique_cookie(header, name)? else {
            return Ok(None);
        };
        let mut jar = CookieJar::new();
        jar.add_original(Cookie::new(name.to_owned(), value.to_owned()));
        jar.signed(&self.cookie_key)
            .get(name)
            .map(|v| Some(v.value().to_owned()))
            .ok_or(Error::Unauthorized)
    }
    fn clear(&self, name: &str) -> String {
        Cookie::build((name.to_owned(), String::new()))
            .http_only(true)
            .secure(self.secure())
            .same_site(SameSite::Lax)
            .path("/")
            .max_age(cookie::time::Duration::ZERO)
            .build()
            .to_string()
    }
    pub async fn begin(&self, origin: &str, return_to: &str) -> Result<BrowserRedirect> {
        let app = self.config.app(origin)?;
        let callback = app
            .join("/api/sso/callback")
            .map_err(|_| Error::Configuration)?
            .to_string();
        self.oauth
            .validate_portal_client(&self.config.credentials(), &callback)
            .await?;
        let return_to = safe_return_to(return_to).ok_or(Error::InvalidInput)?;
        self.prune_expired(32).await?;
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let state = CsrfToken::new_random();
        let nonce = Nonce::new_random();
        let cookie_token = CsrfToken::new_random().secret().clone();
        let flow = Flow {
            verifier: verifier.secret().clone(),
            state: state.secret().clone(),
            nonce: nonce.secret().clone(),
            return_to,
            origin: origin.into(),
        };
        self.db.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.identity_browser_flow(token_hash,app_origin,client_id,payload,expires_at) VALUES($1,$2,$3,$4,statement_timestamp()+interval '5 minutes')",[digest(&cookie_token).into(),origin.into(),self.config.client_id.clone().into(),self.keys.encrypt(&serde_json::to_string(&flow).map_err(|_|Error::Configuration)?)?.into()])).await?;
        let mut authorize = self
            .config
            .issuer
            .join("auth/oauth2/authorize")
            .map_err(|_| Error::Configuration)?;
        authorize.set_path(&format!("{}/oauth2/authorize", self.config.issuer.path()));
        authorize
            .query_pairs_mut()
            .append_pair("client_id", &self.config.client_id)
            .append_pair("redirect_uri", &callback)
            .append_pair("response_type", "code")
            .append_pair("scope", "openid profile email")
            .append_pair("state", state.secret())
            .append_pair("nonce", nonce.secret())
            .append_pair("code_challenge", challenge.as_str())
            .append_pair("code_challenge_method", "S256");
        Ok(BrowserRedirect {
            location: authorize.into(),
            set_cookies: vec![self.cookie(self.flow_name(), cookie_token, 300)],
        })
    }
    pub async fn callback(
        &self,
        origin: &str,
        header: &str,
        code: &str,
        state: &str,
    ) -> Result<BrowserRedirect> {
        let app = self.config.app(origin)?;
        if code.is_empty() || code.len() > 2048 || state.len() > 512 {
            return Err(Error::InvalidInput);
        }
        let callback = app
            .join("/api/sso/callback")
            .map_err(|_| Error::Configuration)?
            .to_string();
        self.oauth
            .validate_portal_client(&self.config.credentials(), &callback)
            .await?;
        let flow = if let Some(token) = self.read(header, self.flow_name())? {
            let tx = self.db.begin().await?;
            let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT payload FROM webdock_auth.identity_browser_flow WHERE token_hash=$1 AND app_origin=$2 AND client_id=$3 AND expires_at>statement_timestamp() FOR UPDATE",[digest(&token).into(),origin.into(),self.config.client_id.clone().into()])).await?.ok_or(Error::Unauthorized)?;
            let flow: Flow =
                serde_json::from_str(&self.keys.decrypt(&row.try_get::<String>("", "payload")?)?)
                    .map_err(|_| Error::Unauthorized)?;
            if !bool::from(flow.state.as_bytes().ct_eq(state.as_bytes())) || flow.origin != origin {
                return Err(Error::Unauthorized);
            }
            tx.execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM webdock_auth.identity_browser_flow WHERE token_hash=$1",
                [digest(&token).into()],
            ))
            .await?;
            tx.commit().await?;
            flow
        } else {
            let old = self
                .legacy
                .as_ref()
                .ok_or(Error::Unauthorized)?
                .read(header, self.old_name(true), 300)?
                .ok_or(Error::Unauthorized)?;
            if old["kind"] != "flow"
                || !bool::from(string(&old, "state")?.as_bytes().ct_eq(state.as_bytes()))
            {
                return Err(Error::Unauthorized);
            }
            Flow {
                verifier: string(&old, "verifier")?.into(),
                state: state.into(),
                nonce: string(&old, "nonce")?.into(),
                return_to: safe_return_to(old["returnTo"].as_str().unwrap_or("/"))
                    .ok_or(Error::Unauthorized)?,
                origin: origin.into(),
            }
        };
        let tokens = self
            .oauth
            .exchange_code(CodeExchange {
                client: self.config.credentials(),
                code: code.into(),
                redirect_uri: callback,
                code_verifier: PkceCodeVerifier::new(flow.verifier).secret().clone(),
            })
            .await?;
        let id_token = tokens.id_token.as_ref().ok_or(Error::Unauthorized)?;
        let id_token = CoreIdToken::from_str(id_token).map_err(|_| Error::Unauthorized)?;
        let jwks: CoreJsonWebKeySet =
            serde_json::from_value(self.oauth.jwks().await?).map_err(|_| Error::Unauthorized)?;
        let verifier = CoreIdTokenVerifier::new_public_client(
            ClientId::new(self.config.client_id.clone()),
            IssuerUrl::new(self.config.issuer.to_string()).map_err(|_| Error::Configuration)?,
            jwks,
        )
        .set_allowed_algs([CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256]);
        let claims = id_token
            .claims(&verifier, &Nonce::new(flow.nonce))
            .map_err(|_| Error::Unauthorized)?;
        let expected = AccessTokenHash::from_token(
            &AccessToken::new(tokens.access_token.clone()),
            id_token.signing_alg().map_err(|_| Error::Unauthorized)?,
            id_token
                .signing_key(&verifier)
                .map_err(|_| Error::Unauthorized)?,
        )
        .map_err(|_| Error::Unauthorized)?;
        if claims.access_token_hash() != Some(&expected) {
            return Err(Error::Unauthorized);
        }
        let current = self
            .oauth
            .introspect_portal(&self.config.credentials(), &tokens.access_token)
            .await?;
        let principal = self
            .principal(&current, Some(claims.subject().as_str()))
            .await?;
        let expires = expiration(&current)?
            .min(principal.expires_at.timestamp())
            .min(chrono::Utc::now().timestamp() + 28800);
        let token = CsrfToken::new_random().secret().clone();
        let stored = StoredTokens {
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            id_token: tokens.id_token,
            role_ceiling: Some(string(&current, "webdock_role")?.into()),
            scopes: string(&current, "scope")?
                .split_whitespace()
                .map(str::to_owned)
                .collect(),
        };
        self.db.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.identity_browser_session(token_hash,app_origin,client_id,user_id,session_id,payload,expires_at) VALUES($1,$2,$3,$4,$5,$6,to_timestamp($7))",[digest(&token).into(),origin.into(),self.config.client_id.clone().into(),principal.user_id.into(),principal.session_id.into(),self.keys.encrypt(&serde_json::to_string(&stored).map_err(|_|Error::Configuration)?)?.into(),expires.into()])).await?;
        Ok(BrowserRedirect {
            location: app
                .join(&flow.return_to)
                .map_err(|_| Error::InvalidInput)?
                .into(),
            set_cookies: vec![
                self.cookie(
                    self.cookie_name(),
                    token,
                    expires - chrono::Utc::now().timestamp(),
                ),
                self.clear(self.flow_name()),
                self.clear(self.old_name(true)),
                self.clear(self.old_name(false)),
                self.clear("payload-token"),
            ],
        })
    }
    pub async fn authenticate(&self, origin: &str, header: &str) -> Result<Option<Principal>> {
        self.resolve_authority(origin, header)
            .await
            .map(|a| a.map(|v| v.principal))
    }
    pub async fn resolve_authority(
        &self,
        origin: &str,
        header: &str,
    ) -> Result<Option<BrowserAuthentication>> {
        let app = self.config.app(origin)?;
        self.oauth
            .validate_portal_client(
                &self.config.credentials(),
                app.join("/api/sso/callback")
                    .map_err(|_| Error::Configuration)?
                    .as_str(),
            )
            .await?;
        let reference = if let Some(token) = self.read(header, self.cookie_name())? {
            digest(&token)
        } else {
            let Some(legacy) = &self.legacy else {
                return Ok(None);
            };
            let Some(stored) = legacy.read(header, self.old_name(false), 28800)? else {
                return Ok(None);
            };
            if stored["kind"] != "session" {
                return Err(Error::Unauthorized);
            }
            let claims = self
                .oauth
                .introspect_portal(&self.config.credentials(), string(&stored, "accessToken")?)
                .await?;
            let principal = self
                .principal(&claims, Some(string(&stored, "sub")?))
                .await?;
            let reference = digest(&format!(
                "legacy:{origin}:{}",
                unique_cookie(header, self.old_name(false))?.ok_or(Error::Unauthorized)?
            ));
            let expires = expiration(&claims)?
                .min(expiration(&stored)?)
                .min(legacy.accept_until)
                .min(principal.expires_at.timestamp());
            let tokens = StoredTokens {
                access_token: string(&stored, "accessToken")?.into(),
                refresh_token: None,
                id_token: stored["idToken"].as_str().map(str::to_owned),
                role_ceiling: Some(
                    if principal.in_tenant_preview() {
                        "operator"
                    } else {
                        string(&claims, "webdock_role")?
                    }
                    .into(),
                ),
                scopes: string(&claims, "scope")?
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect(),
            };
            // This records the same bounded legacy authority; it issues no new browser credential.
            self.db.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.identity_browser_session(token_hash,app_origin,client_id,user_id,session_id,payload,expires_at) VALUES($1,$2,$3,$4,$5,$6,to_timestamp($7)) ON CONFLICT(token_hash) DO NOTHING",[reference.clone().into(),origin.into(),self.config.client_id.clone().into(),principal.user_id.into(),principal.session_id.into(),self.keys.encrypt(&serde_json::to_string(&tokens).map_err(|_|Error::Configuration)?)?.into(),expires.into()])).await?;
            reference
        };
        self.reference(origin, &reference, None).await
    }
    /// Backend/gateway revalidation. Saved role/scopes are a ceiling, never a replacement for live proof.
    pub async fn revalidate(&self, authority: &BrowserAuthority) -> Result<BrowserAuthentication> {
        let tx = self.db.begin().await?;
        let result = self.revalidate_in(&tx, authority).await?;
        tx.commit().await?;
        Ok(result)
    }
    /// Reuses the caller's authentication transaction for every proof and identity read.
    pub async fn revalidate_in(
        &self,
        tx: &DatabaseTransaction,
        authority: &BrowserAuthority,
    ) -> Result<BrowserAuthentication> {
        if authority.client_id != self.config.client_id
            || authority.expires_at <= chrono::Utc::now().timestamp()
        {
            return Err(Error::Unauthorized);
        }
        let mut result = self
            .reference_in(
                tx,
                &authority.app_origin,
                &authority.reference,
                Some(authority),
            )
            .await?
            .ok_or(Error::Unauthorized)?;
        result.authority.expires_at = result.authority.expires_at.min(authority.expires_at);
        result.principal.expires_at =
            chrono::DateTime::from_timestamp(result.authority.expires_at, 0)
                .ok_or(Error::Unauthorized)?;
        Ok(result)
    }
    async fn reference(
        &self,
        origin: &str,
        reference: &str,
        ceiling: Option<&BrowserAuthority>,
    ) -> Result<Option<BrowserAuthentication>> {
        let tx = self.db.begin().await?;
        let result = self.reference_in(&tx, origin, reference, ceiling).await?;
        tx.commit().await?;
        Ok(result)
    }
    async fn reference_in(
        &self,
        tx: &DatabaseTransaction,
        origin: &str,
        reference: &str,
        ceiling: Option<&BrowserAuthority>,
    ) -> Result<Option<BrowserAuthentication>> {
        let app = self.config.app(origin)?;
        if reference.len() != 64 || !reference.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::Unauthorized);
        }
        self.oauth
            .validate_portal_client_in(
                tx,
                &self.config.credentials(),
                app.join("/api/sso/callback")
                    .map_err(|_| Error::Configuration)?
                    .as_str(),
            )
            .await?;
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT user_id,session_id,payload,expires_at FROM webdock_auth.identity_browser_session WHERE token_hash=$1 AND app_origin=$2 AND client_id=$3 AND expires_at>statement_timestamp() FOR SHARE",[reference.into(),origin.into(),self.config.client_id.clone().into()])).await?;
        let Some(row) = row else { return Ok(None) };
        let stored: StoredTokens =
            serde_json::from_str(&self.keys.decrypt(&row.try_get::<String>("", "payload")?)?)
                .map_err(|_| Error::Unauthorized)?;
        let stored_role = stored.role_ceiling.as_deref().ok_or(Error::Unauthorized)?;
        if stored.scopes.is_empty() {
            return Err(Error::Unauthorized);
        }
        let claims = self
            .oauth
            .introspect_portal_in(tx, &self.config.credentials(), &stored.access_token)
            .await?;
        let mut principal = self
            .principal_in(tx, &claims, Some(&row.try_get::<String>("", "user_id")?))
            .await?;
        let role = string(&claims, "webdock_role")?;
        if principal.session_id != row.try_get::<String>("", "session_id")?
            || principal.is_operator() && stored_role != "operator"
        {
            return Err(Error::Unauthorized);
        }
        let scopes: Vec<String> = string(&claims, "scope")?
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        if !stored.scopes.is_empty() && scopes.iter().any(|v| !stored.scopes.contains(v)) {
            return Err(Error::Unauthorized);
        }
        if let Some(ceiling) = ceiling
            && (ceiling.customer_scope.as_deref() != principal.customer_scope()
                || (principal.is_operator() && ceiling.role != "operator")
                || scopes.iter().any(|v| !ceiling.scopes.contains(v)))
        {
            return Err(Error::Unauthorized);
        }
        let expires = expiration(&claims)?
            .min(
                row.try_get::<chrono::DateTime<chrono::Utc>>("", "expires_at")?
                    .timestamp(),
            )
            .min(principal.expires_at.timestamp());
        principal.expires_at =
            chrono::DateTime::from_timestamp(expires, 0).ok_or(Error::Unauthorized)?;
        let authority = BrowserAuthority {
            reference: reference.into(),
            client_id: self.config.client_id.clone(),
            app_origin: origin.into(),
            expires_at: expires,
            role: role.into(),
            scopes,
            read_only: principal.in_read_only_preview() || ceiling.is_some_and(|v| v.read_only),
            customer_scope: principal.customer_scope().map(str::to_owned),
        };
        Ok(Some(BrowserAuthentication {
            principal,
            authority,
        }))
    }
    async fn principal(&self, claims: &Value, expected_subject: Option<&str>) -> Result<Principal> {
        let tx = self.db.begin().await?;
        let result = self.principal_in(&tx, claims, expected_subject).await?;
        tx.commit().await?;
        Ok(result)
    }
    async fn principal_in(
        &self,
        tx: &DatabaseTransaction,
        claims: &Value,
        expected_subject: Option<&str>,
    ) -> Result<Principal> {
        if claims["active"] != true
            || claims["client_id"] != self.config.client_id
            || claims["disabled"] == true
            || expiration(claims)? <= chrono::Utc::now().timestamp()
            || claims["token_type"] != "Bearer"
            || ["openid", "profile", "email"].iter().any(|required| {
                !string(claims, "scope")
                    .unwrap_or("")
                    .split_whitespace()
                    .any(|v| v == *required)
            })
        {
            return Err(Error::Unauthorized);
        }
        let subject = string(claims, "sub")?;
        if expected_subject.is_some_and(|v| v != subject) {
            return Err(Error::Unauthorized);
        }
        let principal = self
            .identity
            .authenticate_session_in(tx, subject, string(claims, "sid")?)
            .await?
            .ok_or(Error::Unauthorized)?;
        let role = string(claims, "webdock_role")?;
        if !principal.can_access()
            || !matches!(role, "operator" | "admin" | "editor" | "reader")
            || if principal.in_tenant_preview() {
                claims["webdock_preview"] != true
                    || role
                        != if principal.is_acting_customer() {
                            "admin"
                        } else {
                            "reader"
                        }
                    || principal.role() != "operator"
            } else {
                principal.is_operator() != (role == "operator")
            }
        {
            return Err(Error::Unauthorized);
        }
        Ok(principal)
    }
    pub fn has_cookie(&self, header: &str) -> bool {
        header.split(';').map(str::trim).any(|v| {
            [self.cookie_name(), self.old_name(false)]
                .iter()
                .any(|name| v.starts_with(&format!("{name}=")))
        })
    }
    pub async fn logout(&self, origin: &str, header: &str) -> Result<BrowserRedirect> {
        let app = self.config.app(origin)?;
        let mut proof = None;
        if let Some(token) = self.read(header, self.cookie_name())? {
            if let Some(row)=self.db.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"DELETE FROM webdock_auth.identity_browser_session WHERE token_hash=$1 AND app_origin=$2 AND client_id=$3 RETURNING payload,user_id,session_id",[digest(&token).into(),origin.into(),self.config.client_id.clone().into()])).await? {
                let stored:StoredTokens=serde_json::from_str(&self.keys.decrypt(&row.try_get::<String>("","payload")?)?).map_err(|_|Error::Unauthorized)?;
                self.oauth.revoke(&self.config.credentials(),&stored.access_token).await?;
                if let Some(refresh)=stored.refresh_token.as_deref(){self.oauth.revoke(&self.config.credentials(),refresh).await?;}
                proof=Some(serde_json::json!({"kind":"logout","subject":row.try_get::<String>("","user_id")?,"session_id":row.try_get::<String>("","session_id")?,"origin":origin}));
            }
        } else if let Some(legacy) = &self.legacy
            && let Some(stored) = legacy.read(header, self.old_name(false), 28800)?
        {
            let claims = self
                .oauth
                .introspect_portal(&self.config.credentials(), string(&stored, "accessToken")?)
                .await?;
            self.oauth
                .revoke(&self.config.credentials(), string(&stored, "accessToken")?)
                .await?;
            if claims["active"] == true && claims["sub"] == stored["sub"] {
                proof = Some(
                    serde_json::json!({"kind":"logout","subject":stored["sub"],"session_id":claims["sid"],"origin":origin}),
                );
            }
        }
        let mut location = app.join("/login").map_err(|_| Error::Configuration)?;
        if let Some(proof) = proof {
            let token = CsrfToken::new_random().secret().clone();
            self.db.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.identity_browser_flow(token_hash,app_origin,client_id,payload,expires_at) VALUES($1,$2,$3,$4,statement_timestamp()+interval '5 minutes')",[digest(&token).into(),origin.into(),self.config.client_id.clone().into(),self.keys.encrypt(&proof.to_string())?.into()])).await?;
            location = self.config.issuer.clone();
            location.set_path("/logout");
            location.query_pairs_mut().append_pair("request", &token);
        }
        Ok(BrowserRedirect {
            location: location.into(),
            set_cookies: vec![
                self.clear(self.cookie_name()),
                self.clear(self.flow_name()),
                self.clear(self.old_name(false)),
                self.clear(self.old_name(true)),
                self.clear("payload-token"),
            ],
        })
    }
    /// Bounded housekeeping for expired authentication-only sidecars; no active authority is removed.
    pub async fn prune_expired(&self, limit: u32) -> Result<u64> {
        if limit == 0 || limit > 10000 {
            return Err(Error::InvalidInput);
        }
        let mut count = 0;
        for table in ["identity_browser_flow", "identity_browser_session"] {
            count+=self.db.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,format!("WITH expired AS (SELECT token_hash FROM webdock_auth.{table} WHERE expires_at<=statement_timestamp() ORDER BY expires_at LIMIT $1 FOR UPDATE SKIP LOCKED) DELETE FROM webdock_auth.{table} t USING expired e WHERE t.token_hash=e.token_hash"),[i64::from(limit).into()])).await?.rows_affected();
        }
        Ok(count)
    }
    pub async fn logout_details(&self, cookie: &str, request: &str) -> Result<Value> {
        let proof = self.logout_proof(cookie, request).await?;
        Ok(serde_json::json!({"applicationOrigin":proof["origin"],"request":request}))
    }
    async fn logout_proof(&self, cookie: &str, request: &str) -> Result<Value> {
        if request.len() > 512 {
            return Err(Error::InvalidInput);
        }
        let row=self.db.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT payload FROM webdock_auth.identity_browser_flow WHERE token_hash=$1 AND client_id=$2 AND expires_at>statement_timestamp()",[digest(request).into(),self.config.client_id.clone().into()])).await?.ok_or(Error::Unauthorized)?;
        let proof: Value =
            serde_json::from_str(&self.keys.decrypt(&row.try_get::<String>("", "payload")?)?)
                .map_err(|_| Error::Unauthorized)?;
        if proof["kind"] != "logout" {
            return Err(Error::Unauthorized);
        }
        let principal = self
            .identity
            .authenticate_cookie(cookie)
            .await?
            .ok_or(Error::Unauthorized)?;
        if proof["subject"] != principal.user_id || proof["session_id"] != principal.session_id {
            return Err(Error::Unauthorized);
        }
        self.config.app(string(&proof, "origin")?)?;
        Ok(proof)
    }
    pub async fn finish_logout(
        &self,
        cookie: &str,
        request: &str,
        confirmed: bool,
    ) -> Result<BrowserRedirect> {
        let proof = self.logout_proof(cookie, request).await?;
        let deleted=self.db.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"DELETE FROM webdock_auth.identity_browser_flow WHERE token_hash=$1 AND client_id=$2 AND expires_at>statement_timestamp()",[digest(request).into(),self.config.client_id.clone().into()])).await?;
        if deleted.rows_affected() != 1 {
            return Err(Error::Unauthorized);
        }
        let app = self.config.app(string(&proof, "origin")?)?;
        let mut set_cookies = vec![];
        if confirmed {
            // Explicit confirmation already proves ownership of this exact current native session.
            // A retired provider key or disabled client must not prevent its owner signing out.
            self.identity
                .revoke_session(string(&proof, "subject")?, string(&proof, "session_id")?)
                .await?;
            set_cookies.push(self.identity.clear_cookie());
        }

        Ok(BrowserRedirect {
            location: app
                .join(if confirmed { "/login" } else { "/projects" })
                .map_err(|_| Error::Configuration)?
                .into(),
            set_cookies,
        })
    }
}
pub(crate) fn unique_cookie<'a>(header: &'a str, name: &str) -> Result<Option<&'a str>> {
    if header.len() > 16384 {
        return Err(Error::InvalidInput);
    }
    let prefix = format!("{name}=");
    let mut values = header
        .split(';')
        .filter_map(|v| v.trim().strip_prefix(&prefix));
    let value = values.next();
    if values.next().is_some() || value.is_some_and(|v| v.len() > 4096) {
        return Err(Error::Unauthorized);
    }
    Ok(value)
}
fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .filter(|v| !v.is_empty())
        .ok_or(Error::Unauthorized)
}
fn expiration(value: &Value) -> Result<i64> {
    value["exp"].as_i64().ok_or(Error::Unauthorized)
}
fn digest(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}
