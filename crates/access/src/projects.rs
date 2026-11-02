//! Projects inside a customer's workspace: members read, owners/admins and operators create.
use crate::{
    contracts::{id, text},
    service::{json_rows, tenant},
    *,
};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::{Value, json};
use webdock_identity::Principal;

impl AccessService {
    pub async fn list_projects_as(&self, actor: &Principal, customer: &str) -> Result<Vec<Value>> {
        let actor = self.current_actor(actor).await?;
        let tx = self.domain(&actor).await?;
        tenant(&tx, &actor, customer).await?;
        let rows = json_rows(&tx, "SELECT jsonb_build_object('id',id,'name',name,'status',status) AS data FROM webdock_admin.projects WHERE customer_id=$1 AND status='active' ORDER BY name,id", vec![id(customer)?.into()]).await?;
        tx.commit().await?;
        Ok(rows)
    }
    pub async fn create_project_as(
        &self,
        actor: &Principal,
        customer: &str,
        name: &str,
    ) -> Result<Value> {
        let actor = self.current_actor(actor).await?;
        let name = text(name, 120)?;
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
        let project = self.next_id().await?;
        let created: bool = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_rls.create_customer_project($1,$2,$3) AS created",
                [
                    project.clone().into(),
                    name.clone().into(),
                    id(customer)?.into(),
                ],
            ))
            .await
            .map_err(project_limit)?
            .ok_or(Error::Forbidden)?
            .try_get("", "created")?;
        if !created {
            return Err(Error::Forbidden);
        }
        self.audit(&tx, &actor, "project-created", Some(customer), false)
            .await?;
        tx.commit().await?;
        Ok(json!({"id": project, "name": name, "status": "active"}))
    }
    pub async fn get_project_as(&self, actor: &Principal, project: &str) -> Result<Value> {
        let actor = self.current_actor(actor).await?;
        let tx = self.domain(&actor).await?;
        let rows = json_rows(&tx, "SELECT jsonb_build_object('id',p.id,'name',p.name,'customer',p.customer_id) AS data FROM webdock_admin.projects p WHERE p.id=$1 AND p.status='active'", vec![id(project)?.into()]).await?;
        let row = rows.into_iter().next().ok_or(Error::NotFound)?;
        let customer = row["customer"].as_str().ok_or(Error::NotFound)?.to_owned();
        let profile = tenant(&tx, &actor, &customer).await?;
        tx.commit().await?;
        let role = if actor.is_operator() {
            json!("operator")
        } else {
            profile["member_role"].clone()
        };
        Ok(
            json!({"id": row["id"], "name": row["name"], "customer": {"id": customer, "name": profile["name"]}, "role": role}),
        )
    }
}

/// The plan's project limit (SQLSTATE WD429 from the projects trigger) as its own error.
pub(crate) fn project_limit(error: sea_orm::DbErr) -> Error {
    if let sea_orm::DbErr::Query(sea_orm::RuntimeErr::SqlxError(e))
    | sea_orm::DbErr::Exec(sea_orm::RuntimeErr::SqlxError(e)) = &error
        && e.as_database_error()
            .and_then(|e| e.code())
            .is_some_and(|code| code == "WD429")
    {
        return Error::ProjectLimit;
    }
    Error::Database(error)
}
