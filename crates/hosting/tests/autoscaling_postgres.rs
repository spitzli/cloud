//! Owned, disposable PostgreSQL only. No production connection or provider access.
use sea_orm::{ConnectionTrait, DbBackend, Statement, TransactionTrait};
use serde_json::{Value, json};
use spitzli_identity::{CookiePolicy, IdentityService};
use spitzli_plugin_host::{AdmissionInput, FileBindings, HostService};
use spitzli_plugin_runtime::PeerIdentity;
use std::sync::Arc;
#[allow(
    dead_code,
    reason = "Reuse the existing owned PostgreSQL and identity fixture"
)]
#[path = "../../plugin-host/tests/support/jobs_fixture.rs"]
mod fixture;
fn peer() -> PeerIdentity {
    PeerIdentity::Unix {
        uid: rustix::process::geteuid().as_raw(),
    }
}
async fn sql(db: &impl ConnectionTrait, text: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        text,
        values,
    ))
    .await
    .unwrap();
}

#[tokio::test]
#[ignore = "owned local PostgreSQL with SPITZLI_HOSTING_AUTOSCALING_ENABLED=true"]
async fn concurrent_autoscaling_admissions_reserve_max_and_preserve_tenant_scope() {
    use sha2::{Digest, Sha256};
    use spitzli_hosting::HostingService;
    assert_eq!(
        std::env::var("SPITZLI_HOSTING_AUTOSCALING_ENABLED").unwrap(),
        "true"
    );
    let f = fixture::JobFixture::start().await;
    sql(&f.owner,"INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version) VALUES('hosting','builtin','enabled','enabled','0.1.0')",vec![]).await;
    let cap = json!({"apps":4,"cpuMillicores":500,"memoryBytes":1073741824,"ephemeralBytes":1073741824,"volumeBytes":0,"replicasPerApp":5,"concurrentDeployments":4});
    sql(&f.owner,"INSERT INTO webdock_auth.tenant_subscription(customer_id,name,description,base,extras,revision,updated_by) VALUES('123','Owned','Fixture',$1,'{}',1,'operator')",vec![json!({"hosting":cap}).into()]).await;
    sql(&f.owner,"INSERT INTO webdock_auth.hosting_cluster(id,name,provider,region,location_evidence,dedicated_customer_id,generation,verified,capacity,runtime_kind,runtime_installation) VALUES('200','Owned','fixture','local','owned fixture','123',1,true,$1,'kubernetes','test-autoscaling')",vec![cap.clone().into()]).await;
    let credential = "a".repeat(43);
    let observation = json!({"sequence":1,"generation":1,"observedAt":chrono::Utc::now().to_rfc3339(),"version":"owned","runtimeKind":"kubernetes","nodes":[],"capacity":cap,"capabilities":{"networkIsolation":true,"restrictedPods":true,"storage":false,"ingress":false,"environment":true,"replicas":true,"autoscalingVersion":1}});
    sql(&f.owner,"INSERT INTO webdock_auth.hosting_agent(cluster_id,credential_hash,generation,last_sequence,last_seen,observation) VALUES('200',$1,1,1,now(),$2)",vec![hex::encode(Sha256::digest(credential.as_bytes())).into(),observation.into()]).await;
    let service = Arc::new(
        HostingService::new(
            f.database
                .role_with_connections("webdock_auth_runtime", 4)
                .await,
            f.database
                .role_with_connections("webdock_auth_bootstrap", 4)
                .await,
            f.database
                .role_with_connections("webdock_worker_runtime", 4)
                .await,
            "owned-plugin-jobs-rls-secret-32bytes".into(),
            "owned-hosting-secret-at-least-32bytes".into(),
            Arc::new(
                spitzli_persistence::IdAllocator::connect(
                    f.database.url.as_str(),
                    spitzli_ids::Identity::platform(0, 2).unwrap(),
                )
                .await
                .unwrap(),
            ),
        )
        .await
        .unwrap(),
    );
    service.execute(&f.actor,json!({"action":"projects.create","projectID":"124","provider":"k3s","clusterID":"200","mode":"selfservice","ownImages":true,"idempotencyKey":"scaling-project-1"})).await.unwrap();
    let input = |key: &str| json!({"action":"apps.create","projectID":"124","name":key,"subscriptionRevision":1,"idempotencyKey":key,"spec":{"template":"custom","image":format!("registry.example/app@sha256:{}","a".repeat(64)),"args":[],"port":8080,"healthPath":"/","cpuMillicores":100,"memoryBytes":16777216,"ephemeralBytes":1048576,"volumeBytes":0,"replicas":1,"autoscaling":{"minReplicas":1,"maxReplicas":3,"targetCPUUtilizationPercent":70}}});
    let (first, second) = tokio::join!(
        service.execute(&f.actor, input("concurrent-first")),
        service.execute(&f.actor, input("concurrent-second"))
    );
    assert_eq!(
        usize::from(first.is_ok()) + usize::from(second.is_ok()),
        1,
        "exactly one maximum reservation fits"
    );
    let winner = if first.is_ok() {
        "concurrent-first"
    } else {
        "concurrent-second"
    };
    let saved = first.or(second).unwrap();
    assert_eq!(
        service.execute(&f.actor, input(winner)).await.unwrap(),
        saved,
        "retry is idempotent"
    );
    let row = f.owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT sum((demand->>'cpuMillicores')::bigint)::bigint AS cpu FROM webdock_auth.hosting_reservation WHERE status<>'released'" )).await.unwrap().unwrap();
    assert_eq!(row.try_get::<i64>("", "cpu").unwrap(), 300);
    let agent = service
        .authenticate_agent("200", &credential)
        .await
        .unwrap();
    sql(&f.owner,"UPDATE webdock_auth.hosting_agent SET observation=observation #- '{capabilities,autoscalingVersion}' WHERE cluster_id='200'",vec![]).await;
    assert!(
        service.claim_runtime(&agent).await.unwrap().is_none(),
        "old agent must not lease automatic work"
    );
    sql(&f.owner,"UPDATE webdock_auth.hosting_agent SET observation=jsonb_set(observation,'{capabilities,autoscalingVersion}','1') WHERE cluster_id='200'",vec![]).await;
    let command = service.claim_runtime(&agent).await.unwrap().unwrap();
    assert_eq!(command.spec.autoscaling.as_ref().unwrap().max_replicas, 3);
    sql(&f.owner,"UPDATE webdock_auth.hosting_agent SET observation=observation #- '{capabilities,autoscalingVersion}' WHERE cluster_id='200'",vec![]).await;
    assert!(
        service
            .guard_runtime_command(&agent, &command)
            .await
            .is_err(),
        "capability loss invalidates execution authority"
    );
    sql(&f.owner,"UPDATE webdock_auth.hosting_agent SET observation=jsonb_set(observation,'{capabilities,autoscalingVersion}','1') WHERE cluster_id='200'",vec![]).await;
    let mut observed: spitzli_supervisor::ObservedState = serde_json::from_value(json!({
        "resourceId":command.spec.resource_id.to_string(),"revision":command.spec.revision,"phase":"running","ready":true,
        "observedAt":spitzli_supervisor::timestamp().unwrap(),"runtimeId":"owned-uid","conditions":[],
        "autoscaling":{"minReplicas":1,"maxReplicas":3,"currentReplicas":4,"readyReplicas":4,"desiredReplicas":4,"metricsAvailable":true,"reason":"MetricsAvailable"}
    })).unwrap();
    assert!(
        service
            .complete_runtime(&agent, &command, &observed)
            .await
            .is_err(),
        "over-limit observation is not success"
    );
    let scaling = observed.autoscaling.as_mut().unwrap();
    scaling.current_replicas = 2;
    scaling.ready_replicas = 2;
    scaling.desired_replicas = 2;
    service
        .complete_runtime(&agent, &command, &observed)
        .await
        .unwrap();
    let reserved=f.owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT sum((demand->>'cpuMillicores')::bigint)::bigint AS cpu FROM webdock_auth.hosting_reservation WHERE status<>'released'")).await.unwrap().unwrap();
    assert_eq!(
        reserved.try_get::<i64>("", "cpu").unwrap(),
        300,
        "running below max does not release reserved capacity"
    );
    let seed = f.owner.begin().await.unwrap();
    seed.execute_unprepared("SELECT set_config('webdock.organization_allocation','{\"customer_id\":\"223\",\"organization_id\":\"225\"}',true); INSERT INTO webdock_admin.customers(id,name) VALUES('223','Other'); INSERT INTO webdock_admin.projects(id,name,customer_id) VALUES('224','Other','223')").await.unwrap();
    seed.commit().await.unwrap();
    let tx = f.auth.begin().await.unwrap();
    let member = f
        .identity
        .authenticate_session_in(&tx, "member", "member-session")
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    let mut foreign = input("foreign-scaling-1");
    foreign["projectID"] = json!("224");
    assert!(service.execute(&member, foreign).await.is_err());
}
