use super::*;
use crate::mail::client::Client;

struct Step {
    resource: ConfigurationResource,
    request_sha256: String,
    handed_off: bool,
    applied: Option<String>,
}
impl Execution<'_> {
    fn step(&self, kind: ConfigurationResourceKind, id: &str) -> Result<Step> {
        let resource = ConfigurationResource {
            kind,
            id: id.into(),
        };
        let request_sha256 = webdock_agent_protocol::mail::fingerprint(&(
            self.claim
                .spec
                .fingerprint()
                .map_err(|_| ExecutionError::Denied)?,
            &resource,
        ))
        .map_err(|_| ExecutionError::Denied)?;
        let mut step = Step {
            resource,
            request_sha256,
            handed_off: false,
            applied: None,
        };
        for checkpoint in &self.claim.checkpoints {
            let (resource, hash, provider) = match &checkpoint.phase {
                ConfigurationCheckpointPhase::Bound {} => continue,
                ConfigurationCheckpointPhase::Handoff {
                    resource,
                    request_sha256,
                } => (resource, request_sha256, None),
                ConfigurationCheckpointPhase::Applied {
                    resource,
                    request_sha256,
                    provider_id,
                } => (resource, request_sha256, Some(provider_id)),
            };
            if resource.kind != kind || resource.id != id {
                continue;
            }
            if hash != &step.request_sha256 {
                return Err(ExecutionError::Conflict);
            }
            step.handed_off = true;
            if let Some(provider) = provider {
                step.applied = Some(provider.clone());
            }
        }
        Ok(step)
    }
    async fn handoff(&self, step: &Step) -> Result<()> {
        if !step.handed_off {
            self.checkpoint(ConfigurationCheckpointPhase::Handoff {
                resource: step.resource.clone(),
                request_sha256: step.request_sha256.clone(),
            })
            .await?;
        }
        Ok(())
    }
    async fn applied(&self, step: &Step, id: &str) -> Result<()> {
        if let Some(previous) = &step.applied {
            if previous != id {
                return Err(ExecutionError::Ownership);
            }
            return Ok(());
        }
        self.checkpoint(ConfigurationCheckpointPhase::Applied {
            resource: step.resource.clone(),
            request_sha256: step.request_sha256.clone(),
            provider_id: id.into(),
        })
        .await
    }
    fn marker(&self, kind: &str, id: &str) -> String {
        format!("webdock:{kind}:{}:{id}", self.claim.spec.customer_id)
    }
    async fn domain_rows(&self, client: &Client) -> Result<Vec<Domain>> {
        let rows: Rows<Domain> = self
            .call(
                client,
                "x:Domain/get",
                json!({"ids":null,"properties":["id","name","description","dkimManagement"]}),
            )
            .await?;
        if rows.list.len() > 256 {
            return Err(ExecutionError::Ownership);
        }
        Ok(rows.list)
    }
    pub(super) async fn domain(
        &self,
        desired: &ConfigurationDomain,
    ) -> Result<(String, Vec<ConfigurationDkim>)> {
        if self.claim.spec.mode != ConfigurationMode::Apply {
            return Err(ExecutionError::Denied);
        }
        let step = self.step(ConfigurationResourceKind::Domain, &desired.id)?;
        let binding = self.inspect().await?;
        let provider = step.applied.as_deref().or(desired.provider_id.as_deref());
        let marker = self.marker("domain", &desired.id);
        let rows = self.domain_rows(&binding.client).await?;
        let current = select_domain(&rows, provider, &desired.name, &marker, step.handed_off)?;
        self.handoff(&step).await?;
        let id = match current {
            Some(current) => current.id.clone(),
            None if step.handed_off => return Err(ExecutionError::Conflict),
            None => {
                let bound = self.inspect().await?;
                let created: Option<Created> = self.mutation(&bound.client, "x:Domain/set", json!({"create":{"resource":{"name":desired.name,"description":marker,"certificateManagement":{"@type":"Manual"},"dnsManagement":{"@type":"Manual"},"dkimManagement":dkim_management()}}})).await?;
                match created {
                    Some(created) => created_id(created)?,
                    None => {
                        let rows = self.domain_rows(&self.inspect().await?.client).await?;
                        select_domain(&rows, None, &desired.name, &marker, true)?
                            .ok_or(ExecutionError::Conflict)?
                            .id
                            .clone()
                    }
                }
            }
        };
        let mut keys = self.dkim(&binding.client, &id).await?;
        if keys.is_empty() {
            let rows = self.domain_rows(&binding.client).await?;
            let current = select_domain(&rows, Some(&id), &desired.name, &marker, true)?
                .ok_or(ExecutionError::Ownership)?;
            if current.dkim_management != dkim_management() {
                if step.handed_off {
                    return Err(ExecutionError::Conflict);
                }
                let bound = self.inspect().await?;
                let _response: Option<Value> = self
                    .mutation(
                        &bound.client,
                        "x:Domain/set",
                        json!({"update":{(id.as_str()):{"dkimManagement":dkim_management()}}}),
                    )
                    .await?;
                let rows = self.domain_rows(&bound.client).await?;
                if select_domain(&rows, Some(&id), &desired.name, &marker, true)?
                    .is_none_or(|row| row.dkim_management != dkim_management())
                {
                    return Err(ExecutionError::Conflict);
                }
            }
            for _ in 0..100 {
                keys = self.dkim(&binding.client, &id).await?;
                if !keys.is_empty() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        }
        if keys.is_empty() {
            return Err(ExecutionError::Conflict);
        }
        let rows = self.domain_rows(&binding.client).await?;
        let final_domain = select_domain(&rows, Some(&id), &desired.name, &marker, true)?
            .ok_or(ExecutionError::Ownership)?;
        if desired.provider_id.is_none() && final_domain.description.as_deref() != Some(&marker) {
            return Err(ExecutionError::Ownership);
        }
        self.applied(&step, &id).await?;
        Ok((id, keys))
    }
    async fn dkim(&self, client: &Client, domain: &str) -> Result<Vec<ConfigurationDkim>> {
        let rows: Rows<Dkim> = self
            .call(
                client,
                "x:DkimSignature/get",
                json!({"ids":null,"properties":["id","domainId","selector","publicKey","stage"]}),
            )
            .await?;
        if rows.list.len() > 256 {
            return Err(ExecutionError::Ownership);
        }
        let active = rows
            .list
            .into_iter()
            .filter(|key| key.domain_id == domain && key.stage == "active")
            .collect::<Vec<_>>();
        let mut keys = active
            .iter()
            .filter(|key| {
                key.selector.starts_with("webdock-rsa-") || key.selector.starts_with("v1-rsa-")
            })
            .map(|key| ConfigurationDkim {
                id: key.id.clone(),
                selector: key.selector.clone(),
                public_key: key.public_key.split_whitespace().collect(),
            })
            .collect::<Vec<_>>();
        if keys.len() > 2
            || (!active.is_empty() && keys.is_empty())
            || keys.iter().any(|key| !valid_dkim_public(key))
        {
            return Err(ExecutionError::Ownership);
        }
        keys.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(keys)
    }
    async fn accounts(
        &self,
        client: &Client,
        provider: Option<&str>,
        domain: &str,
    ) -> Result<Vec<Account>> {
        let ids = match provider {
            Some(id) => vec![id.to_owned()],
            None => {
                let query: Query = self
                    .call(
                        client,
                        "x:Account/query",
                        json!({"filter":{"domainId":domain},"limit":256,"calculateTotal":true}),
                    )
                    .await?;
                if query.total > 256 || query.ids.len() > 256 || query.total != query.ids.len() {
                    return Err(ExecutionError::Ownership);
                }
                query.ids
            }
        };
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let rows: Rows<Account> = self.call(client, "x:Account/get", json!({"ids":ids,"properties":["id","@type","name","domainId","description","roles","permissions","quotas","memberGroupIds"]})).await?;
        if rows.list.len() > 256 {
            return Err(ExecutionError::Ownership);
        }
        Ok(rows.list)
    }
    pub(super) async fn mailbox(
        &self,
        desired: &ConfigurationMailbox,
        domain: &str,
    ) -> Result<String> {
        if self.claim.spec.mode != ConfigurationMode::Apply {
            return Err(ExecutionError::Denied);
        }
        let marker = self.marker("mailbox", &desired.id);
        let step = self.step(ConfigurationResourceKind::Mailbox, &desired.id)?;
        let binding = self.inspect().await?;
        let provider = step.applied.as_deref().or(desired.provider_id.as_deref());
        let rows = self.accounts(&binding.client, provider, domain).await?;
        let current = select_account(
            &rows,
            provider,
            &desired.local_part,
            domain,
            &marker,
            AccountKind::Group,
            step.handed_off,
        )?;
        self.handoff(&step).await?;
        let id = match current {
            Some(current) => {
                if current.roles != (Roles::Default {})
                    || current.permissions != (Permissions::Inherit {})
                {
                    return Err(ExecutionError::Ownership);
                }
                if current.quotas.get("maxDiskQuota") != Some(&desired.quota_bytes) {
                    if step.handed_off {
                        return Err(ExecutionError::Conflict);
                    }
                    let mut quotas = current.quotas.clone();
                    quotas.insert("maxDiskQuota".into(), desired.quota_bytes);
                    let bound = self.inspect().await?;
                    let _response: Option<Value> = self
                        .mutation(
                            &bound.client,
                            "x:Account/set",
                            json!({"update":{(current.id.as_str()):{"quotas":quotas}}}),
                        )
                        .await?;
                }
                current.id.clone()
            }
            None if step.handed_off => return Err(ExecutionError::Conflict),
            None => {
                let bound = self.inspect().await?;
                let created: Option<Created> = self.mutation(&bound.client, "x:Account/set", json!({"create":{"resource":{"@type":"Group","name":desired.local_part,"domainId":domain,"description":marker,"roles":{"@type":"Default"},"permissions":{"@type":"Inherit"},"quotas":{"maxDiskQuota":desired.quota_bytes}}}})).await?;
                match created {
                    Some(created) => created_id(created)?,
                    None => {
                        let rows = self.accounts(&bound.client, None, domain).await?;
                        select_account(
                            &rows,
                            None,
                            &desired.local_part,
                            domain,
                            &marker,
                            AccountKind::Group,
                            true,
                        )?
                        .ok_or(ExecutionError::Conflict)?
                        .id
                        .clone()
                    }
                }
            }
        };
        let rows = self.accounts(&binding.client, Some(&id), domain).await?;
        let observed = select_account(
            &rows,
            Some(&id),
            &desired.local_part,
            domain,
            &marker,
            AccountKind::Group,
            true,
        )?
        .ok_or(ExecutionError::Ownership)?;
        if observed.roles != (Roles::Default {})
            || observed.permissions != (Permissions::Inherit {})
            || observed.quotas.get("maxDiskQuota") != Some(&desired.quota_bytes)
        {
            return Err(ExecutionError::Conflict);
        }
        self.applied(&step, &id).await?;
        Ok(id)
    }
}

