use crate::{
    IdentityError, IdentityService, IssuedSession, Result,
    crypto::random,
    security::{Actor, credential, digest},
    service::externally_managed,
};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use chrono::{DateTime, Utc};
use sea_orm::{
    ConnectionTrait, DatabaseTransaction, DbBackend, QueryResult, Statement, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use serde_cbor::Value;
use std::time::Duration;
use webauthn_rs::prelude::*;
use webauthn_rs_core::{
    internals::AuthenticatorData,
    proto::{
        AttestationFormat, COSEKey, Credential, ParsedAttestation, Registration,
        ResidentKeyRequirement, UserVerificationPolicy,
    },
};
/// Cookie carries only an opaque handle; all ceremony state remains in PostgreSQL.
pub struct PasskeyChallenge<T> {
    pub options: T,
    pub set_cookie: String,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PasskeySummary {
    pub id: String,
    pub name: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
}
/// Mapping of existing public credential material; does not contain a private key.
pub struct LegacyPasskey {
    pub credential_id: String,
    pub public_key: String,
    pub counter: i32,
    pub device_type: String,
    pub backed_up: bool,
    pub transports: Option<String>,
}
pub fn import_legacy_passkey(row: &LegacyPasskey) -> Result<Passkey> {
    if row.public_key.len() > 16384
        || row.credential_id.len() > 4096
        || row.counter < 0
        || !["singleDevice", "multiDevice"].contains(&row.device_type.as_str())
    {
        return Err(IdentityError::InvalidEncoding);
    }
    let bytes = STANDARD
        .decode(&row.public_key)
        .map_err(|_| IdentityError::InvalidEncoding)?;
    let cbor: Value = serde_cbor::from_slice(&bytes).map_err(|_| IdentityError::InvalidEncoding)?;
    let cred = COSEKey::try_from(&cbor).map_err(|_| IdentityError::InvalidEncoding)?;
    let id = URL_SAFE_NO_PAD
        .decode(&row.credential_id)
        .map_err(|_| IdentityError::InvalidEncoding)?;
    if id.is_empty() || row.backed_up && row.device_type != "multiDevice" {
        return Err(IdentityError::InvalidEncoding);
    }
    let transports = row
        .transports
        .as_ref()
        .map(|raw| {
            raw.split(',')
                .filter(|t| !t.is_empty())
                .map(|t| {
                    serde_json::from_value(serde_json::Value::String(t.to_owned()))
                        .map_err(|_| IdentityError::InvalidEncoding)
                })
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?;
    Ok(Passkey::from(Credential {
        cred_id: id.into(),
        cred,
        counter: row.counter as u32,
        transports,
        user_verified: true,
        backup_eligible: row.device_type == "multiDevice",
        backup_state: row.backed_up,
        registration_policy: UserVerificationPolicy::Required,
        extensions: Default::default(),
        attestation: ParsedAttestation::default(),
        attestation_format: AttestationFormat::None,
    }))
}
impl IdentityService {
    pub fn with_passkeys(mut self, origin: url::Url) -> Result<Self> {
        self = self.with_account_origin(origin.clone())?;
        self.webauthn = Some(
            WebauthnBuilder::new(
                origin.host_str().ok_or(IdentityError::Configuration)?,
                &origin,
            )
            .map_err(|_| IdentityError::Configuration)?
            .rp_name("Spitzli Cloud")
            .timeout(Duration::from_secs(300))
            .build()
            .map_err(|_| IdentityError::Configuration)?,
        );
        Ok(self)
    }
    pub fn clear_passkey_cookie(&self) -> String {
        self.cookies.clear_passkey_header()
    }
    async fn passkey_actor(&self, tx: &DatabaseTransaction, cookie: &str) -> Result<Actor> {
        let actor = self.locked_actor(tx, cookie).await?;
        if !actor.verified
            || actor.must_change
            || actor.preview
            || (actor.role == "operator" && !actor.mfa)
        {
            return Err(IdentityError::SecurityState);
        }
        Ok(actor)
    }
    async fn store_passkey_state<T: Serialize>(
        &self,
        tx: &DatabaseTransaction,
        kind: &str,
        user: Option<&str>,
        session: Option<&str>,
        state: &T,
    ) -> Result<String> {
        tx.execute_unprepared("DELETE FROM webdock_auth.identity_passkey_ceremony WHERE token_hash IN (SELECT token_hash FROM webdock_auth.identity_passkey_ceremony WHERE expires_at<=statement_timestamp() ORDER BY expires_at LIMIT 100)").await?;
        let token = hex::encode(random::<32>()?);
        let state = serde_json::to_value(state).map_err(|_| IdentityError::InvalidEncoding)?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.identity_passkey_ceremony(token_hash,kind,user_id,session_id,state,expires_at) VALUES($1,$2,$3,$4,$5,statement_timestamp()+interval '5 minutes')",[digest(&token).into(),kind.into(),user.map(str::to_owned).into(),session.map(str::to_owned).into(),state.into()])).await?;
        self.cookies.passkey_header(&token)
    }
    async fn take_passkey_state(&self, cookie: &str, kind: &str) -> Result<QueryResult> {
        let token = self
            .cookies
            .read_named(cookie, self.cookies.passkey_name())?
            .ok_or(IdentityError::Unauthorized)?;
        // Consume before verification, including failed attempts. No session or mutation is authorized yet.
        self.db.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"DELETE FROM webdock_auth.identity_passkey_ceremony WHERE token_hash=$1 AND kind=$2 AND expires_at>statement_timestamp() RETURNING user_id,session_id,state",[digest(&token).into(),kind.into()])).await?.ok_or(IdentityError::Unauthorized)
    }
    pub async fn begin_passkey_registration(
        &self,
        cookie: &str,
    ) -> Result<PasskeyChallenge<CreationChallengeResponse>> {
        let webauthn = self.webauthn.as_ref().ok_or(IdentityError::Configuration)?;
        let tx = self.db.begin().await?;
        let actor = self.passkey_actor(&tx, cookie).await?;
        let rows = tx
            .query_all_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT \"credentialID\" FROM webdock_auth.passkey WHERE \"userId\"=$1",
                [actor.id.clone().into()],
            ))
            .await?;
        let exclude = rows
            .iter()
            .map(|r| {
                URL_SAFE_NO_PAD
                    .decode(r.try_get::<String>("", "credentialID")?)
                    .map(Into::into)
                    .map_err(|_| IdentityError::InvalidEncoding)
            })
            .collect::<Result<Vec<_>>>()?;
        // UUID is only the WebAuthn opaque user handle; the canonical database ID remains unchanged.
        let handle = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_URL,
            format!("webdock:identity:{}", actor.id).as_bytes(),
        );
        let (mut options, state) = webauthn
            .start_passkey_registration(handle, &actor.email, &actor.email, Some(exclude))
            .map_err(|_| IdentityError::InvalidInput)?;
        if let Some(selection) = options.public_key.authenticator_selection.as_mut() {
            selection.require_resident_key = true;
            selection.resident_key = Some(ResidentKeyRequirement::Required);
        }
        let set_cookie = self
            .store_passkey_state(
                &tx,
                "registration",
                Some(&actor.id),
                Some(&actor.session_id),
                &state,
            )
            .await?;
        tx.commit().await?;
        Ok(PasskeyChallenge {
            options,
            set_cookie,
        })
    }
    pub async fn finish_passkey_registration(
        &self,
        cookie: &str,
        response: &RegisterPublicKeyCredential,
        name: Option<&str>,
    ) -> Result<PasskeySummary> {
        let webauthn = self.webauthn.as_ref().ok_or(IdentityError::Configuration)?;
        if name.is_some_and(|n| n.len() > 160) {
            return Err(IdentityError::InvalidInput);
        }
        let row = self.take_passkey_state(cookie, "registration").await?;
        let state: PasskeyRegistration = decode_state(&row)?;
        let tx = self.db.begin().await?;
        let actor = self.passkey_actor(&tx, cookie).await?;
        if row.try_get::<Option<String>>("", "user_id")?.as_deref() != Some(&actor.id)
            || row.try_get::<Option<String>>("", "session_id")?.as_deref()
                != Some(&actor.session_id)
        {
            return Err(IdentityError::Unauthorized);
        }
        let passkey = webauthn
            .finish_passkey_registration(response, &state)
            .map_err(|_| IdentityError::InvalidCode)?;
        let credential: Credential = passkey.into();
        if !credential.user_verified {
            return Err(IdentityError::InvalidCode);
        }
        // Decode only after the maintained WebAuthn verifier accepted the attestation/signature/origin.
        let object: Value = serde_cbor::from_slice(response.response.attestation_object.as_ref())
            .map_err(|_| IdentityError::InvalidEncoding)?;
        let Value::Map(object) = object else {
            return Err(IdentityError::InvalidEncoding);
        };
        let Some(Value::Bytes(auth_data)) = object.get(&Value::Text("authData".into())) else {
            return Err(IdentityError::InvalidEncoding);
        };
        let parsed = AuthenticatorData::<Registration>::try_from(auth_data.as_slice())
            .map_err(|_| IdentityError::InvalidEncoding)?;
        let acd = parsed.acd.ok_or(IdentityError::InvalidEncoding)?;
        let public_key = STANDARD.encode(
            serde_cbor::to_vec(&acd.credential_pk).map_err(|_| IdentityError::InvalidEncoding)?,
        );
        let encoded_id = URL_SAFE_NO_PAD.encode(credential.cred_id.as_ref());
        let counter: i32 = credential
            .counter
            .try_into()
            .map_err(|_| IdentityError::InvalidInput)?;
        let transports = credential.transports.as_ref().map(|values| {
            values
                .iter()
                .filter_map(|v| serde_json::to_value(v).ok()?.as_str().map(str::to_owned))
                .collect::<Vec<_>>()
                .join(",")
        });
        let id = self.allocate_id(&tx).await?;
        let saved=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.passkey(id,name,\"publicKey\",\"userId\",\"credentialID\",counter,\"deviceType\",\"backedUp\",transports,\"createdAt\",aaguid) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,statement_timestamp(),$10) RETURNING id,name,\"createdAt\"",[id.into(),name.map(str::to_owned).into(),public_key.into(),actor.id.into(),encoded_id.into(),counter.into(),if credential.backup_eligible{"multiDevice"}else{"singleDevice"}.into(),credential.backup_state.into(),transports.into(),uuid::Uuid::from_slice(&acd.aaguid).map_err(|_|IdentityError::InvalidEncoding)?.to_string().into()])).await?.ok_or(IdentityError::InvalidInput)?;
        let summary = summary(&saved)?;
        tx.commit().await?;
        Ok(summary)
    }
    pub async fn begin_passkey_authentication(
        &self,
    ) -> Result<PasskeyChallenge<RequestChallengeResponse>> {
        let (options, state) = self
            .webauthn
            .as_ref()
            .ok_or(IdentityError::Configuration)?
            .start_discoverable_authentication()
            .map_err(|_| IdentityError::InvalidInput)?;
        let tx = self.db.begin().await?;
        let set_cookie = self
            .store_passkey_state(&tx, "authentication", None, None, &state)
            .await?;
        tx.commit().await?;
        Ok(PasskeyChallenge {
            options,
            set_cookie,
        })
    }
    pub async fn finish_passkey_authentication(
        &self,
        cookie: &str,
        response: &PublicKeyCredential,
    ) -> Result<IssuedSession> {
        let webauthn = self.webauthn.as_ref().ok_or(IdentityError::Configuration)?;
        let row = self.take_passkey_state(cookie, "authentication").await?;
        let state: DiscoverableAuthentication = decode_state(&row)?;
        let encoded_id = URL_SAFE_NO_PAD.encode(response.get_credential_id());
        let tx = self.db.begin().await?;
        let lookup = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT \"userId\" FROM webdock_auth.passkey WHERE \"credentialID\"=$1",
                [encoded_id.clone().into()],
            ))
            .await?
            .ok_or(IdentityError::Unauthorized)?;
        let user_id: String = lookup.try_get("", "userId")?;
        if tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id FROM webdock_auth.\"user\" WHERE id=$1 AND \"emailVerified\" AND NOT coalesce(banned,false) AND role IN ('user','operator') FOR UPDATE",[user_id.clone().into()])).await?.is_none()||externally_managed(&tx,&user_id).await?{return Err(IdentityError::Unauthorized)}
        let record=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT * FROM webdock_auth.passkey WHERE \"credentialID\"=$1 AND \"userId\"=$2 FOR UPDATE",[encoded_id.into(),user_id.clone().into()])).await?.ok_or(IdentityError::Unauthorized)?;
        let legacy = LegacyPasskey {
            credential_id: record.try_get("", "credentialID")?,
            public_key: record.try_get("", "publicKey")?,
            counter: record.try_get("", "counter")?,
            device_type: record.try_get("", "deviceType")?,
            backed_up: record.try_get("", "backedUp")?,
            transports: record.try_get("", "transports")?,
        };
        let key = DiscoverableKey::from(import_legacy_passkey(&legacy)?);
        // Legacy user handles were random bytes, not UUIDs. Credential-ID ownership is authoritative;
        // never infer a local user from the browser's userHandle or an email address.
        let verified = webauthn
            .finish_discoverable_authentication(response, state, &[key])
            .map_err(|_| IdentityError::InvalidCode)?;
        if !verified.user_verified()
            || ((verified.counter() != 0 || legacy.counter != 0)
                && verified.counter() <= legacy.counter as u32)
        {
            return Err(IdentityError::InvalidCode);
        }
        let counter: i32 = verified
            .counter()
            .try_into()
            .map_err(|_| IdentityError::InvalidInput)?;
        tx.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE webdock_auth.passkey SET counter=$1,\"backedUp\"=$2 WHERE id=$3",
            [
                counter.into(),
                verified.backup_state().into(),
                record.try_get::<String>("", "id")?.into(),
            ],
        ))
        .await?;
        let mut source = self.pending_neon(&tx, &user_id, cookie).await?;
        if source.is_some() {
            let token = self
                .cookies
                .read_challenge(cookie)?
                .ok_or(IdentityError::Unauthorized)?;
            tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"DELETE FROM webdock_auth.identity_mfa_challenge WHERE token_hash=$1 AND user_id=$2",[digest(&token).into(),user_id.clone().into()])).await?;
        }
        if let Some(source) = &mut source {
            source.mfa_verified = true;
        }
        self.issue_session_with_source(tx, &user_id, source).await
    }
    pub async fn list_passkeys(&self, cookie: &str) -> Result<Vec<PasskeySummary>> {
        let tx = self.db.begin().await?;
        let actor = self.passkey_actor(&tx, cookie).await?;
        let rows=tx.query_all_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id,name,\"createdAt\" FROM webdock_auth.passkey WHERE \"userId\"=$1 ORDER BY \"createdAt\",id",[actor.id.into()])).await?;
        let result = rows.iter().map(summary).collect::<Result<_>>()?;
        tx.commit().await?;
        Ok(result)
    }
    pub async fn delete_passkey(&self, cookie: &str, id: &str) -> Result<bool> {
        let tx = self.db.begin().await?;
        let actor = self.passkey_actor(&tx, cookie).await?;
        credential(&tx, &actor.id).await?;
        let changed = tx
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM webdock_auth.passkey WHERE id=$1 AND \"userId\"=$2",
                [id.into(), actor.id.into()],
            ))
            .await?
            .rows_affected()
            == 1;
        tx.commit().await?;
        Ok(changed)
    }
    pub async fn rename_passkey(&self, cookie: &str, id: &str, name: &str) -> Result<bool> {
        if name.len() > 160 {
            return Err(IdentityError::InvalidInput);
        }
        let tx = self.db.begin().await?;
        let actor = self.passkey_actor(&tx, cookie).await?;
        let changed = tx
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE webdock_auth.passkey SET name=$1 WHERE id=$2 AND \"userId\"=$3",
                [name.into(), id.into(), actor.id.into()],
            ))
            .await?
            .rows_affected()
            == 1;
        tx.commit().await?;
        Ok(changed)
    }
}
fn decode_state<T: for<'a> Deserialize<'a>>(row: &QueryResult) -> Result<T> {
    serde_json::from_value(row.try_get::<serde_json::Value>("", "state")?)
        .map_err(|_| IdentityError::InvalidEncoding)
}
fn summary(row: &QueryResult) -> Result<PasskeySummary> {
    Ok(PasskeySummary {
        id: row.try_get("", "id")?,
        name: row.try_get("", "name")?,
        created_at: row.try_get("", "createdAt")?,
    })
}
