//! Fresh service publication handoff in owned PostgreSQL; no external registry or runtime I/O.
use super::build_kubernetes_admission::{Repository, oci_envelope};
use super::*;
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use webdock_build::BuildService;

async fn state(f: &Fixture, build: &str) -> Value {
    f.owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT to_jsonb(b)||jsonb_build_object('operation_count',(SELECT count(*) FROM webdock_auth.hosting_operation WHERE project_id=b.project_id AND action IN ('apps.deploy','apps.update'))) AS value FROM webdock_auth.git_build b WHERE b.id=$1 AND b.customer_id=$2",
            vec![build.into(), f.cid.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "value")
        .unwrap()
}

fn unpublished(row: &Value) {
    assert_eq!(row["status"], "artifact-ready");
    assert!(row["registry_verified_at"].is_null());
    assert!(row["finished_at"].is_null());
    assert_eq!(row["operation_count"], 0);
}

#[tokio::test]
#[ignore = "owned fresh native database via tools/tests/kubernetes-builds.py"]
async fn kubernetes_publication_stages_fresh_operation_before_build_success_and_replays_receipt() {
    let fixture = Arc::new(Fixture::new("kubernetes").await);
    let namespace = format!("owned-publication-{}", fixture.cluster);
    let evidence = format!("Owned local publication fixture {}", fixture.nonce);
    let f = fixture.clone();
    let build_namespace = namespace.clone();
    let worker_evidence = evidence.clone();
    let outcome = tokio::spawn(async move {
        f.configure("k3s").await;
        sql(&f.owner,"UPDATE webdock_auth.hosting_agent SET observation=jsonb_set(observation,'{capabilities,registryVersion}','1') WHERE cluster_id=$1",vec![f.cluster.clone().into()]).await;
        let service = BuildService::new(
            db("DATABASE_URL").await,
            db("WEBDOCK_PLUGIN_WORKER_DATABASE_URL").await,
            std::env::var("DATABASE_RLS_SECRET").unwrap(),
            Arc::new(webdock_persistence::IdAllocator::new(
                db("DATABASE_URL").await,
                webdock_ids::Identity::platform(0, 2).unwrap(),
            )),
            f.service.clone(),
            "registry.invalid".into(),
        )
        .await
        .unwrap()
        .with_repository_provider(Arc::new(Repository));
        let mut draft = spec();
        draft.as_object_mut().unwrap().remove("image");
        draft.as_object_mut().unwrap().remove("template");
        let app=f.service.execute(&f.principal,json!({"action":"apps.prepare","projectID":f.pid,"name":"kubernetes-publication","spec":draft,"subscriptionRevision":1,"idempotencyKey":"kubernetes-publication-target"})).await.unwrap();
        let target = app["appID"].as_str().or_else(|| app["id"].as_str()).unwrap();
        let connection = id(&f.owner).await;
        sql(&f.owner,"INSERT INTO webdock_auth.git_connection(id,customer_id,installation_id,account_id,account_login,permissions,state) VALUES($1,$2,'1234','1234','fixture','{}','active')",vec![connection.clone().into(),f.cid.clone().into()]).await;
        sql(&f.owner,"INSERT INTO webdock_auth.git_repository(connection_id,customer_id,repository_id,metadata) VALUES($1,$2,'55','{}')",vec![connection.clone().into(),f.cid.clone().into()]).await;
        let enrolled = service
            .enroll_worker(&f.principal, "DE", "artifact-only", &worker_evidence, 1)
            .await
            .unwrap();
        let worker = service
            .authenticate_worker(enrolled["credential"].as_str().unwrap())
            .await
            .unwrap();
        let cluster = service
            .enroll_kubernetes_cluster(
                &f.principal,
                worker.id(),
                &build_namespace,
                "DE",
                &worker_evidence,
                1,
            )
            .await
            .unwrap();
        let cluster = cluster["id"].as_str().unwrap();
        service.enable_kubernetes_cluster(&f.principal, cluster, true).await.unwrap();
        service.configure_kubernetes_admission(&f.principal, true, 1, 1).await.unwrap();
        service.trust_kubernetes_project(&f.principal, &f.cid, &f.pid, true).await.unwrap();
        service.execute(&f.principal,json!({"action":"git.source.configure","projectID":f.pid,"targetID":target,"connectionID":connection,"repositoryID":"55","branch":"main","rootDirectory":".","recipe":"dockerfile","executionBackend":"kubernetes","autoPublish":false,"revision":0})).await.unwrap();
        let requested=service.execute(&f.principal,json!({"action":"git.builds.request","projectID":f.pid,"targetID":target,"idempotencyKey":"kubernetes-publication-build"})).await.unwrap();
        let build = requested["id"].as_str().unwrap();
        let claim = service.claim_kubernetes_build(&worker, cluster, &"1".repeat(64)).await.unwrap().unwrap();
        assert_eq!(claim["buildID"], build);
        let generation = claim["generation"].as_i64().unwrap() as i32;
        service.record_kubernetes_job_intent(&worker, build, generation, &"2".repeat(64)).await.unwrap();
        service.record_kubernetes_job(&worker, build, generation, "owned-publication-job").await.unwrap();
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let store = webdock_build::artifact::ArtifactStore::open(directory.path().into(), webdock_build::artifact::MAX_OUTPUT * 2).unwrap();
        let artifact = store.put(&f.cid, build, generation, &oci_envelope()).unwrap();
        let completed = service.complete_build(&worker, build, generation, Some(&artifact), "owned publication build output", None).await.unwrap();
        assert_eq!(completed["status"], "artifact-ready");
        assert_eq!(completed["release"]["status"], "awaiting-approval");
        unpublished(&state(&f, build).await);
        service.retire_kubernetes_attempt(&worker, build, generation, Some("owned-publication-job")).await.unwrap();

        let repository = format!("registry.invalid/customers/{}/projects/{}", f.cid, f.pid);
        let projects = BTreeMap::from([(f.pid.clone(), repository.clone())]);
        assert!(service.claim_release_with_storage(&worker, &[], &projects).await.unwrap().is_none(), "manual approval is required");
        let release = completed["release"]["id"].as_str().unwrap();
        let approved=service.execute(&f.principal,json!({"action":"git.releases.approve","projectID":f.pid,"releaseID":release,"revision":completed["release"]["revision"],"idempotencyKey":"kubernetes-publication-approval"})).await.unwrap();
        assert_eq!(approved["status"], "queued");
        unpublished(&state(&f, build).await);
        let leased = service.claim_release_with_storage(&worker, &[], &projects).await.unwrap().unwrap();
        assert_eq!(leased["id"], release);
        assert_eq!(leased["status"], "deploying");
        let release_generation = leased["generation"].as_i64().unwrap() as i32;
        unpublished(&state(&f, build).await);
        let prepared = service.prepare_publication(&worker, release, release_generation).await.unwrap();
        assert_eq!(prepared["repository"], repository);
        assert_eq!(prepared["imageDigest"], artifact.image_digest());
        assert_eq!(prepared["storageKey"], artifact.storage_key());
        let pending = state(&f, build).await;
        unpublished(&pending);
        assert_eq!(pending["phase"], "pushing");
        let image = format!("{repository}@{}", artifact.image_digest());
        assert!(service.publish_image(&worker, release, release_generation, &format!("{repository}@sha256:{}", "f".repeat(64))).await.is_err());
        unpublished(&state(&f, build).await);

        // The trusted worker supplies the exact validated fixture digest after registry I/O.
        let published = service.publish_image(&worker, release, release_generation, &image).await.unwrap();
        let operation = published["id"].as_str().unwrap();
        assert_eq!(published["status"], "queued");
        let built = state(&f, build).await;
        assert_eq!(built["status"], "succeeded");
        assert_eq!(built["phase"], "succeeded");
        assert!(!built["registry_verified_at"].is_null());
        assert!(!built["finished_at"].is_null());
        assert_eq!(built["operation_count"], 1);
        let staged=f.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT to_jsonb(o)||jsonb_build_object('release_operation',r.operation_id,'release_status',r.status) AS value FROM webdock_auth.hosting_operation o JOIN webdock_auth.git_release r ON r.id=o.git_release_id WHERE o.id=$1 AND o.project_id=$2 AND o.customer_id=$3",vec![operation.into(),f.pid.clone().into(),f.cid.clone().into()])).await.unwrap().unwrap().try_get::<Value>("","value").unwrap();
        assert_eq!(staged["git_release_id"], release);
        assert_eq!(staged["release_operation"], operation);
        assert_eq!(staged["release_status"], "deploying");
        assert_eq!(staged["app_id"], target);
        assert_eq!(staged["desired"]["spec"]["image"], image);
        assert_eq!(staged["action"], "apps.deploy");
        let deploying = service.observe_release(&worker, release, release_generation).await.unwrap();
        assert_eq!(deploying["status"], "deploying");
        assert_eq!(deploying["healthVerified"], false);

        sql(&f.owner,"UPDATE webdock_auth.hosting_operation SET status='failed' WHERE id=$1 AND project_id=$2 AND git_release_id=$3",vec![operation.into(),f.pid.clone().into(),release.into()]).await;
        let failed = service.observe_release(&worker, release, release_generation).await.unwrap();
        assert_eq!(failed["status"], "failed");
        assert_eq!(failed["operationID"], operation);
        assert_eq!(failed["healthVerified"], false);
        assert_eq!(state(&f, build).await, built, "rollout failure does not rewrite build publication success");
        sql(&f.owner,"UPDATE webdock_auth.git_release SET lease_until=now()-interval '1 second' WHERE id=$1 AND project_id=$2",vec![release.into(),f.pid.clone().into()]).await;
        for _ in 0..2 {
            let replay = service.publish_image(&worker, release, release_generation, &image).await.unwrap();
            assert_eq!(replay["replayed"], true);
            assert_eq!(replay["operationID"], operation);
            assert_eq!(replay["status"], "failed");
        }
        assert_eq!(service.publish_image(&worker, release, release_generation, &format!("{image}changed")).await.unwrap_err().code, "publication_image_conflict");
        assert!(service.publish_image(&worker, release, release_generation + 1, &image).await.is_err());
        assert_eq!(state(&f, build).await, built, "replays and rejected callbacks cannot create another operation");
    }).await;
    sql(&fixture.owner,"UPDATE webdock_auth.git_build_admission SET enabled=false,global_slots=1,tenant_slots=1 WHERE singleton",vec![]).await;
    for table in [
        "hosting_reservation",
        "hosting_operation",
        "git_build_log_entry",
        "git_build_attempt",
        "git_build_trusted_project",
        "git_release",
        "git_artifact",
        "git_check_outbox",
        "git_build",
        "git_source",
        "git_repository",
        "git_connection",
    ] {
        let column = if table == "git_check_outbox" {
            "build_id IN (SELECT id FROM webdock_auth.git_build WHERE customer_id=$1)"
        } else {
            "customer_id=$1"
        };
        sql(
            &fixture.owner,
            &format!("DELETE FROM webdock_auth.{table} WHERE {column}"),
            vec![fixture.cid.clone().into()],
        )
        .await;
    }
    sql(
        &fixture.owner,
        "DELETE FROM webdock_auth.git_build_cluster WHERE namespace=$1 AND evidence=$2",
        vec![namespace.into(), evidence.clone().into()],
    )
    .await;
    sql(
        &fixture.owner,
        "DELETE FROM webdock_auth.git_worker WHERE evidence=$1",
        vec![evidence.into()],
    )
    .await;
    fixture.cleanup().await;
    outcome.unwrap();
}
