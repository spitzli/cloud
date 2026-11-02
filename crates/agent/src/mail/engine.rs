use super::{
    MailAuthority,
    client::{Account, Client, Domain, Listener, Rows},
};
use serde_json::{Value, json};
use std::collections::HashMap;
use webdock_agent_protocol::mail::MailClaim;
use webdock_supervisor::{ExecutionError, Result};
pub(super) async fn configure_certificate(
    claim: &MailClaim,
    authority: &dyn MailAuthority,
    admin: &Client,
) -> Result<()> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Certificate {
        id: String,
        subject_alternative_names: HashMap<String, bool>,
    }
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Settings {
        default_certificate_id: Option<String>,
    }
    #[derive(serde::Deserialize)]
    struct Created {
        created: HashMap<String, CreatedId>,
    }
    #[derive(serde::Deserialize)]
    struct CreatedId {
        id: String,
    }
    let settings: Rows<Settings> = admin
        .call("x:SystemSettings/get", json!({"ids":["singleton"]}))
        .await?;
    if settings.list.len() != 1 {
        return Err(ExecutionError::Ownership);
    }
    let certificates: Rows<Certificate> =
        admin.call("x:Certificate/get", json!({"ids":null})).await?;
    let candidates = certificates
        .list
        .iter()
        .filter(|c| c.subject_alternative_names.get(&claim.spec.hostname) == Some(&true))
        .collect::<Vec<_>>();
    if candidates.len() > 1 {
        return Err(ExecutionError::Ownership);
    }
    if let Some(current) = &settings.list[0].default_certificate_id {
        if candidates.first().is_none_or(|c| &c.id != current) {
            return Err(ExecutionError::Ownership);
        }
        return Ok(());
    }
    let id = if let Some(existing) = candidates.first() {
        existing.id.clone()
    } else {
        let rcgen::CertifiedKey { cert, signing_key } =
            rcgen::generate_simple_self_signed(vec![claim.spec.hostname.clone()])
                .map_err(|_| ExecutionError::Runtime)?;
        authority.guard(claim).await?;
        let mut result:Created=admin.call("x:Certificate/set",json!({"create":{"resource":{"certificate":{"@type":"Text","value":cert.pem()},"privateKey":{"@type":"Text","secret":signing_key.serialize_pem()}}}})).await?;
        result
            .created
            .remove("resource")
            .ok_or(ExecutionError::Runtime)?
            .id
    };
    authority.guard(claim).await?;
    let _: Value = admin
        .call(
            "x:SystemSettings/set",
            json!({"update":{"singleton":{"defaultCertificateId":id}}}),
        )
        .await?;
    Ok(())
}
pub(super) async fn configure_account(
    claim: &MailClaim,
    authority: &dyn MailAuthority,
    admin: &Client,
    username: &str,
    password: &str,
) -> Result<()> {
    let (name, domain) = username
        .split_once('@')
        .ok_or(ExecutionError::Invalid("Mail management username"))?;
    if domain != claim.spec.hostname || name != "webdock-admin" {
        return Err(ExecutionError::Invalid("Mail management username"));
    }
    let domains: Rows<Domain> = admin.call("x:Domain/get", json!({"ids":null})).await?;
    let matches = domains
        .list
        .iter()
        .filter(|d| d.name == domain)
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(ExecutionError::Ownership);
    }
    let domain_id = &matches[0].id;
    let accounts: Rows<Account> = admin.call("x:Account/get", json!({"ids":null})).await?;
    // Recovery is limited to the single bootstrap administrator, never a customer mailbox store.
    if accounts.list.len() != 1 {
        return Err(ExecutionError::Ownership);
    }
    let account = &accounts.list[0];
    let super::client::AccountRole::Admin {} = &account.roles;
    if account.domain_id != *domain_id || !["admin", name].contains(&account.name.as_str()) {
        return Err(ExecutionError::Ownership);
    }
    authority.guard(claim).await?;
    let _:Value=admin.call("x:Account/set",json!({"update":{&account.id:{"name":name,"credentials":{"0":{"@type":"Password","secret":password}},"roles":{"@type":"Admin"}}}})).await?;
    Ok(())
}
pub(super) async fn configure_listeners(
    claim: &MailClaim,
    authority: &dyn MailAuthority,
    admin: &Client,
) -> Result<()> {
    let rows: Rows<Listener> = admin
        .call("x:NetworkListener/get", json!({"ids":null}))
        .await?;
    let ports = [
        ("http", 8080),
        ("smtp", 2525),
        ("submissions", 2465),
        ("imaps", 1993),
    ];
    let mut update = serde_json::Map::new();
    let mut destroy = Vec::new();
    let mut create = serde_json::Map::new();
    let mut found = std::collections::HashSet::new();
    for row in rows.list {
        match ports.iter().find(|(name, _)| *name == row.name) {
            Some((name, port)) => {
                if !found.insert(*name) {
                    return Err(ExecutionError::Ownership);
                }
                update.insert(row.id, json!({"bind":{format!("[::]:{port}"):true}}));
            }
            None => destroy.push(row.id),
        }
    }
    for (name, port) in ports {
        if !found.contains(name) {
            create.insert(name.into(),json!({"name":name,"bind":{format!("[::]:{port}"):true},"protocol":match name{"http"=>"http","imaps"=>"imap",_=>"smtp"},"useTls":name!="http","tlsImplicit":matches!(name,"imaps"|"submissions")}));
        }
    }
    authority.guard(claim).await?;
    let _: Value = admin
        .call(
            "x:NetworkListener/set",
            json!({"update":update,"destroy":destroy,"create":create}),
        )
        .await?;
    Ok(())
}
