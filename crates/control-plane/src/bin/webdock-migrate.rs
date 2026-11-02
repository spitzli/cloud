use clap::Parser;
use sea_orm::{ConnectOptions, Database};
use webdock_persistence::migrations::{self, MigrationError};

#[derive(Parser)]
#[command(about = "Explicit offline Webdock schema setup/upgrade; runtime never migrates")]
struct Args {
    /// Exact host:port/database copied from the reviewed target (no credentials).
    #[arg(long)]
    target: String,
    /// Initialize only an empty dedicated database with pre-provisioned restricted roles.
    #[arg(long)]
    initialize_empty: bool,
    /// Provision missing restricted roles from a private JSON password map on an empty target.
    /// Existing passwords/privileges are never changed. Requires a superuser connection.
    #[arg(long, requires = "initialize_empty")]
    provision_roles_from: Option<std::path::PathBuf>,
}

#[tokio::main]
async fn main() -> Result<(), MigrationError> {
    let args = Args::parse();
    let raw =
        std::env::var("WEBDOCK_MIGRATION_DATABASE_URL").map_err(|_| MigrationError::Target)?;
    let connection = webdock_persistence::connection::ConnectionUrl::parse(&raw, false)
        .map_err(|_| MigrationError::Target)?;
    connection
        .require_target(&args.target)
        .map_err(|_| MigrationError::Target)?;
    let url = url::Url::parse(connection.as_str()).map_err(|_| MigrationError::Target)?;
    let database = url.path().strip_prefix('/').ok_or(MigrationError::Target)?;
    let mut options = ConnectOptions::new(connection.as_str());
    options
        .max_connections(1)
        .min_connections(0)
        .sqlx_logging(false);
    let db = Database::connect(options)
        .await
        .map_err(|_| MigrationError::Database)?;
    if args.initialize_empty {
        if let Some(path) = args.provision_roles_from {
            let bytes = webdock_control_plane::config::read_private_file(&path)
                .map_err(|_| MigrationError::BootstrapPrerequisite)?;
            let passwords = serde_json::from_slice(&bytes)
                .map_err(|_| MigrationError::BootstrapPrerequisite)?;
            webdock_persistence::roles::provision_roles(&db, database, &passwords).await?;
        }
        let secret = std::env::var("DATABASE_RLS_SECRET")
            .map_err(|_| MigrationError::BootstrapPrerequisite)?;
        migrations::bootstrap(&db, database, &secret).await?;
    } else {
        migrations::upgrade(&db, database).await?;
    }
    db.close().await.map_err(|_| MigrationError::Database)?;
    println!("Offline migration completed for the explicitly verified target.");
    Ok(())
}
