use super::*;
#[derive(serde::Deserialize)]
#[serde(tag = "action", deny_unknown_fields)]
enum Command {
    #[serde(rename = "git.connections.list")]
    List {
        #[serde(rename = "customerID")]
        customer: String,
    },
    #[serde(rename = "git.repositories.list")]
    Repositories {
        #[serde(rename = "customerID")]
        customer: String,
        #[serde(rename = "connectionID")]
        connection: String,
    },
    #[serde(rename = "git.connections.disconnect")]
    Disconnect {
        #[serde(rename = "customerID")]
        customer: String,
        #[serde(rename = "connectionID")]
        connection: String,
        revision: i32,
    },
}
impl BuildService {
    fn repository_view(row: &Value) -> Result<Value> {
        let mut metadata = row["metadata"]
            .as_object()
            .cloned()
            .ok_or_else(|| error("invalid_repository_metadata"))?;
        metadata.insert("repositoryID".into(), json!(field(row, "repository_id")?));
        Ok(Value::Object(metadata))
    }
    async fn connection_authority(
        tx: &DatabaseTransaction,
        principal: &Principal,
        customer: &str,
        write: bool,
    ) -> Result<()> {
        let row=rows(tx,"SELECT jsonb_build_object('actor',webdock_rls.actor(),'allowed',webdock_rls.customer_allowed(c.id,false),'manage',webdock_rls.customer_permission(c.id,'org:projects:manage')) AS value FROM webdock_admin.customers c WHERE c.id=$1 AND c.status='active' FOR SHARE",vec![customer.into()]).await?.pop().ok_or_else(||error("customer_unavailable"))?;
        if row["actor"]["subject"] != principal.user_id
            || row["actor"]["sessionID"] != principal.session_id
            || row["allowed"] != true
            || row["manage"] != true
            || write && principal.in_read_only_preview()
        {
            return Err(error("permission_denied"));
        }
        Ok(())
    }
    async fn connection_view(tx: &DatabaseTransaction, r: &Value) -> Result<Value> {
        let repositories = rows(
            tx,
            "SELECT to_jsonb(r) AS value FROM webdock_auth.git_repository r WHERE connection_id=$1 ORDER BY repository_id",
            vec![field(r, "id")?.into()],
        )
        .await?;
        let repositories = repositories
            .iter()
            .map(Self::repository_view)
            .collect::<Result<Vec<_>>>()?;
        Ok(
            json!({"id":r["id"],"customerID":r["customer_id"],"installationID":r["installation_id"],"accountLogin":r["account_login"],"state":r["state"],"generation":r["generation"],"revision":r["revision"],"repositories":repositories}),
        )
    }
    pub(super) async fn execute_connections(
        &self,
        principal: &Principal,
        input: Value,
        authority: Option<&webdock_browser_session::StoredAuthority>,
    ) -> Result<Value> {
        let command: Command =
            serde_json::from_value(input).map_err(|_| error("invalid_command"))?;
        let customer = match &command {
            Command::List { customer }
            | Command::Repositories { customer, .. }
            | Command::Disconnect { customer, .. } => customer,
        };
        crate::contracts::id(customer)?;
        let write = matches!(command, Command::Disconnect { .. });
        let authority_guard = self.begin_authority(principal, authority, write).await?;
        self.require_execution(write)?;
        let tx = self
            .signer
            .begin(&self.db, principal)
            .await
            .map_err(|_| error("authentication_required"))?;
        Self::module(&tx, !write).await?;
        Self::connection_authority(&tx, principal, customer, write).await?;
        let result = match &command {
            Command::List { .. } => {
                let records=rows(&tx,"SELECT to_jsonb(c) AS value FROM webdock_auth.git_connection c WHERE customer_id=$1 ORDER BY created_at,id",vec![customer.clone().into()]).await?;
                let mut docs = Vec::new();
                for r in records {
                    docs.push(Self::connection_view(&tx, &r).await?);
                }
                json!({"docs":docs})
            }
            Command::Repositories { connection, .. } => {
                crate::contracts::id(connection)?;
                let repositories=rows(&tx,"SELECT to_jsonb(r) AS value FROM webdock_auth.git_repository r JOIN webdock_auth.git_connection c ON c.id=r.connection_id WHERE c.id=$1 AND c.customer_id=$2 AND c.state='active' ORDER BY r.repository_id",vec![connection.clone().into(),customer.clone().into()]).await?;
                let docs = repositories
                    .iter()
                    .map(Self::repository_view)
                    .collect::<Result<Vec<_>>>()?;
                json!({"docs":docs})
            }
            Command::Disconnect {
                connection,
                revision,
                ..
            } => {
                crate::contracts::id(connection)?;
                if *revision < 0 || *revision == i32::MAX {
                    return Err(error("invalid_revision"));
                }
                let row=rows(&tx,"UPDATE webdock_auth.git_connection SET state='disconnected',generation=generation+1,revision=revision+1 WHERE id=$1 AND customer_id=$2 AND revision=$3 RETURNING to_jsonb(git_connection.*) AS value",vec![connection.clone().into(),customer.clone().into(),(*revision).into()]).await?.pop().ok_or_else(||error("connection_revision_conflict"))?;
                execute(
                    &tx,
                    "SELECT webdock_rls.cancel_obsolete_git_builds()",
                    vec![],
                )
                .await?;
                self.audit(
                    &tx,
                    &principal.user_id,
                    "git.connections.disconnect",
                    connection,
                    "disconnected",
                )
                .await?;
                Self::connection_view(&tx, &row).await?
            }
        };
        Self::connection_authority(&tx, principal, customer, write).await?;
        session::verify(&authority_guard).await?;
        tx.commit().await.map_err(db_error)?;
        session::commit(authority_guard).await?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_projection_exposes_the_authoritative_binding_not_metadata_ids() {
        let row = json!({"repository_id":"123456","metadata":{"fullName":"owned/project","repositoryID":"999999"}});
        assert_eq!(
            BuildService::repository_view(&row).unwrap(),
            json!({"fullName":"owned/project","repositoryID":"123456"})
        );
        assert_eq!(
            BuildService::repository_view(&json!({"repository_id":"55","metadata":{}})).unwrap(),
            json!({"repositoryID":"55"})
        );
        assert!(BuildService::repository_view(&json!({"metadata":{}})).is_err());
    }
}
