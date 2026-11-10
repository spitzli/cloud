//! Owned PostgreSQL scheduler proof only; retirement acknowledgments are simulated, no cluster calls.
use super::*;
use spitzli_agent_protocol::build_storage::BuildStorageAction;
use spitzli_build::{BuildService, KubernetesStoragePolicy};

async fn row(owner: &DatabaseConnection, query: &str, build: &str) -> Value {
    owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            query,
            [build.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "value")
        .unwrap()
}
async fn enqueue(f: &Fixture, customer: &str, project: &str, source: &str, key: &str) -> String {
    let build = id(&f.owner).await;
    sql(&f.owner,"INSERT INTO webdock_auth.git_build(id,customer_id,project_id,source_id,source_sha,source_revision,connection_generation,environment_revision,subject,idempotency_key) VALUES($1,$2,$3,$4,$5,1,1,1,$6,$7)",vec![build.clone().into(),customer.into(),project.into(),source.into(),"a".repeat(40).into(),f.uid.clone().into(),key.into()]).await;
    build
}

#[tokio::test]
#[ignore = "owned fresh native database via tools/tests/kubernetes-builds.py"]
async fn kubernetes_fair_queue_and_retired_infrastructure_retry() {
    let f = Arc::new(Fixture::new("kubernetes").await);
    let p2 = id(&f.owner).await;
    let cb = id(&f.owner).await;
    let pb = id(&f.owner).await;
    let extra_projects = [p2.clone(), pb.clone()];
    let result=tokio::spawn({let f=f.clone();let p2=p2.clone();let cb=cb.clone();let pb=pb.clone();async move {
        f.configure("k3s").await;
        sql(&f.owner,"INSERT INTO webdock_admin.customers(id,name) VALUES($1,'owned fairness second tenant')",vec![cb.clone().into()]).await;
        for (project,customer) in [(&p2,&f.cid),(&pb,&cb)] {
            sql(&f.owner,"INSERT INTO webdock_admin.projects(id,name,customer_id) VALUES($1,'owned fairness project',$2)",vec![project.clone().into(),customer.clone().into()]).await;
            sql(&f.owner,"INSERT INTO webdock_auth.hosting_project(project_id,customer_id,provider,cluster_id,namespace,mode) SELECT $1,$2,provider,cluster_id,namespace||'-'||$1,mode FROM webdock_auth.hosting_project WHERE project_id=$3",vec![project.clone().into(),customer.clone().into(),f.pid.clone().into()]).await;
        }
        let service=BuildService::new(db("DATABASE_URL").await,db("WEBDOCK_PLUGIN_WORKER_DATABASE_URL").await,std::env::var("DATABASE_RLS_SECRET").unwrap(),Arc::new(spitzli_persistence::IdAllocator::new(db("DATABASE_URL").await,spitzli_ids::Identity::platform(0,2).unwrap())),f.service.clone(),"registry.invalid".into()).await.unwrap();
        let enrolled=service.enroll_worker(&f.principal,"DE","artifact-only","Owned fair queue and retry fixture",4).await.unwrap();
        let worker=service.authenticate_worker(enrolled["credential"].as_str().unwrap()).await.unwrap();
        let cluster=service.enroll_kubernetes_cluster(&f.principal,worker.id(),"owned-fair-retry","DE","Owned fair queue and retry fixture",4).await.unwrap();let cluster=cluster["id"].as_str().unwrap();
        service.enable_kubernetes_cluster(&f.principal,cluster,true).await.unwrap();
        service.configure_kubernetes_admission(&f.principal,true,1,2).await.unwrap();
        let runtime="7".repeat(64);let mut sources=vec![];
        for (project,customer) in [(&f.pid,&f.cid),(&p2,&f.cid),(&pb,&cb)] {
            service.trust_kubernetes_project(&f.principal,customer,project,true).await.unwrap();
            let connection=id(&f.owner).await;let source=id(&f.owner).await;let target=id(&f.owner).await;
            sql(&f.owner,"INSERT INTO webdock_auth.git_connection(id,customer_id,installation_id,account_id,account_login,permissions,state) VALUES($1,$2,$1,'1234','fair-fixture','{}','active')",vec![connection.clone().into(),customer.clone().into()]).await;
            sql(&f.owner,"INSERT INTO webdock_auth.git_repository(connection_id,customer_id,repository_id,metadata) VALUES($1,$2,'55','{}')",vec![connection.clone().into(),customer.clone().into()]).await;
            sql(&f.owner,"INSERT INTO webdock_auth.git_source(id,customer_id,project_id,connection_id,repository_id,branch,root_directory,recipe,target_id,execution_backend) VALUES($1,$2,$3,$4,'55','main','.','dockerfile',$5,'kubernetes')",vec![source.clone().into(),customer.clone().into(),project.clone().into(),connection.into(),target.into()]).await;
            sources.push(source);
        }
        let mut a_backlog=vec![];
        for n in 0..8 {a_backlog.push(enqueue(&f,&f.cid,&f.pid,&sources[0],&format!("a-{n}")).await);}
        let a2=enqueue(&f,&f.cid,&p2,&sources[1],"a2").await;
        let b1=enqueue(&f,&cb,&pb,&sources[2],"b1").await;
        for expected in [&a_backlog[0],&b1,&a2,&a_backlog[1]] {
            let (a,b)=tokio::join!(service.claim_kubernetes_build(&worker,cluster,&runtime),service.claim_kubernetes_build(&worker,cluster,&runtime));
            let a=a.unwrap();let b=b.unwrap();assert_eq!(usize::from(a.is_some())+usize::from(b.is_some()),1);
            let claim=a.or(b).unwrap();assert_eq!(claim["buildID"],*expected,"tenant and project turn survives a large older backlog");
            sql(&f.owner,"UPDATE webdock_auth.git_build SET lease_until=now()-interval '1 second' WHERE id=$1",vec![expected.as_str().into()]).await;
            assert!(service.claim_kubernetes_build(&worker,cluster,&runtime).await.unwrap().is_none(),"stale lease still owns physical capacity");
            service.retire_kubernetes_attempt(&worker,expected,1,None).await.unwrap();
        }
        // Higher global/tenant caps still permit only one live attempt per project.
        service.configure_kubernetes_admission(&f.principal,true,4,2).await.unwrap();
        let claim=service.claim_kubernetes_build(&worker,cluster,&runtime).await.unwrap().unwrap();
        assert!(service.claim_kubernetes_build(&worker,cluster,&runtime).await.unwrap().is_none());
        service.retire_kubernetes_attempt(&worker,claim["buildID"].as_str().unwrap(),1,None).await.unwrap();
        sql(&f.owner,"UPDATE webdock_auth.git_build SET status='cancelled',phase='cancelled' WHERE status='queued' AND customer_id=$1",vec![f.cid.clone().into()]).await;
        service.configure_kubernetes_admission(&f.principal,true,1,1).await.unwrap();
        service.enable_kubernetes_cluster(&f.principal,cluster,false).await.unwrap();
        service.configure_kubernetes_storage(&f.principal,cluster,Some(KubernetesStoragePolicy{node:"webdock-build-eu-01".into(),pool_id:"123e4567-e89b-42d3-a456-426614174000".into(),volume_bytes:27*1024*1024*1024}),Some(&runtime)).await.unwrap();
        service.enable_kubernetes_cluster(&f.principal,cluster,true).await.unwrap();
        let build=enqueue(&f,&f.cid,&f.pid,&sources[0],"retry").await;
        let original=row(&f.owner,"SELECT to_jsonb(b) AS value FROM webdock_auth.git_build b WHERE id=$1",&build).await;
        let mut prior_job=String::new();
        for generation in 1..=3 {
            let claim=service.claim_kubernetes_build_with_storage(&worker,cluster,&runtime,true).await.unwrap().unwrap();assert_eq!(claim["buildID"],build);assert_eq!(claim["generation"],generation);assert_ne!(claim["jobName"],prior_job);prior_job=claim["jobName"].as_str().unwrap().to_owned();
            assert!(!service.retry_kubernetes_infrastructure(&worker,&build,generation,"KUBERNETES_NODE_UNAVAILABLE").await.unwrap(),"running or unretired resources cannot requeue");
            assert!(service.retire_kubernetes_attempt(&worker,&build,generation,None).await.is_err(),"fixed volume retirement required");
            let closing=service.issue_kubernetes_storage(&worker,&build,generation,BuildStorageAction::Retire).await.unwrap();
            service.record_kubernetes_volume_retired(&worker,&build,generation,closing.epoch).await.unwrap();
            let accepted=if generation==1 {
                let accepted=service.retire_kubernetes_attempt_with_retry(&worker,&build,generation,None,Some("KUBERNETES_NODE_UNAVAILABLE")).await.unwrap();
                assert_eq!(row(&f.owner,"SELECT to_jsonb(b) AS value FROM webdock_auth.git_build b WHERE id=$1",&build).await["status"],"queued","retirement returns the final retry state, without a separately observable terminal failure");
                accepted
            }else{
                service.retire_kubernetes_attempt(&worker,&build,generation,None).await.unwrap();
                service.retry_kubernetes_infrastructure(&worker,&build,generation,"KUBERNETES_NODE_UNAVAILABLE").await.unwrap()
            };
            for deterministic in ["BUILD_FAILED","OOM_KILLED","DEADLINE_EXCEEDED","CANCELLED","DISK_LIMIT_EXCEEDED"] {assert_eq!(service.retry_kubernetes_infrastructure(&worker,&build,generation,deterministic).await.unwrap_err().code,"kubernetes_failure_not_retryable");}
            assert_eq!(accepted,generation<3);
            if !accepted {break;}
            let scheduled=row(&f.owner,"SELECT to_jsonb(b)||jsonb_build_object('backoff',extract(epoch FROM next_eligible_at-now())) AS value FROM webdock_auth.git_build b WHERE id=$1",&build).await;
            assert_eq!(scheduled["retry_count"],generation);assert!(scheduled["backoff"].as_f64().unwrap()>if generation==1{28.0}else{118.0});
            for field in ["source_sha","source_revision","environment_revision","connection_generation","environment_identity"] {assert_eq!(scheduled[field],original[field]);}
            assert!(service.retry_kubernetes_infrastructure(&worker,&build,generation,"KUBERNETES_NODE_UNAVAILABLE").await.unwrap());
            let replay=row(&f.owner,"SELECT to_jsonb(b) AS value FROM webdock_auth.git_build b WHERE id=$1",&build).await;assert_eq!(replay["next_eligible_at"],scheduled["next_eligible_at"]);assert_eq!(replay["retry_count"],generation);
            assert!(service.claim_kubernetes_build_with_storage(&worker,cluster,&runtime,true).await.unwrap().is_none(),"durable backoff survives a fresh claim transaction");
            // Test clock advancement only: the owner bypasses this one trigger in the disposable DB.
            sql(&f.owner,"ALTER TABLE webdock_auth.git_build DISABLE TRIGGER git_build_infrastructure_retry",vec![]).await;
            sql(&f.owner,"UPDATE webdock_auth.git_build SET next_eligible_at=now()-interval '1 second' WHERE id=$1",vec![build.clone().into()]).await;
            sql(&f.owner,"ALTER TABLE webdock_auth.git_build ENABLE TRIGGER git_build_infrastructure_retry",vec![]).await;
        }
        assert_eq!(row(&f.owner,"SELECT to_jsonb(b.retry_count) AS value FROM webdock_auth.git_build b WHERE id=$1",&build).await,2);
        let deterministic=enqueue(&f,&f.cid,&f.pid,&sources[0],"deterministic").await;
        let claim=service.claim_kubernetes_build_with_storage(&worker,cluster,&runtime,true).await.unwrap().unwrap();assert_eq!(claim["buildID"],deterministic);
        let closing=service.issue_kubernetes_storage(&worker,&deterministic,1,BuildStorageAction::Retire).await.unwrap();service.record_kubernetes_volume_retired(&worker,&deterministic,1,closing.epoch).await.unwrap();service.retire_kubernetes_attempt(&worker,&deterministic,1,None).await.unwrap();
        sql(&f.owner,"UPDATE webdock_auth.git_build SET failure_code='BUILD_FAILED' WHERE id=$1",vec![deterministic.clone().into()]).await;
        assert!(!service.retry_kubernetes_infrastructure(&worker,&deterministic,1,"KUBERNETES_API_UNAVAILABLE").await.unwrap(),"customer completion cannot be relabeled as infrastructure");
        sql(&f.owner,"UPDATE webdock_auth.git_build SET status='cancelled',failure_code='KUBERNETES_JOB_RETIRED' WHERE id=$1",vec![deterministic.clone().into()]).await;
        assert!(!service.retry_kubernetes_infrastructure(&worker,&deterministic,1,"KUBERNETES_API_UNAVAILABLE").await.unwrap(),"cancelled build cannot retry");
        let revoked=enqueue(&f,&f.cid,&f.pid,&sources[0],"revoked").await;
        let claim=service.claim_kubernetes_build_with_storage(&worker,cluster,&runtime,true).await.unwrap().unwrap();assert_eq!(claim["buildID"],revoked);
        let closing=service.issue_kubernetes_storage(&worker,&revoked,1,BuildStorageAction::Retire).await.unwrap();service.record_kubernetes_volume_retired(&worker,&revoked,1,closing.epoch).await.unwrap();service.retire_kubernetes_attempt(&worker,&revoked,1,None).await.unwrap();
        sql(&f.owner,"UPDATE webdock_auth.git_source SET environment_revision=environment_revision+1 WHERE id=$1",vec![sources[0].clone().into()]).await;
        assert!(!service.retry_kubernetes_infrastructure(&worker,&revoked,1,"KUBERNETES_API_UNAVAILABLE").await.unwrap(),"environment revocation fences retry");
    }}).await;
    sql(&f.owner,"UPDATE webdock_auth.git_build_admission SET enabled=false,global_slots=1,tenant_slots=1 WHERE singleton",vec![]).await;
    for customer in [&f.cid, &cb] {
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
                &f.owner,
                &format!("DELETE FROM webdock_auth.{table} WHERE {predicate}"),
                vec![customer.clone().into()],
            )
            .await;
        }
    }
    sql(&f.owner,"DELETE FROM webdock_auth.git_build_cluster WHERE evidence='Owned fair queue and retry fixture'",vec![]).await;
    sql(
        &f.owner,
        "DELETE FROM webdock_auth.git_worker WHERE evidence='Owned fair queue and retry fixture'",
        vec![],
    )
    .await;
    for project in extra_projects {
        sql(
            &f.owner,
            "DELETE FROM webdock_auth.hosting_project WHERE project_id=$1",
            vec![project.clone().into()],
        )
        .await;
        sql(
            &f.owner,
            "DELETE FROM webdock_admin.projects WHERE id=$1",
            vec![project.into()],
        )
        .await;
    }
    sql(
        &f.owner,
        "DELETE FROM webdock_admin.customers WHERE id=$1",
        vec![cb.into()],
    )
    .await;
    f.cleanup().await;
    result.unwrap();
}