fn created_id(created: Created) -> Result<String> {
    if created.created.len() != 1 {
        return Err(ExecutionError::Conflict);
    }
    created
        .created
        .get("resource")
        .map(|value| value.id.clone())
        .filter(|id| {
            !id.is_empty()
                && id.len() <= 64
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
        })
        .ok_or(ExecutionError::Conflict)
}
fn select_domain<'a>(
    rows: &'a [Domain],
    provider: Option<&str>,
    name: &str,
    marker: &str,
    handed_off: bool,
) -> Result<Option<&'a Domain>> {
    let rows = rows
        .iter()
        .filter(|row| provider.map_or(row.name == name, |provider| row.id == provider))
        .collect::<Vec<_>>();
    if rows.len() > 1 || provider.is_some() && rows.is_empty() {
        return Err(ExecutionError::Ownership);
    }
    let Some(row) = rows.first().copied() else {
        return Ok(None);
    };
    if row.name != name
        || (provider.is_none() && (!handed_off || row.description.as_deref() != Some(marker)))
        || row
            .description
            .as_ref()
            .is_some_and(|description| description.starts_with("webdock:") && description != marker)
    {
        return Err(ExecutionError::Ownership);
    }
    Ok(Some(row))
}
fn select_account<'a>(
    rows: &'a [Account],
    provider: Option<&str>,
    name: &str,
    domain: &str,
    marker: &str,
    kind: AccountKind,
    handed_off: bool,
) -> Result<Option<&'a Account>> {
    let rows = rows
        .iter()
        .filter(|row| {
            provider.map_or(row.name == name && row.domain_id == domain, |provider| {
                row.id == provider
            })
        })
        .collect::<Vec<_>>();
    if rows.len() > 1 || provider.is_some() && rows.is_empty() {
        return Err(ExecutionError::Ownership);
    }
    let Some(row) = rows.first().copied() else {
        return Ok(None);
    };
    if row.name != name
        || row.domain_id != domain
        || row.kind != kind
        || row.description.as_deref() != Some(marker)
        || (provider.is_none() && !handed_off)
    {
        return Err(ExecutionError::Ownership);
    }
    Ok(Some(row))
}

