//! Database admission only: no VM, registry, provider or real Kubernetes API calls.
use super::*;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use spitzli_build::{BuildService, RepositoryProvider, RepositoryRef, VerifiedCommit};
use tower::ServiceExt;

pub(super) fn oci_envelope() -> Vec<u8> {
    use spitzli_build::artifact::Archive;
    use std::collections::BTreeMap;
    let config = b"{}".to_vec();
    let config_hash = hex::encode(Sha256::digest(&config));
    let manifest=serde_json::to_vec(&json!({"schemaVersion":2,"mediaType":"application/vnd.oci.image.manifest.v1+json","config":{"mediaType":"application/vnd.oci.image.config.v1+json","size":config.len(),"digest":format!("sha256:{config_hash}")},"layers":[]})).unwrap();
    let digest = hex::encode(Sha256::digest(&manifest));
    let index=serde_json::to_vec(&json!({"schemaVersion":2,"manifests":[{"mediaType":"application/vnd.oci.image.manifest.v1+json","size":manifest.len(),"digest":format!("sha256:{digest}")}]})).unwrap();
    let image = Archive::from_files(BTreeMap::from([
        (
            "oci-layout".into(),
            br#"{"imageLayoutVersion":"1.0.0"}"#.to_vec(),
        ),
        ("index.json".into(), index),
        (format!("blobs/sha256/{config_hash}"), config),
        (format!("blobs/sha256/{digest}"), manifest),
    ]))
    .unwrap()
    .encode()
    .unwrap();
    Archive::from_files(BTreeMap::from([("image.tar".into(), image)]))
        .unwrap()
        .encode()
        .unwrap()
}

pub(super) struct Repository;
#[async_trait::async_trait]
impl RepositoryProvider for Repository {
    async fn resolve(
        &self,
        _: &RepositoryRef<'_>,
        _: &str,
    ) -> spitzli_build::Result<VerifiedCommit> {
        VerifiedCommit::new("a".repeat(40))
    }
    async fn archive(&self, _: &RepositoryRef<'_>, _: &str) -> spitzli_build::Result<Vec<u8>> {
        unreachable!("admission fixture never exports repository bytes")
    }
}

