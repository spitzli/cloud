//! Native token discovery, ownership fences and current policy. No generic ORM translation.
use super::{
    OAuthService, audiences, authenticate_client, json_strings, native_client_store, scope_vec,
    valid_id,
};
use crate::{ClientAuthentication, Error, Result, legacy_token_hash, types::Client};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use std::{collections::BTreeSet, time::Duration};
use tokio_postgres::types::Type;
use webdock_identity::Principal;
use webdock_persistence::postgres::Transaction;
const BUDGET: Duration = Duration::from_secs(30);
struct TokenRecord {
    id: String,
    hash: String,
    client: String,
    user: Option<String>,
    session: Option<String>,
    scopes: Value,
    resources: Option<Value>,
    expires: DateTime<Utc>,
    created: DateTime<Utc>,
    revoked: Option<DateTime<Utc>>,
    alive: bool,
}
impl TokenRecord {
    fn decode(row: &tokio_postgres::Row) -> std::result::Result<Self, tokio_postgres::Error> {
        Ok(Self {
            id: row.try_get("id")?,
            hash: row.try_get("token")?,
            client: row.try_get("clientId")?,
            user: row.try_get("userId")?,
            session: row.try_get("sessionId")?,
            scopes: row.try_get("scopes")?,
            resources: row.try_get("resources")?,
            expires: row.try_get("expiresAt")?,
            created: row.try_get("createdAt")?,
            revoked: row.try_get("revoked")?,
            alive: row.try_get("alive")?,
        })
    }
    fn same_owner(&self, other: &Self) -> bool {
        self.id == other.id
            && self.hash == other.hash
            && self.client == other.client
            && self.user == other.user
            && self.session == other.session
    }
}
const COLUMNS: &str = r#"id,token,"clientId","userId","sessionId",scopes,resources,"expiresAt","createdAt",revoked,(revoked IS NULL AND "expiresAt">statement_timestamp()) AS alive"#;
fn inactive() -> Value {
    json!({"active":false})
}
fn disabled() -> Value {
    json!({"active":false,"disabled":true})
}
fn table(refresh: bool) -> &'static str {
    if refresh {
        "oauthRefreshToken"
    } else {
        "oauthAccessToken"
    }
}
async fn discover(tx: &Transaction<'_>, hash: &str, refresh: bool) -> Result<Option<TokenRecord>> {
    let sql = format!(
        "SELECT {COLUMNS} FROM webdock_auth.\"{}\" WHERE token=$1",
        table(refresh)
    );
    tx.query_typed_opt(&sql, &[(&hash, Type::TEXT)])
        .await
        .map_err(|_| Error::Configuration)?
        .as_ref()
        .map(TokenRecord::decode)
        .transpose()
        .map_err(|_| Error::Configuration)
}
async fn ownership_locks(tx: &Transaction<'_>, record: &TokenRecord) -> Result<()> {
    tx.query_typed_opt(
        r#"SELECT id FROM webdock_auth."oauthClient" WHERE "clientId"=$1 FOR SHARE"#,
        &[(&record.client, Type::TEXT)],
    )
    .await
    .map_err(|_| Error::Configuration)?;
    if let Some(user) = &record.user {
        tx.query_typed_opt(
            r#"SELECT id FROM webdock_auth."user" WHERE id=$1 FOR SHARE"#,
            &[(user, Type::TEXT)],
        )
        .await
        .map_err(|_| Error::Configuration)?;
        if let Some(session) = &record.session {
            tx.query_typed_opt(
                r#"SELECT id FROM webdock_auth.session WHERE id=$1 AND "userId"=$2 FOR SHARE"#,
                &[(session, Type::TEXT), (user, Type::TEXT)],
            )
            .await
            .map_err(|_| Error::Configuration)?;
        }
    }
    Ok(())
}
async fn reload_fenced(
    tx: &Transaction<'_>,
    discovered: TokenRecord,
    refresh: bool,
) -> Result<Option<TokenRecord>> {
    if !discovered.alive || discovered.revoked.is_some() {
        return Ok(Some(discovered));
    }
    ownership_locks(tx, &discovered).await?;
    let lock = format!(
        "SELECT id FROM webdock_auth.\"{}\" WHERE id=$1 AND token=$2 FOR SHARE",
        table(refresh)
    );
    if tx
        .query_typed_opt(
            &lock,
            &[(&discovered.id, Type::TEXT), (&discovered.hash, Type::TEXT)],
        )
        .await
        .map_err(|_| Error::Configuration)?
        .is_none()
    {
        return Ok(None);
    }
    // Fresh statement after all lock waits; never revive missing/replaced discovery.
    let current = discover(tx, &discovered.hash, refresh).await?;
    Ok(current.filter(|current| discovered.same_owner(current)))
}
async fn lock_resources(
    tx: &Transaction<'_>,
    resources: &[String],
    clients: &[&str],
) -> Result<()> {
    let resources = resources
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let clients = clients.iter().copied().collect::<BTreeSet<_>>();
    for resource in &resources {
        tx.query_typed_opt(
            r#"SELECT identifier FROM webdock_auth."oauthResource" WHERE identifier=$1 FOR SHARE"#,
            &[(resource, Type::TEXT)],
        )
        .await
        .map_err(|_| Error::Configuration)?;
    }
    for resource in &resources {
        for client in &clients {
            tx.query_typed(r#"SELECT id FROM webdock_auth."oauthClientResource" WHERE "resourceId"=$1 AND "clientId"=$2 ORDER BY id FOR SHARE"#,&[(resource,Type::TEXT),(client,Type::TEXT)]).await.map_err(|_|Error::Configuration)?;
        }
    }
    Ok(())
}
async fn linked_any(
    tx: &Transaction<'_>,
    caller: &str,
    issuer: &str,
    resources: &[String],
) -> Result<bool> {
    lock_resources(tx, resources, &[caller, issuer]).await?;
    for resource in resources.iter().collect::<BTreeSet<_>>() {
        if tx.query_typed_opt(r#"SELECT id FROM webdock_auth."oauthClientResource" WHERE "clientId"=$1 AND "resourceId"=$2 FOR SHARE"#,&[(&caller,Type::TEXT),(resource,Type::TEXT)]).await.map_err(|_|Error::Configuration)?.is_some(){return Ok(true);}
    }
    Ok(false)
}
impl OAuthService {
    pub(crate) async fn native_introspect(
        &self,
        credentials: &ClientAuthentication,
        token: &str,
        portal: bool,
        mcp: bool,
    ) -> Result<Value> {
        tokio::time::timeout(BUDGET, async {
            let mut connection = self
                .native_introspection_pool()
                .ok_or(Error::Configuration)?
                .acquire()
                .await
                .map_err(|_| Error::Configuration)?;
            let tx = connection
                .transaction()
                .await
                .map_err(|_| Error::Configuration)?;
            let (tx, value) = self
                .native_introspect_in(tx, credentials, token, portal, mcp)
                .await?;
            tx.commit().await.map_err(|_| Error::Configuration)?;
            Ok(value)
        })
        .await
        .map_err(|_| Error::Configuration)?
    }
    pub(super) async fn native_introspect_in<'a>(
        &self,
        tx: Transaction<'a>,
        credentials: &ClientAuthentication,
        token: &str,
        portal: bool,
        mcp: bool,
    ) -> Result<(Transaction<'a>, Value)> {
        if portal {
            self.portal_credentials(credentials)?;
        }
        tokio::time::timeout(BUDGET,async move{
   let client=native_client_store::load_client(&tx,&credentials.client_id).await?;authenticate_client(&client,credentials)?;
   if portal{let binding=client.metadata["webdock_binding"].as_str().ok_or(Error::InvalidClient)?;if client.metadata.get("webdock_operator_only").is_some()||client.metadata.get("webdock_mcp").is_some()||client.metadata.get("webdock_hosting").is_some()||tx.query_typed_opt("SELECT id FROM webdock_auth.app_binding WHERE id=$1 AND client_id=$2 AND enabled AND organization_id IS NULL",&[(&binding,Type::TEXT),(&client.id,Type::TEXT)]).await.map_err(|_|Error::Configuration)?.is_none(){return Err(Error::InvalidClient);}}
   let(tx,claims)=self.inspect_native(tx,token,Some(&client.id),!portal,portal).await?;
   if mcp{
    if claims["active"]!=true||claims["token_type"]!="Bearer"||!audiences(&claims).contains(&self.config.mcp_resource.to_string())||!["operator","hosting-operator","hosting-customer"].contains(&claims["webdock_role"].as_str().unwrap_or("")){return Ok((tx,inactive()));}
    let issuer=native_client_store::load_client(&tx,claims["client_id"].as_str().ok_or(Error::AccessDenied)?).await?;if issuer.metadata["webdock_mcp"]!=true&&issuer.metadata["webdock_hosting"]!=true{return Ok((tx,inactive()));}
   }Ok((tx,claims))
  }).await.map_err(|_|Error::Configuration)?
    }
    async fn native_principal<'a>(
        &self,
        tx: Transaction<'a>,
        user: &str,
        session: &str,
        preview: bool,
    ) -> Result<(Transaction<'a>, Option<Principal>)> {
        let (tx, principal) = self
            .identity
            .authenticate_session_native_in(tx, user, session)
            .await?;
        let principal = principal.filter(|p| {
            p.can_access()
                && (preview || !p.in_tenant_preview())
                && (!p.in_tenant_preview() || p.role() == "operator")
        });
        Ok((tx, principal))
    }
    async fn inspect_native<'a>(
        &self,
        tx: Transaction<'a>,
        token: &str,
        caller: Option<&str>,
        allow_refresh: bool,
        allow_preview: bool,
    ) -> Result<(Transaction<'a>, Value)> {
        if token.len() > 16384 {
            return Ok((tx, inactive()));
        }
        let hash = legacy_token_hash(token);
        let access = discover(&tx, &hash, false).await?;
        if access.is_none() && token.matches('.').count() == 2 {
            return self
                .inspect_native_jwt(tx, token, caller, allow_preview)
                .await;
        }
        let (refresh, record) = match access {
            Some(row) => (false, reload_fenced(&tx, row, false).await?),
            None if allow_refresh => {
                let row = discover(&tx, &hash, true).await?;
                (
                    true,
                    match row {
                        Some(row) => reload_fenced(&tx, row, true).await?,
                        None => None,
                    },
                )
            }
            None => (false, None),
        };
        let Some(record) = record else {
            return Ok((tx, inactive()));
        };
        if !record.alive || record.revoked.is_some() {
            return Ok((tx, inactive()));
        }
        let client = match native_client_store::load_client(&tx, &record.client).await {
            Ok(client) => client,
            Err(Error::InvalidClient) => return Ok((tx, inactive())),
            Err(error) => return Err(error),
        };
        let resources = json_strings(record.resources.clone().unwrap_or(json!([])))?;
        if let Some(caller) = caller
            && caller != client.id
            && (refresh || !linked_any(&tx, caller, &client.id, &resources).await?)
        {
            return Ok((tx, inactive()));
        }
        let (Some(user), Some(session)) = (&record.user, &record.session) else {
            return Ok((tx, inactive()));
        };
        let preview = allow_preview && caller == Some(client.id.as_str());
        let (tx, principal) = self.native_principal(tx, user, session, preview).await?;
        let Some(principal) = principal else {
            return Ok((tx, disabled()));
        };
        match self
            .native_claims(&tx, &client, &principal, allow_preview)
            .await
        {
            Ok(_) => {}
            Err(Error::AccessDenied) => return Ok((tx, disabled())),
            Err(error) => return Err(error),
        }
        let scopes = json_strings(record.scopes.clone())?;
        match self
            .native_resources(&tx, &client, &resources, &scopes)
            .await
        {
            Ok(()) => {}
            Err(Error::AccessDenied | Error::InvalidRequest | Error::InvalidScope) => {
                return Ok((tx, inactive()));
            }
            Err(error) => return Err(error),
        }
        let (tx, principal) = self.native_principal(tx, user, session, preview).await?;
        let Some(principal) = principal else {
            return Ok((tx, disabled()));
        };
        let mut claims = match self
            .native_claims(&tx, &client, &principal, allow_preview)
            .await
        {
            Ok(claims) => claims,
            Err(Error::AccessDenied) => return Ok((tx, disabled())),
            Err(error) => return Err(error),
        };
        let current = discover(&tx, &hash, refresh).await?;
        if !current.is_some_and(|current| record.same_owner(&current) && current.alive) {
            return Ok((tx, inactive()));
        }
        claims["active"] = json!(true);
        claims["iss"] = json!(self.config.issuer.as_str());
        claims["sub"] = json!(user);
        claims["sid"] = json!(session);
        claims["client_id"] = json!(client.id);
        claims["azp"] = claims["client_id"].clone();
        claims["exp"] = json!(record.expires.timestamp());
        claims["iat"] = json!(record.created.timestamp());
        claims["scope"] = json!(scopes.join(" "));
        claims["token_type"] = json!(if refresh { "refresh_token" } else { "Bearer" });
        set_audience(&mut claims, resources, &scopes, self.config.issuer.as_str());
        Ok((tx, claims))
    }
    async fn native_claims(
        &self,
        tx: &Transaction<'_>,
        client: &Client,
        user: &Principal,
        allow_preview: bool,
    ) -> Result<Value> {
        if user.in_tenant_preview() {
            if !allow_preview || self.config.studio_client_id.as_deref() != Some(&client.id) {
                return Err(Error::AccessDenied);
            }
            return Ok(
                json!({"disabled":false,"webdock_role":if user.is_acting_customer(){"admin"}else{"reader"},"webdock_preview":true,"webdock_email":user.email,"webdock_name":user.name}),
            );
        }
        let mut claims =
            json!({"disabled":false,"webdock_email":user.email,"webdock_name":user.name});
        if client.metadata["webdock_hosting"] == true {
            return Ok(
                json!({"disabled":false,"webdock_hosting":true,"webdock_role":if user.is_operator(){"hosting-operator"}else{"hosting-customer"}}),
            );
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
        let row=tx.query_typed_opt("SELECT organization_id FROM webdock_auth.app_binding WHERE id=$1 AND client_id=$2 AND enabled",&[(&binding,Type::TEXT),(&client.id,Type::TEXT)]).await.map_err(|_|Error::Configuration)?.ok_or(Error::AccessDenied)?;
        let org: Option<String> = row
            .try_get("organization_id")
            .map_err(|_| Error::Configuration)?;
        if client.metadata.get("webdock_operator_only").is_some() {
            let subjects = client.metadata["webdock_allowed_subjects"]
                .as_array()
                .ok_or(Error::AccessDenied)?;
            if client.metadata["webdock_operator_only"] != true
                || org.is_some()
                || subjects.is_empty()
                || subjects.iter().any(|v| !v.as_str().is_some_and(valid_id))
                || !subjects.iter().any(|v| v == &json!(user.user_id))
                || !user.is_operator()
            {
                return Err(Error::AccessDenied);
            }
        }
        if let Some(org) = org.as_ref()
            && !organization_local(tx, org).await?
        {
            return Err(Error::AccessDenied);
        }
        if user.is_operator() {
            claims["webdock_role"] = json!("operator");
            return Ok(claims);
        }
        if self.config.studio_client_id.as_deref() == Some(&client.id) && org.is_none() {
            let allowed=tx.query_typed_one(r#"SELECT EXISTS(SELECT 1 FROM webdock_auth.member m JOIN webdock_auth.tenant_customer t ON t.organization_id=m."organizationId" JOIN webdock_admin.customers c ON c.id=t.customer_id AND c.status='active' WHERE m."userId"=$1 AND webdock_rls.local_organization_authority(m."organizationId")) AS allowed"#,&[(&user.user_id,Type::TEXT)]).await.map_err(|_|Error::Configuration)?.try_get::<_,bool>("allowed").map_err(|_|Error::Configuration)?;
            if !allowed {
                return Err(Error::AccessDenied);
            }
            claims["webdock_role"] = json!("reader");
            return Ok(claims);
        }
        let org = org.ok_or(Error::AccessDenied)?;
        let enabled=tx.query_typed_opt("SELECT coalesce((settings->>'cmsEnabled')::boolean,true) AS enabled FROM webdock_auth.platform_settings WHERE id=true",&[]).await.map_err(|_|Error::Configuration)?;
        if enabled.is_some_and(|r| r.try_get::<_, bool>("enabled").ok() == Some(false)) {
            return Err(Error::AccessDenied);
        }
        if !tx
            .query_typed_one(
                "SELECT webdock_rls.billing_cms_allowed($1) AS allowed",
                &[(&binding, Type::TEXT)],
            )
            .await
            .map_err(|_| Error::Configuration)?
            .try_get::<_, bool>("allowed")
            .map_err(|_| Error::Configuration)?
        {
            return Err(Error::AccessDenied);
        }
        let row=tx.query_typed_opt(r#"SELECT CASE WHEN c.status='active' AND m.role IN ('owner','admin') THEN 'admin' ELSE g.role END AS role FROM webdock_auth.member m LEFT JOIN webdock_auth.tenant_customer t ON t.organization_id=m."organizationId" LEFT JOIN webdock_admin.customers c ON c.id=t.customer_id LEFT JOIN webdock_auth.project_grant g ON g.user_id=m."userId" AND g.binding_id=$2 WHERE m."userId"=$1 AND m."organizationId"=$3 AND (t.customer_id IS NULL OR c.status='active') AND (g.id IS NULL OR(g.enabled AND g.organization_id=$3)) AND ((c.status='active' AND m.role IN ('owner','admin')) OR g.enabled) LIMIT 1"#,&[(&user.user_id,Type::TEXT),(&binding,Type::TEXT),(&org,Type::TEXT)]).await.map_err(|_|Error::Configuration)?.ok_or(Error::AccessDenied)?;
        let role: String = row.try_get("role").map_err(|_| Error::Configuration)?;
        if !["reader", "editor", "admin"].contains(&role.as_str()) {
            return Err(Error::AccessDenied);
        }
        claims["webdock_role"] = json!(role);
        Ok(claims)
    }
    async fn native_resources(
        &self,
        tx: &Transaction<'_>,
        client: &Client,
        resources: &[String],
        scopes: &[String],
    ) -> Result<()> {
        if resources.len() > 8 || scopes.iter().any(|s| !client.scopes.contains(s)) {
            return Err(Error::InvalidScope);
        }
        if client.metadata.get("webdock_operator_only").is_some()
            && !scopes.iter().any(|s| s == "openid")
        {
            return Err(Error::AccessDenied);
        }
        lock_resources(tx, resources, &[&client.id]).await?;
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
            let row=tx.query_typed_opt(r#"SELECT r."accessTokenTtl",r."refreshTokenTtl",r."allowedScopes" FROM webdock_auth."oauthResource" r JOIN webdock_auth."oauthClientResource" c ON c."resourceId"=r.identifier AND c."clientId"=$1 WHERE r.identifier=$2 AND NOT coalesce(r.disabled,false) AND NOT coalesce(r."dpopBoundAccessTokensRequired",false) FOR SHARE OF r,c"#,&[(&client.id,Type::TEXT),(resource,Type::TEXT)]).await.map_err(|_|Error::Configuration)?.ok_or(Error::AccessDenied)?;
            let allowed = json_strings(
                row.try_get::<_, Option<Value>>("allowedScopes")
                    .map_err(|_| Error::Configuration)?
                    .unwrap_or(json!([])),
            )?;
            if scopes.iter().any(|s| {
                !["openid", "profile", "email"].contains(&s.as_str()) && !allowed.contains(s)
            }) {
                return Err(Error::InvalidScope);
            }
            access = access.min(i64::from(
                row.try_get::<_, Option<i32>>("accessTokenTtl")
                    .map_err(|_| Error::Configuration)?
                    .unwrap_or(28800),
            ));
            refresh = refresh.min(i64::from(
                row.try_get::<_, Option<i32>>("refreshTokenTtl")
                    .map_err(|_| Error::Configuration)?
                    .unwrap_or(28800),
            ));
        }
        if access <= 0 || refresh <= 0 {
            return Err(Error::Configuration);
        }
        Ok(())
    }
    pub(super) async fn verify_native_signed_claims(
        &self,
        tx: &Transaction<'_>,
        token: &str,
        check_expiry: bool,
    ) -> Result<Value> {
        let kid = crate::signing::header_key_id(token)?;
        let row=tx.query_typed_opt(r#"SELECT "publicKey" FROM webdock_auth.jwks WHERE id=$1 AND(alg='RS256' OR alg IS NULL)"#,&[(&kid,Type::TEXT)]).await.map_err(|_|Error::Configuration)?.ok_or(Error::InvalidGrant)?;
        let key: String = row.try_get("publicKey").map_err(|_| Error::Configuration)?;
        crate::signing::verified_public_claims(
            token,
            &key,
            self.config.issuer.as_str(),
            check_expiry,
        )
    }
    async fn inspect_native_jwt<'a>(
        &self,
        tx: Transaction<'a>,
        token: &str,
        caller: Option<&str>,
        allow_preview: bool,
    ) -> Result<(Transaction<'a>, Value)> {
        let raw = match self.verify_native_signed_claims(&tx, token, true).await {
            Ok(raw) => raw,
            Err(Error::InvalidGrant) => return Ok((tx, inactive())),
            Err(error) => return Err(error),
        };
        let (Some(scope), Some(user), Some(session), Some(client_id)) = (
            raw["scope"].as_str(),
            raw["sub"].as_str(),
            raw["sid"].as_str(),
            raw["azp"].as_str().or(raw["client_id"].as_str()),
        ) else {
            return Ok((tx, inactive()));
        };
        if !raw["cnf"].is_null() {
            return Ok((tx, inactive()));
        }
        let client = match native_client_store::load_client(&tx, client_id).await {
            Ok(client) => client,
            Err(Error::InvalidClient) => return Ok((tx, inactive())),
            Err(error) => return Err(error),
        };
        let preview = allow_preview && caller == Some(client_id);
        let (tx, principal) = self.native_principal(tx, user, session, preview).await?;
        let Some(principal) = principal else {
            return Ok((tx, inactive()));
        };
        match self
            .native_claims(&tx, &client, &principal, allow_preview)
            .await
        {
            Ok(_) => {}
            Err(Error::AccessDenied) => return Ok((tx, inactive())),
            Err(error) => return Err(error),
        }
        let resources = audiences(&raw)
            .into_iter()
            .filter(|v| v != &format!("{}/oauth2/userinfo", self.config.issuer))
            .collect::<Vec<_>>();
        if let Some(caller) = caller
            && caller != client_id
            && !linked_any(&tx, caller, &client.id, &resources).await?
        {
            return Ok((tx, inactive()));
        }
        let scopes = scope_vec(scope)?;
        match self
            .native_resources(&tx, &client, &resources, &scopes)
            .await
        {
            Ok(()) => {}
            Err(Error::AccessDenied | Error::InvalidRequest | Error::InvalidScope) => {
                return Ok((tx, inactive()));
            }
            Err(error) => return Err(error),
        }
        let (tx, principal) = self.native_principal(tx, user, session, preview).await?;
        let Some(principal) = principal else {
            return Ok((tx, inactive()));
        };
        let mut claims = match self
            .native_claims(&tx, &client, &principal, allow_preview)
            .await
        {
            Ok(claims) => claims,
            Err(Error::AccessDenied) => return Ok((tx, inactive())),
            Err(error) => return Err(error),
        };
        let now: i64 = tx
            .query_typed_one(
                "SELECT floor(extract(epoch FROM clock_timestamp()))::bigint AS now",
                &[],
            )
            .await
            .map_err(|_| Error::Configuration)?
            .try_get("now")
            .map_err(|_| Error::Configuration)?;
        if raw["exp"].as_i64().is_none_or(|exp| exp <= now)
            || discover(&tx, &legacy_token_hash(token), false)
                .await?
                .is_some()
        {
            return Ok((tx, inactive()));
        }
        for key in ["iss", "aud", "sub", "sid", "exp", "iat", "scope"] {
            claims[key] = raw[key].clone();
        }
        claims["client_id"] = json!(client_id);
        claims["azp"] = json!(client_id);
        claims["active"] = json!(true);
        claims["token_type"] = json!("Bearer");
        Ok((tx, claims))
    }
    pub(crate) async fn native_userinfo(&self, token: &str) -> Result<Value> {
        tokio::time::timeout(BUDGET, async {
            let mut connection = self
                .native_introspection_pool()
                .ok_or(Error::Configuration)?
                .acquire()
                .await
                .map_err(|_| Error::Configuration)?;
            let tx = connection
                .transaction()
                .await
                .map_err(|_| Error::Configuration)?;
            let (tx, claims) = self.inspect_native(tx, token, None, false, false).await?;
            if claims["active"] != true
                || !claims["scope"]
                    .as_str()
                    .is_some_and(|s| s.split_whitespace().any(|s| s == "openid"))
            {
                return Err(Error::AccessDenied);
            }
            let (tx, principal) = self
                .native_principal(
                    tx,
                    claims["sub"].as_str().ok_or(Error::AccessDenied)?,
                    claims["sid"].as_str().ok_or(Error::AccessDenied)?,
                    false,
                )
                .await?;
            let principal = principal.ok_or(Error::AccessDenied)?;
            let mut user = json!({"sub":claims["sub"]});
            let scopes = claims["scope"].as_str().unwrap_or("");
            if scopes.split_whitespace().any(|s| s == "profile") {
                user["name"] = json!(principal.name);
            }
            if scopes.split_whitespace().any(|s| s == "email") {
                user["email"] = json!(principal.email);
                user["email_verified"] = json!(true);
            }
            tx.commit().await.map_err(|_| Error::Configuration)?;
            Ok(user)
        })
        .await
        .map_err(|_| Error::Configuration)?
    }
}
async fn organization_local(tx: &Transaction<'_>, org: &str) -> Result<bool> {
    tx.query_typed_one(
        "SELECT webdock_rls.local_organization_authority($1) AS allowed",
        &[(&org, Type::TEXT)],
    )
    .await
    .map_err(|_| Error::Configuration)?
    .try_get("allowed")
    .map_err(|_| Error::Configuration)
}
fn set_audience(claims: &mut Value, mut resources: Vec<String>, scopes: &[String], issuer: &str) {
    if !resources.is_empty() && scopes.iter().any(|s| s == "openid") {
        resources.push(format!("{issuer}/oauth2/userinfo"));
    }
    if resources.len() == 1 {
        claims["aud"] = json!(resources[0]);
    } else if !resources.is_empty() {
        claims["aud"] = json!(resources);
    }
}
