use super::*;
pub(super) fn canonical(value: &Value) -> Result<String> {
    let collator = icu_collator::Collator::try_new(
        icu_locale::locale!("en-US").into(),
        icu_collator::options::CollatorOptions::default(),
    )
    .map_err(|_| error(503, "Request hashing unavailable."))?;
    fn encode(v: &Value, c: &icu_collator::CollatorBorrowed<'_>, depth: usize) -> Result<String> {
        if depth > 64 {
            return Err(error(400, "Input nesting exceeds limit."));
        }
        Ok(match v {
            Value::Object(map) => {
                let mut fields = map.iter().collect::<Vec<_>>();
                fields.sort_by(|a, b| c.compare(a.0, b.0));
                format!(
                    "{{{}}}",
                    fields
                        .into_iter()
                        .map(|(k, v)| Ok(format!(
                            "{}:{}",
                            serde_json::to_string(k).unwrap(),
                            encode(v, c, depth + 1)?
                        )))
                        .collect::<Result<Vec<_>>>()?
                        .join(",")
                )
            }
            Value::Array(values) => format!(
                "[{}]",
                values
                    .iter()
                    .map(|v| encode(v, c, depth + 1))
                    .collect::<Result<Vec<_>>>()?
                    .join(",")
            ),
            Value::Number(v) => ryu_js::Buffer::new()
                .format_finite(
                    v.as_f64()
                        .ok_or_else(|| error(400, "Invalid JSON number."))?,
                )
                .into(),
            v => v.to_string(),
        })
    }
    encode(value, &collator, 0)
}
impl HostingService {
    pub(super) async fn replay(
        &self,
        tx: &DatabaseTransaction,
        actor: &HostingActor,
        key: &str,
        cmd: &Value,
    ) -> Result<(String, Option<Value>)> {
        let raw = canonical(cmd)?;
        let hash = if cmd.get("environment").is_some() {
            let mut mac = Hmac::<Sha256>::new_from_slice(self.environment.hash_secret().as_bytes())
                .map_err(|_| error(503, "Environment unavailable."))?;
            mac.update(raw.as_bytes());
            hex::encode(mac.finalize().into_bytes())
        } else {
            hex::encode(Sha256::digest(raw.as_bytes()))
        };
        Self::lock(tx, format!("hosting-request:{}:{key}", actor.user_id)).await?;
        let existing=rows(tx,"SELECT to_jsonb(o) AS value FROM webdock_auth.hosting_operation o WHERE subject=$1 AND idempotency_key=$2",vec![actor.user_id.clone().into(),key.into()]).await?.pop();
        if existing.as_ref().is_some_and(|v| v["payload_hash"] != hash) {
            return Err(error(
                409,
                "Idempotency key already used with different input.",
            ));
        }
        Ok((hash, existing))
    }
    pub(super) async fn record_result(
        &self,
        tx: &DatabaseTransaction,
        actor: &HostingActor,
        cmd: &Value,
        receipt: (String, &Value),
        access: &Access,
        cluster: Option<&str>,
    ) -> Result<()> {
        let (hash, result) = receipt;
        execute(tx,"INSERT INTO webdock_auth.hosting_operation(id,subject,actor_session,idempotency_key,payload_hash,action,status,result,customer_id,project_id,cluster_id) VALUES($1,$2,$3,$4,$5,$6,'succeeded',$7,$8,$9,$10)",vec![self.id().await?.into(),actor.user_id.clone().into(),actor.session_id.clone().into(),field(cmd,"idempotencyKey")?.into(),hash.into(),field(cmd,"action")?.into(),result.clone().into(),if access.customer.is_empty(){None::<String>}else{Some(access.customer.clone())}.into(),access.project.get("id").and_then(Value::as_str).into(),cluster.into()]).await?;
        Ok(())
    }
    pub(super) async fn management(
        &self,
        tx: &DatabaseTransaction,
        actor: &HostingActor,
        cmd: &Value,
    ) -> Result<(Value, Target)> {
        let action = field(cmd, "action")?;
        match action {
            "addresses.list" => self.public_addresses(tx, actor, cmd).await,
            "clusters.publicSites.configure" => self.configure_public_sites(tx, actor, cmd).await,
            "projects.get" => {
                let id = field(cmd, "projectID")?;
                let access =
                    Self::authorize(tx, actor, Some(id), None, false, false, false).await?;
                let value=rows(tx,"SELECT jsonb_build_object('projectID',p.id,'name',p.name,'customerID',c.id,'customerName',c.name) AS value FROM webdock_admin.projects p JOIN webdock_admin.customers c ON c.id=p.customer_id WHERE p.id=$1",vec![id.into()]).await?.pop().ok_or_else(||error(404,"Project unavailable."))?;
                let mut value = value;
                value["operator"] = json!(access.operator);
                value["configured"] = json!(!access.project["provider"].is_null());
                value["provider"] = access.project["provider"].clone();
                value["publicSitePolicy"] = Self::public_site_policy(tx, id).await?;
                value["automaticScalingAvailable"] = json!(
                    self.automatic_scaling_available(tx, access.project["cluster_id"].as_str())
                        .await?
                );
                Ok((value, Target::project(id, false, false, false)))
            }
            "clusters.get" => {
                Self::authorize(tx, actor, None, None, false, true, false).await?;
                Ok((
                    Self::cluster_view(tx, field(cmd, "clusterID")?).await?,
                    Target::operator(false),
                ))
            }
            "clusters.list" => {
                Self::authorize(tx, actor, None, None, false, true, false).await?;
                let ids=rows(tx,"SELECT jsonb_build_object('id',id,'name',name,'provider',provider,'region',region) AS value FROM webdock_auth.hosting_cluster ORDER BY name,id",vec![]).await?;
                let page = search_page(ids, cmd, &["name", "provider", "region"])?;
                let mut docs = Vec::new();
                for row in page["docs"].as_array().unwrap() {
                    docs.push(Self::cluster_view(tx, field(row, "id")?).await?);
                }
                let mut page = page;
                page["docs"] = Value::Array(docs);
                Ok((page, Target::operator(false)))
            }
            "projects.list" => {
                let customer = field(cmd, "customerID")?;
                let access =
                    Self::authorize(tx, actor, None, Some(customer), false, false, false).await?;
                let values=rows(tx,"SELECT jsonb_build_object('projectID',p.id,'name',p.name,'provider',h.provider,'mode',h.mode,'ownImages',h.own_images,'revision',h.revision,'clusterID',h.cluster_id,'clusterName',c.name,'country',c.country,'region',c.region) AS value FROM webdock_auth.hosting_project h JOIN webdock_admin.projects p ON p.id=h.project_id LEFT JOIN webdock_auth.hosting_cluster c ON c.id=h.cluster_id WHERE h.customer_id=$1 ORDER BY p.name,p.id",vec![customer.into()]).await?;
                let mut page = search_page(values, cmd, &["name", "provider", "clusterName"])?;
                page["operator"] = json!(access.operator);
                Ok((
                    page,
                    Target {
                        project: None,
                        customer: Some(customer.into()),
                        write: false,
                        operator: false,
                        live: false,
                    },
                ))
            }
            "limits.get" | "usage.get" => {
                let customer = field(cmd, "customerID")?;
                Self::authorize(tx, actor, None, Some(customer), false, false, false).await?;
                Ok((
                    if action == "limits.get" {
                        Self::limits(tx, customer, None, None).await?
                    } else {
                        Self::usage(tx, customer).await?
                    },
                    Target {
                        project: None,
                        customer: Some(customer.into()),
                        write: false,
                        operator: false,
                        live: false,
                    },
                ))
            }
            "operations.get" => {
                let op = rows(
                    tx,
                    "SELECT to_jsonb(o) AS value FROM webdock_auth.hosting_operation o WHERE id=$1",
                    vec![field(cmd, "operationID")?.into()],
                )
                .await?
                .pop()
                .ok_or_else(|| error(404, "Operation is unavailable."))?;
                let project = op["project_id"].as_str();
                let customer = op["customer_id"].as_str();
                Self::authorize(
                    tx,
                    actor,
                    project,
                    customer,
                    false,
                    customer.is_none(),
                    false,
                )
                .await?;
                if field(&op, "action")? == "byok.workload" {
                    return Err(error(503, "External workload adapter unavailable."));
                }
                Ok((
                    json!({"id":op["id"],"action":op["action"],"status":op["status"],"customerID":op["customer_id"],"projectID":op["project_id"],"createdAt":timestamp(&op["created_at"])}),
                    Target {
                        project: project.map(str::to_owned),
                        customer: customer.map(str::to_owned),
                        write: false,
                        operator: customer.is_none(),
                        live: false,
                    },
                ))
            }
            "projects.create" | "projects.update" => {
                let id = field(cmd, "projectID")?;
                let access = Self::authorize(tx, actor, Some(id), None, true, true, false).await?;
                Self::lock(tx, format!("plan:{}", access.customer)).await?;
                let cluster = if action == "projects.create" {
                    cmd["clusterID"].as_str()
                } else {
                    access.project["cluster_id"].as_str()
                };
                let provider = if action == "projects.create" {
                    field(cmd, "provider")?
                } else {
                    field(&access.project, "provider")?
                };
                if !["k3s", "docker"].contains(&provider) {
                    return Err(error(503, "External provider adapter unavailable."));
                }
                let cluster = cluster.ok_or_else(|| error(400, "Select a managed cluster."))?;
                Self::lock(tx, format!("hosting-cluster:{cluster}")).await?;
                let state = Self::cluster_view(tx, cluster).await?;
                if state["ownership"] == "tenant"
                    || state["dedicatedCustomerID"]
                        .as_str()
                        .is_some_and(|owner| owner != access.customer)
                {
                    return Err(error(403, "Cluster is assigned to another customer."));
                }
                if (provider == "docker") != (state["runtimeKind"] == "docker") {
                    return Err(error(
                        400,
                        "Cluster runtime kind does not match project backend.",
                    ));
                }
                if cmd["ownImages"] == true
                    && state["dedicatedCustomerID"].is_null()
                    && cmd["confirmSharedImages"] != true
                {
                    return Err(error(
                        409,
                        "Confirm permission to run this project's images on a shared cluster.",
                    ));
                }
                let result = if action == "projects.create" {
                    let (hash, prior) = self
                        .replay(tx, actor, field(cmd, "idempotencyKey")?, cmd)
                        .await?;
                    if let Some(prior) = prior {
                        return Ok((
                            prior["result"].clone(),
                            Target::project(id, true, true, false),
                        ));
                    }
                    if execute(tx,"INSERT INTO webdock_auth.hosting_project(project_id,customer_id,provider,cluster_id,namespace,mode,own_images) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT DO NOTHING",vec![id.into(),access.customer.clone().into(),provider.into(),cluster.into(),format!("wd-{id}").into(),field(cmd,"mode")?.into(),cmd["ownImages"].as_bool().unwrap_or(false).into()]).await?!=1{return Err(error(409,"Hosting is already assigned to this project."));}
                    let result = json!({"projectID":id,"revision":1});
                    self.record_result(tx, actor, cmd, (hash, &result), &access, Some(cluster))
                        .await?;
                    result
                } else {
                    let row=one(tx,"UPDATE webdock_auth.hosting_project SET mode=$2,own_images=$3,revision=revision+1 WHERE project_id=$1 AND revision=$4 RETURNING revision",vec![id.into(),field(cmd,"mode")?.into(),cmd["ownImages"].as_bool().unwrap().into(),numeric(cmd,"revision")?.into()]).await?.ok_or_else(||error(409,"Hosting project changed. Refresh and try again."))?;
                    json!({"projectID":id,"revision":row.try_get::<i32>("","revision").map_err(db_error)?})
                };
                self.audit(tx, &actor.user_id, action, id, "succeeded")
                    .await?;
                Ok((result, Target::project(id, true, true, false)))
            }
            "clusters.register" => {
                let access = Self::authorize(
                    tx,
                    actor,
                    None,
                    cmd["dedicatedCustomerID"].as_str(),
                    true,
                    true,
                    false,
                )
                .await?;
                let (hash, prior) = self
                    .replay(tx, actor, field(cmd, "idempotencyKey")?, cmd)
                    .await?;
                if let Some(prior) = prior {
                    return Ok((prior["result"].clone(), Target::operator(true)));
                }
                let id = self.id().await?;
                execute(tx,"INSERT INTO webdock_auth.hosting_cluster(id,name,provider,country,region,location_evidence,dedicated_customer_id,runtime_kind) VALUES($1,$2,$3,$4,$5,$6,$7,$8)",vec![id.clone().into(),field(cmd,"name")?.into(),field(cmd,"provider")?.into(),cmd["country"].as_str().into(),field(cmd,"region")?.into(),field(cmd,"locationEvidence")?.into(),cmd["dedicatedCustomerID"].as_str().into(),cmd["runtimeKind"].as_str().unwrap_or("kubernetes").into()]).await?;
                let result = Self::cluster_view(tx, &id).await?;
                self.record_result(tx, actor, cmd, (hash, &result), &access, Some(&id))
                    .await?;
                self.audit(tx, &actor.user_id, action, &id, "succeeded")
                    .await?;
                Ok((result, Target::operator(true)))
            }
            "clusters.revoke" => {
                Self::authorize(tx, actor, None, None, true, true, false).await?;
                let id = field(cmd, "clusterID")?;
                Self::lock(tx, format!("hosting-cluster:{id}")).await?;
                if execute(tx,"UPDATE webdock_auth.hosting_cluster SET revision=revision+1,verified=false WHERE id=$1 AND revision=$2 AND ownership='platform'",vec![id.into(),numeric(cmd,"revision")?.into()]).await?!=1{return Err(error(409,"Cluster changed. Refresh and try again."));}
                execute(
                    tx,
                    "UPDATE webdock_auth.hosting_agent SET revoked=true WHERE cluster_id=$1",
                    vec![id.into()],
                )
                .await?;
                execute(tx,"UPDATE webdock_auth.hosting_enrollment SET consumed_at=now() WHERE cluster_id=$1 AND consumed_at IS NULL",vec![id.into()]).await?;
                self.audit(tx, &actor.user_id, action, id, "succeeded")
                    .await?;
                Ok((Self::cluster_view(tx, id).await?, Target::operator(true)))
            }
            "operations.overview" => {
                Self::authorize(tx, actor, None, None, false, true, false).await?;
                let search = format!(
                    "%{}%",
                    field(cmd, "search")?
                        .replace('\\', "\\\\")
                        .replace('%', "\\%")
                        .replace('_', "\\_")
                );
                let mut values=rows(tx,"SELECT jsonb_build_object('id',a.id,'name',a.name,'status',a.status,'revision',a.revision,'observedRevision',a.observed_revision,'projectID',p.id,'projectName',p.name,'customerID',c.id,'customerName',c.name) AS value FROM webdock_auth.hosting_app a JOIN webdock_admin.projects p ON p.id=a.project_id JOIN webdock_admin.customers c ON c.id=p.customer_id WHERE a.status<>'deleted' AND(a.name ILIKE $1 OR p.name ILIKE $1 OR c.name ILIKE $1) ORDER BY a.name,a.id",vec![search.into()]).await?;
                if cmd["sort"] == "-name" {
                    values.sort_by(|a, b| {
                        b["name"]
                            .as_str()
                            .cmp(&a["name"].as_str())
                            .then(a["id"].as_str().cmp(&b["id"].as_str()))
                    });
                }
                let total = values.len() as u64;
                let page = amount(&cmd["page"])?;
                let limit = amount(&cmd["limit"])?;
                let docs = values
                    .into_iter()
                    .skip(((page - 1) * limit) as usize)
                    .take(limit as usize)
                    .collect::<Vec<_>>();
                let operations=rows(tx,"SELECT jsonb_build_object('id',o.id,'action',o.action,'status',o.status,'createdAt',o.created_at,'projectID',p.id,'projectName',p.name,'customerName',c.name) AS value FROM webdock_auth.hosting_operation o LEFT JOIN webdock_admin.projects p ON p.id=o.project_id LEFT JOIN webdock_admin.customers c ON c.id=o.customer_id ORDER BY CASE WHEN o.status IN('queued','running','needs-reconciliation') THEN 0 ELSE 1 END,o.created_at DESC,o.id DESC LIMIT 30",vec![]).await?.into_iter().map(|mut op|{op["createdAt"]=timestamp(&op["createdAt"]);op}).collect::<Vec<_>>();
                Ok((
                    json!({"docs":docs,"operations":operations,"page":page,"limit":limit,"totalDocs":total,"totalPages":total.div_ceil(limit),"readAt":chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis,true)}),
                    Target::operator(false),
                ))
            }
            "limits.set" => {
                let customer = field(cmd, "customerID")?;
                Self::authorize(
                    tx,
                    actor,
                    cmd["projectID"].as_str(),
                    Some(customer),
                    true,
                    true,
                    false,
                )
                .await?;
                Self::lock(tx, format!("plan:{customer}")).await?;
                if cmd.get("projectID").is_some() && cmd.get("provider").is_some() {
                    return Err(error(400, "Choose either a project or a provider scope."));
                }
                let current = Self::limits(tx, customer, None, None).await?;
                if current["subscriptionRevision"] != cmd["subscriptionRevision"] {
                    return Err(error(409, "Subscription changed. Refresh and try again."));
                }
                let scope = if let Some(project) = cmd["projectID"].as_str() {
                    format!("project:{project}")
                } else if let Some(provider) = cmd["provider"].as_str() {
                    format!("provider:{provider}")
                } else {
                    "customer".into()
                };
                let old = current["limits"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|r| r["scope"] == scope);
                if old.map(|r| r["revision"].clone()).unwrap_or(json!(0)) != cmd["revision"] {
                    return Err(error(409, "Hosting limits changed. Refresh and try again."));
                }
                execute(tx,"INSERT INTO webdock_auth.hosting_limit(customer_id,scope,values,revision) VALUES($1,$2,$3,1) ON CONFLICT(customer_id,scope) DO UPDATE SET values=$3,revision=hosting_limit.revision+1",vec![customer.into(),scope.into(),cmd["values"].clone().into()]).await?;
                self.audit(tx, &actor.user_id, action, customer, "succeeded")
                    .await?;
                Ok((
                    Self::limits(tx, customer, None, None).await?,
                    Target {
                        project: cmd["projectID"].as_str().map(str::to_owned),
                        customer: Some(customer.into()),
                        write: true,
                        operator: true,
                        live: false,
                    },
                ))
            }
            "clusters.activate" => {
                Self::authorize(tx, actor, None, None, true, true, false).await?;
                let id = field(cmd, "clusterID")?;
                Self::lock(tx, format!("hosting-cluster:{id}")).await?;
                let current = Self::cluster_view(tx, id).await?;
                if current["ownership"] == "tenant"
                    || current["revision"] != cmd["revision"]
                    || current["state"] != "connected"
                    || current["observation"].is_null()
                {
                    return Err(error(
                        409,
                        "A fresh connected cluster and current revision are required.",
                    ));
                }
                let capacity = contracts::allowances(cmd.get("capacity"))?;
                if DIMENSIONS.iter().any(|key| capacity[key].is_null()) {
                    return Err(error(400, "Managed capacity must be finite."));
                }
                let observation = &current["observation"];
                let nodes = observation["nodes"]
                    .as_array()
                    .ok_or_else(|| error(409, "Node capacity unavailable."))?;
                let cpu = nodes
                    .iter()
                    .map(|n| amount(&n["cpuMillicores"]))
                    .collect::<Result<Vec<_>>>()?
                    .iter()
                    .try_fold(0u64, |a, n| a.checked_add(*n))
                    .ok_or_else(|| error(409, "Node capacity exceeds supported range."))?;
                let memory = nodes
                    .iter()
                    .map(|n| amount(&n["memoryBytes"]))
                    .collect::<Result<Vec<_>>>()?
                    .iter()
                    .try_fold(0u64, |a, n| a.checked_add(*n))
                    .ok_or_else(|| error(409, "Node capacity exceeds supported range."))?;
                let app_storage = if observation["capabilities"]["storage"] == true
                    && observation["capabilities"]["storageVersion"] == 1
                {
                    observation["capacity"]["volumeBytes"].as_u64().unwrap_or(0)
                } else {
                    0
                };
                let db_storage = if observation["capabilities"]["databaseVersion"] == 1 {
                    observation["capabilities"]["databasePools"]
                        .as_array()
                        .map(|pools| {
                            pools
                                .iter()
                                .filter(|p| {
                                    p["ready"] == true && nodes.iter().any(|n| n["id"] == p["node"])
                                })
                                .map(|p| amount(&p["capacityBytes"]))
                                .collect::<Result<Vec<_>>>()
                        })
                        .transpose()?
                        .unwrap_or_default()
                        .into_iter()
                        .try_fold(0u64, |a, n| a.checked_add(n))
                        .ok_or_else(|| error(409, "Storage capacity exceeds supported range."))?
                } else {
                    0
                };
                if amount(&capacity["volumeBytes"])?
                    > app_storage
                        .checked_add(db_storage)
                        .ok_or_else(|| error(409, "Storage capacity exceeds supported range."))?
                {
                    return Err(error(
                        400,
                        "Managed storage exceeds verified persistent capacity.",
                    ));
                }
                if ["apps", "cpuMillicores", "memoryBytes", "ephemeralBytes"]
                    .iter()
                    .any(|k| capacity[k] == 0)
                    || amount(&capacity["cpuMillicores"])? > cpu * 8 / 10
                    || amount(&capacity["memoryBytes"])? > memory * 8 / 10
                {
                    return Err(error(
                        409,
                        "Reserve at least twenty percent of node CPU and memory for the platform.",
                    ));
                }
                execute(tx,"UPDATE webdock_auth.hosting_cluster SET verified=true,capacity=$2,verification_evidence=$3,revision=revision+1 WHERE id=$1",vec![id.into(),capacity.into(),field(cmd,"verificationEvidence")?.into()]).await?;
                self.audit(tx, &actor.user_id, action, id, "succeeded")
                    .await?;
                Ok((Self::cluster_view(tx, id).await?, Target::operator(true)))
            }
            _ => Err(error(
                503,
                "This hosting capability requires its configured adapter.",
            )),
        }
    }
}
pub(super) fn search_page(mut values: Vec<Value>, cmd: &Value, fields: &[&str]) -> Result<Value> {
    use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};
    let normalize = |text: &str| {
        text.nfkd()
            .filter(|c| !is_combining_mark(*c))
            .collect::<String>()
            .replace('ß', "ss")
            .to_lowercase()
    };
    let search = normalize(cmd["search"].as_str().unwrap_or("").trim());
    if !search.is_empty() {
        values.retain(|row| {
            let text = normalize(
                &fields
                    .iter()
                    .filter_map(|field| row[*field].as_str())
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            text.contains(&search)
                || search
                    .split(|c: char| !c.is_alphanumeric())
                    .filter(|v| !v.is_empty())
                    .all(|term| {
                        text.split(|c: char| !c.is_alphanumeric()).any(|word| {
                            word.starts_with(term)
                                || term.len() >= 4
                                    && !term.chars().all(|c| c.is_ascii_digit())
                                    && one_edit(term, word)
                        })
                    })
        });
    }
    if cmd["sort"] == "-name" {
        values.sort_by(|a, b| {
            b["name"]
                .as_str()
                .cmp(&a["name"].as_str())
                .then(a["id"].as_str().cmp(&b["id"].as_str()))
        });
    }
    let page = amount(&cmd["page"])?;
    let limit = amount(&cmd["limit"])?;
    let total = values.len() as u64;
    let docs = values
        .into_iter()
        .skip(((page - 1) * limit) as usize)
        .take(limit as usize)
        .collect::<Vec<_>>();
    Ok(
        json!({"docs":docs,"page":page,"limit":limit,"totalDocs":total,"totalPages":total.div_ceil(limit)}),
    )
}
fn one_edit(a: &str, b: &str) -> bool {
    let (a, b) = (a.chars().collect::<Vec<_>>(), b.chars().collect::<Vec<_>>());
    if a.len().abs_diff(b.len()) > 1 {
        return false;
    }
    let (mut i, mut j, mut edits) = (0, 0, 0);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            i += 1;
            j += 1;
        } else {
            edits += 1;
            if edits > 1 {
                return false;
            }
            if a.len() >= b.len() {
                i += 1;
            }
            if b.len() >= a.len() {
                j += 1;
            }
        }
    }
    edits + (a.len() - i) + (b.len() - j) <= 1
}