#[tokio::test]
#[ignore = "owned fresh native database via tools/tests/kubernetes-builds.py"]
async fn kubernetes_admission_reserves_uncertain_jobs_and_fences_revoked_generations() {
    let fixture = Arc::new(Fixture::new("kubernetes").await);
    let other_project = id(&fixture.owner).await;
    let other_cleanup = other_project.clone();
    let f = fixture.clone();
    let task = async move {
        f.configure("k3s").await;
        let service = Arc::new(
            BuildService::new(
                db("DATABASE_URL").await,
                db("SPITZLI_PLUGIN_WORKER_DATABASE_URL").await,
                std::env::var("DATABASE_RLS_SECRET").unwrap(),
                Arc::new(spitzli_persistence::IdAllocator::new(
                    db("DATABASE_URL").await,
                    spitzli_ids::Identity::platform(0, 2).unwrap(),
                )),
                f.service.clone(),
                "registry.invalid".into(),
            )
            .await
            .unwrap()
            .with_repository_provider(Arc::new(Repository)),
        );
        let mut draft = spec();
        draft.as_object_mut().unwrap().remove("image");
        draft.as_object_mut().unwrap().remove("template");
        let app=f.service.execute(&f.principal,json!({"action":"apps.prepare","projectID":f.pid,"name":"kubernetes-admission","spec":draft,"subscriptionRevision":1,"idempotencyKey":"kubernetes-admission-target"})).await.unwrap();
        let target = app["appID"]
            .as_str()
            .or_else(|| app["id"].as_str())
            .unwrap();
        let connection = id(&f.owner).await;
        sql(&f.owner,"INSERT INTO webdock_auth.git_connection(id,customer_id,installation_id,account_id,account_login,permissions,state) VALUES($1,$2,'1234','1234','fixture','{}','active')",vec![connection.clone().into(),f.cid.clone().into()]).await;
        sql(&f.owner,"INSERT INTO webdock_auth.git_repository(connection_id,customer_id,repository_id,metadata) VALUES($1,$2,'55','{}')",vec![connection.clone().into(),f.cid.clone().into()]).await;
        let configured=service.execute(&f.principal,json!({"action":"git.source.configure","projectID":f.pid,"targetID":target,"connectionID":connection,"repositoryID":"55","branch":"main","rootDirectory":".","recipe":"dockerfile","revision":0,"buildEnvironment":[{"name":"BUILD_TOKEN","value":"owned-private-log-secret"}]})).await.unwrap();
        assert_eq!(configured["executionBackend"], "microvm");
        assert_eq!(service.execute(&f.principal,json!({"action":"git.source.configure","projectID":f.pid,"targetID":target,"connectionID":connection,"repositoryID":"55","branch":"main","rootDirectory":".","recipe":"dockerfile","executionBackend":"kubernetes","revision":1})).await.unwrap_err().code,"kubernetes_project_not_trusted");
        let source = configured["id"].as_str().unwrap();
        sql(&f.owner,"UPDATE webdock_auth.git_source SET execution_backend='kubernetes',revision=revision+1 WHERE id=$1",vec![source.into()]).await;
        let enrolled = service
            .enroll_worker(
                &f.principal,
                "DE",
                "artifact-only",
                "Owned disposable local admission fixture",
                2,
            )
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
                "owned-build-fixture",
                "DE",
                "Owned disposable local admission fixture",
                2,
            )
            .await
            .unwrap();
        let cluster = cluster["id"].as_str().unwrap();
        let runtime = "1".repeat(64);
        assert!(
            service
                .claim_kubernetes_build(&worker, cluster, &runtime)
                .await
                .is_err(),
            "new cluster starts disabled"
        );
        service
            .enable_kubernetes_cluster(&f.principal, cluster, true)
            .await
            .unwrap();
        assert_eq!(
            service
                .claim_kubernetes_build(&worker, cluster, &runtime)
                .await
                .unwrap_err()
                .code,
            "kubernetes_disabled"
        );
        service
            .configure_kubernetes_admission(&f.principal, true, 4, 1)
            .await
            .unwrap();
        let request = |key: &str| json!({"action":"git.builds.request","projectID":f.pid,"targetID":target,"idempotencyKey":key});
        let first = service
            .execute(&f.principal, request("kubernetes-first"))
            .await
            .unwrap();
        let second = service
            .execute(&f.principal, request("kubernetes-second"))
            .await
            .unwrap();
        assert_ne!(first["id"], second["id"]);
        assert!(
            service
                .claim_kubernetes_build(&worker, cluster, &runtime)
                .await
                .unwrap()
                .is_none(),
            "explicit project trust is required"
        );
        service
            .trust_kubernetes_project(&f.principal, &f.cid, &f.pid, true)
            .await
            .unwrap();
        let vm = service
            .enroll_worker(
                &f.principal,
                "DE",
                "microvm",
                "Owned disposable legacy guard fixture",
                1,
            )
            .await
            .unwrap();
        let vm_credential = vm["credential"].as_str().unwrap().to_owned();
        let vm = service
            .authenticate_worker(vm["credential"].as_str().unwrap())
            .await
            .unwrap();
        assert!(
            service.claim_build(&vm).await.unwrap().is_none(),
            "legacy VM claims cannot consume Kubernetes builds"
        );
        assert_eq!(
            service.claim_build(&worker).await.unwrap_err().code,
            "build_isolation_unavailable"
        );
        let router = spitzli_build::http::worker_router(service.clone());
        let post = |path: &str, credential: Option<&str>, body: Value| {
            let mut request = Request::builder()
                .method("POST")
                .uri(format!("/{path}"))
                .header("content-type", "application/json");
            if let Some(credential) = credential {
                request = request.header("authorization", format!("Bearer {credential}"));
            }
            request.body(Body::from(body.to_string())).unwrap()
        };
        let credential = enrolled["credential"].as_str().unwrap();
        let claim = json!({"clusterID":cluster,"runtimeFingerprint":runtime});
        assert_eq!(
            router
                .clone()
                .oneshot(post("kubernetes-claim", None, claim.clone()))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            router
                .clone()
                .oneshot(post(
                    "kubernetes-claim",
                    Some(&vm_credential),
                    claim.clone()
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            router
                .clone()
                .oneshot(post(
                    "kubernetes-claim",
                    Some(credential),
                    json!({"clusterID":cluster,"runtimeFingerprint":runtime,"extra":true})
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            router
                .clone()
                .oneshot(post(
                    "kubernetes-intent",
                    Some(credential),
                    json!({"buildID":first["id"],"generation":0,"specFingerprint":"2".repeat(64)})
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
        let (a, b) = tokio::join!(
            service.claim_kubernetes_build(&worker, cluster, &runtime),
            service.claim_kubernetes_build(&worker, cluster, &runtime)
        );
        let a = a.unwrap();
        let b = b.unwrap();
        assert_eq!(
            u8::from(a.is_some()) + u8::from(b.is_some()),
            1,
            "concurrent claims respect the slot transaction"
        );
        let claimed = a.or(b).unwrap();
        let build = claimed["buildID"].as_str().unwrap();
        let generation = claimed["generation"].as_i64().unwrap() as i32;
        assert_eq!(claimed["attempt"], claimed["generation"]);
        let spec = "2".repeat(64);
        service
            .record_kubernetes_job_intent(&worker, build, generation, &spec)
            .await
            .unwrap();
        service
            .record_kubernetes_job_intent(&worker, build, generation, &spec)
            .await
            .unwrap();
        assert_eq!(
            service
                .record_kubernetes_job_intent(&worker, build, generation, &"3".repeat(64))
                .await
                .unwrap_err()
                .code,
            "kubernetes_fingerprint_conflict"
        );
        service
            .record_kubernetes_job(&worker, build, generation, "owned-job-uid")
            .await
            .unwrap();
        service
            .record_kubernetes_job(&worker, build, generation, "owned-job-uid")
            .await
            .unwrap();
        assert_eq!(
            service
                .record_kubernetes_job(&worker, build, generation, "different-job-uid")
                .await
                .unwrap_err()
                .code,
            "kubernetes_uid_conflict"
        );
        service
            .guard_build(&worker, build, generation)
            .await
            .unwrap();
        let entry = service
            .append_kubernetes_log(
                &worker,
                build,
                generation,
                "owned-log-entry-0001",
                "building",
                "secret owned-private-log-secret",
            )
            .await
            .unwrap();
        assert_eq!(
            service
                .append_kubernetes_log(
                    &worker,
                    build,
                    generation,
                    "owned-log-entry-0001",
                    "building",
                    "secret owned-private-log-secret"
                )
                .await
                .unwrap(),
            entry,
            "log retries use durable idempotency keys"
        );
        assert_eq!(
            service
                .append_kubernetes_log(
                    &worker,
                    build,
                    generation,
                    "owned-log-entry-0001",
                    "building",
                    "changed content"
                )
                .await
                .unwrap_err()
                .code,
            "build_log_conflict"
        );
        let page=service.execute(&f.principal,json!({"action":"git.builds.logs","projectID":f.pid,"buildID":build,"after":"0","limit":25})).await.unwrap();
        assert_eq!(page["entries"].as_array().unwrap().len(), 1);
        assert!(!page.to_string().contains("owned-private-log-secret"));
        assert!(
            page["entries"][0]["text"]
                .as_str()
                .unwrap()
                .contains("[REDACTED]")
        );
        let cursor = page["cursor"].as_str().unwrap();
        assert!(service.execute(&f.principal,json!({"action":"git.builds.logs","projectID":f.pid,"buildID":build,"after":cursor,"limit":25})).await.unwrap()["entries"].as_array().unwrap().is_empty());
        // A real Railpack build produced 282 durable entries. Its successful
        // collector test originally searched only page one; exercise the shared
        // bounded reader against the actual authorized PostgreSQL API here.
        for index in 1..=281 {
            let text = if index == 281 {
                "owned-pagination-terminal-proof owned-private-log-secret"
            } else {
                "owned pagination progress"
            };
            service
                .append_kubernetes_log(
                    &worker,
                    build,
                    generation,
                    &format!("owned-pagination-{index:04}"),
                    "building",
                    text,
                )
                .await
                .unwrap();
        }
        let history =
            super::build_kubernetes_live::retained_log_pages(&service, &f.principal, &f.pid, build)
                .await;
        assert_eq!(history["pages"], 3);
        assert_eq!(history["entries"].as_array().unwrap().len(), 282);
        assert!(
            history["entries"].as_array().unwrap().last().unwrap()["text"]
                .as_str()
                .unwrap()
                .contains("owned-pagination-terminal-proof [REDACTED]")
        );
        assert!(!history.to_string().contains("owned-private-log-secret"));
        sql(&f.owner,"INSERT INTO webdock_admin.projects(id,name,customer_id) VALUES($1,'wrong log project',$2)",vec![other_project.clone().into(),f.cid.clone().into()]).await;
        sql(&f.owner,"INSERT INTO webdock_auth.hosting_project(project_id,customer_id,provider,cluster_id,namespace,mode) SELECT $1,customer_id,provider,cluster_id,namespace||'-logs',mode FROM webdock_auth.hosting_project WHERE project_id=$2",vec![other_project.clone().into(),f.pid.clone().into()]).await;
        assert_eq!(service.execute(&f.principal,json!({"action":"git.builds.logs","projectID":other_project,"buildID":build,"after":"0","limit":25})).await.unwrap_err().code,"build_unavailable","cursor reads remain bound to their authorized project");
        let response = router
            .clone()
            .oneshot(post(
                "kubernetes-attempts",
                Some(credential),
                json!({"clusterID":cluster}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 16384)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["attempts"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let unsigned = db("SPITZLI_PLUGIN_WORKER_DATABASE_URL").await;
        let row = unsigned
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT count(*) AS count FROM webdock_auth.git_build_attempt",
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            row.try_get::<i64>("", "count").unwrap(),
            0,
            "unsigned runtime has no attempt visibility"
        );
        assert!(
            unsigned
                .execute_unprepared("UPDATE webdock_auth.git_build_admission SET enabled=true")
                .await
                .is_err(),
            "worker cannot enable private admission"
        );
        sql(
            &f.owner,
            "UPDATE webdock_auth.git_build SET lease_until=now()-interval '1 second' WHERE id=$1",
            vec![build.into()],
        )
        .await;
        assert!(
            service
                .claim_kubernetes_build(&worker, cluster, &runtime)
                .await
                .unwrap()
                .is_none(),
            "expired uncertain Job still occupies its slot"
        );
        let attempts = service
            .list_kubernetes_attempts(&worker, cluster)
            .await
            .unwrap();
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0]["cancelRequested"], true);
        assert_eq!(attempts[0]["sourceSHA"], claimed["sourceSHA"]);
        assert_eq!(
            claimed["kubernetesLimits"]["cache_mib"],
            json!(16384),
            "cache budget must be in the durable claim, not a worker default"
        );
        assert_eq!(
            attempts[0]["kubernetesLimits"], claimed["kubernetesLimits"],
            "restart must retain the original complete Kubernetes resource budget"
        );
        assert_eq!(
            attempts[0]["request"]["limits"],
            claimed["request"]["limits"]
        );
        assert_eq!(
            attempts[0]["containerBuild"], claimed["containerBuild"],
            "reconciliation preserves original typed container settings"
        );
        assert!(
            attempts[0]["request"].get("buildEnvironment").is_none(),
            "reconciliation snapshot excludes build secrets"
        );
        assert!(f.owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.git_build_attempt SET execution_request=jsonb_set(execution_request,'{rootDirectory}','\"changed\"') WHERE build_id=$1",vec![build.into()])).await.is_err(),"execution request is immutable even under owner writes");
        assert!(f.owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.git_build_attempt SET kubernetes_limits=jsonb_set(kubernetes_limits,'{cache_mib}','64') WHERE build_id=$1",vec![build.into()])).await.is_err(),"attempt budget cannot change after dispatch or a worker restart");
        assert!(
            service
                .guard_build(&worker, build, generation)
                .await
                .is_err()
        );
        assert_eq!(
            service
                .retire_kubernetes_attempt(&worker, build, generation, Some("wrong-job-uid"))
                .await
                .unwrap_err()
                .code,
            "kubernetes_uid_conflict"
        );
        service
            .retire_kubernetes_attempt(&worker, build, generation, Some("owned-job-uid"))
            .await
            .unwrap();
        service
            .retire_kubernetes_attempt(&worker, build, generation, Some("owned-job-uid"))
            .await
            .unwrap();
        let claimed = service
            .claim_kubernetes_build(&worker, cluster, &runtime)
            .await
            .unwrap()
            .unwrap();
        let next = claimed["buildID"].as_str().unwrap();
        let next_generation = claimed["generation"].as_i64().unwrap() as i32;
        assert_ne!(next, build);
        service
            .configure_kubernetes_admission(&f.principal, true, 1, 4)
            .await
            .unwrap();
        service
            .execute(&f.principal, request("kubernetes-third"))
            .await
            .unwrap();
        assert!(
            service
                .claim_kubernetes_build(&worker, cluster, &runtime)
                .await
                .unwrap()
                .is_none(),
            "global capacity applies independently of tenant capacity"
        );
        service
            .configure_kubernetes_admission(&f.principal, true, 4, 4)
            .await
            .unwrap();
        assert!(
            service
                .claim_kubernetes_build(&worker, cluster, &runtime)
                .await
                .unwrap()
                .is_none(),
            "project capacity remains one even when global and tenant limits are raised"
        );
        service
            .execute(&f.principal, request("kubernetes-fourth"))
            .await
            .unwrap();
        assert!(
            service
                .claim_kubernetes_build(&worker, cluster, &runtime)
                .await
                .unwrap()
                .is_none(),
            "project capacity applies independently of global and tenant capacity"
        );
        sql(
            &f.owner,
            "UPDATE webdock_auth.git_source SET enabled=false,revision=revision+1 WHERE id=$1",
            vec![source.into()],
        )
        .await;
        assert!(
            service
                .record_kubernetes_job_intent(&worker, next, next_generation, &spec)
                .await
                .is_err(),
            "source revision fences dispatch"
        );
        assert!(
            service
                .claim_kubernetes_build(&worker, cluster, &runtime)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            service
                .list_kubernetes_attempts(&worker, cluster)
                .await
                .unwrap()[0]["cancelRequested"],
            true
        );
        assert!(
            service
                .append_kubernetes_log(
                    &worker,
                    next,
                    next_generation,
                    "owned-log-entry-late",
                    "building",
                    "late after cancellation"
                )
                .await
                .is_err(),
            "cancellation fences new log writes"
        );
        service
            .revoke_worker(&f.principal, worker.id(), worker.generation())
            .await
            .unwrap();
        assert_eq!(
            router
                .oneshot(post(
                    "kubernetes-attempts",
                    Some(credential),
                    json!({"clusterID":cluster})
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            service
                .list_kubernetes_attempts(&worker, cluster)
                .await
                .unwrap_err()
                .code,
            "worker_revoked"
        );
        assert_eq!(
            service
                .record_kubernetes_job_intent(&worker, next, next_generation, &spec)
                .await
                .unwrap_err()
                .code,
            "worker_revoked"
        );
        let replacement = service
            .enroll_worker(
                &f.principal,
                "DE",
                "artifact-only",
                "Owned disposable local admission fixture",
                2,
            )
            .await
            .unwrap();
        let replacement = service
            .authenticate_worker(replacement["credential"].as_str().unwrap())
            .await
            .unwrap();
        service
            .rebind_kubernetes_cluster(&f.principal, cluster, replacement.id())
            .await
            .unwrap();
        let outstanding = service
            .list_kubernetes_attempts(&replacement, cluster)
            .await
            .unwrap();
        assert_eq!(
            outstanding.len(),
            1,
            "operator rotation preserves cleanup authority over old attempts"
        );
        for attempt in outstanding {
            assert_eq!(attempt["cancelRequested"], true);
            service
                .retire_kubernetes_attempt(
                    &replacement,
                    attempt["build_id"].as_str().unwrap(),
                    attempt["generation"].as_i64().unwrap() as i32,
                    None,
                )
                .await
                .unwrap();
        }
        assert!(
            service
                .list_kubernetes_attempts(&replacement, cluster)
                .await
                .unwrap()
                .is_empty()
        );
        let selected=service.execute(&f.principal,json!({"action":"git.source.configure","projectID":f.pid,"targetID":target,"connectionID":connection,"repositoryID":"55","branch":"main","rootDirectory":".","recipe":"dockerfile","executionBackend":"kubernetes","revision":3})).await.unwrap();
        assert_eq!(
            selected["executionBackend"], "kubernetes",
            "trusted project can explicitly select the native backend"
        );
        service
            .enable_kubernetes_cluster(&f.principal, cluster, true)
            .await
            .unwrap();
        let finished = service
            .execute(&f.principal, request("kubernetes-artifact-ready"))
            .await
            .unwrap();
        let finished_id = finished["id"].as_str().unwrap();
        let finished = service
            .claim_kubernetes_build(&replacement, cluster, &runtime)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(finished["buildID"], finished_id);
        let finished_generation = finished["generation"].as_i64().unwrap() as i32;
        service
            .record_kubernetes_job_intent(&replacement, finished_id, finished_generation, &spec)
            .await
            .unwrap();
        service
            .record_kubernetes_job(
                &replacement,
                finished_id,
                finished_generation,
                "owned-complete-job",
            )
            .await
            .unwrap();
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let store = spitzli_build::artifact::ArtifactStore::open(
            directory.path().into(),
            spitzli_build::artifact::MAX_OUTPUT * 2,
        )
        .unwrap();
        let artifact = store
            .put(&f.cid, finished_id, finished_generation, &oci_envelope())
            .unwrap();
        let completed = service
            .complete_build(
                &replacement,
                finished_id,
                finished_generation,
                Some(&artifact),
                "owned build output",
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            completed["status"], "artifact-ready",
            "retaining OCI bytes does not claim registry publication success"
        );
        let row=f.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT registry_verified_at IS NULL AND finished_at IS NULL AND phase='awaiting-approval' AS pending FROM webdock_auth.git_build WHERE id=$1",vec![finished_id.into()])).await.unwrap().unwrap();
        assert!(row.try_get::<bool>("", "pending").unwrap());
        service
            .retire_kubernetes_attempt(
                &replacement,
                finished_id,
                finished_generation,
                Some("owned-complete-job"),
            )
            .await
            .unwrap();
        assert_eq!(
            service
                .complete_build(
                    &replacement,
                    finished_id,
                    finished_generation,
                    Some(&artifact),
                    "owned build output",
                    None
                )
                .await
                .unwrap()["status"],
            "artifact-ready",
            "lost completion acknowledgements remain idempotent after Job cleanup"
        );
        assert_eq!(
            service
                .complete_build(
                    &replacement,
                    finished_id,
                    finished_generation,
                    Some(&artifact),
                    "different output",
                    None
                )
                .await
                .unwrap_err()
                .code,
            "completion_conflict"
        );
        // Model an already atomically accepted Hosting operation. A lost publish
        // acknowledgement is a receipt replay even after rollout/lease expiry;
        // it must neither require fresh source authority nor create an operation.
        let release_id = completed["release"]["id"].as_str().unwrap();
        let accepted_operation = id(&f.owner).await;
        let accepted_image = format!(
            "registry.invalid/customers/{}/projects/{}@sha256:{}",
            f.cid,
            f.pid,
            "a".repeat(64)
        );
        sql(&f.owner,"INSERT INTO webdock_auth.hosting_operation(id,subject,actor_session,customer_id,project_id,cluster_id,app_id,action,idempotency_key,payload_hash,status,desired,target_revision,git_release_id) VALUES($1,$2,$3,$4,$5,$6,$7,'apps.deploy','owned-publication-receipt','owned-fixture','succeeded',$8,1,$9)",vec![accepted_operation.clone().into(),f.uid.clone().into(),f.sid.clone().into(),f.cid.clone().into(),f.pid.clone().into(),f.cluster.clone().into(),target.into(),json!({"spec":{"image":accepted_image}}).into(),release_id.into()]).await;
        sql(&f.owner,"UPDATE webdock_auth.git_release SET status='ready',worker_id=$2,worker_generation=$3,generation=1,operation_id=$4,lease_until=now()-interval '1 second' WHERE id=$1",vec![release_id.into(),replacement.id().into(),replacement.generation().into(),accepted_operation.clone().into()]).await;
        for _ in 0..2 {
            let replay = service
                .publish_image(&replacement, release_id, 1, &accepted_image)
                .await
                .unwrap();
            assert_eq!(replay["replayed"], true);
            assert_eq!(replay["operationID"], accepted_operation);
        }
        assert_eq!(
            service
                .publish_image(
                    &replacement,
                    release_id,
                    1,
                    &format!("{accepted_image}changed")
                )
                .await
                .unwrap_err()
                .code,
            "publication_image_conflict"
        );
        assert!(
            service
                .publish_image(&replacement, release_id, 2, &accepted_image)
                .await
                .is_err(),
            "receipt cannot cross a publication generation"
        );
        sql(&f.owner,"UPDATE webdock_auth.git_release SET operation_id=NULL,status='awaiting-approval',lease_until=NULL WHERE id=$1",vec![release_id.into()]).await;
        sql(
            &f.owner,
            "DELETE FROM webdock_auth.hosting_operation WHERE id=$1",
            vec![accepted_operation.into()],
        )
        .await;
        service
            .execute(
                &f.principal,
                json!({"action":"git.builds.cancel","projectID":f.pid,"buildID":finished_id}),
            )
            .await
            .unwrap();
        let release_id = completed["release"]["id"].as_str().unwrap();
        sql(&f.owner,"UPDATE webdock_auth.git_release SET status='deploying',worker_id=$2,worker_generation=$3,generation=1,lease_until=now()+interval '10 minutes',publication_started_at=now() WHERE id=$1",vec![release_id.into(),replacement.id().into(),replacement.generation().into()]).await;
        // The image may exist externally, but cancellation plus no linked operation proves
        // this publication intent cannot have staged a deployment under the atomic fence.
        assert!(
            service
                .claim_release_with_storage(&replacement, &[], &std::collections::BTreeMap::new())
                .await
                .unwrap()
                .is_none()
        );
        let abandoned=f.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT status,operation_id IS NULL AS no_operation FROM webdock_auth.git_release WHERE id=$1",vec![release_id.into()])).await.unwrap().unwrap();
        assert_eq!(
            abandoned.try_get::<String>("", "status").unwrap(),
            "superseded"
        );
        assert!(abandoned.try_get::<bool>("", "no_operation").unwrap());

        assert!(
            service
                .append_kubernetes_log(
                    &replacement,
                    finished_id,
                    finished_generation,
                    "owned-log-postcancel",
                    "pushing",
                    "late"
                )
                .await
                .is_err()
        );
        assert!(
            service
                .complete_build(
                    &replacement,
                    finished_id,
                    finished_generation,
                    Some(&artifact),
                    "owned build output",
                    None
                )
                .await
                .is_err(),
            "cancellation fences the completed attempt generation"
        );
        service
            .configure_kubernetes_admission(&f.principal, false, 1, 1)
            .await
            .unwrap();
        // Unique fixture records are removed below even when an assertion fails.
    };
    let outcome = tokio::spawn(task).await;
    sql(&fixture.owner,"UPDATE webdock_auth.git_build_admission SET enabled=false,global_slots=1,tenant_slots=1 WHERE singleton",vec![]).await;
    for table in [
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
        "DELETE FROM webdock_auth.git_build_cluster WHERE namespace='owned-build-fixture'",
        vec![],
    )
    .await;
    sql(&fixture.owner,"DELETE FROM webdock_auth.git_worker WHERE evidence IN ('Owned disposable local admission fixture','Owned disposable legacy guard fixture')",vec![]).await;
    sql(
        &fixture.owner,
        "DELETE FROM webdock_auth.hosting_project WHERE project_id=$1",
        vec![other_cleanup.clone().into()],
    )
    .await;
    sql(
        &fixture.owner,
        "DELETE FROM webdock_admin.projects WHERE id=$1",
        vec![other_cleanup.into()],
    )
    .await;
    fixture.cleanup().await;
    outcome.unwrap();
}
