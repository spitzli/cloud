//! Owned DB authority proofs; actual ext4/Kubernetes behavior is qualified separately.
use super::*;
use spitzli_agent_protocol::build_storage::{BuildStorageAction, BuildVolumeHandle};
use spitzli_build::{BuildService, KubernetesStoragePolicy};

#[tokio::test]
#[ignore = "owned fresh database via tools/tests/kubernetes-builds.py"]
async fn kubernetes_volume_authority_is_immutable_fenced_and_required_before_retirement() {
    let fixture = Arc::new(Fixture::new("kubernetes").await);
    let f = fixture.clone();
    let result=tokio::spawn(async move {
        f.configure("k3s").await;
        let service=BuildService::new(db("DATABASE_URL").await,db("SPITZLI_PLUGIN_WORKER_DATABASE_URL").await,std::env::var("DATABASE_RLS_SECRET").unwrap(),
            Arc::new(spitzli_persistence::IdAllocator::new(db("DATABASE_URL").await,spitzli_ids::Identity::platform(0,2).unwrap())),f.service.clone(),"registry.invalid".into()).await.unwrap()
            .with_repository_provider(Arc::new(super::build_kubernetes_admission::Repository));
        let mut draft=spec();draft.as_object_mut().unwrap().remove("image");draft.as_object_mut().unwrap().remove("template");
        let app=f.service.execute(&f.principal,json!({"action":"apps.prepare","projectID":f.pid,"name":"volume-authority","spec":draft,"subscriptionRevision":1,"idempotencyKey":"volume-authority-target"})).await.unwrap();
        let target=app["appID"].as_str().or_else(||app["id"].as_str()).unwrap();
        let connection=id(&f.owner).await;
        sql(&f.owner,"INSERT INTO webdock_auth.git_connection(id,customer_id,installation_id,account_id,account_login,permissions,state) VALUES($1,$2,'1234','1234','volume-fixture','{}','active')",vec![connection.clone().into(),f.cid.clone().into()]).await;
        sql(&f.owner,"INSERT INTO webdock_auth.git_repository(connection_id,customer_id,repository_id,metadata) VALUES($1,$2,'55','{}')",vec![connection.clone().into(),f.cid.clone().into()]).await;
        let worker=service.enroll_worker(&f.principal,"DE","artifact-only","Owned build volume authority fixture",1).await.unwrap();
        let worker=service.authenticate_worker(worker["credential"].as_str().unwrap()).await.unwrap();
        let cluster=service.enroll_kubernetes_cluster(&f.principal,worker.id(),"webdock-build-qualification","DE","Owned build volume authority fixture",1).await.unwrap();
        let cluster=cluster["id"].as_str().unwrap();
        let runtime="3".repeat(64);
        let policy=KubernetesStoragePolicy{node:"webdock-build-eu-01".into(),pool_id:"123e4567-e89b-42d3-a456-426614174000".into(),volume_bytes:27*1024*1024*1024};
        service.configure_kubernetes_storage(&f.principal,cluster,Some(policy.clone()),Some(&runtime)).await.unwrap();
        assert!(service.configure_kubernetes_storage(&f.principal,cluster,None,None).await.is_err(),"hard storage cannot silently downgrade to the emptyDir pilot");
        service.enable_kubernetes_cluster(&f.principal,cluster,true).await.unwrap();
        service.configure_kubernetes_admission(&f.principal,true,1,1).await.unwrap();
        service.trust_kubernetes_project(&f.principal,&f.cid,&f.pid,true).await.unwrap();
        service.execute(&f.principal,json!({"action":"git.source.configure","projectID":f.pid,"targetID":target,"connectionID":connection,"repositoryID":"55","branch":"main","rootDirectory":".","recipe":"dockerfile","executionBackend":"kubernetes","revision":0,"buildEnvironment":[{"name":"FIXTURE_BUILD_SECRET","value":"volume-fixture-private-sentinel"}]})).await.unwrap();
        let build=service.execute(&f.principal,json!({"action":"git.builds.request","projectID":f.pid,"targetID":target,"idempotencyKey":"volume-authority-build"})).await.unwrap();let build=build["id"].as_str().unwrap();
        assert_eq!(service.claim_kubernetes_build(&worker,cluster,&runtime).await.unwrap_err().code,"kubernetes_storage_profile_required");
        let claim=service.claim_kubernetes_build_with_storage(&worker,cluster,&runtime,true).await.unwrap().unwrap();
        let generation=claim["generation"].as_i64().unwrap() as i32;
        assert_eq!(claim["request"]["buildEnvironment"]["FIXTURE_BUILD_SECRET"],"volume-fixture-private-sentinel");
        let resumed=service.list_kubernetes_attempts(&worker,cluster).await.unwrap();assert_eq!(resumed.len(),1);
        assert!(resumed[0]["request"].get("buildEnvironment").is_none(),"reconciliation metadata excludes secrets");
        assert_eq!(service.kubernetes_build_environment(&worker,build,generation).await.unwrap()["FIXTURE_BUILD_SECRET"],"volume-fixture-private-sentinel","async preparation uses a fresh scoped execution handoff");
        assert!(resumed[0]["execution_request"]["buildEnvironment"].is_null(),"secrets must not enter immutable durable execution requests");

        let original: spitzli_agent_protocol::build_storage::BuildStorageReservation=serde_json::from_value(claim["storageRequest"].clone()).unwrap();
        assert_eq!(original.customer_id,f.cid);assert_ne!(original.volume_id,claim["attemptID"].as_str().unwrap());
        assert!(service.record_kubernetes_job_intent(&worker,build,generation,&"4".repeat(64)).await.is_err(),"job intent requires a pinned volume handle");
        let reservation=service.issue_kubernetes_storage(&worker,build,generation,BuildStorageAction::Reserve).await.unwrap();
        assert!(reservation.epoch>original.epoch);
        let handle=BuildVolumeHandle::new(reservation.clone(),"webdock-build-qualification","pv-owned-1","pvc-owned-1","123e4567-e89b-42d3-a456-426614174001").unwrap();
        let mut foreign=handle.clone();foreign.reservation.customer_id=id(&f.owner).await;
        assert!(service.record_kubernetes_volume(&worker,build,generation,foreign).await.is_err());
        service.record_kubernetes_volume(&worker,build,generation,handle.clone()).await.unwrap();
        service.record_kubernetes_volume(&worker,build,generation,handle.clone()).await.unwrap();
        let mut replaced=handle;replaced.pvc_uid="replacement".into();
        assert!(service.record_kubernetes_volume(&worker,build,generation,replaced).await.is_err());
        service.record_kubernetes_job_intent(&worker,build,generation,&"4".repeat(64)).await.unwrap();
        assert_eq!(service.retire_kubernetes_attempt(&worker,build,generation,None).await.unwrap_err().code,"kubernetes_volume_not_retired");
        let closing=service.issue_kubernetes_storage(&worker,build,generation,BuildStorageAction::Retire).await.unwrap();
        assert!(closing.epoch>reservation.epoch);
        assert!(service.kubernetes_build_environment(&worker,build,generation).await.is_err(),"closing/cancelled execution cannot refresh secrets");
        let cleanup=service.list_kubernetes_attempts(&worker,cluster).await.unwrap();assert_eq!(cleanup.len(),1);assert!(cleanup[0]["cancelRequested"]==true);assert!(cleanup[0]["request"].get("buildEnvironment").is_none());

        assert!(service.issue_kubernetes_storage(&worker,build,generation,BuildStorageAction::Reserve).await.is_err());
        assert!(service.record_kubernetes_volume_retired(&worker,build,generation,reservation.epoch).await.is_err());
        service.record_kubernetes_volume_retired(&worker,build,generation,closing.epoch).await.unwrap();
        service.record_kubernetes_volume_retired(&worker,build,generation,closing.epoch).await.unwrap();
        service.retire_kubernetes_attempt(&worker,build,generation,None).await.unwrap();
        assert!(service.list_kubernetes_attempts(&worker,cluster).await.unwrap().is_empty());
        let cleanup=service.list_kubernetes_attempts(&worker,cluster).await.unwrap();
        for row in cleanup {assert!(row["cancelRequested"]==true);assert!(row["request"]["buildEnvironment"].as_object().is_none_or(|e|e.is_empty()),"cleanup authority must not receive execution secrets");}
        service.configure_kubernetes_admission(&f.principal,false,1,1).await.unwrap();
    }).await;
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
        let predicate = if table == "git_check_outbox" {
            "build_id IN (SELECT id FROM webdock_auth.git_build WHERE customer_id=$1)"
        } else {
            "customer_id=$1"
        };
        sql(
            &fixture.owner,
            &format!("DELETE FROM webdock_auth.{table} WHERE {predicate}"),
            vec![fixture.cid.clone().into()],
        )
        .await;
    }
    sql(&fixture.owner,"DELETE FROM webdock_auth.git_build_cluster WHERE evidence='Owned build volume authority fixture'",vec![]).await;
    sql(
        &fixture.owner,
        "DELETE FROM webdock_auth.git_worker WHERE evidence='Owned build volume authority fixture'",
        vec![],
    )
    .await;
    fixture.cleanup().await;
    result.unwrap();
}
