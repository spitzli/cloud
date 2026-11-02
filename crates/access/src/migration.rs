//! Offline, reviewed authority transitions. Never expose these methods as public HTTP handlers.
use crate::{
    contracts::{id, text},
    service::{json_rows, local_org},
    *,
};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, Statement,
    TransactionTrait,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivationReview {
    /// Exact inventory digest returned by inspect; a stale review is rejected.
    pub snapshot_sha256: String,
    pub evidence_ref: String,
    pub evidence_sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LocalAccountRole {
    User,
    Operator,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovedMember {
    pub user_id: String,
    pub role: MemberRole,
}
#[derive(Debug, Serialize)]
pub struct AuthorityInventory {
    pub id: String,
    pub bindings: Value,
    pub snapshot_sha256: String,
    /// Identifiers/roles only. Credential and token material never leaves the encrypted archive.
    pub memberships: Value,
    pub grants: Value,
}
impl ActivationReview {
    pub fn validate(&self) -> Result<()> {
        text(&self.evidence_ref, 1024)?;
        for value in [&self.snapshot_sha256, &self.evidence_sha256] {
            if value.len() != 64
                || !value
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(Error::InvalidInput);
            }
        }
        Ok(())
    }
}
impl AccessService {
    /// `offline` must be the separately provisioned migration connection, never a runtime pool.
    pub async fn inspect_identity_activation(
        &self,
        offline: &DatabaseConnection,
        cookie: &str,
        subject: &str,
    ) -> Result<AuthorityInventory> {
        self.require_operator(cookie).await?;
        let tx = offline.begin().await?;
        let snapshot = identity_snapshot(&tx, id(subject)?).await?;
        let result = inventory(subject, &snapshot);
        tx.rollback().await?;
        Ok(result)
    }
    /// Clears projected authority by default. Memberships/grants require later explicit native administration.
    /// Existing mapping tuples remain retained and are never linked or merged by email.
    pub async fn activate_local_identity(
        &self,
        offline: &DatabaseConnection,
        cookie: &str,
        subject: &str,
        role: LocalAccountRole,
        review: ActivationReview,
    ) -> Result<ChangeResult> {
        review.validate()?;
        id(subject)?;
        let actor = self.require_operator(cookie).await?;
        if actor.user_id == subject {
            return Err(Error::Forbidden);
        }
        let tx = offline.begin().await?;
        // Serialize all organization membership edits before locking affected users.
        tx.query_all_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id FROM webdock_auth.organization WHERE id IN (SELECT \"organizationId\" FROM webdock_auth.member WHERE \"userId\"=$1) ORDER BY id FOR UPDATE",[subject.into()])).await?;
        tx.query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM webdock_auth.\"user\" WHERE id=$1 FOR UPDATE",
            [subject.into()],
        ))
        .await?
        .ok_or(Error::NotFound)?;
        self.offline_actor(&tx, &actor).await?;
        let snapshot = identity_snapshot(&tx, subject).await?;
        checked_review(&snapshot, &review)?;
        if snapshot["bindings"].as_array().is_none_or(|v| v.is_empty()) {
            return Err(Error::Conflict);
        }
        if tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT user_id FROM webdock_auth.local_identity_activation WHERE user_id=$1",
                [subject.into()],
            ))
            .await?
            .is_some()
        {
            return Err(Error::Conflict);
        }
        let role = match role {
            LocalAccountRole::User => "user",
            LocalAccountRole::Operator => "operator",
        };
        let archive = self.keys.encrypt(&snapshot.to_string())?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.local_identity_activation(user_id,bindings,approved_state,evidence_ref,evidence_sha256,operator_subject,archive_envelope) VALUES($1,$2,$3,$4,$5,$6,$7)",[subject.into(),snapshot["bindings"].clone().into(),json!({"role":role,"memberships":[],"grants":[]}).into(),review.evidence_ref.into(),review.evidence_sha256.into(),actor.user_id.clone().into(),archive.into()])).await?;
        // Retire links without deleting/reassigning their immutable canonical bindings.
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.clerk_identity SET enabled=false,legacy_eligible=false WHERE user_id=$1",[subject.into()])).await?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.clerk_identity_mapping SET revoked_at=coalesce(revoked_at,statement_timestamp()) WHERE local_subject=$1",[subject.into()])).await?;
        tx.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM webdock_auth.member WHERE \"userId\"=$1",
            [subject.into()],
        ))
        .await?;
        tx.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE webdock_auth.project_grant SET enabled=false WHERE user_id=$1",
            [subject.into()],
        ))
        .await?;
        for sql in [
            "DELETE FROM webdock_auth.session WHERE \"userId\"=$1",
            "DELETE FROM webdock_auth.\"twoFactor\" WHERE \"userId\"=$1",
            "DELETE FROM webdock_auth.passkey WHERE \"userId\"=$1",
            "DELETE FROM webdock_auth.identity_mfa_challenge WHERE user_id=$1",
            "DELETE FROM webdock_auth.identity_recovery_token WHERE user_id=$1",
        ] {
            tx.execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                sql,
                [subject.into()],
            ))
            .await?;
        }
        // A fresh random unknown credential cannot be used until the bound activation is redeemed.
        let mut bytes = [0u8; 32];
        getrandom::getrandom(&mut bytes).map_err(|_| Error::Configuration)?;
        let password = tokio::task::spawn_blocking(move || {
            webdock_identity::hash_password(&hex::encode(bytes))
        })
        .await
        .map_err(|_| Error::Configuration)??;
        tx.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM webdock_auth.account WHERE \"userId\"=$1 AND \"providerId\"='credential'",
            [subject.into()],
        ))
        .await?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.account(id,\"accountId\",\"providerId\",\"userId\",password,\"createdAt\",\"updatedAt\") VALUES($1,$2,'credential',$2,$3,statement_timestamp(),statement_timestamp())",[self.next_id().await?.into(),subject.into(),password.into()])).await?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.\"user\" SET role=$2,\"emailVerified\"=false,\"mustChangePassword\"=true,\"twoFactorEnabled\"=false,\"updatedAt\"=statement_timestamp() WHERE id=$1",[subject.into(),role.into()])).await?;
        self.audit(
            &tx,
            &actor,
            "local-identity-activation",
            Some(subject),
            false,
        )
        .await?;
        tx.commit().await?;
        let delivery = if self
            .identity
            .queue_account_activation(subject, None)
            .await
            .is_ok()
        {
            "queued"
        } else {
            "activation-unavailable"
        };
        Ok(ChangeResult {
            link: None,
            id: Some(subject.into()),
            message:
                "Local authority activated. Account verification and security setup are required."
                    .into(),
            delivery: Some(delivery),
        })
    }
    pub async fn inspect_organization_activation(
        &self,
        offline: &DatabaseConnection,
        cookie: &str,
        organization: &str,
    ) -> Result<AuthorityInventory> {
        self.require_operator(cookie).await?;
        let tx = offline.begin().await?;
        let snapshot = organization_snapshot(&tx, id(organization)?).await?;
        let result = inventory(organization, &snapshot);
        tx.rollback().await?;
        Ok(result)
    }
    /// Applies an explicit reviewed native membership manifest. Existing project grants are disabled.
    pub async fn activate_local_organization(
        &self,
        offline: &DatabaseConnection,
        cookie: &str,
        organization: &str,
        members: Vec<ApprovedMember>,
        review: ActivationReview,
    ) -> Result<ChangeResult> {
        review.validate()?;
        id(organization)?;
        if members.is_empty()
            || members.len() > 100
            || !members.iter().any(|m| m.role == MemberRole::Owner)
        {
            return Err(Error::InvalidInput);
        }
        let mut unique = std::collections::BTreeSet::new();
        for member in &members {
            if !unique.insert(id(&member.user_id)?) {
                return Err(Error::InvalidInput);
            }
        }
        let actor = self.require_operator(cookie).await?;
        let tx = offline.begin().await?;
        tx.query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM webdock_auth.organization WHERE id=$1 FOR UPDATE",
            [organization.into()],
        ))
        .await?
        .ok_or(Error::NotFound)?;
        self.offline_actor(&tx, &actor).await?;
        let snapshot = organization_snapshot(&tx, organization).await?;
        checked_review(&snapshot, &review)?;
        if snapshot["bindings"].as_array().is_none_or(|v| v.is_empty())
            || local_org(&tx, organization).await?
        {
            return Err(Error::Conflict);
        }
        for member in &members {
            if tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id FROM webdock_auth.\"user\" WHERE id=$1 AND role='user' AND NOT coalesce(banned,false) AND \"emailVerified\" AND NOT \"mustChangePassword\" AND webdock_rls.local_identity_authority(id) FOR SHARE",[member.user_id.clone().into()])).await?.is_none(){return Err(Error::Forbidden)}
        }
        // Put a new verified owner in place before demoting/removing projected owners.
        for member in members.iter().filter(|m| m.role == MemberRole::Owner) {
            self.upsert_reviewed_member(&tx, organization, member)
                .await?;
        }
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"DELETE FROM webdock_auth.member WHERE \"organizationId\"=$1 AND NOT(\"userId\"=ANY($2::text[]))",[organization.into(),members.iter().map(|m|m.user_id.clone()).collect::<Vec<_>>().into()])).await?;
        for member in members.iter().filter(|m| m.role != MemberRole::Owner) {
            self.upsert_reviewed_member(&tx, organization, member)
                .await?;
        }
        tx.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE webdock_auth.project_grant SET enabled=false WHERE organization_id=$1",
            [organization.into()],
        ))
        .await?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.invitation SET status='canceled' WHERE \"organizationId\"=$1 AND status='pending'",[organization.into()])).await?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.local_organization_activation(organization_id,bindings,approved_state,evidence_ref,evidence_sha256,operator_subject,archive_envelope) VALUES($1,$2,$3,$4,$5,$6,$7)",[organization.into(),snapshot["bindings"].clone().into(),serde_json::to_value(&members).map_err(|_|Error::InvalidInput)?.into(),review.evidence_ref.into(),review.evidence_sha256.into(),actor.user_id.clone().into(),self.keys.encrypt(&snapshot.to_string())?.into()])).await?;
        tx.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE webdock_auth.clerk_organization SET enabled=false WHERE organization_id=$1",
            [organization.into()],
        ))
        .await?;
        self.audit(
            &tx,
            &actor,
            "local-organization-activation",
            Some(organization),
            false,
        )
        .await?;
        tx.commit().await?;
        Ok(ChangeResult {
            link: None,
            id: Some(organization.into()),
            message: "Reviewed native memberships activated; previous project grants disabled."
                .into(),
            delivery: None,
        })
    }
    async fn upsert_reviewed_member(
        &self,
        tx: &DatabaseTransaction,
        org: &str,
        m: &ApprovedMember,
    ) -> Result<()> {
        let row = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM webdock_auth.member WHERE \"organizationId\"=$1 AND \"userId\"=$2",
                [org.into(), m.user_id.clone().into()],
            ))
            .await?;
        if let Some(row) = row {
            tx.execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE webdock_auth.member SET role=$2 WHERE id=$1",
                [
                    row.try_get::<String>("", "id")?.into(),
                    m.role.as_str().into(),
                ],
            ))
            .await?;
        } else {
            tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.member(id,\"organizationId\",\"userId\",role,\"createdAt\") VALUES($1,$2,$3,$4,statement_timestamp())",[self.next_id().await?.into(),org.into(),m.user_id.clone().into(),m.role.as_str().into()])).await?;
        }
        Ok(())
    }
    async fn offline_actor(
        &self,
        tx: &DatabaseTransaction,
        actor: &webdock_identity::Principal,
    ) -> Result<()> {
        let current = self
            .identity
            .authenticate_session_in(tx, &actor.user_id, &actor.session_id)
            .await?
            .ok_or(Error::Unauthorized)?;
        if !current.is_operator() {
            return Err(Error::Forbidden);
        }
        Ok(())
    }
}
fn checked_review(snapshot: &Value, review: &ActivationReview) -> Result<()> {
    if digest(snapshot) != review.snapshot_sha256 {
        Err(Error::Conflict)
    } else {
        Ok(())
    }
}
fn digest(v: &Value) -> String {
    hex::encode(Sha256::digest(v.to_string().as_bytes()))
}
fn inventory(id: &str, snapshot: &Value) -> AuthorityInventory {
    AuthorityInventory {
        id: id.into(),
        bindings: snapshot["bindings"].clone(),
        snapshot_sha256: digest(snapshot),
        memberships: snapshot["memberships"].clone(),
        grants: snapshot["grants"].clone(),
    }
}
async fn identity_snapshot(tx: &DatabaseTransaction, subject: &str) -> Result<Value> {
    let users = json_rows(
        tx,
        "SELECT to_jsonb(u) AS data FROM webdock_auth.\"user\" u WHERE id=$1",
        vec![subject.into()],
    )
    .await?;
    if users.len() != 1 {
        return Err(Error::NotFound);
    }
    let mut snapshot = json!({"user":users[0]});
    for (name, sql) in [
        (
            "bindings",
            "SELECT jsonb_build_array(source,issuer,external_subject) AS data FROM (SELECT 'legacy'::text AS source,issuer,clerk_user_id AS external_subject FROM webdock_auth.clerk_identity WHERE user_id=$1 UNION ALL SELECT 'mapping',issuer,external_subject FROM webdock_auth.clerk_identity_mapping WHERE local_subject=$1) b ORDER BY source,issuer,external_subject",
        ),
        (
            "accounts",
            "SELECT to_jsonb(a) AS data FROM webdock_auth.account a WHERE \"userId\"=$1 ORDER BY id",
        ),
        (
            "factors",
            "SELECT to_jsonb(t) AS data FROM webdock_auth.\"twoFactor\" t WHERE \"userId\"=$1 ORDER BY id",
        ),
        (
            "passkeys",
            "SELECT to_jsonb(p) AS data FROM webdock_auth.passkey p WHERE \"userId\"=$1 ORDER BY id",
        ),
        (
            "sessions",
            "SELECT to_jsonb(s) AS data FROM webdock_auth.session s WHERE \"userId\"=$1 ORDER BY id",
        ),
        (
            "memberships",
            "SELECT to_jsonb(m) AS data FROM webdock_auth.member m WHERE \"userId\"=$1 ORDER BY id",
        ),
        (
            "grants",
            "SELECT to_jsonb(g) AS data FROM webdock_auth.project_grant g WHERE user_id=$1 ORDER BY id",
        ),
    ] {
        snapshot[name] = json!(json_rows(tx, sql, vec![subject.into()]).await?);
    }
    Ok(snapshot)
}
async fn organization_snapshot(tx: &DatabaseTransaction, org: &str) -> Result<Value> {
    let organizations = json_rows(
        tx,
        "SELECT to_jsonb(o) AS data FROM webdock_auth.organization o WHERE id=$1",
        vec![org.into()],
    )
    .await?;
    if organizations.len() != 1 {
        return Err(Error::NotFound);
    }
    let mut snapshot = json!({"organization":organizations[0]});
    for (name, sql) in [
        (
            "bindings",
            "SELECT jsonb_build_array('organization',issuer,clerk_organization_id) AS data FROM webdock_auth.clerk_organization WHERE organization_id=$1 ORDER BY issuer,clerk_organization_id",
        ),
        (
            "memberships",
            "SELECT to_jsonb(m) AS data FROM webdock_auth.member m WHERE \"organizationId\"=$1 ORDER BY id",
        ),
        (
            "grants",
            "SELECT to_jsonb(g) AS data FROM webdock_auth.project_grant g WHERE organization_id=$1 ORDER BY id",
        ),
        (
            "invitations",
            "SELECT to_jsonb(i) AS data FROM webdock_auth.invitation i WHERE \"organizationId\"=$1 ORDER BY id",
        ),
    ] {
        snapshot[name] = json!(json_rows(tx, sql, vec![org.into()]).await?);
    }
    Ok(snapshot)
}
