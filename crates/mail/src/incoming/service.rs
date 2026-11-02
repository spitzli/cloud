use super::*;
use crate::outbox::run;

impl NativeIncoming {
    pub async fn service(
        &self,
        principal: &Principal,
        stored: &StoredAuthority,
        customer: &str,
    ) -> Result<ServiceDirectory> {
        if !self.config.enabled {
            return Err(Error::Unavailable);
        }
        let access = authority::customer_for_lifecycle(
            &self.dependencies,
            &self.signer,
            principal,
            stored,
            customer,
            false,
        )
        .await?;
        let result = self
            .service_view(&access.business, customer, access.operator)
            .await?;
        commit(access).await?;
        Ok(result)
    }
    pub async fn manage_service(
        &self,
        principal: &Principal,
        stored: &StoredAuthority,
        customer: &str,
        command: ServiceCommand,
    ) -> Result<ServiceDirectory> {
        if !self.config.enabled {
            return Err(Error::Unavailable);
        }
        let (requested, expected, reconcile) = match command {
            ServiceCommand::Activate { revision: value } => (Some(true), revision(&value)?, false),
            ServiceCommand::Suspend { revision: value } => (Some(false), revision(&value)?, false),
            ServiceCommand::Reconcile { revision: value } => (None, revision(&value)?, true),
        };
        let operation_id = self.id().await?;
        let event_id = self.id().await?;
        let access = authority::customer_for_lifecycle(
            &self.dependencies,
            &self.signer,
            principal,
            stored,
            customer,
            requested == Some(true),
        )
        .await?;
        if !access.operator {
            return Err(Error::Unavailable);
        }
        let tx = &access.business;
        // Serializes first activation with outgoing admission and all other customer mutations.
        let customer_row=one(tx,"SELECT c.status::text AS status FROM webdock_admin.customers c JOIN webdock_auth.tenant_customer t ON t.customer_id=c.id WHERE c.id=$1 FOR UPDATE OF c",vec![customer.into()]).await?.ok_or(Error::Unavailable)?;
        let current=one(tx,"SELECT desired_enabled,state,revision FROM webdock_mail.service WHERE customer_id=$1 FOR UPDATE",vec![customer.into()]).await?;
        let current_enabled = current
            .as_ref()
            .map(|r| field::<bool>(r, "desired_enabled"))
            .transpose()?
            .unwrap_or(false);
        let enabled = requested.unwrap_or(current_enabled);
        if enabled {
            if field::<String>(&customer_row, "status")? != "active" {
                return Err(Error::Unavailable);
            }
            if !platform_enabled(tx).await? {
                return Err(Error::Unavailable);
            }
        }
        if reconcile {
            check_reconcile(tx, customer, current.as_ref()).await?;
        }
        if !reconcile && current_enabled == enabled {
            let result = self.service_view(tx, customer, true).await?;
            commit(access).await?;
            return Ok(result);
        }
        let current_revision = current
            .as_ref()
            .map(|r| field::<i64>(r, "revision"))
            .transpose()?
            .unwrap_or(0);
        if current_revision != expected {
            return Err(Error::Conflict);
        }
        if current.is_none() && enabled {
            check_outgoing_drained(tx, customer).await?;
        }
        let next = current_revision.checked_add(1).ok_or(Error::Conflict)?;
        let row=one(tx,"INSERT INTO webdock_mail.service(customer_id,instance_key,host_id,desired_enabled,state,revision) VALUES($1,$2,$3,$4,'pending',$5) ON CONFLICT(customer_id) DO UPDATE SET desired_enabled=$4,state=CASE WHEN service.state='needs_review' AND NOT $6 THEN 'needs_review' ELSE 'pending' END,revision=$5,updated_at=now() RETURNING host_id",vec![customer.into(),format!("mail-{customer}").into(),self.config.managed_cluster.clone().into(),enabled.into(),next.into(),reconcile.into()]).await?.ok_or(Error::Unavailable)?;
        if enabled {
            capacity::reserve(
                tx,
                customer,
                &field::<String>(&row, "host_id")?,
                self.local_docker,
            )
            .await?;
        }
        run(tx,"UPDATE webdock_mail.operation SET state='superseded',finished_at=coalesce(finished_at,now()) WHERE customer_id=$1 AND (state='pending' OR ($2 AND state='needs_review'))",vec![customer.into(),reconcile.into()]).await?;
        run(tx,"INSERT INTO webdock_mail.operation(id,customer_id,revision,desired_enabled,requested_by) VALUES($1,$2,$3,$4,$5)",vec![operation_id.clone().into(),customer.into(),next.into(),enabled.into(),access.principal.user_id.clone().into()]).await?;
        run(tx,"INSERT INTO webdock_mail.native_operation(operation_id,customer_id,cluster_id,subject,session_id,native_authority) VALUES($1,$2,$3,$4,$5,$6)",vec![operation_id.into(),customer.into(),field::<String>(&row,"host_id")?.into(),access.principal.user_id.clone().into(),access.principal.session_id.clone().into(),serde_json::to_value(stored).map_err(|_|Error::Invalid)?.into()]).await?;
        let action = match (reconcile, enabled) {
            (true, _) => "incoming.reconcile",
            (false, true) => "incoming.activate",
            (false, false) => "incoming.suspend",
        };
        run(tx,"INSERT INTO webdock_mail.access_event(id,customer_id,subject,action,target_id) VALUES($1,$2,$3,$4,$2)",vec![event_id.into(),customer.into(),access.principal.user_id.clone().into(),action.into()]).await?;
        let result = self.service_view(tx, customer, true).await?;
        commit(access).await?;
        Ok(result)
    }
    async fn service_view(
        &self,
        tx: &DatabaseTransaction,
        customer: &str,
        operator: bool,
    ) -> Result<ServiceDirectory> {
        let row = one(
            tx,
            "SELECT desired_enabled,state,revision FROM webdock_mail.service WHERE customer_id=$1",
            vec![customer.into()],
        )
        .await?;
        let service = match row {
            Some(row) => Service {
                customer_id: customer.into(),
                enabled: field(&row, "desired_enabled")?,
                state: state(&row, "state")?,
                revision: field::<i64>(&row, "revision")?.to_string(),
            },
            None => Service {
                customer_id: customer.into(),
                enabled: false,
                state: ServiceState::Disabled,
                revision: "0".into(),
            },
        };
        let platform = platform_enabled(tx).await?;
        let active = one(
            tx,
            "SELECT 1 FROM webdock_admin.customers WHERE id=$1 AND status='active'",
            vec![customer.into()],
        )
        .await?
        .is_some();
        let module_enabled = one(
            tx,
            "SELECT 1 FROM webdock_auth.plugin_installation WHERE id='mail' AND state='enabled'",
            vec![],
        )
        .await?
        .is_some();
        let verified = one(
            tx,
            "SELECT 1 FROM webdock_mail.instance WHERE customer_id=$1 AND verified_at IS NOT NULL",
            vec![customer.into()],
        )
        .await?
        .is_some();
        let pending_review = one(tx,"SELECT 1 FROM webdock_mail.operation WHERE customer_id=$1 AND state='needs_review' LIMIT 1",vec![customer.into()]).await?.is_some();
        Ok(ServiceDirectory {
            mailboxes_available: self.config.mailboxes_enabled,
            sending_available: self.config.sending_enabled,
            can_reconcile: operator
                && (service.state == ServiceState::NeedsReview || pending_review)
                && verified
                && (!service.enabled || (active && platform)),
            can_activate: operator && module_enabled && active && platform,
            can_suspend: operator && service.enabled,
            pending_review,
            service,
        })
    }
}
async fn platform_enabled(tx: &DatabaseTransaction) -> Result<bool> {
    Ok(one(tx,"SELECT 1 FROM webdock_auth.platform_settings WHERE id=true AND settings->>'mailEnabled'='true' FOR SHARE",vec![]).await?.is_some())
}
async fn check_reconcile(
    tx: &DatabaseTransaction,
    customer: &str,
    current: Option<&QueryResult>,
) -> Result<()> {
    let current = current.ok_or(Error::Conflict)?;
    let blocker=one(tx,"SELECT 1 FROM webdock_mail.operation WHERE customer_id=$1 AND state='needs_review' LIMIT 1",vec![customer.into()]).await?.is_some();
    if state::<ServiceState>(current, "state")? != ServiceState::NeedsReview && !blocker {
        return Err(Error::Conflict);
    }
    if one(tx,"SELECT 1 FROM webdock_mail.instance WHERE customer_id=$1 AND verified_at IS NOT NULL FOR SHARE",vec![customer.into()]).await?.is_none() {return Err(Error::Conflict);}
    if one(
        tx,
        "SELECT 1 FROM webdock_mail.operation WHERE customer_id=$1 AND state='running'",
        vec![customer.into()],
    )
    .await?
    .is_some()
    {
        return Err(Error::Conflict);
    }
    Ok(())
}
async fn check_outgoing_drained(tx: &DatabaseTransaction, customer: &str) -> Result<()> {
    let row=one(tx,"SELECT EXISTS(SELECT 1 FROM webdock_mail.budget_reservation WHERE customer_id=$1 AND state='reserved') OR EXISTS(SELECT 1 FROM webdock_auth.mail_message WHERE customer_id=$1 AND NOT simulated AND dispatch_status IN ('scheduled','dispatching','outcome_unknown')) OR EXISTS(SELECT 1 FROM webdock_mail.send_job WHERE customer_id=$1 AND state IN ('staging','queued','leased','preparation_review','handoff','unknown')) AS pending",vec![customer.into()]).await?.ok_or(Error::Unavailable)?;
    if field::<bool>(&row, "pending")? {
        return Err(Error::Conflict);
    }
    Ok(())
}
