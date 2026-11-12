use crate::{
    config::{AuthCompatibility, RuntimeConfig},
    http::{Services, build_app_with_domains},
    runtime::{Error, Result},
};
use axum::Router;

/// Initialize existing services and return their established native router.
pub async fn build_runtime(runtime: &RuntimeConfig) -> Result<Router> {
    let env = std::env::vars().collect();
    let config = runtime.platform.clone();
    let db = crate::db::connect_bootstrap(runtime).await?;
    let auth_compatibility = AuthCompatibility::from_map(&env, &config)
        .map_err(|error| Error::Message(error.to_string()))?;
    let secret = std::env::var("WEBDOCK_AUTH_SECRET")
        .or_else(|_| std::env::var("BETTER_AUTH_SECRET"))
        .map_err(|_| Error::Message("Missing WEBDOCK_AUTH_SECRET".into()))?;
    let cookies = webdock_identity::CookiePolicy::new(
        config.auth_origin.starts_with("https://"),
        secret.clone(),
    )
    .map_err(|_| Error::Message("Invalid identity configuration".into()))?;
    let keys = webdock_identity::SecretKeys::new(
        std::collections::BTreeMap::from([(1, secret.clone())]),
        1,
        Some(secret.clone()),
    )
    .map_err(|_| Error::Message("Invalid identity encryption configuration".into()))?;
    let generator = webdock_ids::Identity::platform(0, 2)
        .map_err(|_| Error::Message("Invalid platform generator".into()))?;
    let allocator = match runtime.id_backend {
        crate::config::IdBackend::Legacy => {
            webdock_persistence::IdAllocator::connect(runtime.database.uri.as_str(), generator)
                .await
        }
        crate::config::IdBackend::Native => {
            webdock_persistence::IdAllocator::connect_native(
                runtime.database.uri.as_str(),
                generator,
                "webdock_auth_bootstrap",
                config.production,
            )
            .await
        }
    }
    .map_err(|_| Error::Message("ID allocation database unavailable".into()))?;
    let ids = std::sync::Arc::new(allocator);
    // Account mail (activation, reset, invitations) leaves from Core itself. Only a loopback
    // catch-all is supported here; hosted installations use the separate Mail API dispatcher.
    let account_mail = match std::env::var("WEBDOCK_ACCOUNT_SMTP") {
        Ok(smtp) => Some(std::sync::Arc::new(
            webdock_mail::account_delivery::MailDelivery::loopback(
                smtp.parse()
                    .map_err(|_| Error::Message("Invalid WEBDOCK_ACCOUNT_SMTP".into()))?,
                &std::env::var("WEBDOCK_ACCOUNT_MAIL_FROM")
                    .unwrap_or_else(|_| "webdock@example.test".into()),
                &config.auth_origin,
            )
            .map_err(|_| Error::Message("Account mail must be loopback".into()))?,
        )),
        Err(_) => None,
    };
    let identity = std::sync::Arc::new({
        let service = webdock_identity::IdentityService::new(db.clone(), cookies)
            .with_secret_keys(keys)
            .with_allocator(ids.clone())
            .with_passkeys_for_account(
                url::Url::parse(&config.auth_origin)
                    .map_err(|_| Error::Message("Invalid account origin".into()))?,
                auth_compatibility.passkey_rp_origin,
            )
            .map_err(|_| Error::Message("Invalid native identity configuration".into()))?;
        let service = if matches!(
            runtime.session_read_backend,
            crate::config::SessionReadBackend::Native
        ) {
            let pool = webdock_persistence::postgres::Pool::connect(
                runtime.database.uri.as_str(),
                "webdock_auth_bootstrap",
                config.production,
                webdock_persistence::postgres::Limits {
                    max_connections: 4,
                    min_connections: 0,
                    ..Default::default()
                },
            )
            .await
            .map_err(|_| Error::Message("Native session database unavailable".into()))?;
            if matches!(
                runtime.session_management_backend,
                crate::config::SessionManagementBackend::Native
            ) {
                service.with_native_session_management(pool)
            } else {
                service.with_native_session_reads(pool)
            }
        } else {
            service
        };
        let service = if let Ok(base) = std::env::var("NEON_AUTH_BASE_URL") {
            service.with_neon_auth(
                webdock_identity::NeonAuthClient::new(&base, &config.auth_origin)
                    .map_err(|_| Error::Message("Invalid Managed identity endpoint".into()))?,
            )
        } else {
            service
        };
        match &account_mail {
            Some(mail) => service.with_delivery(mail.clone()),
            None => service,
        }
    });
    let registry_url = std::env::var("WEBDOCK_REGISTRY_DATABASE_URL")
        .map_err(|_| Error::Message("Missing WEBDOCK_REGISTRY_DATABASE_URL".into()))?;
    let registry_db =
        crate::db::connect_runtime(&registry_url, "webdock_registry_runtime", config.production)
            .await?;
    let rls_secret = std::env::var("DATABASE_RLS_SECRET")
        .map_err(|_| Error::Message("Missing DATABASE_RLS_SECRET".into()))?;
    let signer = webdock_identity::RlsSigner::new(rls_secret.clone(), "webdock_registry_runtime")
        .map_err(|_| Error::Message("Invalid database scope configuration".into()))?;
    let registry = std::sync::Arc::new(
        webdock_registry::RegistryService::new(registry_db, signer).with_allocator(ids.clone()),
    );
    let oauth_keys = std::sync::Arc::new(
        webdock_identity::SecretKeys::new(
            std::collections::BTreeMap::from([(1, secret.clone())]),
            1,
            Some(secret.clone()),
        )
        .map_err(|_| Error::Message("Invalid OAuth encryption configuration".into()))?,
    );
    let oauth_config = webdock_oauth::OAuthConfig::new(
        &auth_compatibility.oauth_issuer,
        &std::env::var("WEBDOCK_MCP_RESOURCE")
            .unwrap_or_else(|_| format!("{}/api/mcp", config.studio_origin)),
        std::env::var("WEBDOCK_STUDIO_CLIENT_ID")
            .or_else(|_| std::env::var("WEBDOCK_SSO_CLIENT_ID"))
            .ok(),
    )
    .map_err(|_| Error::Message("Invalid OAuth configuration".into()))?;
    let oauth = std::sync::Arc::new(webdock_oauth::OAuthService::new(
        db.clone(),
        identity.clone(),
        ids.clone(),
        oauth_keys.clone(),
        oauth_config,
    ));
    let browser = if let Ok(client_id) = std::env::var("WEBDOCK_SSO_CLIENT_ID") {
        let client_secret = std::env::var("WEBDOCK_SSO_CLIENT_SECRET")
            .map_err(|_| Error::Message("Missing first-party client secret".into()))?;
        let cookie_secret = std::env::var("WEBDOCK_SSO_COOKIE_SECRET")
            .map_err(|_| Error::Message("Missing first-party cookie secret".into()))?;
        let browser_config = webdock_browser_session::BrowserConfig::new(
            config.console_origin(),
            &auth_compatibility.oauth_issuer,
            client_id,
            client_secret,
        )
        .and_then(|v| v.with_additional_origins(std::slice::from_ref(&config.studio_origin)))
        .map_err(|_| Error::Message("Invalid browser session configuration".into()))?;
        let mut browser = webdock_browser_session::BrowserSessions::new(
            db.clone(),
            identity.clone(),
            oauth.clone(),
            oauth_keys.clone(),
            &cookie_secret,
            browser_config,
        )
        .map_err(|_| Error::Message("Invalid browser session configuration".into()))?;
        if let Ok(until) = std::env::var("WEBDOCK_SSO_LEGACY_ACCEPT_UNTIL") {
            let until = until
                .parse::<i64>()
                .map_err(|_| Error::Message("Invalid legacy cookie deadline".into()))?;
            let legacy = webdock_browser_session::LegacyCookies::new(
                std::collections::HashMap::from([("1".into(), cookie_secret)]),
                until,
            )
            .map_err(|_| {
                Error::Message("Legacy cookie transition must end within eight hours".into())
            })?;
            browser = browser.with_legacy(legacy);
        }
        Some(std::sync::Arc::new(browser))
    } else {
        None
    };
    let session_url = std::env::var("WEBDOCK_SESSION_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .map_err(|_| Error::Message("Missing session database URL".into()))?;
    let session_db =
        crate::db::connect_runtime(&session_url, "webdock_auth_runtime", config.production).await?;
    // Existing Core/session database and retained platform encryption key only.
    let neon_connections = Some(std::sync::Arc::new(
        crate::neon_connections::NeonConnections::new(
            session_db.clone(),
            db.clone(),
            identity.clone(),
            browser.clone(),
            &rls_secret,
            crate::neon_connections::encryption_key(&env)
                .map_err(|e| Error::Message(e.to_string()))?,
        )
        .map_err(|_| Error::Message("Neon connection initialization failed".into()))?,
    ));
    let access = std::sync::Arc::new(
        webdock_access::AccessService::new(
            session_db.clone(),
            db.clone(),
            identity.clone(),
            ids.clone(),
            webdock_identity::RlsSigner::new(rls_secret.clone(), "webdock_auth_runtime")
                .map_err(|_| Error::Message("Invalid access scope".into()))?,
            oauth_keys,
            webdock_access::AccessConfig {
                auth_origin: url::Url::parse(&config.auth_origin).expect("validated origin"),
                console_origins: vec![
                    url::Url::parse(config.console_origin()).expect("validated origin"),
                    url::Url::parse(&config.studio_origin).expect("validated origin"),
                ],
                allow_local_http: !config.production,
            },
        )
        .map_err(|_| Error::Message("Invalid access configuration".into()))
        .map(|service| match &account_mail {
            Some(mail) => service.with_delivery(mail.clone()),
            None => service,
        })?,
    );
    if account_mail.is_some() {
        let (identity, access) = (identity.clone(), access.clone());
        tokio::spawn(async move {
            loop {
                let (sent, invited) = tokio::join!(
                    identity.dispatch_delivery_jobs(10),
                    access.dispatch_invitation_jobs(10)
                );
                if sent.is_err() || invited.is_err() {
                    tracing::warn!("Account mail delivery unavailable; jobs stay queued");
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        });
    }
    let hosting = if config.features.state("hosting")
        != webdock_core::features::FeatureState::Disabled
        || config.features.state("databases") != webdock_core::features::FeatureState::Disabled
    {
        let session_url = std::env::var("WEBDOCK_SESSION_DATABASE_URL")
            .map_err(|_| Error::Message("Missing WEBDOCK_SESSION_DATABASE_URL".into()))?;
        let worker_url = std::env::var("WEBDOCK_WORKER_DATABASE_URL")
            .map_err(|_| Error::Message("Missing WEBDOCK_WORKER_DATABASE_URL".into()))?;
        // Retained application ciphertext uses the auth secret, distinct from
        // native Mail's separate MAIL_ENCRYPTION_KEY. Preserve it during rotation.
        let environment_secret = std::env::var("WEBDOCK_HOSTING_ENCRYPTION_KEY")
            .or_else(|_| std::env::var("BETTER_AUTH_SECRET"))
            .unwrap_or_else(|_| secret.clone());
        let session_db =
            crate::db::connect_runtime(&session_url, "webdock_auth_runtime", config.production)
                .await?;
        let worker_db =
            crate::db::connect_runtime(&worker_url, "webdock_worker_runtime", config.production)
                .await?;
        Some(std::sync::Arc::new(
            webdock_hosting::HostingService::new(
                session_db,
                db.clone(),
                worker_db,
                rls_secret.clone(),
                environment_secret,
                ids.clone(),
            )
            .await
            .map_err(|_| Error::Message("Hosting initialization failed".into()))?
            .with_public_site_origins(&[
                config.api_origin.clone(),
                config.auth_origin.clone(),
                config.studio_origin.clone(),
                config.console_origin().to_owned(),
            ])
            .map_err(|_| Error::Message("Invalid public site control origins".into()))?
            .with_session_authority(identity.clone(), browser.clone())
            .with_recovery_signer_configuration(
                std::env::var_os("WEBDOCK_RECOVERY_SIGNER_KEY").map(std::path::PathBuf::from),
                std::env::var_os("WEBDOCK_RECOVERY_SIGNER_DIRECTORY").map(std::path::PathBuf::from),
            )
            .map_err(|_| {
                Error::Message("Independent recovery signer configuration is invalid".into())
            })?,
        ))
    } else {
        None
    };
    let plugin_config = std::env::var("WEBDOCK_NATIVE_PLUGIN_CONFIG")
        .ok()
        .map(|path| crate::plugin_config::PluginConfig::load(std::path::Path::new(&path)))
        .transpose()
        .map_err(|_| Error::Message("Invalid native plugin configuration".into()))?;
    let plugins = if let Some(plugin_config) = &plugin_config {
        let worker_url = std::env::var("WEBDOCK_WORKER_DATABASE_URL")
            .map_err(|_| Error::Message("Missing WEBDOCK_WORKER_DATABASE_URL".into()))?;
        Some(std::sync::Arc::new(
            crate::plugins::PluginEndpoints::initialize(
                crate::plugins::PluginDependencies {
                    business: crate::db::connect_runtime(
                        &session_url,
                        "webdock_auth_runtime",
                        config.production,
                    )
                    .await?,
                    worker: crate::db::connect_runtime(
                        &worker_url,
                        "webdock_worker_runtime",
                        config.production,
                    )
                    .await?,
                    auth: db.clone(),
                    identity: identity.clone(),
                    browser: browser.clone(),
                    oauth: oauth.clone(),
                    ids: ids.clone(),
                    rls_secret: rls_secret.clone(),
                    database_new_orders_enabled: config.database_new_orders_enabled,
                    neon_connections: neon_connections.clone(),
                },
                plugin_config,
            )
            .await
            .map_err(|_| Error::Message("Native plugin initialization failed".into()))?,
        ))
    } else {
        None
    };
    if let Some(plugins) = &plugins {
        let host = plugins.host.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
            loop {
                interval.tick().await;
                if host.prune_history().await.is_err() {
                    tracing::warn!("Plugin history retention unavailable; will retry");
                }
            }
        });
    }
    // Notices about units that need attention go out with the account mail.
    if let (Some(mail), Some(plugins)) = (&account_mail, &plugins) {
        let (mail, host) = (mail.clone(), plugins.host.clone());
        let sign_in = format!("{}/", config.auth_origin.trim_end_matches('/'));
        tokio::spawn(async move {
            loop {
                if host.dispatch_notifications(&*mail, &sign_in).await.is_err() {
                    tracing::warn!("Notice mail unavailable; notices stay due");
                }
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            }
        });
    }
    let build = if config.features.state("hosting")
        != webdock_core::features::FeatureState::Disabled
        && let Some(hosting) = &hosting
    {
        let worker_url = std::env::var("WEBDOCK_WORKER_DATABASE_URL")
            .map_err(|_| Error::Message("Missing WEBDOCK_WORKER_DATABASE_URL".into()))?;
        let mut service = webdock_build::BuildService::new_inventory(
            crate::db::connect_runtime(&session_url, "webdock_auth_runtime", config.production)
                .await?,
            crate::db::connect_runtime(&worker_url, "webdock_worker_runtime", config.production)
                .await?,
            rls_secret.clone(),
            ids.clone(),
            hosting.clone(),
        )
        .await
        .map_err(|_| Error::Message("Build initialization failed".into()))?
        .with_session_authority(db.clone(), identity.clone(), browser.clone())
        .await
        .map_err(|_| Error::Message("Build authority initialization failed".into()))?;
        if let Ok(registry_host) = std::env::var("WEBDOCK_BUILD_REGISTRY_HOST") {
            service = service
                .with_execution_registry(registry_host)
                .map_err(|_| Error::Message("Invalid build registry configuration".into()))?;
        }
        if let Some(repository) = plugins
            .as_ref()
            .and_then(|plugins| plugins.repository.clone())
        {
            service = service.with_repository_provider(std::sync::Arc::new(
                webdock_build::PluginRepositoryProvider::new(repository.clone()),
            ));
            if let Some(path) = std::env::var_os("WEBDOCK_GITHUB_BUILD_WEBHOOK_SECRET_FILE") {
                let plugin =
                    std::env::var("WEBDOCK_GITHUB_REPOSITORY_PLUGIN_ID").map_err(|_| {
                        Error::Message("Missing GitHub repository plugin binding".into())
                    })?;
                let verifier = webdock_build::GithubWebhookVerifier::from_private_file(
                    std::path::Path::new(&path),
                    plugin,
                )
                .map_err(|_| Error::Message("Invalid GitHub webhook secret reference".into()))?;
                service = service.with_github_webhooks(verifier, repository);
            }
        }
        Some(std::sync::Arc::new(service))
    } else {
        None
    };
    let databases = if config.features.state("databases")
        != webdock_core::features::FeatureState::Disabled
    {
        let session_url = std::env::var("WEBDOCK_SESSION_DATABASE_URL")
            .map_err(|_| Error::Message("Missing WEBDOCK_SESSION_DATABASE_URL".into()))?;
        let worker_url = std::env::var("WEBDOCK_WORKER_DATABASE_URL")
            .map_err(|_| Error::Message("Missing WEBDOCK_WORKER_DATABASE_URL".into()))?;
        let encryption_secret = std::env::var("WEBDOCK_HOSTING_ENCRYPTION_KEY")
            .or_else(|_| std::env::var("BETTER_AUTH_SECRET"))
            .unwrap_or_else(|_| secret.clone());
        let gateway_origin = std::env::var("WEBDOCK_DATABASE_GATEWAY_ORIGIN")
            .map_err(|_| Error::Message("Missing WEBDOCK_DATABASE_GATEWAY_ORIGIN".into()))?;
        let gateway_secret = std::env::var("WEBDOCK_DATABASE_GATEWAY_SECRET")
            .map_err(|_| Error::Message("Missing WEBDOCK_DATABASE_GATEWAY_SECRET".into()))?;
        let service = webdock_databases::DatabaseService::new(
            crate::db::connect_runtime(&session_url, "webdock_auth_runtime", config.production)
                .await?,
            db.clone(),
            crate::db::connect_runtime(&worker_url, "webdock_worker_runtime", config.production)
                .await?,
            identity.clone(),
            browser.clone(),
            ids.clone(),
            rls_secret.clone(),
            &encryption_secret,
            &gateway_origin,
        )
        .await
        .map_err(|_| Error::Message("Database initialization failed".into()))?
        .with_new_orders_enabled(config.database_new_orders_enabled)
        .with_management(webdock_databases::ManagementConfig {
            cluster_id: std::env::var("WEBDOCK_DATABASE_CLUSTER_ID").ok(),
            endpoint_domain: std::env::var("WEBDOCK_DATABASE_DOMAIN").ok(),
            public_ordering: std::env::var("WEBDOCK_DATABASE_PUBLIC_ENABLED").as_deref()
                == Ok("true"),
            protected_database_id: std::env::var("WEBDOCK_CONTROL_DATABASE_ID").ok(),
            lease_secret: Some(
                std::env::var("BETTER_AUTH_SECRET").unwrap_or_else(|_| secret.clone()),
            ),
        })
        .map_err(|_| Error::Message("Invalid database management configuration".into()))?;
        Some(std::sync::Arc::new(
            crate::databases::DatabaseEndpoints::new(std::sync::Arc::new(service), &gateway_secret)
                .map_err(|e| Error::Message(e.into()))?,
        ))
    } else {
        None
    };
    let incoming_mail = std::env::var("WEBDOCK_NATIVE_MAILBOXES_ENABLED").as_deref() == Ok("true");
    let incoming_mail_admin = std::env::var("WEBDOCK_NATIVE_MAIL_ENABLED").as_deref() == Ok("true");
    let sending_mail =
        std::env::var("WEBDOCK_NATIVE_MAIL_SENDING_ENABLED").as_deref() == Ok("true");
    let mail = if config.features.state("mail") != webdock_core::features::FeatureState::Disabled
        && (incoming_mail || sending_mail || incoming_mail_admin)
    {
        let required =
            |key: &str| std::env::var(key).map_err(|_| Error::Message(format!("Missing {key}")));
        let dependencies = webdock_mail::MailDependencies {
            auth: db.clone(),
            business: crate::db::connect_runtime(
                &required("WEBDOCK_SESSION_DATABASE_URL")?,
                "webdock_auth_runtime",
                config.production,
            )
            .await?,
            worker: crate::db::connect_runtime(
                &required("WEBDOCK_WORKER_DATABASE_URL")?,
                "webdock_worker_runtime",
                config.production,
            )
            .await?,
            identity: identity.clone(),
            browser: browser.clone(),
            ids: ids.clone(),
        };
        let mut endpoints = None;
        if incoming_mail {
            let service = webdock_mail::webmail::WebmailService::new(
                dependencies.clone(),
                webdock_mail::webmail::WebmailConfig {
                    enabled: true,
                    gateway_origin: required("MAIL_GATEWAY_ORIGIN")?,
                    managed_cluster: required("MAIL_MANAGED_CLUSTER_ID")?,
                    rls_secret: rls_secret.clone(),
                    encryption_key: required("MAIL_ENCRYPTION_KEY")?,
                },
            )
            .await
            .map_err(|_| Error::Message("Webmail initialization failed".into()))?;
            endpoints = Some(
                crate::mail::MailEndpoints::new(
                    std::sync::Arc::new(service),
                    &required("MAIL_GATEWAY_SECRET")?,
                )
                .map_err(|e| Error::Message(e.into()))?,
            );
        }
        if sending_mail {
            let keys = std::sync::Arc::new(
                webdock_mail::keys::NativeKeys::new(dependencies.clone(), &rls_secret, true)
                    .await
                    .map_err(|_| Error::Message("Mail sending initialization failed".into()))?,
            );
            let queue = std::sync::Arc::new(
                webdock_mail::submissions::Submissions::new(
                    dependencies.clone(),
                    &rls_secret,
                    true,
                )
                .await
                .map_err(|_| Error::Message("Mail queue initialization failed".into()))?,
            );
            let secret = required("MAIL_SEND_WORKER_SECRET")?;
            let sender = if let Some(sender_config) = plugin_config
                .as_ref()
                .and_then(|config| config.mail.as_ref())
            {
                let host = plugins
                    .as_ref()
                    .ok_or_else(|| Error::Message("Mail provider host is unavailable".into()))?
                    .host
                    .clone();
                Some(
                    crate::mail::initialize_sender(
                        dependencies.clone(),
                        host,
                        sender_config,
                        &std::env::vars().collect(),
                        endpoints.as_ref().and_then(|value| value.service.clone()),
                    )
                    .await
                    .map_err(|error| Error::Message(error.to_string()))?,
                )
            } else {
                None
            };
            let mut sending_endpoints = match endpoints {
                Some(endpoints) => endpoints.with_sending(keys, &secret),
                None => crate::mail::MailEndpoints::sending_only(keys, &secret),
            }
            .map_err(|e| Error::Message(e.into()))?
            .with_queue(queue);
            if let Some(sender) = sender {
                sending_endpoints = sending_endpoints.with_sender(sender);
            }
            endpoints = Some(sending_endpoints);
        }
        if incoming_mail_admin {
            let incoming =
                crate::mail::initialize_incoming(dependencies.clone(), &std::env::vars().collect())
                    .await
                    .map_err(|error| Error::Message(error.to_string()))?;
            endpoints = Some(match endpoints {
                Some(endpoint) => endpoint.with_incoming(incoming),
                None => crate::mail::MailEndpoints::incoming_only(incoming),
            });
        }
        endpoints.map(std::sync::Arc::new)
    } else {
        None
    };
    let realtime = crate::realtime::Realtime::from_map(&std::env::vars().collect())
        .map_err(|e| Error::Message(e.into()))?;
    let ui = crate::ui::UiAssets::from_map(&std::env::vars().collect(), config.production)
        .map_err(|e| Error::Message(e.into()))?
        .map(std::sync::Arc::new);
    // A gateway the Console is told to call but its CSP blocks fails only in the browser.
    if let Some(ui) = &ui {
        let database_gateway = databases
            .as_ref()
            .and_then(|_| std::env::var("WEBDOCK_DATABASE_GATEWAY_ORIGIN").ok());
        let mail_gateway = mail
            .as_deref()
            .and_then(crate::mail::MailEndpoints::gateway_origin)
            .map(str::to_owned);
        let realtime_origin = realtime
            .as_ref()
            .map(|value| value.url.replacen("wss://", "https://", 1));
        for origin in database_gateway
            .iter()
            .chain(&mail_gateway)
            .chain(&realtime_origin)
        {
            ui.require_connect(origin)
                .map_err(|e| Error::Message(e.into()))?;
        }
    }
    Ok(build_app_with_domains(
        config,
        db.clone(),
        Services {
            realtime,
            resource_client: crate::resource_auth::ResourceClientConfig::from_environment()
                .map_err(|message| Error::Message(message.into()))?,
            identity: Some(identity),
            registry: Some(registry),
            oauth: Some(oauth),
            hosting,
            build,
            plugins,
            databases,
            mail,
            neon_connections,
            browser,
            access: Some(access),
            ui,
        },
    ))
}
