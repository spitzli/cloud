//! Explicit offline first-install transaction. HTTP and normal startup never call this module.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DatabaseTransaction, DbBackend,
    Statement, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use spitzli_persistence::{IdAllocator, connection::ConnectionUrl};
use std::{
    collections::{BTreeMap, HashMap},
    fs::File,
    io::Write,
    os::unix::fs::MetadataExt,
    path::Path,
    time::Duration,
};
#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    #[error("Invalid private bootstrap input or canonical configuration")]
    Input,
    #[error("Explicit offline database target is required")]
    Target,
    #[error(
        "The original authenticated login must be the dedicated non-superuser offline migrator"
    )]
    Role,
    #[error("Exact installed baseline and migration history are required")]
    History,
    #[error(
        "Initial setup already has identity, client, signing-key or audit state; reviewed account management is required"
    )]
    Existing,
    #[error("Exclusive private bootstrap output could not be made durable")]
    Output,
    #[error("Initial setup failed before commit; no account changes committed")]
    Database,
    #[error(
        "Initial setup commit is uncertain; retain the private output and inspect its recorded IDs before retrying"
    )]
    CommitUncertain,
}
type Result<T> = std::result::Result<T, BootstrapError>;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperatorInput {
    pub email: String,
    pub name: String,
    #[serde(default, deserialize_with = "optional_password")]
    pub password: Option<String>,
    pub auth_origin: String,
    pub studio_origin: String,
    pub console_origin: String,
    #[serde(default, deserialize_with = "optional_initial_mail_enabled")]
    pub initial_mail_enabled: Option<bool>,
}
fn optional_password<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<String>, D::Error> {
    String::deserialize(d).map(Some)
}
fn optional_initial_mail_enabled<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<bool>, D::Error> {
    bool::deserialize(deserializer).map(Some)
}
struct ReviewedInput {
    email: String,
    name: String,
    password: String,
    auth_origin: String,
    studio_origin: String,
    console_origin: String,
    initial_mail_enabled: Option<bool>,
}
fn canonical(value: &str, local: bool) -> Result<String> {
    let origin =
        spitzli_core::config::canonical_origin(value, !local).map_err(|_| BootstrapError::Input)?;
    if value != origin {
        return Err(BootstrapError::Input);
    }
    Ok(origin)
}
impl OperatorInput {
    fn review(self, environment: &HashMap<String, String>, local: bool) -> Result<ReviewedInput> {
        let email = self.email.to_lowercase();
        let Some((name, domain)) = email.split_once('@') else {
            return Err(BootstrapError::Input);
        };
        if email.encode_utf16().count() > 254
            || name.is_empty()
            || domain.is_empty()
            || !domain
                .rsplit_once('.')
                .is_some_and(|(before, after)| !before.is_empty() && !after.is_empty())
            || domain.contains('@')
            || email.chars().any(|c| c.is_whitespace() || c < ' ')
            || self.name.trim().is_empty()
            || self.name.encode_utf16().count() > 160
            || self.name.chars().any(|c| c < ' ')
        {
            return Err(BootstrapError::Input);
        }
        let env_password = environment.get("SPITZLI_INITIAL_OPERATOR_PASSWORD");
        if self.password.is_some() && env_password.is_some() {
            return Err(BootstrapError::Input);
        }
        let password = self
            .password
            .or_else(|| env_password.cloned())
            .ok_or(BootstrapError::Input)?;
        if !(16..=128).contains(&password.encode_utf16().count())
            || password.chars().any(|c| c < ' ')
        {
            return Err(BootstrapError::Input);
        }
        let auth_origin = canonical(&self.auth_origin, local)?;
        let studio_origin = canonical(&self.studio_origin, local)?;
        let console_origin = canonical(&self.console_origin, local)?;
        if auth_origin == studio_origin
            || auth_origin == console_origin
            || studio_origin == console_origin
        {
            return Err(BootstrapError::Input);
        }
        let expected_auth = environment
            .get("SPITZLI_AUTH_ORIGIN")
            .or_else(|| environment.get("BETTER_AUTH_URL"));
        if expected_auth != Some(&auth_origin)
            || environment
                .get("BETTER_AUTH_URL")
                .is_some_and(|v| v != &auth_origin)
            || environment.get("SPITZLI_STUDIO_ORIGIN") != Some(&studio_origin)
            || environment.get("SPITZLI_CONSOLE_ORIGIN") != Some(&console_origin)
        {
            return Err(BootstrapError::Input);
        }
        Ok(ReviewedInput {
            email,
            name: self.name.trim().into(),
            password,
            auth_origin,
            studio_origin,
            console_origin,
            initial_mail_enabled: self.initial_mail_enabled,
        })
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Specification<'a> {
    version: u8,
    email: &'a str,
    name: &'a str,
    auth_origin: &'a str,
    studio_origin: &'a str,
    console_origin: &'a str,
    #[serde(rename = "redirectURI")]
    redirect_uri: String,
    #[serde(rename = "logoutURI")]
    logout_uri: String,
    scopes: [&'static str; 3],
    #[serde(rename = "requirePKCE")]
    require_pkce: bool,
    token_endpoint_auth_method: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    initial_mail_enabled: Option<bool>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapSummary {
    pub created: bool,
    #[serde(rename = "operatorID")]
    pub operator_id: String,
    #[serde(rename = "clientID")]
    pub client_id: String,
    pub checksum: String,
    pub security_setup_required: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Secrets {
    status: &'static str,
    #[serde(rename = "operatorID")]
    operator_id: String,
    #[serde(rename = "portalUserID")]
    portal_user_id: String,
    #[serde(rename = "bindingID")]
    binding_id: String,
    #[serde(rename = "clientID")]
    client_id: String,
    #[serde(rename = "signingKeyID")]
    signing_key_id: String,
    checksum: String,
    environment: BTreeMap<&'static str, String>,
}
struct DurableOutput {
    file: File,
    directory: File,
    name: std::ffi::OsString,
    retain: bool,
}
impl DurableOutput {
    fn create(path: &Path) -> Result<Self> {
        if !path.is_absolute() {
            return Err(BootstrapError::Output);
        }
        let parent = path.parent().ok_or(BootstrapError::Output)?;
        let name = path.file_name().ok_or(BootstrapError::Output)?.to_owned();
        let directory = File::from(
            rustix::fs::open(
                parent,
                rustix::fs::OFlags::RDONLY
                    | rustix::fs::OFlags::DIRECTORY
                    | rustix::fs::OFlags::CLOEXEC,
                rustix::fs::Mode::empty(),
            )
            .map_err(|_| BootstrapError::Output)?,
        );
        let file = File::from(
            rustix::fs::openat(
                &directory,
                &name,
                rustix::fs::OFlags::WRONLY
                    | rustix::fs::OFlags::CREATE
                    | rustix::fs::OFlags::EXCL
                    | rustix::fs::OFlags::NOFOLLOW
                    | rustix::fs::OFlags::CLOEXEC,
                rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
            )
            .map_err(|_| BootstrapError::Output)?,
        );
        Ok(Self {
            file,
            directory,
            name,
            retain: false,
        })
    }
    fn persist(&mut self, secrets: &Secrets) -> Result<()> {
        let meta = self.file.metadata().map_err(|_| BootstrapError::Output)?;
        if !meta.is_file()
            || meta.uid() != rustix::process::geteuid().as_raw()
            || meta.mode() & 0o077 != 0
            || meta.nlink() != 1
        {
            return Err(BootstrapError::Output);
        }
        serde_json::to_writer_pretty(&mut self.file, secrets)
            .map_err(|_| BootstrapError::Output)?;
        self.file
            .write_all(b"\n")
            .map_err(|_| BootstrapError::Output)?;
        self.file.sync_all().map_err(|_| BootstrapError::Output)?;
        self.directory
            .sync_all()
            .map_err(|_| BootstrapError::Output)
    }
}
impl Drop for DurableOutput {
    fn drop(&mut self) {
        if !self.retain {
            let _ = rustix::fs::unlinkat(&self.directory, &self.name, rustix::fs::AtFlags::empty());
            let _ = self.directory.sync_all();
        }
    }
}
async fn connect(raw: &str, ca: Option<&String>) -> Result<DatabaseConnection> {
    let mut options = ConnectOptions::new(raw);
    options
        .max_connections(1)
        .min_connections(1)
        .connect_timeout(Duration::from_secs(10))
        .acquire_timeout(Duration::from_secs(10))
        .sqlx_logging(false);
    if let Some(ca) = ca {
        if ca.is_empty() || ca.len() > 65536 || ca.contains("PRIVATE KEY") {
            return Err(BootstrapError::Input);
        }
        let ca = ca.as_bytes().to_vec();
        options.map_sqlx_postgres_opts(move |options| options.ssl_root_cert_from_pem(ca.clone()));
    }
    Database::connect(options)
        .await
        .map_err(|_| BootstrapError::Database)
}
async fn offline_role(db: &impl ConnectionTrait) -> Result<()> {
    let row=db.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT session_user::text AS session,current_user::text AS active,a.usename::text AS original,r.rolsuper,r.rolbypassrls FROM pg_catalog.pg_stat_activity a JOIN pg_catalog.pg_roles r ON r.rolname=a.usename WHERE a.pid=pg_catalog.pg_backend_pid()")).await.map_err(|_|BootstrapError::Role)?.ok_or(BootstrapError::Role)?;
    for field in ["session", "active", "original"] {
        if row
            .try_get::<String>("", field)
            .map_err(|_| BootstrapError::Role)?
            != "webdock_offline_migrate"
        {
            return Err(BootstrapError::Role);
        }
    }
    if row
        .try_get::<bool>("", "rolsuper")
        .map_err(|_| BootstrapError::Role)?
        || row
            .try_get::<bool>("", "rolbypassrls")
            .map_err(|_| BootstrapError::Role)?
    {
        return Err(BootstrapError::Role);
    }
    Ok(())
}
async fn execute(tx: &DatabaseTransaction, query: &str, values: Vec<sea_orm::Value>) -> Result<()> {
    if values.is_empty() {
        tx.execute_unprepared(query)
            .await
            .map_err(|_| BootstrapError::Database)?;
        return Ok(());
    }
    tx.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        query,
        values,
    ))
    .await
    .map_err(|_| BootstrapError::Database)?;
    Ok(())
}
async fn exists(
    tx: &DatabaseTransaction,
    query: &str,
    values: Vec<sea_orm::Value>,
) -> Result<bool> {
    Ok(tx
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            query,
            values,
        ))
        .await
        .map_err(|_| BootstrapError::Database)?
        .is_some())
}
struct Allocated {
    user: String,
    account: String,
    portal: String,
    client: String,
    client_row: String,
    binding: String,
    key: String,
    operator_audit: String,
    client_audit: String,
}
impl Allocated {
    async fn new(ids: &IdAllocator) -> Result<Self> {
        let mut next = Vec::new();
        for _ in 0..9 {
            next.push(
                ids.allocate()
                    .await
                    .map_err(|_| BootstrapError::Database)?
                    .to_string(),
            );
        }
        let [
            user,
            account,
            portal,
            client,
            client_row,
            binding,
            key,
            operator_audit,
            client_audit,
        ]: [String; 9] = next.try_into().map_err(|_| BootstrapError::Database)?;
        Ok(Self {
            user,
            account,
            portal,
            client,
            client_row,
            binding,
            key,
            operator_audit,
            client_audit,
        })
    }
}
fn specification(input: &ReviewedInput) -> Specification<'_> {
    Specification {
        version: 1,
        email: &input.email,
        name: &input.name,
        auth_origin: &input.auth_origin,
        studio_origin: &input.studio_origin,
        console_origin: &input.console_origin,
        redirect_uri: format!("{}/api/sso/callback", input.studio_origin),
        logout_uri: format!("{}/login", input.studio_origin),
        scopes: ["openid", "profile", "email"],
        require_pkce: true,
        token_endpoint_auth_method: "client_secret_post",
        initial_mail_enabled: input.initial_mail_enabled,
    }
}
fn token() -> Result<String> {
    let mut raw = [0u8; 48];
    getrandom::getrandom(&mut raw).map_err(|_| BootstrapError::Database)?;
    Ok(URL_SAFE_NO_PAD.encode(raw))
}
async fn provision(
    tx: &DatabaseTransaction,
    input: ReviewedInput,
    ids: &IdAllocator,
    secret: &str,
) -> Result<Secrets> {
    execute(tx,"LOCK TABLE webdock_auth.\"user\",webdock_auth.account,webdock_auth.\"oauthClient\",webdock_auth.app_binding,webdock_admin.users,webdock_auth.jwks,webdock_auth.platform_settings,webdock_auth.access_event IN SHARE ROW EXCLUSIVE MODE",vec![]).await?;
    if exists(tx,"SELECT 1 FROM webdock_auth.\"user\" WHERE role='operator' UNION ALL SELECT 1 FROM webdock_admin.users WHERE role='operator' UNION ALL SELECT 1 FROM webdock_auth.access_event WHERE action IN ('operator.bootstrap','console.bootstrap') LIMIT 1",vec![]).await?||exists(tx,"SELECT 1 FROM webdock_auth.\"user\" WHERE lower(email)=lower($1) UNION ALL SELECT 1 FROM webdock_admin.users WHERE lower(email)=lower($1) LIMIT 1",vec![input.email.clone().into()]).await?||exists(tx,"SELECT 1 FROM webdock_auth.\"oauthClient\" UNION ALL SELECT 1 FROM webdock_auth.app_binding UNION ALL SELECT 1 FROM webdock_auth.jwks LIMIT 1",vec![]).await? {return Err(BootstrapError::Existing);}
    if let Some(enabled) = input.initial_mail_enabled
        && exists(tx,"SELECT 1 FROM webdock_auth.platform_settings WHERE id=true AND settings ? 'mailEnabled' AND settings->'mailEnabled' IS DISTINCT FROM $1::jsonb",vec![serde_json::json!(enabled).into()]).await?
    {
        return Err(BootstrapError::Existing);
    }
    let ids = Allocated::new(ids).await?;
    let specification = specification(&input);
    let checksum = hex::encode(Sha256::digest(
        serde_json::to_vec(&specification).map_err(|_| BootstrapError::Input)?,
    ));
    let mut metadata = serde_json::to_value(&specification).map_err(|_| BootstrapError::Input)?;
    metadata["checksum"] = serde_json::json!(checksum);
    metadata["operatorID"] = serde_json::json!(ids.user);
    let metadata =
        serde_json::json!({"webdock_binding":ids.binding,"webdock_initial_setup":metadata});
    let password = input.password;
    let hash = tokio::task::spawn_blocking(move || spitzli_identity::hash_password(&password))
        .await
        .map_err(|_| BootstrapError::Database)?
        .map_err(|_| BootstrapError::Input)?;
    let client_secret = token()?;
    execute(tx,"INSERT INTO webdock_auth.\"user\"(id,name,email,\"emailVerified\",role,\"twoFactorEnabled\",\"mustChangePassword\") VALUES($1,$2,$3,true,'operator',false,true)",vec![ids.user.clone().into(),input.name.clone().into(),input.email.clone().into()]).await?;
    execute(tx,"INSERT INTO webdock_auth.account(id,\"accountId\",\"providerId\",\"userId\",password,\"updatedAt\") VALUES($1,$2,'credential',$2,$3,now())",vec![ids.account.into(),ids.user.clone().into(),hash.into()]).await?;
    execute(tx,"INSERT INTO webdock_admin.users(id,name,email,role,auth_subject) VALUES($1,$2,$3,'operator',$4)",vec![ids.portal.clone().into(),input.name.clone().into(),input.email.clone().into(),ids.user.clone().into()]).await?;
    // The native Console and retained Studio protocol origin are both explicitly reviewed inputs.
    let redirects = serde_json::json!([
        format!("{}/api/sso/callback", input.studio_origin),
        format!("{}/api/sso/callback", input.console_origin)
    ]);
    let logouts = serde_json::json!([
        format!("{}/login", input.studio_origin),
        format!("{}/login", input.console_origin)
    ]);
    execute(tx,"INSERT INTO webdock_auth.\"oauthClient\"(id,\"clientId\",\"clientSecret\",disabled,\"skipConsent\",\"enableEndSession\",scopes,\"clientCredentialsScopes\",\"userId\",\"createdAt\",\"updatedAt\",name,\"redirectUris\",\"postLogoutRedirectUris\",\"tokenEndpointAuthMethod\",\"applicationType\",\"grantTypes\",\"responseTypes\",\"requirePKCE\",\"subjectType\",metadata) VALUES($1,$2,$3,false,true,true,'[\"openid\",\"profile\",\"email\"]'::jsonb,'[]'::jsonb,$4,now(),now(),'Spitzli Cloud Console',$5,$6,'client_secret_post',$7,'[\"authorization_code\"]'::jsonb,'[\"code\"]'::jsonb,true,'public',$8)",vec![ids.client_row.into(),ids.client.clone().into(),spitzli_oauth::legacy_token_hash(&client_secret).into(),ids.user.clone().into(),redirects.into(),logouts.into(),if input.studio_origin.starts_with("http:"){"native"}else{"web"}.into(),metadata.into()]).await?;
    execute(tx,"INSERT INTO webdock_auth.app_binding(id,client_id,label,organization_id,enabled) VALUES($1,$2,'Spitzli Cloud Console',NULL,true)",vec![ids.binding.clone().into(),ids.client.clone().into()]).await?;
    let mut defaults = serde_json::json!({"supportEmail":input.email,"cmsEnabled":false});
    if let Some(enabled) = input.initial_mail_enabled {
        defaults["mailEnabled"] = serde_json::json!(enabled);
    }
    execute(tx,"INSERT INTO webdock_auth.platform_settings(id,settings) VALUES(true,$1) ON CONFLICT(id) DO UPDATE SET settings=EXCLUDED.settings || webdock_auth.platform_settings.settings",vec![defaults.into()]).await?;
    execute(tx,"INSERT INTO webdock_auth.access_event(id,actor_id,action,target_id,outcome) VALUES($1,$3,'operator.bootstrap',$3,'succeeded'),($2,$3,'console.bootstrap',$4,'succeeded')",vec![ids.operator_audit.into(),ids.client_audit.into(),ids.user.clone().into(),ids.client.clone().into()]).await?;
    let keys = spitzli_identity::SecretKeys::new(
        BTreeMap::from([(1, secret.into())]),
        1,
        Some(secret.into()),
    )
    .map_err(|_| BootstrapError::Input)?;
    spitzli_oauth::stage_signing_key(tx, &ids.key, &keys)
        .await
        .map_err(|_| BootstrapError::Database)?;
    let environment = BTreeMap::from([
        ("BETTER_AUTH_URL", input.auth_origin.clone()),
        ("SPITZLI_AUTH_ORIGIN", input.auth_origin.clone()),
        (
            "SPITZLI_AUTH_ISSUER",
            format!("{}/api/auth", input.auth_origin),
        ),
        ("SPITZLI_STUDIO_ORIGIN", input.studio_origin.clone()),
        ("SPITZLI_CONSOLE_ORIGIN", input.console_origin),
        ("SPITZLI_SSO_APP_ORIGIN", input.studio_origin),
        ("SPITZLI_SSO_CLIENT_ID", ids.client.clone()),
        ("SPITZLI_STUDIO_CLIENT_ID", ids.client.clone()),
        ("SPITZLI_SSO_CLIENT_SECRET", client_secret),
        ("SPITZLI_SSO_COOKIE_SECRET", token()?),
    ]);
    Ok(Secrets {
        status: "verify-command-result",
        operator_id: ids.user,
        portal_user_id: ids.portal,
        binding_id: ids.binding,
        client_id: ids.client,
        signing_key_id: ids.key,
        checksum,
        environment,
    })
}
/// An exclusive output is created before the transaction; only a confirmed commit reports success.
pub async fn bootstrap_operator(
    environment: &HashMap<String, String>,
    target: &str,
    input_file: &Path,
    destination: &Path,
) -> Result<BootstrapSummary> {
    let raw = environment
        .get("DATABASE_URL_UNPOOLED")
        .ok_or(BootstrapError::Target)?;
    let connection = ConnectionUrl::parse(raw, false).map_err(|_| BootstrapError::Target)?;
    connection
        .require_target(target)
        .map_err(|_| BootstrapError::Target)?;
    let url = url::Url::parse(connection.as_str()).map_err(|_| BootstrapError::Target)?;
    if percent_encoding::percent_decode_str(url.username())
        .decode_utf8()
        .map_err(|_| BootstrapError::Role)?
        != "webdock_offline_migrate"
    {
        return Err(BootstrapError::Role);
    }
    let local = matches!(url.host_str(), Some("127.0.0.1" | "localhost"));
    let ca = environment.get("DATABASE_CA_CERT");
    if !local && ca.is_none() {
        return Err(BootstrapError::Input);
    }
    let bytes = crate::config::read_private_file(input_file).map_err(|_| BootstrapError::Input)?;
    if bytes.len() > 16384 {
        return Err(BootstrapError::Input);
    }
    let input: OperatorInput = serde_json::from_slice(&bytes).map_err(|_| BootstrapError::Input)?;
    let input = input.review(environment, local)?;
    let secret = environment
        .get("SPITZLI_AUTH_SECRET")
        .ok_or(BootstrapError::Input)?;
    if secret.len() < 32 {
        return Err(BootstrapError::Input);
    }
    let mut output = DurableOutput::create(destination)?;
    let db = connect(connection.as_str(), ca).await?;
    offline_role(&db).await?;
    let allocation = connect(connection.as_str(), ca).await?;
    offline_role(&allocation).await?;
    allocation
        .execute_unprepared("SET ROLE webdock_platform_owner")
        .await
        .map_err(|_| BootstrapError::Role)?;
    let ids = IdAllocator::new(
        allocation.clone(),
        spitzli_ids::Identity::platform(0, 2).map_err(|_| BootstrapError::Input)?,
    );
    let tx = db.begin().await.map_err(|_| BootstrapError::Database)?;
    execute(
        &tx,
        "SET LOCAL lock_timeout='5s'; SET LOCAL statement_timeout='30s'",
        vec![],
    )
    .await?;
    execute(
        &tx,
        "SELECT pg_advisory_xact_lock(hashtext('webdock-api-migration'))",
        vec![],
    )
    .await?;
    offline_role(&tx).await?;
    execute(&tx, "SET LOCAL ROLE webdock_platform_owner", vec![]).await?;
    let database: String = tx
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT current_database()::text AS name",
        ))
        .await
        .map_err(|_| BootstrapError::Database)?
        .ok_or(BootstrapError::Target)?
        .try_get("", "name")
        .map_err(|_| BootstrapError::Target)?;
    if url.path().strip_prefix('/') != Some(&database) {
        return Err(BootstrapError::Target);
    }
    spitzli_persistence::migrations::verify_bootstrap_baseline(&tx)
        .await
        .map_err(|_| BootstrapError::History)?;
    if !spitzli_persistence::migrations::native_schema_ready(&tx).await {
        return Err(BootstrapError::History);
    }
    let secrets = provision(&tx, input, &ids, secret).await?;
    output.persist(&secrets)?;
    output.retain = true;
    tx.commit()
        .await
        .map_err(|_| BootstrapError::CommitUncertain)?;
    let summary = BootstrapSummary {
        created: true,
        operator_id: secrets.operator_id,
        client_id: secrets.client_id,
        checksum: secrets.checksum,
        security_setup_required: true,
    };
    drop(ids);
    let _ = allocation.close().await;
    let _ = db.close().await;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn secret_output() -> Secrets {
        Secrets {
            status: "verify-command-result",
            operator_id: "123".into(),
            portal_user_id: "124".into(),
            binding_id: "125".into(),
            client_id: "126".into(),
            signing_key_id: "127".into(),
            checksum: "a".repeat(64),
            environment: BTreeMap::from([(
                "SPITZLI_SSO_CLIENT_SECRET",
                "owned-fixture-secret".into(),
            )]),
        }
    }
    #[test]
    fn reviewed_initial_mail_opt_in_is_boolean_and_absence_stays_compatible() {
        let base = serde_json::json!({"email":"operator@example.test","name":"Operator",
            "password":"owned-initial-password-123","authOrigin":"https://auth.example.test",
            "studioOrigin":"https://studio.example.test","consoleOrigin":"https://console.example.test"});
        let environment = HashMap::from([
            ("BETTER_AUTH_URL".into(), "https://auth.example.test".into()),
            (
                "SPITZLI_STUDIO_ORIGIN".into(),
                "https://studio.example.test".into(),
            ),
            (
                "SPITZLI_CONSOLE_ORIGIN".into(),
                "https://console.example.test".into(),
            ),
        ]);
        let reviewed = serde_json::from_value::<OperatorInput>(base.clone())
            .unwrap()
            .review(&environment, false)
            .unwrap();
        let prior = serde_json::to_string(&specification(&reviewed)).unwrap();
        assert_eq!(
            prior,
            r#"{"version":1,"email":"operator@example.test","name":"Operator","authOrigin":"https://auth.example.test","studioOrigin":"https://studio.example.test","consoleOrigin":"https://console.example.test","redirectURI":"https://studio.example.test/api/sso/callback","logoutURI":"https://studio.example.test/login","scopes":["openid","profile","email"],"requirePKCE":true,"tokenEndpointAuthMethod":"client_secret_post"}"#
        );
        for enabled in [false, true] {
            let mut body = base.clone();
            body["initialMailEnabled"] = serde_json::json!(enabled);
            let reviewed = serde_json::from_value::<OperatorInput>(body)
                .unwrap()
                .review(&environment, false)
                .unwrap();
            let next = serde_json::to_string(&specification(&reviewed)).unwrap();
            assert_ne!(
                Sha256::digest(prior.as_bytes()),
                Sha256::digest(next.as_bytes())
            );
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&next).unwrap()["initialMailEnabled"],
                enabled
            );
        }
        let mut enabled = base.clone();
        enabled["initialMailEnabled"] = serde_json::json!(true);
        assert!(serde_json::from_value::<OperatorInput>(enabled).is_ok());
        for invalid in [
            serde_json::Value::Null,
            serde_json::json!("true"),
            serde_json::json!(1),
        ] {
            let mut body = base.clone();
            body["initialMailEnabled"] = invalid;
            assert!(serde_json::from_value::<OperatorInput>(body).is_err());
        }
    }

    #[test]
    fn output_is_exclusive_private_and_removed_only_before_commit_begins() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("output.json");
        let mut output = DurableOutput::create(&path).unwrap();
        output.persist(&secret_output()).unwrap();
        assert_eq!(path.metadata().unwrap().mode() & 0o777, 0o600);
        assert!(DurableOutput::create(&path).is_err());
        drop(output);
        assert!(!path.exists());
        let mut output = DurableOutput::create(&path).unwrap();
        output.persist(&secret_output()).unwrap();
        output.retain = true;
        drop(output);
        assert!(path.exists());
        let target = dir.path().join("link.json");
        std::os::unix::fs::symlink(&path, &target).unwrap();
        assert!(DurableOutput::create(&target).is_err());
        assert!(path.exists());
    }
    #[test]
    fn actual_output_write_failure_removes_precommit_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("failed.json");
        let mut output = DurableOutput::create(&path).unwrap();
        output.file = File::open(&path).unwrap();
        assert!(matches!(
            output.persist(&secret_output()),
            Err(BootstrapError::Output)
        ));
        drop(output);
        assert!(!path.exists());
    }
}
