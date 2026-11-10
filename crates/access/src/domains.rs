//! Domains a customer proves they own with a TXT record. Plugins may route and certify hosts under a
//! verified domain for that customer's projects; Core checks it when it pins their workloads.
use crate::{
    contracts::id,
    service::{json_rows, tenant},
    *,
};
use hickory_resolver::{TokioResolver, proto::rr::RData};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::Value;
use spitzli_identity::Principal;

/// Where the TXT record for `domain` goes.
pub fn challenge_name(domain: &str) -> String {
    format!("_webdock-challenge.{domain}")
}
/// A domain as customers type it: trimmed, lower case, without a trailing dot or scheme, at least two
/// labels, no wildcard.
pub fn domain_name(raw: &str) -> Result<String> {
    let domain = raw.trim().trim_end_matches('.').to_ascii_lowercase();
    let label = |l: &str| {
        (1..=63).contains(&l.len())
            && l.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && !l.starts_with('-')
            && !l.ends_with('-')
    };
    let labels: Vec<&str> = domain.split('.').collect();
    if domain.len() > 253 || labels.len() < 2 || !labels.iter().all(|l| label(l)) {
        return Err(Error::InvalidInput);
    }
    Ok(domain)
}
async fn published(domain: &str, token: &str) -> bool {
    let Ok(resolver) = TokioResolver::builder_tokio().and_then(|b| b.build()) else {
        return false;
    };
    let Ok(lookup) = resolver
        .txt_lookup(format!("{}.", challenge_name(domain)))
        .await
    else {
        return false;
    };
    lookup.answers().iter().any(|record| match &record.data {
        RData::TXT(txt) => txt.txt_data.iter().any(|d| d.as_ref() == token.as_bytes()),
        _ => false,
    })
}
const VIEW: &str = "jsonb_build_object('id',id,'domain',domain,'verified',verified_at IS NOT NULL,'verifiedAt',verified_at,'checkedAt',checked_at,\
    'record',jsonb_build_object('type','TXT','name','_webdock-challenge.'||domain,'value',token),\
    'usedBy',webdock_rls.domain_users(customer_id,domain)) AS data";

async fn reject_platform_domain(tx: &sea_orm::DatabaseTransaction, domain: &str) -> Result<()> {
    let row = tx
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT webdock_rls.platform_host_reserved($1) AS reserved",
            [domain.into()],
        ))
        .await?
        .ok_or(Error::Forbidden)?;
    if row.try_get::<bool>("", "reserved")? {
        return Err(Error::Forbidden);
    }
    Ok(())
}

