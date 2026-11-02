//! Public contact intake never creates an account, membership or infrastructure.
use crate::{
    contracts::{id, text, valid_email},
    service::{json_rows, operator},
    *,
};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use webdock_identity::Principal;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WaitlistSubmission {
    pub email: String,
    pub name: Option<String>,
    pub privacy_accepted: bool,
}
impl WaitlistSubmission {
    pub fn validate(mut self) -> Result<Self> {
        if !self.privacy_accepted {
            return Err(Error::InvalidInput);
        }
        self.email = self.email.trim().to_lowercase();
        valid_email(&self.email)?;
        let (local, _) = self.email.split_once('@').ok_or(Error::InvalidInput)?;
        if local.starts_with('.') || local.ends_with('.') || self.email.contains("..") {
            return Err(Error::InvalidInput);
        }
        self.name = self
            .name
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
            .map(|v| {
                if v.chars().any(char::is_control) {
                    return Err(Error::InvalidInput);
                }
                text(&v, 100)
            })
            .transpose()?;
        Ok(self)
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WaitlistAction {
    Approve,
    Reject,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaitlistDecision {
    pub action: WaitlistAction,
    pub version: i32,
}
const VIEW: &str = "jsonb_build_object('id',w.id,'email',w.email,'name',w.name,'status',w.status,'createdAt',w.created_at,'reviewedAt',w.reviewed_at,'version',w.version) AS data";
impl AccessService {
    pub async fn submit_waitlist(&self, input: WaitlistSubmission) -> Result<()> {
        let input = input.validate()?;
        let entry = self.next_id().await?;
        let accepted = self
            .auth
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_auth.submit_waitlist($1,$2,$3) AS accepted",
                [entry.into(), input.email.into(), input.name.into()],
            ))
            .await?
            .ok_or(Error::Database(sea_orm::DbErr::Custom(
                "Waitlist result absent".into(),
            )))?
            .try_get::<bool>("", "accepted")?;
        if !accepted {
            return Err(Error::RateLimited);
        }
        Ok(())
    }
    pub async fn list_waitlist_as(&self, actor: &Principal, cursor: Option<&str>) -> Result<Value> {
        let cursor = cursor.map(id).transpose()?;
        let (tx, auth, current) = self.fenced_session_domain(actor).await?;
        operator(&tx).await?;
        if !current.is_operator() || current.customer_scope().is_some() {
            return Err(Error::Forbidden);
        }
        let rows=json_rows(&tx,&format!("SELECT {VIEW} FROM webdock_auth.waitlist_entry w WHERE ($1::text IS NULL OR w.id::numeric<$1::numeric) ORDER BY w.id::numeric DESC LIMIT 100"),vec![cursor.map(str::to_owned).into()]).await?;
        let next = if rows.len() == 100 {
            rows.last()
                .and_then(|v| v["id"].as_str())
                .map(str::to_owned)
        } else {
            None
        };
        tx.commit().await?;
        auth.commit().await?;
        Ok(json!({"entries":rows,"nextCursor":next}))
    }
    pub async fn review_waitlist_as(
        &self,
        actor: &Principal,
        entry: &str,
        command: WaitlistDecision,
    ) -> Result<Value> {
        if !(1..i32::MAX).contains(&command.version) {
            return Err(Error::InvalidInput);
        }
        let (tx, auth, current) = self.fenced_session_domain(actor).await?;
        operator(&tx).await?;
        if !current.is_operator() || current.customer_scope().is_some() {
            return Err(Error::Forbidden);
        }
        let status = match command.action {
            WaitlistAction::Approve => "approved",
            WaitlistAction::Reject => "rejected",
        };
        let row=json_rows(&tx,&format!("UPDATE webdock_auth.waitlist_entry w SET status=$2,reviewed_by=$3,reviewed_at=statement_timestamp(),version=version+1 WHERE id=$1 AND status='pending' AND version=$4 RETURNING {VIEW}"),vec![id(entry)?.into(),status.into(),current.user_id.clone().into(),command.version.into()]).await?.into_iter().next().ok_or(Error::Conflict)?;
        self.audit(
            &tx,
            &current,
            if status == "approved" {
                "waitlist-approved"
            } else {
                "waitlist-rejected"
            },
            Some(entry),
            false,
        )
        .await?;
        tx.commit().await?;
        auth.commit().await?;
        Ok(row)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn public_input_cannot_set_status_or_skip_consent() {
        let input = serde_json::from_value::<WaitlistSubmission>(
            json!({"email":" User@Example.test ","name":"Name","privacyAccepted":true}),
        )
        .unwrap()
        .validate()
        .unwrap();
        assert_eq!(input.email, "user@example.test");
        assert!(
            WaitlistSubmission {
                email: "a..b@example.test".into(),
                name: None,
                privacy_accepted: true
            }
            .validate()
            .is_err()
        );
        assert!(
            serde_json::from_value::<WaitlistSubmission>(
                json!({"email":"u@example.test","privacyAccepted":true,"status":"approved"})
            )
            .is_err()
        );
        assert!(
            WaitlistSubmission {
                email: "u@example.test".into(),
                name: None,
                privacy_accepted: false
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn public_contact_names_reject_controls_before_database_intake() {
        for control in ['\u{0}', '\u{1f}', '\u{7f}', '\u{80}', '\u{9f}'] {
            assert!(
                WaitlistSubmission {
                    email: "contact@example.test".into(),
                    name: Some(format!("Contact{control}Name")),
                    privacy_accepted: true,
                }
                .validate()
                .is_err(),
                "accepted embedded control {control:?}"
            );
        }
        assert!(
            WaitlistSubmission {
                email: "contact@example.test".into(),
                name: Some("Zoë Müller 👋".into()),
                privacy_accepted: true,
            }
            .validate()
            .is_ok()
        );
    }
}
