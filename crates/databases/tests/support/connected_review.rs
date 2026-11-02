//! Real connected transition acceptance, separate from the historical recovery drill.
use super::*;
pub(super) async fn run(h: &Harness, phase: &str) {
    match phase {
        "review-initial" => initial(h).await,
        "review-repair" => repair_and_resize(h).await,
        "review-rotate-restore" => rotate_restore(h).await,
        "review-export" => export(h).await,
        "review-stop" => stop(h).await,
        _ => panic!("unknown review phase"),
    }
}
async fn update(h: &Harness, s: &Value, suspended: bool, wal: bool, retention: bool) -> Value {
    let state = h
        .command(json!({"action":"databases.get","databaseID":s["database"]}))
        .await;
    let policy = h
        .command(json!({"action":"databases.backupPolicies.get","databaseID":s["database"]}))
        .await;
    let spec = &policy["policy"]["spec"];
    let result=h.command(json!({"action":"databases.backupPolicies.update","databaseID":s["database"],"revision":state["revision"],"policyRevision":policy["policy"]["revision"],"schedule":spec["schedule"],"suspended":suspended,"walEnabled":wal,"retentionDelete":retention,"retentionDays":spec["retentionDays"],"idempotencyKey":format!("review-policy-{}",uuid::Uuid::new_v4())})).await;
    h.finish(text(&result, "operationID")).await
}
async fn partition(h: &Harness, s: &Value) {
    let before = h
        .command(json!({"action":"databases.get","databaseID":s["database"]}))
        .await;
    assert_eq!(before["backup"]["status"], "configured");
    let request=h.command(json!({"action":"databases.backup","databaseID":s["database"],"revision":before["revision"],"idempotencyKey":format!("review-partition-{}",uuid::Uuid::new_v4())})).await;
    let claim = h.poll().await.unwrap();
    assert_eq!(claim.spec.operation_id, request["operationID"]);
    let client =
        kube::Client::try_from(kube::Config::new("https://127.0.0.1:1".parse().unwrap())).unwrap();
    let unreachable =
        NativeDatabaseExecutor::open(client, "/connected/runtime.json".into()).unwrap();
    assert!(unreachable.execute(&claim, &h.authority).await.is_err());
    let completion = DatabaseCompletion {
        lease: claim.lease.clone(),
        outcome: DatabaseCompletionOutcome::NeedsReconciliation,
        proof: None,
    };
    assert!(
        h.authority
            .client
            .clone()
            .complete_database(wire::DatabaseReceipt {
                identity: Some(h.authority.identity.clone()),
                spec_json: serde_json::to_vec(&claim.spec).unwrap(),
                request_sha256: claim.spec.fingerprint().unwrap(),
                completion_json: serde_json::to_vec(&completion).unwrap()
            })
            .await
            .unwrap()
            .into_inner()
            .accepted
    );
    let after = h
        .command(json!({"action":"databases.get","databaseID":s["database"]}))
        .await;
    let mut e = read("/connected/evidence.json");
    e["observationFailure"] = json!({"transport":"actual enrolled gRPC completion after real unreachable Native executor","before":before["backup"],"after":after["backup"],"noSuccessReceipt":true});
    private("/connected/evidence.json", &e);
    assert_ne!(
        after["backup"]["status"], "configured",
        "known failed observation must immediately stop public configured status"
    );
}
async fn initial(h: &Harness) {
    assert!(
        !Path::new("/connected/review-source.json").exists(),
        "use repair to resume the owned review fixture"
    );
    let customer = h.service_test_id().await;
    let project = h.service_test_id().await;
    let organization = h.service_test_id().await;
    exec(&h.owner,"WITH allocation AS(SELECT set_config('webdock.organization_allocation',$2,true)) INSERT INTO webdock_admin.customers(id,name) SELECT $1,'Review connected customer' FROM allocation",vec![customer.clone().into(),json!({"customer_id":customer,"organization_id":organization}).to_string().into()]).await;
    exec(&h.owner,"INSERT INTO webdock_admin.projects(id,customer_id,name) VALUES($1,$2,'Review connected project')",vec![project.clone().into(),customer.clone().into()]).await;
    h.command(json!({"action":"databases.pools.register","clusterID":h.cluster,"node":"backup-qualification","poolID":h.pool,"revision":1,"idempotencyKey":"review-connected-pool"})).await;
    let plan=h.command(json!({"action":"databases.plans.create","plan":{"kind":"dedicated","postgresMajor":18,"cpuMillicores":500,"memoryBytes":1073741824,"volumeBytes":1610612736,"logicalStorageAllowanceBytes":null,"name":"Review connected plan","monthlyPriceCents":1200,"currency":"EUR","connectionLimit":10,"terms":"Disposable local review fixture; no production service or recovery commitment."},"idempotencyKey":"review-connected-plan"})).await;
    h.command(json!({"action":"databases.plans.setAvailability","planRevisionID":plan["id"],"availability":"pilot","revision":1,"idempotencyKey":"review-connected-available"})).await;
    let accepted = h
        .order(&project, &plan, "Review source", "review-source")
        .await;
    let create = h.finish(text(&accepted, "operationID")).await;
    let ns = text(&create["spec"]["backend"], "namespace");
    let database = text(&accepted, "databaseID");
    sql(
        ns,
        text(&create["spec"]["database"], "name"),
        &format!(
            "SET ROLE wd_migrate_{database}; CREATE TABLE app.review_probe(id integer PRIMARY KEY); INSERT INTO app.review_probe VALUES(1); RESET ROLE;"
        ),
    );
    let destination=h.command(json!({"action":"databases.backupDestinations.register","endpointURL":h.config["endpoint"],"region":"us-east-1","bucket":h.config["bucket"],"keyRecoveryID":"110","writeCredentials":{"accessKeyId":h.config["write"]["access"],"secretAccessKey":h.config["write"]["secret"],"readOnly":false},"recoveryCredentials":{"accessKeyId":h.config["read"]["access"],"secretAccessKey":h.config["read"]["secret"],"readOnly":true},"idempotencyKey":"review-connected-destination"})).await;
    let time = chrono::Utc::now() + chrono::Duration::minutes(1);
    let schedule = format!("0 {} {} * * *", time.format("%M"), time.format("%H"));
    let grant=h.command(json!({"action":"databases.backupPolicies.grant","databaseID":database,"revision":1,"destinationID":destination["destinationID"],"schedule":schedule,"retentionDays":30,"retentionDelete":true,"idempotencyKey":"review-connected-grant"})).await;
    let configured = h.finish(text(&grant, "operationID")).await;
    let source = json!({"customer":customer,"project":project,"database":database,"backend":create["spec"]["backend"]["id"],"namespace":ns,"planID":plan["id"],"policyID":grant["policyID"],"sourceUID":create["proof"]["cnpgUID"]});
    private("/connected/review-source.json", &source);
    private(
        "/connected/evidence.json",
        &json!({"fixtureOnly":true,"currentReviewAcceptance":true,"create":create,"grant":configured,"source":source}),
    );
    let objects=(0..103).map(|index|json!({"apiVersion":"postgresql.cnpg.io/v1","kind":"Backup","metadata":{"name":format!("history-manual-{index:03}"),"namespace":ns},"spec":{"cluster":{"name":"absent-fixture-cluster"},"method":"plugin","pluginConfiguration":{"name":"barman-cloud.cloudnative-pg.io"}}})).collect::<Vec<_>>();
    kubectl(
        &["apply", "-f", "-"],
        Some(&json!({"apiVersion":"v1","kind":"List","items":objects}).to_string()),
    );
    let deadline = Instant::now() + Duration::from_secs(210);
    let observed;
    loop {
        h.observe().await;
        if let Some(claim) = h.poll().await {
            let done = h.finish(&claim.spec.operation_id).await;
            if done["proof"]["backup"]["baseBackupId"].is_string() {
                observed = done;
                break;
            }
        }
        assert!(
            Instant::now() < deadline,
            "real daily backup and paginated observation must complete"
        );
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    let history: Value =
        serde_json::from_str(&kubectl(&["get", "backups", "-n", ns, "-o", "json"], None)).unwrap();
    assert!(history["items"].as_array().unwrap().len() >= 104);
    assert!(serde_json::to_vec(&observed["proof"]).unwrap().len() < 4096);
    let mut e = read("/connected/evidence.json");
    e["paginatedHistory"] = json!({"objectsRetained":history["items"].as_array().unwrap().len(),"actualScheduledCompletedProof":observed,"boundedProof":true});
    private("/connected/evidence.json", &e);
    partition(h, &source).await;
}
async fn repair_and_resize(h: &Harness) {
    let source = read("/connected/review-source.json");
    let repaired = update(h, &source, false, true, true).await;
    partition(h, &source).await;
    let recovered = update(h, &source, false, true, true).await;
    let mut plan = row(
        &h.owner,
        "SELECT plan AS value FROM webdock_auth.hosting_database_plan WHERE id=$1",
        text(&source, "planID"),
    )
    .await;
    plan["cpuMillicores"] = json!(1000);
    let plan=h.command(json!({"action":"databases.plans.create","plan":plan,"idempotencyKey":format!("review-resize-plan-{}",uuid::Uuid::new_v4())})).await;
    h.command(json!({"action":"databases.plans.setAvailability","planRevisionID":plan["id"],"availability":"pilot","revision":1,"idempotencyKey":format!("review-resize-available-{}",uuid::Uuid::new_v4())})).await;
    let offer=h.command(json!({"action":"databases.offers.create","databaseID":source["database"],"projectID":source["project"],"planRevisionID":plan["id"],"name":"Review resize","idempotencyKey":format!("review-resize-offer-{}",uuid::Uuid::new_v4())})).await;
    let started = Instant::now();
    let accepted=h.command(json!({"action":"databases.acceptOrder","projectID":source["project"],"offerID":offer["id"],"confirmInterruption":true,"idempotencyKey":format!("review-resize-accept-{}",uuid::Uuid::new_v4())})).await;
    let resized = h.finish(text(&accepted, "operationID")).await;
    let pending = h
        .poll()
        .await
        .expect("resized backup policy needs a fresh automatic configuration claim");
    assert_eq!(pending.spec.action, DatabaseAction::ConfigureBackup);
    assert_eq!(pending.spec.phase, DatabaseExecutionPhase::Apply);
    let reobserved = h.finish(&pending.spec.operation_id).await;
    let seconds = started.elapsed().as_secs_f64();
    let ns = text(&source, "namespace");
    let quota = object("resourcequota", "database-budget", ns);
    let hard = &quota["spec"]["hard"];
    assert_eq!(
        webdock_agent::kubernetes::parse_quantity(text(hard, "requests.cpu"), true).unwrap(),
        1800
    );
    assert_eq!(
        webdock_agent::kubernetes::parse_quantity(text(hard, "requests.memory"), false).unwrap(),
        2415919104
    );
    assert_eq!(
        webdock_agent::kubernetes::parse_quantity(text(hard, "requests.ephemeral-storage"), false)
            .unwrap(),
        1342177280
    );
    let backend = row(
        &h.owner,
        "SELECT to_jsonb(b) AS value FROM webdock_auth.hosting_database_backend b WHERE id=$1",
        text(&source, "backend"),
    )
    .await;
    assert_eq!(backend["demand"]["cpuMillicores"], 1800);
    assert_eq!(backend["demand"]["memoryBytes"], 2415919104_u64);
    let policy = h
        .command(json!({"action":"databases.backupPolicies.get","databaseID":source["database"]}))
        .await;
    assert_eq!(
        policy["policy"]["spec"]["resourceRevision"],
        backend["revision"]
    );
    assert_eq!(
        policy["policy"]["observedRevision"],
        policy["policy"]["revision"]
    );
    assert_eq!(policy["policy"]["spec"]["walEnabled"], true);
    assert_eq!(policy["policy"]["spec"]["retentionDelete"], true);
    assert_eq!(
        object("cluster", "postgres", ns)["metadata"]["uid"],
        source["sourceUID"]
    );
    let mut e = read("/connected/evidence.json");
    e["repair"] = json!({"first":repaired,"afterKnownFailure":recovered,"sqlRowRepairs":false});
    e["resize"] = json!({"acceptedThroughReobservedBackupSeconds":seconds,"native":resized,"backupReconfiguration":reobserved,"quota":quota,"demand":backend["demand"],"policy":policy["policy"],"cpuBefore":500,"cpuAfter":1000,"memoryBytes":1073741824,"sourceUIDPreserved":true});
    private("/connected/evidence.json", &e);
}
async fn stop(h: &Harness) {
    let source = read("/connected/review-source.json");
    let proof = update(h, &source, true, false, false).await;
    let state = h
        .command(json!({"action":"databases.get","databaseID":source["database"]}))
        .await;
    let rejected=h.service.execute_managed(&h.actor,&StoredAuthority::LocalSession{},json!({"action":"databases.backup","databaseID":source["database"],"revision":state["revision"],"idempotencyKey":"review-live-suspended-request"})).await;
    assert!(rejected.is_err());
    assert_eq!(proof["proof"]["backup"]["controls"]["walEnabled"], false);
    assert_eq!(
        proof["proof"]["backup"]["controls"]["retentionDelete"],
        false
    );
    assert_eq!(proof["proof"]["backup"]["suspended"], true);
    let current = h
        .command(json!({"action":"databases.get","databaseID":source["database"]}))
        .await;
    let request=h.command(json!({"action":"databases.pause","databaseID":source["database"],"revision":current["revision"],"idempotencyKey":format!("review-actual-pause-{}",uuid::Uuid::new_v4())})).await;
    let paused = h.finish(text(&request, "operationID")).await;
    let business = Database::connect(std::env::var("DATABASE_URL").unwrap())
        .await
        .unwrap();
    let previous = read("/connected/evidence.json")["managedExportQualification"].clone();
    let denied = super::export_qualification::reject_transition(
        &h.service,
        &h.actor,
        &business,
        text(&source, "customer"),
        text(&source, "project"),
        text(&source, "database"),
        &previous,
        "suspension",
    )
    .await;
    let mut e = read("/connected/evidence.json");
    e["managedExportSuspensionDenial"] = denied;
    e["actualNativePause"] = paused;
    e["observedStop"] = proof;
    e["suspendedAdmissionRejected"] = json!(true);
    private("/connected/evidence.json", &e);
}

async fn rotate_restore(h: &Harness) {
    let source = read("/connected/review-source.json");
    let state = h
        .command(json!({"action":"databases.get","databaseID":source["database"]}))
        .await;
    let backup=h.command(json!({"action":"databases.backup","databaseID":source["database"],"revision":state["revision"],"idempotencyKey":format!("review-role-backup-{}",uuid::Uuid::new_v4())})).await;
    let backed = h.finish(text(&backup, "operationID")).await;
    let points = h
        .command(json!({"action":"databases.backupPolicies.get","databaseID":source["database"]}))
        .await;
    let point = points["backups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|point| point["baseBackupID"] == backed["proof"]["backup"]["baseBackupId"])
        .unwrap();
    let ns = text(&source, "namespace");
    let name = format!("db_{}", text(&source, "database"));
    let target_time = sql(
        ns,
        "postgres",
        r#"SELECT to_char(clock_timestamp() AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"');"#,
    );
    let digest = sql(
        ns,
        &name,
        include_str!("../../../agent/src/database/backup-digest.sql"),
    );
    kubectl(
        &[
            "patch",
            "cluster",
            "postgres",
            "-n",
            ns,
            "--type=merge",
            "-p",
            r#"{"spec":{"postgresql":{"parameters":{"archive_timeout":"10s"}}}}"#,
        ],
        None,
    );
    tokio::time::sleep(Duration::from_secs(2)).await;
    let rotating=h.command(json!({"action":"databases.rotateCredentials","databaseID":source["database"],"revision":state["revision"],"idempotencyKey":format!("review-readonly-rotation-{}",uuid::Uuid::new_v4())})).await;
    let rotated = h.finish(text(&rotating, "operationID")).await;
    tokio::time::sleep(Duration::from_secs(18)).await;
    let current = h
        .command(json!({"action":"databases.get","databaseID":source["database"]}))
        .await;
    let plan=row(&h.owner,"SELECT jsonb_build_object('id',plan_revision_id) AS value FROM webdock_auth.hosting_database WHERE id=$1",text(&source,"database")).await;
    let accepted = h
        .order(
            text(&source, "project"),
            &plan,
            "Review historical readonly restore",
            "review-restore",
        )
        .await;
    let started = Instant::now();
    let restoring=h.command(json!({"action":"databases.restore","sourceDatabaseID":source["database"],"revision":current["revision"],"restorePointID":point["id"],"targetProjectID":source["project"],"targetOrderID":accepted["orderID"],"name":"Review historical readonly restore","credentialRevision":1,"targetTime":target_time,"timeline":1,"verificationSHA256":digest,"idempotencyKey":format!("review-readonly-restore-{}",uuid::Uuid::new_v4())})).await;
    let restored = h.finish(text(&restoring, "operationID")).await;
    let seconds = started.elapsed().as_secs_f64();
    let target_ns = text(&restored["spec"]["backend"], "namespace");
    let newname = text(&restored["spec"]["database"], "name");
    assert_eq!(
        sql(target_ns, newname, "SELECT count(*) FROM app.review_probe;"),
        "1"
    );
    assert_eq!(
        sql(
            target_ns,
            "postgres",
            &format!(
                "SELECT rolcanlogin FROM pg_roles WHERE rolname='wd_export_{}';",
                text(&source, "database")
            )
        ),
        "f",
        "restored old export identity is disabled"
    );
    let mut e = read("/connected/evidence.json");
    e["readonlyRotationAndRestore"] = json!({"nativeRotation":rotated,"nativeHistoricalRestore":restored,"restoreRequestSeconds":seconds,"historicalCredentialRevision":1,"sourceCredentialRevision":2,"newDerivedReadonlyLoginAndPrivilegesValidatedByNativeExecutor":true,"oldSourceExportIdentityDisabled":true,"passwordTransport":"psql stdin only; shared HMAC derivation, no second credential truth","forcedWalSwitch":false});
    private("/connected/evidence.json", &e);
}

async fn export(h: &Harness) {
    while let Some(pending) = h.poll().await {
        h.finish(&pending.spec.operation_id).await;
    }
    let source = read("/connected/review-source.json");
    let business = Database::connect(std::env::var("DATABASE_URL").unwrap())
        .await
        .unwrap();
    let first = super::export_qualification::qualify(
        &h.service,
        &h.actor,
        &business,
        text(&source, "customer"),
        text(&source, "project"),
        text(&source, "database"),
    )
    .await;
    let state = h
        .command(json!({"action":"databases.get","databaseID":source["database"]}))
        .await;
    let request=h.command(json!({"action":"databases.rotateCredentials","databaseID":source["database"],"revision":state["revision"],"idempotencyKey":format!("review-export-binding-rotation-{}",uuid::Uuid::new_v4())})).await;
    let rotated = h.finish(text(&request, "operationID")).await;
    let result = super::export_qualification::reject_transition(
        &h.service,
        &h.actor,
        &business,
        text(&source, "customer"),
        text(&source, "project"),
        text(&source, "database"),
        &first,
        "rotation",
    )
    .await;
    let mut e = read("/connected/evidence.json");
    e["managedExportQualification"] = result;
    e["managedExportNativeRotation"] = rotated;
    private("/connected/evidence.json", &e);
}