impl AccessService {
    pub async fn list_domains_as(&self, actor: &Principal, customer: &str) -> Result<Vec<Value>> {
        let actor = self.current_actor(actor).await?;
        let tx = self.domain(&actor).await?;
        tenant(&tx, &actor, customer).await?;
        let rows = json_rows(
            &tx,
            &format!("SELECT {VIEW} FROM webdock_auth.customer_domain WHERE customer_id=$1 ORDER BY domain"),
            vec![id(customer)?.into()],
        )
        .await?;
        tx.commit().await?;
        Ok(rows)
    }
    /// Owners, admins and operators add a domain; it needs its TXT record before plugins may use it.
    pub async fn add_domain_as(
        &self,
        actor: &Principal,
        customer: &str,
        raw: &str,
    ) -> Result<Value> {
        let (actor, tx) = self.domain_writer(actor, customer).await?;
        let domain = domain_name(raw)?;
        reject_platform_domain(&tx, &domain).await?;
        let row_id = self.next_id().await?;
        let mut token = [0u8; 16];
        getrandom::getrandom(&mut token).map_err(|_| Error::Allocation)?;
        let inserted = tx
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO webdock_auth.customer_domain(id,customer_id,domain,token,created_by) VALUES($1,$2,$3,$4,$5) ON CONFLICT (customer_id,domain) DO NOTHING",
                [row_id.into(), id(customer)?.into(), domain.clone().into(), hex::encode(token).into(), actor.user_id.clone().into()],
            ))
            .await?;
        if inserted.rows_affected() == 0 {
            return Err(Error::Conflict);
        }
        self.audit(&tx, &actor, "domain-added", Some(customer), false)
            .await?;
        let row = self.domain_row(&tx, customer, &domain).await?;
        tx.commit().await?;
        Ok(row)
    }
    /// Looks for the TXT record now. Local `.localhost` names need none where local HTTP is allowed.
    pub async fn verify_domain_as(
        &self,
        actor: &Principal,
        customer: &str,
        domain: &str,
    ) -> Result<Value> {
        let (actor, tx) = self.domain_writer(actor, customer).await?;
        let domain = domain_name(domain)?;
        reject_platform_domain(&tx, &domain).await?;
        let row = self.domain_row(&tx, customer, &domain).await?;
        let token: String = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT token FROM webdock_auth.customer_domain WHERE customer_id=$1 AND domain=$2",
                [id(customer)?.into(), domain.clone().into()],
            ))
            .await?
            .ok_or(Error::NotFound)?
            .try_get("", "token")?;
        let local = self.config.allow_local_http && domain.ends_with(".localhost");
        let found = row["verified"] == true || local || published(&domain, &token).await;
        let updated = tx
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE webdock_auth.customer_domain SET checked_at=now(),verified_at=CASE WHEN $3 THEN coalesce(verified_at,now()) END WHERE customer_id=$1 AND domain=$2",
                [id(customer)?.into(), domain.clone().into(), found.into()],
            ))
            .await;
        // Another customer verified it first.
        if updated.as_ref().is_err_and(|e| {
            e.sql_err()
                .is_some_and(|e| matches!(e, sea_orm::SqlErr::UniqueConstraintViolation(_)))
        }) {
            return Err(Error::DomainTaken);
        }
        updated?;
        if found {
            self.audit(&tx, &actor, "domain-verified", Some(customer), false)
                .await?;
        }
        let row = self.domain_row(&tx, customer, &domain).await?;
        tx.commit().await?;
        Ok(row)
    }
    /// Removes a domain no unit uses any more.
    pub async fn remove_domain_as(
        &self,
        actor: &Principal,
        customer: &str,
        domain: &str,
    ) -> Result<()> {
        let (actor, tx) = self.domain_writer(actor, customer).await?;
        let domain = domain_name(domain)?;
        // Applies check ownership and publish their hosts under this same customer lock.
        tx.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT pg_advisory_xact_lock(hashtext('webdock-limits:'||$1))",
            [id(customer)?.into()],
        ))
        .await?;
        let row = self.domain_row(&tx, customer, &domain).await?;
        if row["usedBy"].as_array().is_some_and(|u| !u.is_empty()) {
            return Err(Error::DomainInUse);
        }
        tx.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM webdock_auth.customer_domain WHERE customer_id=$1 AND domain=$2",
            [id(customer)?.into(), domain.into()],
        ))
        .await?;
        self.audit(&tx, &actor, "domain-removed", Some(customer), false)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    pub(crate) async fn domain_writer(
        &self,
        actor: &Principal,
        customer: &str,
    ) -> Result<(Principal, sea_orm::DatabaseTransaction)> {
        let actor = self.current_actor(actor).await?;
        if actor.in_read_only_preview() {
            return Err(Error::Forbidden);
        }
        let tx = self.domain(&actor).await?;
        let profile = tenant(&tx, &actor, customer).await?;
        let role = profile["member_role"].as_str().unwrap_or("");
        if profile["status"] != "active"
            || !(actor.is_operator() || ["owner", "admin"].contains(&role))
        {
            return Err(Error::Forbidden);
        }
        Ok((actor, tx))
    }
    async fn domain_row(
        &self,
        tx: &sea_orm::DatabaseTransaction,
        customer: &str,
        domain: &str,
    ) -> Result<Value> {
        json_rows(
            tx,
            &format!(
                "SELECT {VIEW} FROM webdock_auth.customer_domain WHERE customer_id=$1 AND domain=$2"
            ),
            vec![id(customer)?.into(), domain.into()],
        )
        .await?
        .into_iter()
        .next()
        .ok_or(Error::NotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn domains_are_normalised_full_names() {
        assert_eq!(
            domain_name(" Shop.Example.COM. ").unwrap(),
            "shop.example.com"
        );
        for bad in [
            "example",
            "*.example.com",
            "https://example.com",
            "a..b",
            "-a.example",
            &format!("{}.example", "a".repeat(64)),
        ] {
            assert!(domain_name(bad).is_err(), "{bad}");
        }
        assert_eq!(
            challenge_name("shop.example.com"),
            "_webdock-challenge.shop.example.com"
        );
    }
}
