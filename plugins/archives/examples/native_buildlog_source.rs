//! Explicit owned-fixture qualifier. Actual operator/domain/worker completion; never a public API.
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};
use webdock_build::{BuildService, RepositoryProvider, RepositoryRef, VerifiedCommit};
type Error = Box<dyn std::error::Error + Send + Sync>;
fn first_attempt() -> u32 {
    1
}
fn checkpoint(stage: &'static str) {
    eprintln!("owned_buildlog_stage_{stage}");
}
fn private_dir(parent: &Path) -> Result<tempfile::TempDir, Error> {
    use std::os::unix::fs::PermissionsExt;
    Ok(tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(parent)?)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Input {
    run_id: String,
    #[serde(default = "first_attempt")]
    attempt: u32,
    database: String,
    auth_database: String,
    worker_database: String,
    admin_database: String,
    rls_secret: String,
    auth_secret: String,
    cookie: String,
    customer: String,
    project: String,
    cluster: String,
    vm_root: PathBuf,
    scratch: PathBuf,
    #[serde(default)]
    build_id: Option<String>,
    #[serde(default)]
    connection_id: Option<String>,
}
async fn sql(
    db: &DatabaseConnection,
    statement: &str,
    values: Vec<sea_orm::Value>,
) -> Result<(), Error> {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        statement,
        values,
    ))
    .await?;
    Ok(())
}
fn git(path: &Path, args: &[&str]) -> Result<Vec<u8>, Error> {
    let output = std::process::Command::new("git")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", "/nonexistent")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err("owned Git fixture command failed".into());
    }
    Ok(output.stdout)
}
struct LocalRepository(PathBuf);
#[async_trait::async_trait]
impl RepositoryProvider for LocalRepository {
    async fn resolve(
        &self,
        _: &RepositoryRef<'_>,
        reference: &str,
    ) -> webdock_build::Result<VerifiedCommit> {
        if reference != "main" {
            return Err(webdock_build::Error {
                code: "fixture_reference",
                logs: String::new(),
            });
        }
        let bytes = git(&self.0, &["rev-parse", "--verify", "main^{commit}"]).map_err(|_| {
            webdock_build::Error {
                code: "fixture_git",
                logs: String::new(),
            }
        })?;
        VerifiedCommit::new(String::from_utf8_lossy(&bytes).trim().into())
    }
    async fn archive(&self, _: &RepositoryRef<'_>, commit: &str) -> webdock_build::Result<Vec<u8>> {
        if commit.len() != 40 || !commit.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(webdock_build::Error {
                code: "fixture_commit",
                logs: String::new(),
            });
        }
        git(&self.0, &["archive", "--format=tar", commit]).map_err(|_| webdock_build::Error {
            code: "fixture_git",
            logs: String::new(),
        })
    }
}
async fn qualify(stage: &str, input: Input) -> Result<Value, Error> {
    if input.run_id.len() != 12
        || !input.run_id.bytes().all(|b| b.is_ascii_hexdigit())
        || !input.scratch.is_absolute()
        || input.scratch.file_name().and_then(|s| s.to_str()) != Some(input.run_id.as_str())
    {
        return Err("exact owned fixture identity required".into());
    }
    for id in [&input.customer, &input.project, &input.cluster] {
        if !webdock_export_protocol::decimal(id) {
            return Err("actual native ID required".into());
        }
    }
    checkpoint("connect");
    let auth = Database::connect(&input.auth_database).await?;
    let business = Database::connect(&input.database).await?;
    let worker_db = Database::connect(&input.worker_database).await?;
    let admin = Database::connect(&input.admin_database).await?;
    let identity = Arc::new(webdock_identity::IdentityService::new(
        auth.clone(),
        webdock_identity::CookiePolicy::new(false, input.auth_secret.clone())?,
    ));
    let principal = identity
        .authenticate_cookie(&input.cookie)
        .await?
        .ok_or("actual Native operator session required")?;
    checkpoint("operator_verified");
    if !principal.is_operator() {
        return Err("actual Native operator required".into());
    }
    let ids = Arc::new(
        webdock_persistence::IdAllocator::connect(
            &input.admin_database,
            webdock_ids::Identity::platform(0, 2)?,
        )
        .await?,
    );
    let hosting = Arc::new(
        webdock_hosting::HostingService::new(
            business.clone(),
            auth.clone(),
            worker_db.clone(),
            input.rls_secret.clone(),
            input.auth_secret.clone(),
            ids.clone(),
        )
        .await?
        .with_session_authority(identity.clone(), None),
    );
    let service = BuildService::new(
        business.clone(),
        worker_db,
        input.rls_secret.clone(),
        ids.clone(),
        hosting.clone(),
        "registry.invalid".into(),
    )
    .await?
    .with_session_authority(auth, identity, None)
    .await?;
    if stage == "cancel-prior" {
        let id = input
            .build_id
            .as_deref()
            .ok_or("exact prior build required")?;
        let prior = service
            .execute(
                &principal,
                json!({"action":"git.builds.get","projectID":input.project,"buildID":id}),
            )
            .await?;
        if !matches!(
            prior["status"].as_str(),
            Some("running" | "queued" | "cancelled")
        ) {
            return Err("prior terminal outcome must not be overwritten".into());
        }
        if prior["status"] != "cancelled" {
            service
                .execute(
                    &principal,
                    json!({"action":"git.builds.cancel","projectID":input.project,"buildID":id}),
                )
                .await?;
        }
        let signer = webdock_identity::RlsSigner::new(input.rls_secret, "webdock_auth_runtime")?;
        let tx = signer.begin(&business, &principal).await?;
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT w.id,w.generation,w.enabled FROM webdock_auth.git_build b JOIN webdock_auth.git_worker w ON w.id=b.worker_id WHERE b.id=$1 AND b.project_id=$2 AND b.customer_id=$3",vec![id.into(),input.project.clone().into(),input.customer.clone().into()])).await?.ok_or("prior worker required")?;
        let worker_id: String = row.try_get("", "id")?;
        let worker_generation: i32 = row.try_get("", "generation")?;
        let enabled: bool = row.try_get("", "enabled")?;
        tx.commit().await?;
        if enabled {
            service
                .revoke_worker(&principal, &worker_id, worker_generation)
                .await?;
        }
        return Ok(
            json!({"actualDomainCancellation":true,"priorBuildID":id,"workerRevoked":true,"completedOrReadyFabricated":false}),
        );
    }
    if stage == "disconnect" {
        let connection = input
            .connection_id
            .as_deref()
            .ok_or("exact fixture connection required")?;
        let result=service.execute(&principal,json!({"action":"git.connections.disconnect","customerID":input.customer,"connectionID":connection,"revision":1})).await?;
        return Ok(
            json!({"actualCoreDisconnect":true,"connectionID":connection,"state":result["state"]}),
        );
    }
    if stage == "corrupt-log" {
        let id = input
            .build_id
            .as_deref()
            .ok_or("exact fixture build required")?;
        let signer = webdock_identity::RlsSigner::new(input.rls_secret, "webdock_auth_runtime")?;
        let tx = signer.begin(&business, &principal).await?;
        let changed=tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.git_build SET logs=logs||E'\nowned log-corruption probe' WHERE id=$1 AND customer_id=$2 AND project_id=$3 AND status IN ('succeeded','failed') AND completion_hash IS NOT NULL",vec![id.into(),input.customer.into(),input.project.into()])).await?.rows_affected();
        if changed != 1 {
            return Err("exact owned log fault injection refused".into());
        }
        tx.commit().await?;
        return Ok(
            json!({"exactOwnedLogCorruptionInjected":true,"buildID":id,"repairPerformed":false}),
        );
    }
    if stage != "setup" || !(1..=3).contains(&input.attempt) {
        return Err("unsupported owned qualifier stage".into());
    }
    // These are fixture configuration records, not invented completed builds or runtime readiness.
    sql(&admin,"INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version) VALUES('hosting','builtin','enabled','enabled','1.0.0') ON CONFLICT DO NOTHING",vec![]).await?;
    checkpoint("fixture_configuration");
    let cap = json!({"apps":2,"cpuMillicores":1000,"memoryBytes":1073741824,"ephemeralBytes":1073741824,"volumeBytes":1073741824,"replicasPerApp":1,"concurrentDeployments":1});
    sql(&admin,"INSERT INTO webdock_auth.tenant_subscription(customer_id,name,description,base,extras,revision,updated_by) VALUES($1,'Owned buildlog fixture','Finite actual-build qualification',$2,'{}',1,$3) ON CONFLICT(customer_id) DO NOTHING",vec![input.customer.clone().into(),json!({"hosting":cap}).into(),principal.user_id.clone().into()]).await?;
    hosting.execute(&principal,json!({"action":"projects.create","projectID":input.project,"provider":"k3s","clusterID":input.cluster,"mode":"selfservice","ownImages":true,"confirmSharedImages":true,"idempotencyKey":format!("buildlog-hosting-{}",input.run_id)})).await?;
    let draft=hosting.execute(&principal,json!({"action":"apps.prepare","projectID":input.project,"name":format!("owned-buildlog-target-{}",input.attempt),"spec":{"args":[],"port":8080,"healthPath":"/","cpuMillicores":100,"memoryBytes":16777216,"ephemeralBytes":1048576,"replicas":1,"volumeBytes":0},"subscriptionRevision":1,"idempotencyKey":format!("buildlog-target-{}-r{}",input.run_id,input.attempt)})).await?;
    let target = draft["appID"]
        .as_str()
        .or_else(|| draft["id"].as_str())
        .ok_or("actual prepared target required")?
        .to_owned();
    checkpoint("git_configuration");
    let connection = ids.allocate().await?.to_string();
    sql(&admin,"INSERT INTO webdock_auth.git_connection(id,customer_id,installation_id,account_id,account_login,permissions,state) VALUES($1,$2,$1,$1,'owned-local-fixture','{}','active')",vec![connection.clone().into(),input.customer.clone().into()]).await?;
    sql(&admin,"INSERT INTO webdock_auth.git_repository(connection_id,customer_id,repository_id,metadata) VALUES($1,$2,'123456','{}')",vec![connection.clone().into(),input.customer.clone().into()]).await?;
    let repository = private_dir(&input.scratch)?;
    let root = input.vm_root.canonicalize()?;
    let reviewed = std::fs::read_to_string(root.join("SHA256SUMS"))?;
    let expected = |name: &str| -> Result<String, Error> {
        Ok(reviewed
            .lines()
            .find(|l| l.ends_with(&format!("/{name}")))
            .and_then(|l| l.split_whitespace().next())
            .ok_or("reviewed VM digest missing")?
            .into())
    };
    let image =
        webdock_build::execution::VmImage::new(root.join("base.raw"), expected("base.raw")?)?
            .with_boot(
                root.join("kernel"),
                expected("kernel")?,
                root.join("initrd"),
                expected("initrd")?,
            )?;
    std::fs::copy(
        root.join("rootfs/bin/busybox"),
        repository.path().join("busybox"),
    )?;
    std::fs::copy(
        root.join("rootfs/lib/ld-musl-x86_64.so.1"),
        repository.path().join("musl"),
    )?;
    git(repository.path(), &["init", "-b", "main"])?;
    let service = Arc::new(
        service.with_repository_provider(Arc::new(LocalRepository(repository.path().into()))),
    );
    let native_worker = service
        .enroll_worker(
            &principal,
            "ZZ",
            "microvm",
            "unverified owned disposable actual KVM buildlog qualifier",
            1,
        )
        .await?;
    let worker = Arc::new(
        service
            .authenticate_worker(
                native_worker["credential"]
                    .as_str()
                    .ok_or("enrolled worker credential required")?,
            )
            .await?,
    );
    let literal = format!("owned-buildlog-literal-{}", input.run_id);
    service.execute(&principal,json!({"action":"git.source.configure","projectID":input.project,"targetID":target,"connectionID":connection,"repositoryID":"123456","branch":"main","rootDirectory":".","recipe":"dockerfile","revision":0,"autoPublish":false,"buildEnvironment":[{"name":"BUILD_TOKEN","value":literal}]})).await?;
    let mut proofs = BTreeMap::new();
    for outcome in ["succeeded", "failed"] {
        let marker = format!("actual-native-buildlog-{outcome}");
        let ending = if outcome == "failed" { "; exit 22" } else { "" };
        let dockerfile = format!(
            "FROM scratch\nCOPY --chmod=755 busybox /bin/busybox\nCOPY --chmod=755 musl /lib/ld-musl-x86_64.so.1\nSHELL [\"/bin/busybox\",\"sh\",\"-c\"]\nARG BUILD_TOKEN\nRUN echo {marker}; echo $BUILD_TOKEN{ending}\nENTRYPOINT [\"/bin/busybox\",\"true\"]\n"
        );
        std::fs::write(repository.path().join("Dockerfile"), dockerfile)?;
        git(repository.path(), &["add", "--all"])?;
        git(
            repository.path(),
            &[
                "-c",
                "user.name=Owned Fixture",
                "-c",
                "user.email=owned@example.invalid",
                "commit",
                "-m",
                outcome,
            ],
        )?;
        let requested=service.execute(&principal,json!({"action":"git.builds.request","projectID":input.project,"targetID":target,"idempotencyKey":format!("buildlog-{}-git-{connection}-{outcome}",input.run_id)})).await?;
        let id = requested["id"]
            .as_str()
            .ok_or("actual requested build required")?
            .to_owned();
        let claim = service
            .claim_build(&worker)
            .await?
            .ok_or("actual Core claim required")?;
        if claim["buildID"] != id {
            return Err("unexpected build claim".into());
        }
        let generation = i32::try_from(
            claim["generation"]
                .as_i64()
                .ok_or("actual generation required")?,
        )?;
        let source = service.source_archive(&worker, &id, generation).await?;
        let mut job = serde_json::from_value::<webdock_build::execution::GuestRequest>(
            claim["request"].clone(),
        )?;
        job.limits.duration_seconds = 180;
        job.limits.memory_mi_b = 2048;
        let scratch = private_dir(&input.scratch)?;
        let execute = image.clone();
        let runtime = tokio::runtime::Handle::current();
        let lease_service = service.clone();
        let lease_worker = worker.clone();
        let build = id.clone();
        checkpoint("vm_execute");
        let result = tokio::task::spawn_blocking(move || {
            execute.execute(&job, &source, scratch.path(), || {
                runtime.block_on(lease_service.guard_build(&lease_worker, &build, generation))
            })
        })
        .await?;
        service.guard_build(&worker, &id, generation).await?;
        checkpoint("private_artifact_store");
        let store_dir = private_dir(&input.scratch)?;
        let store = webdock_build::artifact::ArtifactStore::open(
            store_dir.path().into(),
            webdock_build::artifact::MAX_OUTPUT,
        )?;
        let logs = match result {
            Ok(output) => {
                if outcome != "succeeded" {
                    return Err("failed VM unexpectedly succeeded".into());
                }
                let artifact = store.put(&input.customer, &id, generation, &output.artifact)?;
                service
                    .complete_build(
                        &worker,
                        &id,
                        generation,
                        Some(&artifact),
                        &output.logs,
                        None,
                    )
                    .await?;
                output.logs
            }
            Err(error) => {
                if outcome != "failed" || error.code != "build_failed" {
                    if !error.code.is_empty()
                        && error.code.len() <= 80
                        && error
                            .code
                            .bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
                    {
                        eprintln!("owned_buildlog_vm_code_{}", error.code);
                    }
                    use std::io::Write;
                    use std::os::unix::fs::OpenOptionsExt;
                    let mut diagnostic = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(
                            input
                                .scratch
                                .join(format!("buildlog-vm-failure-r{}.log", input.attempt)),
                        )?;
                    diagnostic.write_all(error.logs.replace(&literal, "[REDACTED]").as_bytes())?;
                    return Err(error.into());
                }
                service
                    .complete_build(
                        &worker,
                        &id,
                        generation,
                        None,
                        &error.logs,
                        Some("BUILD_FAILED"),
                    )
                    .await?;
                error.logs
            }
        };
        let stored = service
            .execute(
                &principal,
                json!({"action":"git.builds.get","projectID":input.project,"buildID":id}),
            )
            .await?;
        let actual = stored["logs"]
            .as_str()
            .ok_or("actual stored Core logs required")?;
        if stored["status"] != outcome
            || actual != logs
            || actual.contains(&literal)
            || !actual.contains("[REDACTED]")
            || !actual.contains(&marker)
            || actual.len() > 24_000
        {
            return Err("actual log capture/redaction/completion mismatch".into());
        }
        proofs.insert(outcome,json!({"buildID":id,"status":outcome,"sourceSHA":claim["sourceSHA"],"generation":generation,"bytes":actual.len(),"sha256":hex::encode(Sha256::digest(actual.as_bytes())),"actualVMOutput":true,"actualCoreCompletion":true,"knownLiteralMasked":true,"completeOutputClaimed":false}));
    }
    service
        .revoke_worker(
            &principal,
            native_worker["id"].as_str().ok_or("worker ID required")?,
            1,
        )
        .await?;
    Ok(
        json!({"customerID":input.customer,"projectID":input.project,"connectionID":connection,"sources":proofs,"actualOperatorCookieVerified":true,"workerRevokedAfterCompletion":true,"fixtureOnlyConfigurationSeeded":true,"gitConfigurationIdentity":connection,"gitBuildKeysDerivedFromInstallation":true,"fakeCompletedRows":false,"vm":image.identity()}),
    )
}
#[tokio::main]
async fn main() {
    let result = async {
        webdock_archives::initialize_crypto();
        let mut args = std::env::args().skip(1);
        let stage = args.next().ok_or("stage required")?;
        let path = args.next().ok_or("private fixture input required")?;
        if args.next().is_some() {
            return Err::<Value, Error>("extra arguments refused".into());
        }
        let input = serde_json::from_slice::<Input>(&webdock_plugin_kit::read_file(
            Path::new(&path),
            true,
        )?)?;
        qualify(&stage, input).await
    }
    .await;
    match result {
        Ok(value) => println!("{value}"),
        Err(error) => {
            if error.downcast_ref::<sea_orm::DbErr>().is_some() {
                eprintln!("owned_buildlog_error_code_database");
            }
            if let Some(build) = error.downcast_ref::<webdock_build::Error>()
                && !build.code.is_empty()
                && build.code.len() <= 80
                && build
                    .code
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            {
                eprintln!("owned_buildlog_error_code_{}", build.code);
            }
            if let Some(hosting) = error.downcast_ref::<webdock_hosting::HostingError>()
                && (400..=599).contains(&hosting.status)
            {
                eprintln!("owned_buildlog_hosting_status_{}", hosting.status);
            }
            eprintln!("owned_native_buildlog_qualification_failed_private_material_withheld");
            std::process::exit(1);
        }
    }
}
