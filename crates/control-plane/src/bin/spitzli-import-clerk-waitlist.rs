//! Explicit offline import of reviewed legacy pending contacts; never creates accounts or sends mail.
use clap::Parser;
use sea_orm::{ConnectionTrait, Database, DbBackend, Statement, TransactionTrait};
use serde::Deserialize;
use spitzli_persistence::connection::ConnectionUrl;
#[derive(Parser)]
struct Args {
    #[arg(long)]
    target: String,
    #[arg(long)]
    input: std::path::PathBuf,
    #[arg(long)]
    expected_records: usize,
    #[arg(long)]
    source_instance: String,
}
#[derive(Deserialize)]
struct Export {
    data: Vec<Entry>,
    total_count: usize,
}
#[derive(Deserialize)]
struct Entry {
    email_address: String,
    status: String,
    created_at: i64,
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    if args.source_instance != "ins_3Klz8x6R4XYa8w6QfjPQl95EKZf" || args.expected_records > 1000 {
        return Err("Explicit reviewed Webdock production waitlist source required".into());
    }
    let bytes = spitzli_control_plane::config::read_private_file(&args.input)?;
    let export: Export = serde_json::from_slice(&bytes)?;
    if export.total_count != args.expected_records || export.data.len() != args.expected_records {
        return Err("Incomplete or changed source inventory; inspect before importing".into());
    }
    let raw = std::env::var("WEBDOCK_MIGRATION_DATABASE_URL")?;
    let target = ConnectionUrl::parse(&raw, true)?;
    target.require_target(&args.target)?;
    let raw = std::env::var("WEBDOCK_DATABASE_URL")?;
    let bootstrap = ConnectionUrl::parse(&raw, true)?;
    bootstrap.require_target(&args.target)?;
    let db = Database::connect(target.as_str()).await?;
    let allocator = spitzli_persistence::IdAllocator::connect(
        bootstrap.as_str(),
        spitzli_ids::Identity::platform(0, 2)?,
    )
    .await?;
    let tx = db.begin().await?;
    let mut imported = 0;
    for entry in export.data {
        let email = entry.email_address.trim().to_lowercase();
        if entry.status != "pending"
            || email.len() > 254
            || !email.contains('@')
            || email.chars().any(char::is_control)
            || entry.created_at <= 0
        {
            return Err("Unsupported legacy contact; inspect without changing its status".into());
        }
        let id = allocator.allocate().await?.to_string();
        let result=tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.waitlist_entry(id,email,privacy_version,created_at) VALUES($1,$2,'legacy-clerk',to_timestamp($3::double precision/1000)) ON CONFLICT(email) DO NOTHING",[id.into(),email.into(),entry.created_at.into()])).await?;
        imported += result.rows_affected();
    }
    tx.commit().await?;
    db.close().await?;
    println!(
        "Imported {imported} reviewed pending legacy contacts; existing decisions preserved; no account or email created."
    );
    Ok(())
}
