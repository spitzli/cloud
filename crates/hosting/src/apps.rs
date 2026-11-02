use super::management::search_page;
use super::*;
use crate::entities::app;
use sea_orm::{ActiveModelTrait, EntityTrait, IntoActiveModel, QuerySelect, Set};
impl HostingService {
    pub(super) async fn app(tx: &DatabaseTransaction, id: &str) -> Result<Value> {
        rows(tx,"SELECT to_jsonb(a)||jsonb_build_object('public_address',(SELECT jsonb_build_object('id',d.id,'hostname',d.hostname,'url','https://'||d.hostname,'state',d.state,'revision',d.revision) FROM webdock_auth.platform_hostname d WHERE d.app_id=a.id AND d.state<>'retired'),'customer_id',h.customer_id,'cluster_id',h.cluster_id,'namespace',h.namespace,'own_images',h.own_images) AS value FROM webdock_auth.hosting_app a JOIN webdock_auth.hosting_project h ON h.project_id=a.project_id WHERE a.id=$1",vec![id.into()]).await?.pop().ok_or_else(||error(404,"Application is unavailable."))
    }
    fn safe_app(&self, row: &Value) -> Result<Value> {
        Ok(
            json!({"id":row["id"],"lifecycle":row["lifecycle"],"name":row["name"],"projectID":row["project_id"],"revision":row["revision"],"observedRevision":row["observed_revision"],"status":row["status"],"spec":row["spec"],"environmentNames":self.environment.open(field(row,"id")?,row["environment_encrypted"].as_str())?.keys().collect::<Vec<_>>(),"operationID":row["operation_id"],"lastError":row["last_error"],"logs":row["logs"],"publicAddress":row["public_address"]}),
        )
    }
    fn deletion(row: &Value, purge: bool) -> Result<Value> {
        let plan = json!({"appID":row["id"],"name":row["name"],"projectID":row["project_id"],"clusterID":row["cluster_id"],"namespace":row["namespace"],"revision":row["revision"],"observedUID":row["observed_uid"],"resources":if row["lifecycle"]=="draft"{json!([])}else if purge{json!(["PersistentVolume","PersistentVolumeClaim","Filesystem"])}else{json!(["Deployment","Service","Secret"])},"persistentData":row["lifecycle"]!="draft"&&row["spec"]["volumeBytes"].as_u64().unwrap_or(0)>0,"volumeBytes":row["spec"].get("volumeBytes").cloned().unwrap_or(json!(0)),"storagePurged":purge,"namespaceRetained":true});
        let hash = hex::encode(Sha256::digest(serde_json::to_vec(&plan).unwrap()));
        Ok(json!({"plan":plan,"planHash":hash}))
    }
    pub(super) async fn apps(
        &self,
        tx: &DatabaseTransaction,
        actor: &HostingActor,
        cmd: &Value,
    ) -> Result<(Value, Target)> {
        let action = field(cmd, "action")?;
        if action == "apps.list" {
            let project = field(cmd, "projectID")?;
            let access =
                Self::authorize(tx, actor, Some(project), None, false, false, false).await?;
            let app_rows=rows(tx,"SELECT to_jsonb(a)||jsonb_build_object('public_address',(SELECT jsonb_build_object('id',d.id,'hostname',d.hostname,'url','https://'||d.hostname,'state',d.state,'revision',d.revision) FROM webdock_auth.platform_hostname d WHERE d.app_id=a.id AND d.state<>'retired')) AS value FROM webdock_auth.hosting_app a WHERE project_id=$1 AND (status<>'deleted' OR COALESCE((spec->>'volumeBytes')::bigint,0)>0) ORDER BY name,id",vec![project.into()]).await?;
            let mut page = search_page(app_rows, cmd, &["name"])?;
            page["docs"] = Value::Array(
                page["docs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|row| self.safe_app(row))
                    .collect::<Result<Vec<_>>>()?,
            );
            page["subscriptionRevision"] =
                Self::limits(tx, &access.customer, None, None).await?["subscriptionRevision"]
                    .clone();
            page["operator"] = json!(access.operator);
            page["canManage"] = json!(
                access.can_write && (access.operator || access.project["mode"] == "selfservice")
            );
            page["ownImages"] = json!(access.operator || access.project["own_images"] == true);
            return Ok((page, Target::project(project, false, false, false)));
        }
        let mut row = match cmd["appID"].as_str() {
            Some(id) => Some(Self::app(tx, id).await?),
            None => None,
        };
        let project = row
            .as_ref()
            .and_then(|row| row["project_id"].as_str())
            .or_else(|| cmd["projectID"].as_str())
            .ok_or_else(|| error(404, "Application project is unavailable."))?
            .to_owned();
        let read = [
            "apps.get",
            "apps.deletion",
            "apps.storageDeletion",
            "apps.logs",
            "apps.inspect",
            "apps.metrics",
        ]
        .contains(&action);
        let operator = [
            "apps.delete",
            "apps.deletion",
            "apps.storageDeletion",
            "apps.purgeStorage",
            "apps.reconcile",
        ]
        .contains(&action);
        let target = Target::project(&project, !read, operator, is_read_action(action));
        let mut access = Self::authorize(
            tx,
            actor,
            Some(&project),
            None,
            !read,
            operator,
            is_read_action(action),
        )
        .await?;
        if action == "apps.get" {
            let row = row.as_ref().unwrap();
            let mut value = self.safe_app(row)?;
            value["canRequestLogs"] = json!(
                access.live
                    && row["lifecycle"] != "draft"
                    && row["observed_uid"].as_str().is_some()
            );
            value["subscriptionRevision"] =
                Self::limits(tx, &access.customer, None, None).await?["subscriptionRevision"]
                    .clone();
            value["operator"] = json!(access.operator);
            value["canManage"] = json!(
                access.can_write && (access.operator || access.project["mode"] == "selfservice")
            );
            return Ok((value, target));
        }
        if row.as_ref().is_some_and(|r| r["status"] == "quarantined")
            && !read
            && !["apps.delete", "apps.purgeStorage"].contains(&action)
        {
            return Err(error(
                409,
                "Restored application is quarantined. A separate operator recovery cutover is required.",
            ));
        }
        if ["apps.deletion", "apps.storageDeletion"].contains(&action) {
            let row = row.as_ref().unwrap();
            if action == "apps.storageDeletion" && row["status"] != "deleted" {
                return Err(error(
                    409,
                    "Delete the application before removing retained storage.",
                ));
            }
            if action == "apps.storageDeletion"
                && row["spec"]["volumeBytes"].as_u64().unwrap_or(0) == 0
            {
                return Ok((json!({"removed":true,"name":row["name"]}), target));
            }
            return Ok((
                Self::deletion(row, action == "apps.storageDeletion")?,
                target,
            ));
        }
        Self::lock(tx, format!("plan:{}", access.customer)).await?;
        access = Self::authorize(
            tx,
            actor,
            Some(&project),
            None,
            !read,
            operator,
            is_read_action(action),
        )
        .await?;
        let cluster = field(&access.project, "cluster_id")?.to_owned();
        Self::lock(tx, format!("hosting-cluster:{cluster}")).await?;
        if let Some(old) = &row {
            row = Some(Self::app(tx, field(old, "id")?).await?);
        }
        let (hash, prior) = self
            .replay(tx, actor, field(cmd, "idempotencyKey")?, cmd)
            .await?;
        if let Some(prior) = prior {
            if !read && let Some(row) = &row {
                Self::ensure_app_effect_allowed(tx, field(row, "id")?, Some(&prior)).await?;
            }
            return Ok((
                if ["apps.prepare", "apps.reconcile"].contains(&action)
                    || (action == "apps.delete" && prior["app_id"].is_null())
                {
                    prior["result"].clone()
                } else {
                    json!({"id":prior["id"],"appID":prior["app_id"],"status":prior["status"],"revision":prior["target_revision"]})
                },
                target,
            ));
        }
        if !read && let Some(row) = &row {
            Self::ensure_app_effect_allowed(tx, field(row, "id")?, None).await?;
        }
        if action == "apps.prepare" {
            if numeric(
                &Self::limits(tx, &access.customer, None, None).await?,
                "subscriptionRevision",
            )? != numeric(cmd, "subscriptionRevision")?
            {
                return Err(error(
                    409,
                    "Hosting allowance changed. Review the application.",
                ));
            }
            let count:i64=one(tx,"SELECT count(*) AS n FROM webdock_auth.hosting_app WHERE project_id=$1 AND lifecycle='draft' AND status<>'deleted'",vec![project.clone().into()]).await?.unwrap().try_get("","n").map_err(db_error)?;
            if count >= 20 {
                return Err(error(
                    409,
                    "Finish or delete an application draft before creating another.",
                ));
            }
            let id = self.id().await?;
            let environment = crate::environment::Environment::patch(
                Default::default(),
                cmd.get("environment").unwrap_or(&json!([])),
            )?;
            app::ActiveModel {
                id: Set(id.clone()),
                project_id: Set(project.clone()),
                name: Set(field(cmd, "name")?.into()),
                spec: Set(cmd["spec"].clone()),
                lifecycle: Set("draft".into()),
                status: Set("draft".into()),
                environment_encrypted: Set(self.environment.seal(&id, &environment)?),
                ..Default::default()
            }
            .insert(tx)
            .await
            .map_err(db_error)?;
            self.requested_public_route(tx, cmd, &id, &cluster).await?;
            let result = json!({"appID":id,"revision":1,"lifecycle":"draft","status":"draft"});
            self.record_result(tx, actor, cmd, (hash, &result), &access, Some(&cluster))
                .await?;
            self.audit(tx, &actor.user_id, action, &id, "succeeded")
                .await?;
            return Ok((result, target));
        }
        if row.as_ref().is_some_and(|r| r["status"] == "deleted")
            && !["apps.purgeStorage", "apps.reconcile"].contains(&action)
        {
            return Err(error(409, "Application is deleted."));
        }
        if let Some(row) = &row {
            if one(tx,"SELECT 1 FROM webdock_auth.hosting_service_binding WHERE app_id=$1 AND state<>'disconnected' LIMIT 1",vec![field(row,"id")?.into()]).await?.is_some(){return Err(error(503,"Application service binding adapter is required."));}
            if row["lifecycle"] == "draft" && !["apps.deploy", "apps.delete"].contains(&action) {
                return Err(error(
                    409,
                    "Complete the first deployment before using application controls.",
                ));
            }
            if row["lifecycle"] != "draft" && action == "apps.deploy" {
                return Err(error(
                    409,
                    "The first deployment already exists. Update the application instead.",
                ));
            }
        }
        if action == "apps.reconcile" {
            let row = row.as_ref().unwrap();
            let op=rows(tx,"SELECT to_jsonb(o) AS value FROM webdock_auth.hosting_operation o WHERE id=$1 AND status IN ('running','needs-reconciliation') FOR UPDATE",vec![row["operation_id"].as_str().into()]).await?.pop().ok_or_else(||error(409,"No expired operation is available for reconciliation."))?;
            if op["status"] == "running"
                && epoch(&op["lease_until"]).is_some_and(|time| time > now())
            {
                return Err(error(
                    409,
                    "No expired operation is available for reconciliation.",
                ));
            }
            execute(tx,"UPDATE webdock_auth.hosting_operation SET status='queued',authority_subject=$2,authority_session=$3 WHERE id=$1",vec![field(&op,"id")?.into(),actor.user_id.clone().into(),actor.session_id.clone().into()]).await?;
            let result = json!({"id":op["id"],"appID":row["id"],"status":"queued"});
            self.record_result(tx, actor, cmd, (hash, &result), &access, Some(&cluster))
                .await?;
            self.audit(tx, &actor.user_id, action, field(row, "id")?, "queued")
                .await?;
            return Ok((result, target));
        }
        let busy=rows(tx,"SELECT to_jsonb(o) AS value FROM webdock_auth.hosting_operation o WHERE project_id=$1 AND status IN ('queued','running','needs-reconciliation') FOR UPDATE",vec![project.clone().into()]).await?;
        if !busy.is_empty()
            && !(is_read_action(action)
                && busy.iter().all(|v| v["status"] == "needs-reconciliation"))
        {
            if !access.operator
                || row.is_none()
                || busy.iter().any(|v| {
                    v["status"] != "needs-reconciliation"
                        || v["app_id"] != row.as_ref().unwrap()["id"]
                })
            {
                return Err(error(
                    409,
                    "Wait for the current project operation or reconcile it first.",
                ));
            }
            for op in busy {
                execute(
                    tx,
                    "UPDATE webdock_auth.hosting_operation SET status='failed' WHERE id=$1",
                    vec![field(&op, "id")?.into()],
                )
                .await?;
            }
        }
        if let Some(row) = &row
            && let Some(revision) = cmd.get("revision")
            && amount(revision)? != numeric(row, "revision")? as u64
        {
            return Err(error(409, "Application changed. Refresh before editing."));
        }
        if is_read_action(action)
            && row
                .as_ref()
                .is_none_or(|r| r["observed_uid"].as_str().is_none())
        {
            return Err(error(409, "Application has no observed runtime to read."));
        }
        let purging = action == "apps.purgeStorage";
        let deleting = action == "apps.delete" || purging;
        let logging = is_read_action(action);
        if deleting {
            let row = row.as_ref().unwrap();
            if purging
                && (row["status"] != "deleted"
                    || row["spec"]["volumeBytes"].as_u64().unwrap_or(0) == 0)
            {
                return Err(error(
                    409,
                    "Delete the application before removing retained storage.",
                ));
            }
            let preview = Self::deletion(row, purging)?;
            if cmd["confirmName"] != row["name"] || cmd["planHash"] != preview["planHash"] {
                return Err(error(
                    409,
                    "Deletion preview changed. Review the current application.",
                ));
            }
            if row["lifecycle"] == "draft" {
                execute(tx,"UPDATE webdock_auth.hosting_app SET status='deleted',spec=jsonb_set(spec,'{volumeBytes}','0'),environment_encrypted=NULL,revision=revision+1 WHERE id=$1",vec![field(row,"id")?.into()]).await?;
                let result = json!({"appID":row["id"],"status":"deleted","revision":numeric(row,"revision")?+1});
                self.record_result(tx, actor, cmd, (hash, &result), &access, Some(&cluster))
                    .await?;
                self.audit(tx, &actor.user_id, action, field(row, "id")?, "succeeded")
                    .await?;
                return Ok((result, target));
            }
        }
        let mut spec = cmd
            .get("spec")
            .cloned()
            .or_else(|| row.as_ref().map(|v| v["spec"].clone()))
            .ok_or_else(|| error(400, "Application specification required."))?;
        if action == "apps.rollback" {
            spec = row.as_ref().unwrap()["previous_spec"].clone();
            if spec.is_null() {
                return Err(error(409, "No previous application revision is available."));
            }
        }
        if action == "apps.scale" {
            spec["replicas"] = cmd["replicas"].clone();
        }
        if action == "apps.stop" {
            spec["replicas"] = json!(0);
        }
        if action == "apps.start" {
            spec["replicas"] = json!(amount(&spec["replicas"])?.max(1));
        }
        spec = contracts::resolve_spec(spec)?;
        if let Some(row) = &row
            && row["lifecycle"] != "draft"
            && spec["volumeBytes"].as_u64().unwrap_or(0)
                != row["spec"]["volumeBytes"].as_u64().unwrap_or(0)
        {
            return Err(error(
                409,
                "Persistent storage size is fixed at application creation.",
            ));
        }
        if spec["template"] == "custom" && !access.operator && access.project["own_images"] != true
        {
            return Err(error(
                403,
                "Custom images are not enabled for this project.",
            ));
        }
        let mut environment = if let Some(row) = &row {
            self.environment.open(
                field(row, "id")?,
                row[if action == "apps.rollback" {
                    "previous_environment_encrypted"
                } else {
                    "environment_encrypted"
                }]
                .as_str(),
            )?
        } else {
            Default::default()
        };
        if let Some(patch) = cmd.get("environment") {
            environment = Environment::patch(environment, patch)?;
        }
        let snapshot = Self::cluster_view(tx, &cluster).await?;
        let storage = spec["volumeBytes"].as_u64().unwrap_or(0) > 0;
        let environment_required = !environment.is_empty()
            || row.as_ref().is_some_and(|r| {
                !r["environment_encrypted"].is_null()
                    || !r["observed_environment_encrypted"].is_null()
            });
        if storage
            && !logging
            && (snapshot["observation"]["capabilities"]["storageVersion"] != 1
                || (!deleting
                    && !logging
                    && snapshot["observation"]["capabilities"]["storage"] != true))
        {
            return Err(error(
                409,
                "Verified persistent storage is unavailable on this cluster.",
            ));
        }
        if snapshot["runtimeKind"] == "docker"
            && !logging
            && (storage
                && !deleting
                && snapshot["observation"]["capabilities"]["volumeQuotas"] != true
                || (environment_required
                    && !deleting
                    && action != "apps.stop"
                    && snapshot["observation"]["capabilities"]["environment"] != true)
                || amount(&spec["replicas"])? > 1)
        {
            return Err(error(
                503,
                "Docker adapter does not support the requested quota, environment, or replica capability.",
            ));
        }
        if environment_required
            && !logging
            && !deleting
            && action != "apps.stop"
            && snapshot["observation"]["capabilities"]["environment"] != true
        {
            return Err(error(
                409,
                "Update the cluster agent before using environment variables.",
            ));
        }
        let mut additional = contracts::allowances(None)?;
        if !logging {
            let mut completed_spec = spec.clone();
            if purging {
                completed_spec["volumeBytes"] = json!(0);
            }
            let full = contracts::app_demand(&completed_spec, deleting)?;
            let prior_reservations = if let Some(row) = &row {
                rows(tx,"SELECT jsonb_build_object('demand',demand,'status',status) AS value FROM webdock_auth.hosting_reservation WHERE app_id=$1 AND status<>'released'",vec![field(row,"id")?.into()]).await?
            } else {
                vec![]
            };
            let old = contracts::totals(&prior_reservations)?;
            for key in DIMENSIONS {
                additional[key] = json!(if key == "replicasPerApp" {
                    amount(&full[key])?
                } else {
                    amount(&full[key])?.saturating_sub(amount(&old[key])?)
                });
            }
            let subscription = cmd
                .get("subscriptionRevision")
                .map(amount)
                .transpose()?
                .map(|v| v as i64)
                .unwrap_or(numeric(
                    &Self::limits(tx, &access.customer, None, None).await?,
                    "subscriptionRevision",
                )?);
            Self::check_demand(
                tx,
                &access.customer,
                &project,
                &cluster,
                field(&access.project, "provider")?,
                &additional,
                subscription,
            )
            .await?;
            if let Some(old) = &row {
                let model = app::Entity::find_by_id(field(old, "id")?)
                    .lock_exclusive()
                    .one(tx)
                    .await
                    .map_err(db_error)?
                    .ok_or_else(|| error(404, "Application unavailable."))?;
                let mut active = model.clone().into_active_model();
                if !deleting {
                    active.previous_spec = Set(model.observed_spec);
                    active.previous_environment_encrypted =
                        Set(model.observed_environment_encrypted);
                }
                active.spec = Set(spec.clone());
                active.revision = Set(if model.lifecycle == "draft" {
                    model.revision
                } else {
                    model
                        .revision
                        .checked_add(1)
                        .ok_or_else(|| error(409, "Application revision exhausted."))?
                });
                active.lifecycle = Set("deployed".into());
                active.status = Set(if purging { "deleted" } else { "pending" }.into());
                active.last_error = Set(None);
                active.environment_encrypted =
                    Set(self.environment.seal(&model.id, &environment)?);
                active.logs = Set(None);
                active.update(tx).await.map_err(db_error)?;
                row = Some(Self::app(tx, &model.id).await?);
            } else {
                if action != "apps.create" {
                    return Err(error(400, "Create an application first."));
                }
                let id = self.id().await?;
                app::ActiveModel {
                    id: Set(id.clone()),
                    project_id: Set(project.clone()),
                    name: Set(field(cmd, "name")?.into()),
                    spec: Set(spec.clone()),
                    environment_encrypted: Set(self.environment.seal(&id, &environment)?),
                    ..Default::default()
                }
                .insert(tx)
                .await
                .map_err(db_error)?;
                row = Some(Self::app(tx, &id).await?);
            }
        }
        let row = row.as_ref().unwrap();
        let public_route = self
            .requested_public_route(tx, cmd, field(row, "id")?, &cluster)
            .await?;
        let mut desired = json!({"appID":row["id"],"projectID":project,"clusterID":cluster,"namespace":access.project.get("namespace").cloned().unwrap_or(json!(format!("wd-{project}"))),"name":row["name"],"spec":spec,"revision":row["revision"],"action":if logging{match action{"apps.inspect"=>"inspect","apps.metrics"=>"metrics",_=>"logs"}}else if purging{"purge-storage"}else if deleting{"delete"}else{"apply"},"expectedUID":row["observed_uid"]});
        desired["customerScope"] = json!(actor.customer_scope);
        if let Some(route) = public_route.filter(|_| !logging) {
            desired["publicRoute"] = json!(route);
            desired["publicRoutingVersion"] = json!(1);
        }
        if storage {
            desired["storageVersion"] = json!(1);
        }
        if environment_required {
            desired["environmentVersion"] = json!(1);
            desired["environmentEncrypted"] = if row["environment_encrypted"].is_null() {
                json!(
                    self.environment
                        .seal_snapshot(field(row, "id")?, &environment)?
                )
            } else {
                row["environment_encrypted"].clone()
            };
        }
        let op = self.id().await?;
        execute(tx,"INSERT INTO webdock_auth.hosting_operation(id,subject,actor_session,customer_id,project_id,cluster_id,app_id,action,idempotency_key,payload_hash,status,desired,target_revision) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,'queued',$11,$12)",vec![op.clone().into(),actor.user_id.clone().into(),actor.session_id.clone().into(),access.customer.clone().into(),project.clone().into(),cluster.clone().into(),field(row,"id")?.into(),action.into(),field(cmd,"idempotencyKey")?.into(),hash.into(),desired.into(),numeric(row,"revision")?.into()]).await?;
        if !logging {
            execute(tx,"INSERT INTO webdock_auth.hosting_reservation(operation_id,customer_id,project_id,cluster_id,app_id,demand,status) VALUES($1,$2,$3,$4,$5,$6,'reserved')",vec![op.clone().into(),access.customer.into(),project.into(),cluster.into(),field(row,"id")?.into(),additional.into()]).await?;
            execute(
                tx,
                "UPDATE webdock_auth.hosting_app SET operation_id=$2 WHERE id=$1",
                vec![field(row, "id")?.into(), op.clone().into()],
            )
            .await?;
        }
        self.audit(tx, &actor.user_id, action, field(row, "id")?, "queued")
            .await?;
        Ok((
            json!({"id":op,"appID":row["id"],"status":"queued","revision":row["revision"]}),
            target,
        ))
    }
}
