use crate::{Error, OAuthService, Result};
use josekit::{
    jwk::Jwk,
    jws::{self, JwsHeader, RS256},
};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, Statement,
    TransactionTrait,
};
use serde_json::{Value, json};
/// Offline-only staged key creation. The caller owns authority and the surrounding atomic setup.
/// Never call from normal process startup or an HTTP registration path.
pub async fn stage_signing_key(
    tx: &DatabaseTransaction,
    id: &str,
    keys: &webdock_identity::SecretKeys,
) -> Result<()> {
    if id.starts_with('0')
        || !id.bytes().all(|b| b.is_ascii_digit())
        || !id.parse::<i64>().is_ok_and(|id| id > 0)
    {
        return Err(Error::InvalidRequest);
    }
    let mut key = tokio::task::spawn_blocking(|| Jwk::generate_rsa_key(3072))
        .await
        .map_err(|_| Error::Crypto)?
        .map_err(|_| Error::Crypto)?;
    key.set_algorithm("RS256");
    key.set_key_use("sig");
    key.set_key_id(id);
    let public = key.to_public_key().map_err(|_| Error::Crypto)?;
    let encrypted = keys.encrypt(&key.to_string())?;
    // Retained privateKey column is a JSON string containing the encrypted envelope.
    let private = serde_json::to_string(&encrypted).map_err(|_| Error::Crypto)?;
    tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.jwks(id,\"publicKey\",\"privateKey\",\"createdAt\",alg) VALUES($1,$2,$3,statement_timestamp(),'RS256')",[id.into(),public.to_string().into(),private.into()])).await?;
    Ok(())
}
impl OAuthService {
    /// Offline operation only. Never generate/replace a key from an HTTP request or startup hook.
    pub async fn provision_signing_key(&self, offline: &DatabaseConnection) -> Result<String> {
        let id = self.allocate_id().await?;
        let tx = offline.begin().await?;
        stage_signing_key(&tx, &id, &self.keys).await?;
        tx.commit().await?;
        Ok(id)
    }
    pub async fn jwks(&self) -> Result<Value> {
        if self.native_client_pool().is_some() {
            return self.native_jwks().await;
        }
        let rows=self.db.query_all_raw(Statement::from_string(DbBackend::Postgres,"SELECT id,\"publicKey\" FROM webdock_auth.jwks WHERE alg='RS256' OR alg IS NULL ORDER BY \"createdAt\",id")).await?;
        let records = rows
            .iter()
            .map(|row| {
                Ok((
                    row.try_get::<String>("", "id")?,
                    row.try_get::<String>("", "publicKey")?,
                ))
            })
            .collect::<std::result::Result<Vec<_>, sea_orm::DbErr>>()?;
        public_jwks(records)
    }
    pub(crate) async fn verify_signed_claims(
        &self,
        tx: &DatabaseTransaction,
        token: &str,
        check_expiry: bool,
    ) -> Result<Value> {
        let kid = header_key_id(token)?;
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT \"publicKey\" FROM webdock_auth.jwks WHERE id=$1 AND (alg='RS256' OR alg IS NULL)",[kid.into()])).await?.ok_or(Error::InvalidGrant)?;
        verified_public_claims(
            token,
            &row.try_get::<String>("", "publicKey")?,
            self.config.issuer.as_str(),
            check_expiry,
        )
    }
    pub(crate) async fn sign_id_token(
        &self,
        tx: &DatabaseTransaction,
        claims: &Value,
    ) -> Result<String> {
        let row=tx.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT id,\"privateKey\" FROM webdock_auth.jwks WHERE (alg='RS256' OR alg IS NULL) AND (\"expiresAt\" IS NULL OR \"expiresAt\">statement_timestamp()) ORDER BY \"createdAt\" DESC,id DESC LIMIT 1 FOR SHARE")).await?.ok_or(Error::Crypto)?;
        let encrypted: String = serde_json::from_str(&row.try_get::<String>("", "privateKey")?)
            .map_err(|_| Error::Crypto)?;
        let private = self.keys.decrypt(&encrypted)?;
        let key = Jwk::from_bytes(private.as_bytes()).map_err(|_| Error::Crypto)?;
        let signer = RS256.signer_from_jwk(&key).map_err(|_| Error::Crypto)?;
        let mut header = JwsHeader::new();
        header.set_algorithm("RS256");
        header.set_token_type("JWT");
        header.set_key_id(row.try_get::<String>("", "id")?);
        jws::serialize_compact(
            &serde_json::to_vec(claims).map_err(|_| Error::Crypto)?,
            &header,
            &signer,
        )
        .map_err(|_| Error::Crypto)
    }
}

/// Public-key normalization shared by both stores. Private RSA parameters never leave here.
pub(crate) fn public_jwks(records: Vec<(String, String)>) -> Result<Value> {
    let mut keys = Vec::new();
    for (id, raw) in records {
        let mut key = Jwk::from_bytes(raw.as_bytes())
            .map_err(|_| Error::Crypto)?
            .to_public_key()
            .map_err(|_| Error::Crypto)?;
        if key.key_type() != "RSA" {
            continue;
        }
        key.set_key_id(id);
        key.set_algorithm("RS256");
        key.set_key_use("sig");
        keys.push(serde_json::to_value(key).map_err(|_| Error::Crypto)?);
    }
    Ok(json!({"keys":keys}))
}

pub(crate) fn header_key_id(token: &str) -> Result<String> {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    if token.len() > 16384 {
        return Err(Error::InvalidGrant);
    }
    let header = JwsHeader::from_bytes(
        &URL_SAFE_NO_PAD
            .decode(token.split('.').next().ok_or(Error::InvalidGrant)?)
            .map_err(|_| Error::InvalidGrant)?,
    )
    .map_err(|_| Error::InvalidGrant)?;
    if header.algorithm() != Some("RS256") {
        return Err(Error::InvalidGrant);
    }
    Ok(header.key_id().ok_or(Error::InvalidGrant)?.into())
}
pub(crate) fn verified_public_claims(
    token: &str,
    raw_key: &str,
    issuer: &str,
    check_expiry: bool,
) -> Result<Value> {
    header_key_id(token)?;
    let key = Jwk::from_bytes(raw_key.as_bytes()).map_err(|_| Error::Crypto)?;
    let (payload, _) = jws::deserialize_compact(
        token,
        &RS256.verifier_from_jwk(&key).map_err(|_| Error::Crypto)?,
    )
    .map_err(|_| Error::InvalidGrant)?;
    let claims: Value = serde_json::from_slice(&payload).map_err(|_| Error::InvalidGrant)?;
    if claims["iss"] != issuer {
        return Err(Error::InvalidGrant);
    }
    let now = chrono::Utc::now().timestamp();
    if check_expiry
        && (claims["exp"].as_i64().is_none_or(|exp| exp <= now)
            || claims["iat"].as_i64().is_none_or(|iat| iat > now + 60))
    {
        return Err(Error::InvalidGrant);
    }
    Ok(claims)
}
