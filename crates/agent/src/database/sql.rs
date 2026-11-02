use super::executor::*;
use super::*;
use kube::api::AttachParams;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use webdock_agent_protocol::database::DatabaseClaim;
pub(super) struct SqlStatements {
    pub roles: String,
    pub create: String,
    pub mark: String,
    pub database: String,
    pub schema: String,
    pub enable: String,
    pub disable: String,
}
pub(super) fn statements(job: &DatabaseJobSpec) -> Result<SqlStatements> {
    job.validate()
        .map_err(|_| ExecutionError::Invalid("database SQL target"))?;
    let database = job
        .database
        .as_ref()
        .ok_or(ExecutionError::Invalid("logical database required"))?;
    let id = &database.id;
    let name = &database.name;
    let owner = format!("wd_owner_{id}");
    let migration = format!("wd_migrate_{id}");
    let app = format!("wd_app_{id}");
    let marker = format!("webdock:database:{id}");
    let roles=[&owner,&migration,&app].iter().map(|role|format!("DO $webdock$ BEGIN IF NOT EXISTS(SELECT 1 FROM pg_catalog.pg_roles WHERE rolname='{role}') THEN CREATE ROLE \"{role}\" NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS; COMMENT ON ROLE \"{role}\" IS '{marker}'; ELSIF (SELECT pg_catalog.shobj_description(oid,'pg_authid') FROM pg_catalog.pg_roles WHERE rolname='{role}') IS DISTINCT FROM '{marker}' THEN RAISE EXCEPTION 'Database role ownership conflict'; END IF; END $webdock$;")).collect::<Vec<_>>().join("\n");
    let export_disable = super::exports::ExportRole::new(id, name, &job.backend.kind)
        .map_err(|_| ExecutionError::Ownership)?
        .map(|role| role.disable)
        .unwrap_or_default();
    Ok(SqlStatements {
        roles,
        create: format!("CREATE DATABASE \"{name}\" OWNER \"{owner}\";"),
        mark: format!("COMMENT ON DATABASE \"{name}\" IS '{marker}';"),
        database: format!(
            "REVOKE ALL ON DATABASE \"{name}\" FROM PUBLIC; GRANT CONNECT ON DATABASE \"{name}\" TO \"{migration}\", \"{app}\";"
        ),
        schema: format!(
            "REVOKE ALL ON SCHEMA public FROM PUBLIC; CREATE SCHEMA IF NOT EXISTS app AUTHORIZATION \"{migration}\"; GRANT USAGE ON SCHEMA app TO \"{app}\"; ALTER DEFAULT PRIVILEGES FOR ROLE \"{migration}\" IN SCHEMA app GRANT SELECT,INSERT,UPDATE,DELETE ON TABLES TO \"{app}\"; ALTER DEFAULT PRIVILEGES FOR ROLE \"{migration}\" IN SCHEMA app GRANT USAGE,SELECT ON SEQUENCES TO \"{app}\"; ALTER DEFAULT PRIVILEGES FOR ROLE \"{migration}\" IN SCHEMA app REVOKE EXECUTE ON FUNCTIONS FROM PUBLIC; ALTER ROLE \"{migration}\" IN DATABASE \"{name}\" SET search_path=app,pg_catalog; ALTER ROLE \"{app}\" IN DATABASE \"{name}\" SET search_path=app,pg_catalog;"
        ),
        enable: format!(
            "ALTER ROLE \"{migration}\" NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS LOGIN CONNECTION LIMIT {}; ALTER ROLE \"{app}\" NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS LOGIN CONNECTION LIMIT {};",
            database.connection_limit, database.connection_limit
        ),
        disable: format!(
            "ALTER ROLE \"{migration}\" NOLOGIN; ALTER ROLE \"{app}\" NOLOGIN; SELECT pg_catalog.pg_terminate_backend(pid) FROM pg_catalog.pg_stat_activity WHERE usename IN ('{migration}','{app}') AND pid<>pg_catalog.pg_backend_pid();{export_disable}"
        ),
    })
}
async fn bounded(mut stream: impl AsyncRead + Unpin, max: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    (&mut stream)
        .take(max + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| ExecutionError::Runtime)?;
    if bytes.len() as u64 > max {
        return Err(ExecutionError::Invalid("database command output"));
    }
    Ok(bytes)
}
impl NativeDatabaseExecutor {
    pub(super) async fn psql(
        &self,
        claim: &DatabaseClaim,
        authority: &dyn DatabaseAuthority,
        database: &str,
        input: &str,
        extra: &[String],
    ) -> Result<String> {
        if input.len() > 65536
            || !std::iter::once("postgres")
                .chain(claim.spec.database.iter().map(|d| d.name.as_str()))
                .any(|name| name == database)
        {
            return Err(ExecutionError::Invalid("database command input"));
        }
        self.guard(claim, authority).await?;
        let cluster = self
            .get("Cluster", "postgres", Some(&claim.spec.backend.namespace))
            .await?
            .ok_or(ExecutionError::Ownership)?;
        self.owned(&cluster, &claim.spec)?;
        let pod = self
            .get("Pod", "postgres-1", Some(&claim.spec.backend.namespace))
            .await?
            .ok_or(ExecutionError::Ownership)?;
        if !pod.metadata.owner_references.as_ref().is_some_and(|refs| {
            refs.iter()
                .any(|r| Some(&r.uid) == cluster.metadata.uid.as_ref())
        }) || !pod.data["spec"]["containers"].as_array().is_some_and(|cs| {
            cs.iter()
                .any(|c| c["name"] == "postgres" && c["image"] == self.config.image)
        }) {
            return Err(ExecutionError::Ownership);
        }
        let uid = pod.metadata.uid.clone();
        let pods: kube::Api<k8s_openapi::api::core::v1::Pod> =
            kube::Api::namespaced(self.client.clone(), &claim.spec.backend.namespace);
        let mut command = vec![
            "psql".to_owned(),
            "-X".into(),
            "-U".into(),
            "postgres".into(),
            "-d".into(),
            database.into(),
            "-v".into(),
            "ON_ERROR_STOP=1".into(),
            "-At".into(),
            "-q".into(),
        ];
        command.extend_from_slice(extra);
        let work = async {
            let mut process = pods
                .exec(
                    "postgres-1",
                    command,
                    &AttachParams {
                        container: Some("postgres".into()),
                        stdin: true,
                        stdout: true,
                        stderr: true,
                        tty: false,
                        ..Default::default()
                    },
                )
                .await
                .map_err(|_| ExecutionError::Runtime)?;
            let status = process.take_status().ok_or(ExecutionError::Runtime)?;
            let mut stdin = process.stdin().ok_or(ExecutionError::Runtime)?;
            let stdout = process.stdout().ok_or(ExecutionError::Runtime)?;
            let stderr = process.stderr().ok_or(ExecutionError::Runtime)?;
            let write = async {
                stdin
                    .write_all(input.as_bytes())
                    .await
                    .map_err(|_| ExecutionError::Runtime)?;
                stdin.shutdown().await.map_err(|_| ExecutionError::Runtime)
            };
            let (_, stdout, _) = tokio::try_join!(
                write,
                bounded(stdout, 4 * 1024 * 1024),
                bounded(stderr, 65536)
            )?;
            if status
                .await
                .is_none_or(|status| status.status.as_deref() != Some("Success"))
            {
                return Err(ExecutionError::Runtime);
            }
            process.join().await.map_err(|_| ExecutionError::Runtime)?;
            String::from_utf8(stdout)
                .map(|s| s.trim().to_owned())
                .map_err(|_| ExecutionError::Runtime)
        };
        let output = self
            .guarded(claim, authority, async {
                tokio::time::timeout(std::time::Duration::from_secs(60), work)
                    .await
                    .map_err(|_| ExecutionError::Expired)?
            })
            .await?;
        if self
            .get("Pod", "postgres-1", Some(&claim.spec.backend.namespace))
            .await?
            .and_then(|p| p.metadata.uid)
            != uid
        {
            return Err(ExecutionError::Ownership);
        }
        Ok(output)
    }
    pub(super) async fn sql(
        &self,
        claim: &DatabaseClaim,
        authority: &dyn DatabaseAuthority,
        database: &str,
        statement: &str,
    ) -> Result<String> {
        self.psql(
            claim,
            authority,
            database,
            &format!("SET statement_timeout='45s'; SET lock_timeout='10s'; {statement}"),
            &[],
        )
        .await
    }
    pub(super) async fn database_exists(
        &self,
        claim: &DatabaseClaim,
        authority: &dyn DatabaseAuthority,
    ) -> Result<bool> {
        let d = claim
            .spec
            .database
            .as_ref()
            .ok_or(ExecutionError::Invalid("logical database required"))?;
        let result=self.sql(claim,authority,"postgres",&format!("SELECT COALESCE(pg_catalog.shobj_description(oid,'pg_database'),'unowned') FROM pg_catalog.pg_database WHERE datname='{}';",d.name)).await?;
        let marker = format!("webdock:database:{}", d.id);
        if result.is_empty() {
            Ok(false)
        } else if result == marker {
            Ok(true)
        } else {
            Err(ExecutionError::Ownership)
        }
    }
    pub(super) async fn configure_export(
        &self,
        claim: &DatabaseClaim,
        authority: &dyn DatabaseAuthority,
    ) -> Result<()> {
        let d = claim
            .spec
            .database
            .as_ref()
            .ok_or(ExecutionError::Ownership)?;
        if let Some(role) =
            super::exports::ExportRole::new(&d.id, &d.name, &claim.spec.backend.kind)
                .map_err(|_| ExecutionError::Ownership)?
        {
            self.sql(claim, authority, "postgres", &role.create).await?;
            self.sql(claim, authority, &d.name, &role.grants).await?;
        }
        Ok(())
    }
    pub(super) async fn enable_export(
        &self,
        claim: &DatabaseClaim,
        authority: &dyn DatabaseAuthority,
    ) -> Result<()> {
        let d = claim
            .spec
            .database
            .as_ref()
            .ok_or(ExecutionError::Ownership)?;
        if let Some(role) =
            super::exports::ExportRole::new(&d.id, &d.name, &claim.spec.backend.kind)
                .map_err(|_| ExecutionError::Ownership)?
        {
            self.sql(claim, authority, "postgres", &role.enable).await?;
        }
        Ok(())
    }
    pub(super) async fn recovered_export_password(
        &self,
        claim: &DatabaseClaim,
        authority: &dyn DatabaseAuthority,
    ) -> Result<()> {
        let d = claim
            .spec
            .database
            .as_ref()
            .ok_or(ExecutionError::Ownership)?;
        let credentials = claim
            .material
            .credentials
            .as_ref()
            .ok_or(ExecutionError::Denied)?;
        if let Some(role) =
            super::exports::ExportRole::new(&d.id, &d.name, &claim.spec.backend.kind)
                .map_err(|_| ExecutionError::Ownership)?
        {
            let password =
                super::exports::ExportRole::password(&d.id, &credentials.application_password)
                    .map_err(|_| ExecutionError::Denied)?;
            self.psql(
                claim,
                authority,
                "postgres",
                &format!("{password}\n{password}\n"),
                &[
                    "-c".into(),
                    "SET password_encryption='scram-sha-256'".into(),
                    "-c".into(),
                    format!("\\password {}", role.name),
                ],
            )
            .await?;
        }
        Ok(())
    }
    pub(super) async fn passwords(
        &self,
        claim: &DatabaseClaim,
        authority: &dyn DatabaseAuthority,
    ) -> Result<()> {
        let d = claim
            .spec
            .database
            .as_ref()
            .ok_or(ExecutionError::Invalid("logical database required"))?;
        let credentials = claim
            .material
            .credentials
            .as_ref()
            .ok_or(ExecutionError::Denied)?;
        let mut passwords = vec![
            (
                format!("wd_migrate_{}", d.id),
                credentials.migration_password.clone(),
            ),
            (
                format!("wd_app_{}", d.id),
                credentials.application_password.clone(),
            ),
        ];
        if let Some(role) =
            super::exports::ExportRole::new(&d.id, &d.name, &claim.spec.backend.kind)
                .map_err(|_| ExecutionError::Ownership)?
        {
            passwords.push((
                role.name,
                super::exports::ExportRole::password(&d.id, &credentials.application_password)
                    .map_err(|_| ExecutionError::Denied)?,
            ));
        }
        for (role, password) in passwords {
            // psql/libpq generates the SCRAM verifier; plaintext is only stdin, never SQL, argv, logs or a persisted command.
            self.psql(
                claim,
                authority,
                "postgres",
                &format!("{password}\n{password}\n"),
                &[
                    "-c".into(),
                    "SET password_encryption='scram-sha-256'".into(),
                    "-c".into(),
                    format!("\\password {role}"),
                ],
            )
            .await?;
        }
        Ok(())
    }
}
