//! Core's separately supervised agent RPC listener; local sockets by default, explicit mTLS remotely.
use clap::Parser;
use serde::Deserialize;
use std::{path::PathBuf, sync::Arc};
use webdock_agent_control::{ControlPlane, LocalBinding};
use webdock_core::{config::PlatformConfig, features::OperationKind};
use webdock_plugin_runtime::{MutualTls, UnixSocket};
type Error = Box<dyn std::error::Error + Send + Sync>;
#[derive(Parser)]
struct Args {
    #[arg(long)]
    config: PathBuf,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ListenerConfig {
    Unix {
        path: PathBuf,
        installation: String,
        agent_id: String,
        uid: u32,
    },
    Mtls {
        address: std::net::SocketAddr,
        certificate: PathBuf,
        key: PathBuf,
        peer_ca: PathBuf,
    },
}
fn required(key: &str) -> Result<String, Error> {
    std::env::var(key).map_err(|_| format!("Missing {key}").into())
}
#[tokio::main]
async fn main() -> Result<(), Error> {
    let args = Args::parse();
    let listener: ListenerConfig = serde_json::from_slice(
        &webdock_control_plane::config::read_private_file(&args.config)?,
    )?;
    let environment = std::env::vars().collect();
    let configuration_enabled =
        webdock_control_plane::mail::incoming_configuration_enabled(&environment)?;
    let config = PlatformConfig::from_map(&environment)?;
    let plugin_config = std::env::var("WEBDOCK_NATIVE_PLUGIN_CONFIG")
        .ok()
        .map(|path| {
            webdock_control_plane::plugin_config::PluginConfig::load(std::path::Path::new(&path))
        })
        .transpose()?;
    let plugin_installations_enabled = plugin_config
        .as_ref()
        .is_some_and(|config| config.installation_catalog.is_some());
    let hosting_enabled = config
        .features
        .require("hosting", OperationKind::ContinueAccepted)
        .is_ok();
    let databases_enabled = config
        .features
        .require("databases", OperationKind::ContinueAccepted)
        .is_ok();
    let mail_enabled = config
        .features
        .require("mail", OperationKind::ContinueAccepted)
        .is_ok()
        && std::env::var("WEBDOCK_NATIVE_MAIL_AGENT_ENABLED").as_deref() == Ok("true");
    if configuration_enabled && !mail_enabled {
        return Err(
            "Mail configuration execution requires a Mail feature state permitting accepted work"
                .into(),
        );
    }
    // Plugin workloads are served whenever this listener runs; they need no product feature.
    let workloads_enabled = std::env::var("WEBDOCK_WORKLOADS_ENABLED").as_deref() == Ok("true");
    if !hosting_enabled
        && !databases_enabled
        && !mail_enabled
        && !workloads_enabled
        && !plugin_installations_enabled
    {
        return Err(
            "A configured Hosting, Databases, Mail or workload agent domain must permit accepted operations"
                .into(),
        );
    }
    let auth_url = required("WEBDOCK_DATABASE_URL")?;
    let auth = webdock_control_plane::db::connect_runtime(
        &auth_url,
        "webdock_auth_bootstrap",
        config.production,
    )
    .await?;
    let session = webdock_control_plane::db::connect_runtime(
        &required("WEBDOCK_SESSION_DATABASE_URL")?,
        "webdock_auth_runtime",
        config.production,
    )
    .await?;
    let worker = webdock_control_plane::db::connect_runtime(
        &required("WEBDOCK_WORKER_DATABASE_URL")?,
        "webdock_worker_runtime",
        config.production,
    )
    .await?;
    let ids = Arc::new(
        webdock_persistence::IdAllocator::connect(
            &auth_url,
            webdock_ids::Identity::platform(0, 2)?,
        )
        .await?,
    );
    let key = std::env::var("WEBDOCK_HOSTING_ENCRYPTION_KEY")
        .or_else(|_| std::env::var("BETTER_AUTH_SECRET"))
        .or_else(|_| std::env::var("WEBDOCK_AUTH_SECRET"))
        .map_err(|_| "Missing retained hosting encryption key")?;
    let authority = webdock_control_plane::plugin_config::authority_services(
        auth.clone(),
        ids.clone(),
        &config,
    )
    .await?;
    let mail = if mail_enabled {
        Some(
            webdock_control_plane::mail::initialize_incoming_agent(
                webdock_mail::MailDependencies {
                    auth: auth.clone(),
                    business: session.clone(),
                    worker: worker.clone(),
                    identity: authority.identity.clone(),
                    browser: authority.browser.clone(),
                    ids: ids.clone(),
                },
                &environment,
            )
            .await?,
        )
    } else {
        None
    };
    let databases = if databases_enabled {
        Some(
            database_domain(
                auth.clone(),
                session.clone(),
                worker.clone(),
                ids.clone(),
                &key,
                &authority,
                config.database_new_orders_enabled,
            )
            .await?,
        )
    } else {
        None
    };
    let hosting = Arc::new(
        webdock_hosting::HostingService::new(
            session.clone(),
            auth.clone(),
            worker.clone(),
            required("DATABASE_RLS_SECRET")?,
            key,
            ids.clone(),
        )
        .await?
        .with_session_authority(authority.identity.clone(), authority.browser.clone()),
    );
    let plugin_backend = if plugin_installations_enabled {
        let endpoints = webdock_control_plane::plugins::PluginEndpoints::initialize(
            webdock_control_plane::plugins::PluginDependencies {
                business: session,
                worker,
                auth: auth.clone(),
                identity: authority.identity.clone(),
                browser: authority.browser.clone(),
                oauth: authority.oauth.clone(),
                ids,
                rls_secret: required("DATABASE_RLS_SECRET")?,
                database_new_orders_enabled: config.database_new_orders_enabled,
                neon_connections: None,
            },
            plugin_config
                .as_ref()
                .ok_or("Native plugin configuration required")?,
        )
        .await?;
        Some(Arc::new(
            webdock_control_plane::plugins::installation::AgentBackend {
                host: endpoints.host,
            },
        ))
    } else {
        None
    };
    let shutdown = async {
        #[cfg(unix)]
        {
            let mut term =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("signal handler");
            tokio::select! {_=tokio::signal::ctrl_c()=>{},_=term.recv()=>{}}
        }
    };
    match listener {
        ListenerConfig::Unix {
            path,
            installation,
            agent_id,
            uid,
        } => {
            let mut service = ControlPlane::new(
                auth,
                hosting,
                Some(LocalBinding {
                    installation,
                    agent_id,
                    uid,
                }),
            )
            .await?
            .with_hosting_enabled(hosting_enabled);
            if let Some(databases) = databases {
                service = service.with_databases(databases);
            }
            if let Some(mail) = mail {
                service = service.with_mail(mail);
            }
            if let Some(backend) = plugin_backend {
                service = service.with_plugin_runtime(backend);
            }
            service
                .serve_unix(UnixSocket::bind(path)?, shutdown)
                .await?;
        }
        ListenerConfig::Mtls {
            address,
            certificate,
            key,
            peer_ca,
        } => {
            let tls = MutualTls {
                identity: tonic::transport::Identity::from_pem(
                    webdock_control_plane::config::read_private_file(&certificate)?,
                    webdock_control_plane::config::read_private_file(&key)?,
                ),
                peer_ca: tonic::transport::Certificate::from_pem(
                    webdock_control_plane::config::read_private_file(&peer_ca)?,
                ),
            };
            let mut service = ControlPlane::new(auth, hosting, None)
                .await?
                .with_hosting_enabled(hosting_enabled);
            if let Some(databases) = databases {
                service = service.with_databases(databases);
            }
            if let Some(mail) = mail {
                service = service.with_mail(mail);
            }
            if let Some(backend) = plugin_backend {
                service = service.with_plugin_runtime(backend);
            }
            service
                .serve_mtls(tokio::net::TcpListener::bind(address).await?, tls, shutdown)
                .await?;
        }
    }
    Ok(())
}

async fn database_domain(
    auth: sea_orm::DatabaseConnection,
    business: sea_orm::DatabaseConnection,
    worker: sea_orm::DatabaseConnection,
    ids: Arc<webdock_persistence::IdAllocator>,
    encryption_key: &str,
    authority: &webdock_control_plane::plugin_config::AuthorityServices,
    new_orders_enabled: bool,
) -> Result<Arc<webdock_databases::DatabaseService>, Error> {
    let lease_secret = std::env::var("BETTER_AUTH_SECRET")
        .or_else(|_| std::env::var("WEBDOCK_AUTH_SECRET"))
        .map_err(|_| "Missing retained database lease secret")?;
    Ok(Arc::new(
        webdock_databases::DatabaseService::new(
            business,
            auth,
            worker,
            authority.identity.clone(),
            authority.browser.clone(),
            ids,
            required("DATABASE_RLS_SECRET")?,
            encryption_key,
            &required("WEBDOCK_DATABASE_GATEWAY_ORIGIN")?,
        )
        .await?
        .with_new_orders_enabled(new_orders_enabled)
        .with_management(webdock_databases::ManagementConfig {
            cluster_id: std::env::var("WEBDOCK_DATABASE_CLUSTER_ID").ok(),
            endpoint_domain: std::env::var("WEBDOCK_DATABASE_DOMAIN").ok(),
            public_ordering: std::env::var("WEBDOCK_DATABASE_PUBLIC_ENABLED").as_deref()
                == Ok("true"),
            protected_database_id: std::env::var("WEBDOCK_CONTROL_DATABASE_ID").ok(),
            lease_secret: Some(lease_secret),
        })?,
    ))
}
