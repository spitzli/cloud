//! Native monotone security writes. No grant issuance, fallback or uncertain retry.
use super::{OAuthService, audiences, authenticate_client, date, native_client_store, scope_vec};
use crate::{ClientAuthentication, Error, LogoutOutcome, LogoutRequest, Result, legacy_token_hash};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};
use tokio_postgres::types::Type;
use webdock_persistence::postgres::Transaction;
const BUDGET: Duration = Duration::from_secs(30);
#[derive(PartialEq, Eq)]
struct Record {
    table: &'static str,
    id: String,
    user: Option<String>,
    session: Option<String>,
    session_owner: Option<String>,
}
async fn discover(tx: &Transaction<'_>, hash: &str, client: &str) -> Result<Vec<Record>> {
    let mut result = vec![];
    for table in ["oauthAccessToken", "oauthRefreshToken"] {
        let sql = format!(
            r#"SELECT t.id,t."userId",t."sessionId",s."userId" AS session_owner FROM webdock_auth."{table}" t LEFT JOIN webdock_auth.session s ON s.id=t."sessionId" WHERE t.token=$1 AND t."clientId"=$2 ORDER BY t.id"#
        );
        for row in tx
            .query_typed(&sql, &[(&hash, Type::TEXT), (&client, Type::TEXT)])
            .await
            .map_err(|_| Error::Configuration)?
        {
            result.push(
                (|| -> std::result::Result<_, tokio_postgres::Error> {
                    Ok(Record {
                        table,
                        id: row.try_get("id")?,
                        user: row.try_get("userId")?,
                        session: row.try_get("sessionId")?,
                        session_owner: row.try_get("session_owner")?,
                    })
                })()
                .map_err(|_| Error::Configuration)?,
            );
        }
    }
    Ok(result)
}
impl OAuthService {
    pub(crate) async fn native_revoke(
        &self,
        credentials: &ClientAuthentication,
        token: &str,
    ) -> Result<()> {
        let operation = async {
            let mut connection = self
                .native_security_pool()
                .ok_or(Error::Configuration)?
                .acquire()
                .await
                .map_err(|_| Error::Configuration)?;
            let tx = connection
                .transaction()
                .await
                .map_err(|_| Error::Configuration)?;
            let client = native_client_store::load_client(&tx, &credentials.client_id).await?;
            authenticate_client(&client, credentials)?;
            let hash = legacy_token_hash(token);
            let discovered = discover(&tx, &hash, &client.id).await?;
            let claims = if token.matches('.').count() == 2 {
                match self.verify_native_signed_claims(&tx, token, true).await {
                    Ok(c)
                        if c["azp"].as_str().or(c["client_id"].as_str()) == Some(&client.id)
                            && c["scope"].is_string() =>
                    {
                        Some(c)
                    }
                    Ok(_) | Err(Error::InvalidGrant) => None,
                    Err(e) => return Err(e),
                }
            } else {
                None
            };
            let mut users = BTreeSet::new();
            let mut sessions = BTreeMap::new();
            for r in &discovered {
                users.extend(r.user.iter().cloned());
                users.extend(r.session_owner.iter().cloned());
                if let Some(id) = &r.session {
                    sessions.insert(id.clone(), r.session_owner.clone());
                }
            }
            if let Some(c) = &claims
                && let (Some(user), Some(session), Some(_), Some(_)) = (
                    c["sub"].as_str(),
                    c["sid"].as_str(),
                    c["exp"].as_i64(),
                    c["iat"].as_i64(),
                )
            {
                users.insert(user.into());
                let row = tx
                    .query_typed_opt(
                        r#"SELECT "userId" FROM webdock_auth.session WHERE id=$1"#,
                        &[(&session, Type::TEXT)],
                    )
                    .await
                    .map_err(|_| Error::Configuration)?;
                let owner = row
                    .map(|r| r.try_get::<_, String>("userId"))
                    .transpose()
                    .map_err(|_| Error::Configuration)?;
                users.extend(owner.iter().cloned());
                sessions.insert(session.into(), owner);
            }
            // Owner-exclusive serialization matches refresh; sessions precede token rows.
            let mut fenced_users = BTreeSet::new();
            for user in users {
                if tx
                    .query_typed_opt(
                        r#"SELECT id FROM webdock_auth."user" WHERE id=$1 FOR UPDATE"#,
                        &[(&user, Type::TEXT)],
                    )
                    .await
                    .map_err(|_| Error::Configuration)?
                    .is_some()
                {
                    fenced_users.insert(user);
                }
            }
            for (session, expected) in sessions {
                let row = tx
                    .query_typed_opt(
                        r#"SELECT "userId" FROM webdock_auth.session WHERE id=$1 FOR SHARE"#,
                        &[(&session, Type::TEXT)],
                    )
                    .await
                    .map_err(|_| Error::Configuration)?;
                let owner = row
                    .map(|r| r.try_get::<_, String>("userId"))
                    .transpose()
                    .map_err(|_| Error::Configuration)?;
                if owner != expected {
                    return Err(Error::Configuration);
                }
            }
            for r in &discovered {
                let sql = format!(
                    r#"SELECT id FROM webdock_auth."{}" WHERE id=$1 AND token=$2 AND "clientId"=$3 FOR UPDATE"#,
                    r.table
                );
                tx.query_typed_opt(
                    &sql,
                    &[
                        (&r.id, Type::TEXT),
                        (&hash, Type::TEXT),
                        (&client.id, Type::TEXT),
                    ],
                )
                .await
                .map_err(|_| Error::Configuration)?;
            }
            let current = discover(&tx, &hash, &client.id).await?;
            if current.iter().any(|r| !discovered.contains(r)) {
                return Err(Error::Configuration);
            }
            for r in current {
                let sql = format!(
                    r#"UPDATE webdock_auth."{}" SET revoked=statement_timestamp() WHERE id=$1 AND token=$2 AND "clientId"=$3"#,
                    r.table
                );
                if tx
                    .execute_typed(
                        &sql,
                        &[
                            (&r.id, Type::TEXT),
                            (&hash, Type::TEXT),
                            (&client.id, Type::TEXT),
                        ],
                    )
                    .await
                    .map_err(|_| Error::Configuration)?
                    != 1
                {
                    return Err(Error::Configuration);
                }
            }
            if let Some(c) = claims {
                self.native_tombstone(&tx, &client.id, &hash, &c, &fenced_users)
                    .await?;
            }
            tx.commit().await.map_err(|_| Error::Configuration)
        };
        tokio::time::timeout(BUDGET, operation)
            .await
            .map_err(|_| Error::Configuration)?
    }
    async fn native_tombstone(
        &self,
        tx: &Transaction<'_>,
        client: &str,
        hash: &str,
        claims: &Value,
        fenced_users: &BTreeSet<String>,
    ) -> Result<()> {
        let (Some(user), Some(session), Some(exp), Some(iat)) = (
            claims["sub"].as_str(),
            claims["sid"].as_str(),
            claims["exp"].as_i64(),
            claims["iat"].as_i64(),
        ) else {
            return Ok(());
        };
        let owner=tx.query_typed_opt(r#"SELECT u.id,s.id AS session_id FROM webdock_auth."user" u LEFT JOIN webdock_auth.session s ON s.id=$2 AND s."userId"=u.id WHERE u.id=$1"#,&[(&user,Type::TEXT),(&session,Type::TEXT)]).await.map_err(|_|Error::Configuration)?;
        let Some(owner) = owner else {
            return Ok(());
        };
        // A missing owner may appear during another owner's lock wait.
        if !fenced_users.contains(user) {
            return Err(Error::Configuration);
        }
        let session: Option<String> = owner
            .try_get("session_id")
            .map_err(|_| Error::Configuration)?;
        let resources = audiences(claims)
            .into_iter()
            .filter(|r| r != &format!("{}/oauth2/userinfo", self.config.issuer))
            .collect::<Vec<_>>();
        let scopes = scope_vec(claims["scope"].as_str().ok_or(Error::InvalidRequest)?)?;
        let id = self.allocate_id().await?;
        let expires = date(exp)?;
        let created = date(iat)?;
        tx.execute_typed(r#"INSERT INTO webdock_auth."oauthAccessToken"(id,token,"clientId","userId","sessionId",scopes,resources,"expiresAt","createdAt",revoked) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,statement_timestamp()) ON CONFLICT(token) DO UPDATE SET revoked=statement_timestamp() WHERE webdock_auth."oauthAccessToken"."clientId"=excluded."clientId""#,&[(&id,Type::TEXT),(&hash,Type::TEXT),(&client,Type::TEXT),(&user,Type::TEXT),(&session,Type::TEXT),(&json!(scopes),Type::JSONB),(&json!(resources),Type::JSONB),(&expires,Type::TIMESTAMPTZ),(&created,Type::TIMESTAMPTZ)]).await.map_err(|_|Error::Configuration)?;
        Ok(())
    }
    pub(crate) async fn native_end_session(
        &self,
        cookie: &str,
        request: LogoutRequest,
    ) -> Result<LogoutOutcome> {
        if request.state.as_ref().is_some_and(|s| s.len() > 2048) {
            return Err(Error::InvalidRequest);
        }
        tokio::time::timeout(BUDGET, async {
            let current = self.identity.authenticate_cookie(cookie).await?;
            let mut connection = self
                .native_security_pool()
                .ok_or(Error::Configuration)?
                .acquire()
                .await
                .map_err(|_| Error::Configuration)?;
            let tx = connection
                .transaction()
                .await
                .map_err(|_| Error::Configuration)?;
            let claims = match self
                .verify_native_signed_claims(&tx, &request.id_token_hint, false)
                .await
            {
                Ok(c) => c,
                Err(Error::InvalidGrant) => return Ok(LogoutOutcome::ConfirmationRequired),
                Err(e) => return Err(e),
            };
            let client = native_client_store::load_client(
                &tx,
                claims["aud"].as_str().ok_or(Error::InvalidRequest)?,
            )
            .await?;
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
            tx.query_typed_opt(
                r#"SELECT id FROM webdock_auth."user" WHERE id=$1 FOR UPDATE"#,
                &[(&current.user_id, Type::TEXT)],
            )
            .await
            .map_err(|_| Error::Configuration)?;
            let (tx, principal) = self
                .identity
                .authenticate_session_native_in(tx, &current.user_id, &current.session_id)
                .await?;
            if principal.is_none() {
                return Ok(LogoutOutcome::ConfirmationRequired);
            }
            if tx
                .execute_typed(
                    r#"DELETE FROM webdock_auth.session WHERE id=$1 AND "userId"=$2"#,
                    &[
                        (&current.session_id, Type::TEXT),
                        (&current.user_id, Type::TEXT),
                    ],
                )
                .await
                .map_err(|_| Error::Configuration)?
                != 1
            {
                return Err(Error::Configuration);
            }
            tx.commit().await.map_err(|_| Error::Configuration)?;
            Ok(LogoutOutcome::Complete {
                redirect,
                set_cookie: self.identity.clear_cookie(),
            })
        })
        .await
        .map_err(|_| Error::Configuration)?
    }
}
