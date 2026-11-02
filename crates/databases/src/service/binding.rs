use super::*;
impl DatabaseService {
    /// Trusted app-binding hydration only. Caller supplies its live authenticated RLS transaction,
    /// holds current app/binding authority through use, and never persists the returned material.
    pub async fn binding_material_in(
        &self,
        tx: &DatabaseTransaction,
        input: &DatabaseBindingRequest,
    ) -> Result<DatabaseBindingMaterial> {
        if input.variable.is_empty()
            || input.variable.len() > 128
            || !input
                .variable
                .bytes()
                .enumerate()
                .all(|(i, b)| b == b'_' || b.is_ascii_uppercase() || (i > 0 && b.is_ascii_digit()))
        {
            return Err(invalid());
        }
        self.installed_scoped(tx, false).await?;
        let row=one(tx,"SELECT to_jsonb(d)||jsonb_build_object('cluster_id',b.cluster_id,'ca_certificate',b.ca_certificate,'endpoint_ready',b.endpoint_ready,'backend_state',b.state) AS value FROM webdock_auth.hosting_database d JOIN webdock_auth.hosting_database_backend b ON b.id=d.backend_id WHERE d.id=$1 AND d.project_id=$2 AND d.customer_id=$3 FOR SHARE OF d,b",vec![input.database_id.clone().into(),input.project_id.clone().into(),input.customer_id.clone().into()]).await?;
        if row["cluster_id"] != input.cluster_id {
            return Err(denied());
        }
        if row["state"] != "ready"
            || row["endpoint_ready"] != true
            || row["backend_state"] != "ready"
            || row["confirmed_credentials_encrypted"].is_null()
            || row["confirmed_credential_revision"] != row["credential_revision"]
        {
            return Err(conflict(
                "Wait for confirmed database readiness before connecting the application.",
            ));
        }
        let ca = text(&row, "ca_certificate")?;
        validate_certificate(ca, false)?;
        let revision = number(&row, "confirmed_credential_revision")?;
        let id = text(&row, "id")?;
        let backend = text(&row, "backend_id")?;
        let secret: Value = self
            .cipher
            .open(
                text(&row, "confirmed_credentials_encrypted")?,
                &format!("database:{id}:{revision}"),
            )
            .map_err(|_| unavailable())?;
        let environment = std::collections::BTreeMap::from([(
            input.variable.clone(),
            database_connection_url(
                &format!("postgres-rw.wd-db-{backend}.svc.cluster.local"),
                5432,
                text(&row, "database_name")?,
                &format!("wd_app_{id}"),
                text(&secret, "applicationPassword")?,
                &input.binding_id,
            )?,
        )]);
        Ok(DatabaseBindingMaterial {
            credential_revision: revision,
            environment,
            runtime: DatabaseBindingRuntime {
                id: input.binding_id.clone(),
                revision: 1,
                backend_id: backend.into(),
                ca_certificate: ca.into(),
            },
        })
    }
    /// Explicit user-requested credential disclosure, gated by current management authority.
    pub async fn display_credentials(
        &self,
        principal: &Principal,
        authority: &StoredAuthority,
        id: &str,
    ) -> Result<Value> {
        let (auth, current) = self
            .verify(&principal.user_id, &principal.session_id, authority)
            .await?;
        let tx = self.begin_actor(&current).await?;
        self.installed(&tx, false).await?;
        let d = self
            .control_access(&tx, &current, authority, id, true, false, true)
            .await?;
        if !["ready", "paused"].contains(&text(&d, "state")?)
            || d["confirmed_credentials_encrypted"].is_null()
            || d["confirmed_credential_revision"] != d["credential_revision"]
        {
            return Err(conflict(
                "Confirmed database credentials are not available yet.",
            ));
        }
        let b = one(
            &tx,
            "SELECT to_jsonb(b) AS value FROM webdock_auth.hosting_database_backend b WHERE id=$1",
            vec![text(&d, "backend_id")?.into()],
        )
        .await?;
        if b["endpoint_ready"] != true && d["state"] != "paused"
            || text(&b, "ca_certificate")?.is_empty()
        {
            return Err(conflict(
                "Verified database connection details are not available yet.",
            ));
        }
        let secret: Value = self
            .cipher
            .open(
                text(&d, "confirmed_credentials_encrypted")?,
                &format!(
                    "database:{id}:{}",
                    number(&d, "confirmed_credential_revision")?
                ),
            )
            .map_err(|_| unavailable())?;
        self.control_audit(&tx, &current, "databases.credentials.display", id)
            .await?;
        let value = json!({"databaseID":id,"databaseName":d["database_name"],"host":b["host"],"directPort":b["direct_port"],"pooledPort":b["pooled_port"],"credentialRevision":d["confirmed_credential_revision"],"application":{"username":format!("wd_app_{id}"),"password":text(&secret,"applicationPassword")?},"migration":{"username":format!("wd_migrate_{id}"),"password":text(&secret,"migrationPassword")?},"sslmode":"verify-full","caCertificate":b["ca_certificate"]});
        tx.commit().await?;
        auth.commit().await?;
        Ok(value)
    }
}
pub(super) fn validate_certificate(value: &str, authority: bool) -> Result<()> {
    use x509_parser::prelude::FromDer;
    if value.len() > 262144 || value.contains("PRIVATE KEY") {
        return Err(conflict(
            "Verified database connection details are not available yet.",
        ));
    }
    let (_, pem) = x509_parser::pem::parse_x509_pem(value.as_bytes())
        .map_err(|_| conflict("Verified database connection details are not available yet."))?;
    let (_, certificate) = x509_parser::certificate::X509Certificate::from_der(&pem.contents)
        .map_err(|_| conflict("Verified database connection details are not available yet."))?;
    if authority && (!certificate.is_ca() || !certificate.validity().is_valid()) {
        return Err(conflict(
            "A current database certificate authority is required.",
        ));
    }
    Ok(())
}