impl Execution<'_> {
    pub(super) async fn identity(
        &self,
        desired: &ConfigurationIdentity,
        internal: &str,
        groups: &BTreeMap<String, String>,
    ) -> Result<Option<String>> {
        let step = self.step(ConfigurationResourceKind::Identity, &desired.id)?;
        let binding = self.inspect().await?;
        let provider = step.applied.as_deref().or(desired.provider_id.as_deref());
        let marker = self.marker("identity", &desired.id);
        let name = format!("wd-{}", desired.id);
        let rows = self.accounts(&binding.client, provider, internal).await?;
        let current = select_account(
            &rows,
            provider,
            &name,
            internal,
            &marker,
            AccountKind::User,
            step.handed_off,
        )?;
        if current.is_none() && !desired.enabled && provider.is_none() && !step.handed_off {
            return Ok(None);
        }
        let mut membership = desired
            .mailbox_ids
            .iter()
            .map(|id| {
                groups
                    .get(id)
                    .map(|id| (id.clone(), true))
                    .ok_or(ExecutionError::Ownership)
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        let restrictive = self.claim.spec.mode == ConfigurationMode::Restrict;
        let desired_permissions = match current {
            Some(current) if restrictive => {
                membership.retain(|id, _| current.member_group_ids.get(id) == Some(&true));
                if membership.is_empty() {
                    permissions(false)
                } else {
                    current.permissions.clone()
                }
            }
            _ if restrictive => return Err(ExecutionError::Ownership),
            _ => permissions(desired.enabled),
        };
        self.handoff(&step).await?;
        let id = match current {
            Some(current) => {
                if current.roles != (Roles::User {}) {
                    return Err(ExecutionError::Ownership);
                }
                let matches = current.member_group_ids == membership
                    && current.permissions == desired_permissions
                    && (restrictive || current.quotas.get("maxDiskQuota") == Some(&1048576));
                if !matches {
                    if step.handed_off {
                        return Err(ExecutionError::Conflict);
                    }
                    let mut change =
                        json!({"permissions":desired_permissions,"memberGroupIds":membership});
                    if !restrictive {
                        let mut quotas = current.quotas.clone();
                        quotas.insert("maxDiskQuota".into(), 1048576);
                        change["quotas"] = json!(quotas);
                    }
                    let bound = self.inspect().await?;
                    let _response: Option<Value> = self
                        .mutation(
                            &bound.client,
                            "x:Account/set",
                            json!({"update":{(current.id.as_str()):change}}),
                        )
                        .await?;
                }
                current.id.clone()
            }
            None if step.handed_off => return Err(ExecutionError::Conflict),
            None => {
                let credential = self
                    .claim
                    .material
                    .identities
                    .get(&desired.id)
                    .ok_or(ExecutionError::Denied)?;
                let bound = self.inspect().await?;
                let created: Option<Created> = self.mutation(&bound.client, "x:Account/set", json!({"create":{"resource":{"@type":"User","name":name,"domainId":internal,"description":marker,"roles":{"@type":"User"},"permissions":desired_permissions,"quotas":{"maxDiskQuota":1048576},"encryptionAtRest":{"@type":"Disabled"},"credentials":{"0":{"@type":"Password","secret":credential.password}},"memberGroupIds":membership}}})).await?;
                match created {
                    Some(created) => created_id(created)?,
                    None => {
                        let rows = self.accounts(&bound.client, None, internal).await?;
                        select_account(
                            &rows,
                            None,
                            &name,
                            internal,
                            &marker,
                            AccountKind::User,
                            true,
                        )?
                        .ok_or(ExecutionError::Conflict)?
                        .id
                        .clone()
                    }
                }
            }
        };
        let rows = self.accounts(&binding.client, Some(&id), internal).await?;
        let observed = select_account(
            &rows,
            Some(&id),
            &name,
            internal,
            &marker,
            AccountKind::User,
            true,
        )?
        .ok_or(ExecutionError::Ownership)?;
        if observed.roles != (Roles::User {})
            || observed.permissions != desired_permissions
            || observed.member_group_ids != membership
            || (!restrictive && observed.quotas.get("maxDiskQuota") != Some(&1048576))
        {
            return Err(ExecutionError::Conflict);
        }
        if !restrictive {
            let credential = self
                .claim
                .material
                .identities
                .get(&desired.id)
                .ok_or(ExecutionError::Denied)?;
            let bound = self.inspect().await?;
            super::guard(self.claim, self.authority).await?;
            bound
                .client
                .identity_access(
                    &credential.username,
                    &credential.password,
                    &id,
                    desired.enabled,
                )
                .await?;
            super::guard(self.claim, self.authority).await?;
        }
        self.applied(&step, &id).await?;
        Ok(Some(id))
    }
    pub(super) async fn retained_mailbox(&self, desired: &ConfigurationMailbox) -> Result<String> {
        let provider = desired
            .provider_id
            .as_deref()
            .ok_or(ExecutionError::Ownership)?;
        let binding = self.inspect().await?;
        let rows = self.accounts(&binding.client, Some(provider), "").await?;
        let matching = rows
            .iter()
            .filter(|row| row.id == provider)
            .collect::<Vec<_>>();
        if matching.len() != 1 {
            return Err(ExecutionError::Ownership);
        }
        let row = matching[0];
        if row.name != desired.local_part
            || row.kind != AccountKind::Group
            || row.roles != (Roles::Default {})
            || row.description.as_deref() != Some(self.marker("mailbox", &desired.id).as_str())
        {
            return Err(ExecutionError::Ownership);
        }
        Ok(row.id.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn create_loss_requires_exact_marked_handoff_and_never_adopts_an_unmarked_domain() {
        let mut domain = Domain {
            id: "abc".into(),
            name: "customer.example.test".into(),
            description: None,
            dkim_management: DkimManagement::Manual {},
        };
        let marker = "webdock:domain:123:456";
        assert!(
            select_domain(
                std::slice::from_ref(&domain),
                None,
                &domain.name,
                marker,
                true
            )
            .is_err()
        );
        domain.description = Some(marker.into());
        assert!(
            select_domain(
                std::slice::from_ref(&domain),
                None,
                &domain.name,
                marker,
                false
            )
            .is_err()
        );
        assert_eq!(
            select_domain(
                std::slice::from_ref(&domain),
                None,
                &domain.name,
                marker,
                true
            )
            .unwrap()
            .unwrap()
            .id,
            "abc"
        );
        assert!(
            select_domain(
                std::slice::from_ref(&domain),
                Some("other"),
                &domain.name,
                marker,
                true
            )
            .is_err()
        );
        assert!(
            select_domain(
                &[domain.clone(), domain.clone()],
                None,
                &domain.name,
                marker,
                true
            )
            .is_err()
        );
    }
    #[test]
    fn known_account_references_still_require_type_marker_name_and_domain() {
        let account = Account {
            id: "abc".into(),
            kind: AccountKind::User,
            name: "wd-456".into(),
            domain_id: "domain".into(),
            description: Some("webdock:identity:123:456".into()),
            roles: Roles::User {},
            permissions: permissions(true),
            quotas: BTreeMap::new(),
            member_group_ids: BTreeMap::new(),
        };
        for (name, domain, marker, kind) in [
            (
                "other",
                "domain",
                "webdock:identity:123:456",
                AccountKind::User,
            ),
            (
                "wd-456",
                "foreign",
                "webdock:identity:123:456",
                AccountKind::User,
            ),
            ("wd-456", "domain", "foreign", AccountKind::User),
            (
                "wd-456",
                "domain",
                "webdock:identity:123:456",
                AccountKind::Group,
            ),
        ] {
            assert!(
                select_account(
                    std::slice::from_ref(&account),
                    Some("abc"),
                    name,
                    domain,
                    marker,
                    kind,
                    true
                )
                .is_err()
            );
        }
        assert!(
            select_account(
                std::slice::from_ref(&account),
                None,
                &account.name,
                &account.domain_id,
                account.description.as_deref().unwrap(),
                account.kind,
                false
            )
            .is_err()
        );
    }
}
