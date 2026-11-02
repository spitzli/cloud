//! Exact-target offline prerequisites. Neither ordinary Core startup nor HTTP invokes these.
mod agent;
mod files;
pub use agent::enroll_agent;
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DatabaseTransaction, DbBackend,
    Statement, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};
use webdock_persistence::connection::ConnectionUrl;
#[derive(Debug, thiserror::Error)]
pub enum SetupError {
    #[error("Invalid private plugin setup input")]
    Input,
    #[error("Exact offline target required")]
    Target,
    #[error("Original restricted offline migrator login required")]
    Role,
    #[error("Verified exact native history required")]
    History,
    #[error("Existing setup differs or is incomplete")]
    Existing,
    #[error("Owned durable private output required")]
    Output,
    #[error("Offline plugin setup unavailable")]
    Database,
    #[error("Commit outcome unknown; retain private output")]
    CommitUncertain,
}
type Result<T> = std::result::Result<T, SetupError>;
fn sha(bytes: impl AsRef<[u8]>) -> String {
    hex::encode(Sha256::digest(bytes.as_ref()))
}
fn slug(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
fn encode(value: &impl Serialize) -> Result<Vec<u8>> {
    serde_json::to_vec_pretty(value).map_err(|_| SetupError::Input)
}
fn input<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    serde_json::from_value(
        webdock_plugin_protocol::release::strict_json(&files::read(path, 16384)?)
            .map_err(|_| SetupError::Input)?,
    )
    .map_err(|_| SetupError::Input)
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
            return Err(SetupError::Input);
        }
        let ca = ca.as_bytes().to_vec();
        options.map_sqlx_postgres_opts(move |opts| opts.ssl_root_cert_from_pem(ca.clone()));
    }
    Database::connect(options)
        .await
        .map_err(|_| SetupError::Database)
}
async fn offline_role(db: &impl ConnectionTrait) -> Result<()> {
    let row=db.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT session_user::text AS session,current_user::text AS active,a.usename::text AS original,r.rolsuper,r.rolbypassrls,r.rolcreatedb,r.rolcreaterole,r.rolreplication FROM pg_catalog.pg_stat_activity a JOIN pg_catalog.pg_roles r ON r.rolname=a.usename WHERE a.pid=pg_catalog.pg_backend_pid()")).await.map_err(|_|SetupError::Role)?.ok_or(SetupError::Role)?;
    for field in ["session", "active", "original"] {
        if row
            .try_get::<String>("", field)
            .map_err(|_| SetupError::Role)?
            != "webdock_offline_migrate"
        {
            return Err(SetupError::Role);
        }
    }
    for field in [
        "rolsuper",
        "rolbypassrls",
        "rolcreatedb",
        "rolcreaterole",
        "rolreplication",
    ] {
        if row
            .try_get::<bool>("", field)
            .map_err(|_| SetupError::Role)?
        {
            return Err(SetupError::Role);
        }
    }
    Ok(())
}
struct Offline {
    db: DatabaseConnection,
    tx: DatabaseTransaction,
    raw: String,
    ca: Option<String>,
}
impl Offline {
    async fn open(environment: &HashMap<String, String>, target: &str) -> Result<Self> {
        let raw = environment
            .get("DATABASE_URL_UNPOOLED")
            .ok_or(SetupError::Target)?;
        let connection = ConnectionUrl::parse(raw, false).map_err(|_| SetupError::Target)?;
        connection
            .require_target(target)
            .map_err(|_| SetupError::Target)?;
        let url = url::Url::parse(connection.as_str()).map_err(|_| SetupError::Target)?;
        if percent_encoding::percent_decode_str(url.username())
            .decode_utf8()
            .map_err(|_| SetupError::Role)?
            != "webdock_offline_migrate"
        {
            return Err(SetupError::Role);
        }
        let ca = environment.get("DATABASE_CA_CERT").cloned();
        let local = url.host_str().is_some_and(|h| {
            h == "localhost"
                || h.parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        });
        if !local && ca.is_none() {
            return Err(SetupError::Input);
        }
        let db = connect(connection.as_str(), ca.as_ref()).await?;
        offline_role(&db).await?;
        let tx = db.begin().await.map_err(|_| SetupError::Database)?;
        tx.execute_unprepared("SET LOCAL lock_timeout='5s'; SET LOCAL statement_timeout='30s'")
            .await
            .map_err(|_| SetupError::Database)?;
        tx.execute_unprepared(
            "SELECT pg_catalog.pg_advisory_xact_lock(pg_catalog.hashtext('webdock-api-migration'))",
        )
        .await
        .map_err(|_| SetupError::Database)?;
        offline_role(&tx).await?;
        tx.execute_unprepared("SET LOCAL ROLE webdock_platform_owner")
            .await
            .map_err(|_| SetupError::Role)?;
        let actual = tx
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT current_database()::text AS name",
            ))
            .await
            .map_err(|_| SetupError::Database)?
            .ok_or(SetupError::Target)?
            .try_get::<String>("", "name")
            .map_err(|_| SetupError::Target)?;
        if url.path().strip_prefix('/') != Some(actual.as_str()) {
            return Err(SetupError::Target);
        }
        webdock_persistence::migrations::verify_bootstrap_baseline(&tx)
            .await
            .map_err(|_| SetupError::History)?;
        if !webdock_persistence::migrations::native_schema_ready(&tx).await {
            return Err(SetupError::History);
        }
        Ok(Self {
            db,
            tx,
            raw: connection.as_str().into(),
            ca,
        })
    }
    async fn allocate(&self) -> Result<String> {
        let db = connect(&self.raw, self.ca.as_ref()).await?;
        offline_role(&db).await?;
        db.execute_unprepared("SET ROLE webdock_platform_owner")
            .await
            .map_err(|_| SetupError::Role)?;
        let ids = webdock_persistence::IdAllocator::new(
            db.clone(),
            webdock_ids::Identity::platform(0, 2).map_err(|_| SetupError::Input)?,
        );
        let id = ids
            .allocate()
            .await
            .map_err(|_| SetupError::Database)?
            .to_string();
        drop(ids);
        db.close().await.map_err(|_| SetupError::Database)?;
        Ok(id)
    }
    async fn finish(self) -> Result<()> {
        self.tx
            .commit()
            .await
            .map_err(|_| SetupError::CommitUncertain)?;
        self.db.close().await.map_err(|_| SetupError::Database)
    }
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct KernelInput {
    #[serde(rename = "platformID")]
    platform_id: String,
    installation_catalog: PathBuf,
    callback_origin: String,
    listen_address: SocketAddr,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct KernelRecord {
    version: u8,
    #[serde(rename = "platformID")]
    platform_id: String,
    checksum: String,
    core_certificate_sha256: String,
    files: BTreeMap<String, String>,
}
const KERNEL_FILES: [&str; 8] = [
    "plugin-config.json",
    "invocation.key",
    "host-bindings.json",
    "connections.json",
    "ca.crt",
    "ca.key",
    "core.crt",
    "core.key",
];
/// Generates only global kernel material. Catalog releases and all trust keys remain operator inputs.
/// A new output directory is published atomically; exact existing output is verified, never replaced.
pub async fn bootstrap_plugins(
    environment: &HashMap<String, String>,
    target: &str,
    input_file: &Path,
    destination: &Path,
) -> Result<()> {
    let offline = Offline::open(environment, target).await?;
    let input: KernelInput = input(input_file)?;
    if !slug(&input.platform_id, 24) || !input.installation_catalog.is_absolute() {
        return Err(SetupError::Input);
    }
    let origin = url::Url::parse(&input.callback_origin).map_err(|_| SetupError::Input)?;
    if origin.scheme() != "https"
        || !origin.username().is_empty()
        || origin.password().is_some()
        || origin.query().is_some()
        || origin.fragment().is_some()
        || origin.path() != "/"
        || input.callback_origin.ends_with('/')
    {
        return Err(SetupError::Input);
    }
    let domain = origin.host_str().ok_or(SetupError::Input)?;
    let raw_catalog = files::read(&input.installation_catalog, 8 * 1024 * 1024)?;
    let value = webdock_plugin_protocol::release::strict_json(&raw_catalog)
        .map_err(|_| SetupError::Input)?;
    if value["platformID"] != input.platform_id {
        return Err(SetupError::Input);
    }
    webdock_plugin_host::catalog::Catalog::from_bytes(&raw_catalog)
        .map_err(|_| SetupError::Input)?;
    files::parent(destination)?;
    let checksum = sha(encode(
        &json!({"target":target,"destination":destination,"input":input,"catalogSha256":sha(&raw_catalog)}),
    )?);
    if destination.try_exists().map_err(|_| SetupError::Output)? {
        files::directory(destination)?;
        let record: KernelRecord = serde_json::from_value(
            webdock_plugin_protocol::release::strict_json(&files::read(
                &destination.join("bootstrap.json"),
                16384,
            )?)
            .map_err(|_| SetupError::Existing)?,
        )
        .map_err(|_| SetupError::Existing)?;
        if record.version != 1
            || record.platform_id != input.platform_id
            || record.checksum != checksum
            || record.files.len() != KERNEL_FILES.len()
        {
            return Err(SetupError::Existing);
        }
        let entries = std::fs::read_dir(destination)
            .map_err(|_| SetupError::Output)?
            .count();
        if entries != KERNEL_FILES.len() + 1 {
            return Err(SetupError::Existing);
        }
        for file in KERNEL_FILES {
            if record.files.get(file) != Some(&sha(files::read(&destination.join(file), 65536)?)) {
                return Err(SetupError::Existing);
            }
        }
        return offline.finish().await;
    }
    let key = rcgen::KeyPair::generate().map_err(|_| SetupError::Database)?;
    let mut ca =
        rcgen::CertificateParams::new(Vec::<String>::new()).map_err(|_| SetupError::Input)?;
    ca.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca.key_usages = vec![
        rcgen::KeyUsagePurpose::KeyCertSign,
        rcgen::KeyUsagePurpose::CrlSign,
    ];
    let now = x509_parser::time::ASN1Time::now().to_datetime();
    ca.not_before = now - Duration::from_secs(60);
    ca.not_after = now + Duration::from_secs(3650 * 24 * 60 * 60);
    let ca_pem = ca
        .self_signed(&key)
        .map_err(|_| SetupError::Database)?
        .pem();
    let ca_key = key.serialize_pem();
    let core =
        webdock_plugin_host::provisioning::tls::issue_runtime_identity(&ca_pem, &ca_key, domain)
            .map_err(|_| SetupError::Input)?;
    let mut invocation = [0u8; 32];
    getrandom::getrandom(&mut invocation).map_err(|_| SetupError::Database)?;
    let configuration = json!({"hostBindings":destination.join("host-bindings.json"),"invocationSigningKey":destination.join("invocation.key"),"connections":destination.join("connections.json"),"installationCatalog":input.installation_catalog,"control":{"kind":"mutualTls","address":input.listen_address,"certificate":destination.join("core.crt"),"privateKey":destination.join("core.key"),"authority":destination.join("ca.crt")}});
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::from([
        ("plugin-config.json".into(), encode(&configuration)?),
        (
            "host-bindings.json".into(),
            encode(&json!({"runtimes":{}}))?,
        ),
        ("connections.json".into(), encode(&json!({"runtimes":{}}))?),
        (
            "invocation.key".into(),
            hex::encode(invocation).into_bytes(),
        ),
        ("ca.crt".into(), ca_pem.into_bytes()),
        ("ca.key".into(), ca_key.into_bytes()),
        ("core.crt".into(), core.certificate_pem.into_bytes()),
        ("core.key".into(), core.private_key_pem.into_bytes()),
    ]);
    let record = KernelRecord {
        version: 1,
        platform_id: input.platform_id,
        checksum,
        core_certificate_sha256: core.certificate_sha256,
        files: files
            .iter()
            .map(|(name, bytes)| (name.clone(), sha(bytes)))
            .collect(),
    };
    files.insert("bootstrap.json".into(), encode(&record)?);
    files::publish(destination, &files)?;
    offline.finish().await
}
