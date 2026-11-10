//! Exact public app names; PostgreSQL owns allocation, retained uniqueness and tenant budgets.
use super::*;
use spitzli_core::id::RecordId;
use spitzli_supervisor::PublicRoute;

fn allocation_error(cause: sea_orm::DbErr) -> crate::HostingError {
    use sea_orm::{DbErr, RuntimeErr};
    let code = match &cause {
        DbErr::Query(RuntimeErr::SqlxError(error)) | DbErr::Exec(RuntimeErr::SqlxError(error)) => {
            error.as_database_error().and_then(|e| e.code())
        }
        _ => None,
    };
    match code.as_deref() {
        Some("42501") => error(403, "Public address access is unavailable."),
        Some("23505") => error(
            409,
            "This public address is unavailable or already assigned.",
        ),
        Some("22023") => error(400, "Choose an available lowercase public address name."),
        Some("54000") => error(
            429,
            "Public address allocation is temporarily limited. Try again later.",
        ),
        _ => db_error(cause),
    }
}

impl HostingService {
    pub fn with_public_site_origins(mut self, origins: &[String]) -> Result<Self> {
        self.public_site_origins = origins
            .iter()
            .map(|raw| {
                url::Url::parse(raw)
                    .ok()
                    .and_then(|u| u.host_str().map(str::to_owned))
                    .ok_or_else(|| error(503, "Public site origin configuration is invalid."))
            })
            .collect::<Result<_>>()?;
        Ok(self)
    }
    pub(super) async fn public_site_policy(
        tx: &DatabaseTransaction,
        project: &str,
    ) -> Result<Value> {
        let row = one(
            tx,
            "SELECT webdock_rls.public_site_policy($1) AS policy",
            vec![project.into()],
        )
        .await?
        .ok_or_else(|| error(503, "Public site policy is unavailable."))?;
        // No eligible domain rule is a valid project state, represented by SQL NULL.
        row.try_get::<Option<Value>>("", "policy")
            .map(|policy| policy.unwrap_or(Value::Null))
            .map_err(db_error)
    }
    pub(super) async fn public_route(
        tx: &DatabaseTransaction,
        app: &str,
    ) -> Result<Option<PublicRoute>> {
        let Some(row)=rows(tx,"SELECT jsonb_build_object('allocationId',id,'hostname',hostname,'revision',revision) AS value FROM webdock_auth.platform_hostname WHERE app_id=$1 AND state<>'retired'",vec![app.into()]).await?.pop() else{return Ok(None)};
        let route: PublicRoute = serde_json::from_value(row)
            .map_err(|_| error(503, "Stored public address is invalid."))?;
        route
            .validate()
            .map_err(|_| error(503, "Stored public address is invalid."))?;
        Ok(Some(route))
    }
    pub(super) async fn requested_public_route(
        &self,
        tx: &DatabaseTransaction,
        cmd: &Value,
        app: &str,
        cluster: &str,
    ) -> Result<Option<PublicRoute>> {
        let current = Self::public_route(tx, app).await?;
        if cmd.get("publicAddress").is_none() {
            return Ok(current);
        }
        let snapshot = Self::cluster_view(tx, cluster).await?;
        if snapshot["runtimeKind"] != "kubernetes"
            || snapshot["workloadReady"] != true
            || snapshot["observation"]["capabilities"]["publicRoutingVersion"] != 1
        {
            return Err(error(
                409,
                "Update and qualify the cluster's public routing agent first.",
            ));
        }
        let label = cmd["publicAddress"]["label"].as_str().map(str::to_owned);
        let result = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_rls.allocate_platform_hostname($1,$2,$3) AS allocation",
                [self.id().await?.into(), app.into(), label.into()],
            ))
            .await
            .map_err(allocation_error)?
            .ok_or_else(|| error(503, "Public address allocation is unavailable."))?;
        let data: Value = result.try_get("", "allocation").map_err(db_error)?;
        let route = PublicRoute {
            allocation_id: RecordId::parse(field(&data, "id")?)
                .map_err(|_| error(503, "Invalid public address identity."))?,
            hostname: field(&data, "hostname")?.into(),
            revision: numeric(&data, "revision")? as u64,
        };
        route
            .validate()
            .map_err(|_| error(503, "Invalid public address."))?;
        Ok(Some(route))
    }
    pub(super) async fn public_addresses(
        &self,
        tx: &DatabaseTransaction,
        actor: &HostingActor,
        cmd: &Value,
    ) -> Result<(Value, Target)> {
        let customer = field(cmd, "customerID")?;
        Self::authorize(tx, actor, None, Some(customer), false, false, false).await?;
        let values=rows(tx,"SELECT jsonb_build_object('id',h.id,'hostname',h.hostname,'url','https://'||h.hostname,'state',h.state,'revision',h.revision,'projectID',h.project_id,'appID',h.app_id,'projectName',p.name,'appName',a.name) AS value FROM webdock_auth.platform_hostname h JOIN webdock_admin.projects p ON p.id=h.project_id JOIN webdock_auth.hosting_app a ON a.id=h.app_id WHERE h.customer_id=$1 AND h.state<>'retired' ORDER BY h.hostname",vec![customer.into()]).await?;
        Ok((
            management::search_page(values, cmd, &["hostname", "projectName", "appName"])?,
            Target {
                project: None,
                customer: Some(customer.into()),
                write: false,
                operator: false,
                live: false,
            },
        ))
    }
    pub(super) async fn configure_public_sites(
        &self,
        tx: &DatabaseTransaction,
        actor: &HostingActor,
        cmd: &Value,
    ) -> Result<(Value, Target)> {
        Self::authorize(tx, actor, None, None, true, true, false).await?;
        let cluster = field(cmd, "clusterID")?;
        let domain = field(cmd, "domain")?;
        if self.public_site_origins.is_empty() {
            return Err(error(
                503,
                "Public site control origins are not configured.",
            ));
        }
        if ["spitzli.cloud", "webdock.dev", "spitzli.dev"]
            .iter()
            .any(|root| domain == *root || domain.ends_with(&format!(".{root}")))
            || domain.parse::<std::net::IpAddr>().is_ok()
            || self
                .public_site_origins
                .iter()
                .any(|host| host == domain || host.ends_with(&format!(".{domain}")))
        {
            return Err(error(
                400,
                "Customer hosting must use a separate domain from platform services.",
            ));
        }
        if amount(&cmd["globalDailyLimit"])? < amount(&cmd["tenantDailyLimit"])? {
            return Err(error(
                400,
                "Global public address budget must cover the tenant budget.",
            ));
        }
        if one(tx,"SELECT 1 FROM webdock_auth.customer_domain WHERE verified_at IS NOT NULL AND (domain=$1 OR right(domain,char_length($1)+1)='.'||$1 OR right($1,char_length(domain)+1)='.'||domain) LIMIT 1",vec![domain.into()]).await?.is_some() {
            return Err(error(409,"A verified custom domain overlaps this hosting domain. Review its migration first."));
        }
        let state = Self::cluster_view(tx, cluster).await?;
        if state["ownership"] != "platform"
            || state["runtimeKind"] != "kubernetes"
            || state["verified"] != true
        {
            return Err(error(409, "Use a verified platform Kubernetes cluster."));
        }
        if cmd["enabled"] == true
            && (state["workloadReady"] != true
                || state["observation"]["capabilities"]["publicRoutingVersion"] != 1)
        {
            return Err(error(
                409,
                "Public routing must be qualified before enabling admission.",
            ));
        }
        Self::lock(tx, "public-site-policy".into()).await?;
        let old=rows(tx,"SELECT to_jsonb(p) AS value FROM webdock_auth.public_hosting_domain p WHERE domain=$1 OR cluster_id=$2 FOR UPDATE",vec![domain.into(),cluster.into()]).await?;
        if old.len() > 1
            || old
                .first()
                .is_some_and(|p| p["domain"] != domain || p["cluster_id"] != cluster)
        {
            return Err(error(
                409,
                "Public site domain/cluster identity requires a separate migration.",
            ));
        }
        let revision = old
            .first()
            .map(|p| numeric(p, "revision"))
            .transpose()?
            .unwrap_or(0);
        if amount(&cmd["policyRevision"])? != revision as u64 {
            return Err(error(
                409,
                "Public site policy changed. Refresh before saving.",
            ));
        }
        let trusted = cmd["trustedCustomerIDs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        execute(tx,"INSERT INTO webdock_auth.public_hosting_domain(domain,cluster_id,enabled,tenant_daily_limit,global_daily_limit,trusted_customers,configured_by,revision) VALUES($1,$2,$3,$4,$5,$6,$7,1) ON CONFLICT(domain) DO UPDATE SET cluster_id=EXCLUDED.cluster_id,enabled=EXCLUDED.enabled,tenant_daily_limit=EXCLUDED.tenant_daily_limit,global_daily_limit=EXCLUDED.global_daily_limit,trusted_customers=EXCLUDED.trusted_customers,configured_by=EXCLUDED.configured_by,revision=public_hosting_domain.revision+1,updated_at=now()",vec![domain.into(),cluster.into(),cmd["enabled"].as_bool().unwrap().into(),(amount(&cmd["tenantDailyLimit"])? as i32).into(),(amount(&cmd["globalDailyLimit"])? as i32).into(),trusted.into(),actor.user_id.clone().into()]).await?;
        self.audit(
            tx,
            &actor.user_id,
            "clusters.publicSites.configure",
            cluster,
            "succeeded",
        )
        .await?;
        Ok((
            json!({"domain":domain,"clusterID":cluster,"enabled":cmd["enabled"],"policyRevision":revision+1}),
            Target::operator(true),
        ))
    }
    pub(super) async fn verify_public_route(tx: &DatabaseTransaction, op: &Value) -> Result<()> {
        let Some(raw) = op["desired"].get("publicRoute") else {
            return Ok(());
        };
        let expected: PublicRoute = serde_json::from_value(raw.clone())
            .map_err(|_| error(503, "Invalid public route authority."))?;
        let actual = Self::public_route(tx, field(op, "app_id")?).await?;
        if actual.as_ref() != Some(&expected) {
            // Acknowledgment replay after retirement must retain its exact old signed authority.
            let retired = if op["status"] == "succeeded"
                && matches!(
                    op["desired"]["action"].as_str(),
                    Some("delete" | "purge-storage")
                ) {
                one(tx,"SELECT 1 FROM webdock_auth.platform_hostname WHERE id=$1 AND app_id=$2 AND hostname=$3 AND state='retired' AND revision=$4",vec![expected.allocation_id.to_string().into(),field(op,"app_id")?.into(),expected.hostname.clone().into(),((expected.revision+1) as i64).into()]).await?.is_some()
            } else {
                false
            };
            if !retired {
                return Err(error(403, "Public address authority changed."));
            }
        }
        Ok(())
    }
}
