//! Plugin workloads over the real agent transport against the retained local fixture: an enrolled
//! agent sees only its cluster's due units and reports observations and removals.
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tonic::transport::{Endpoint, Server};
use webdock_agent_control::{ControlPlane, LocalBinding};
use webdock_agent_protocol::{self as wire, workload_control_client::WorkloadControlClient};

async fn db(key: &str) -> DatabaseConnection {
    let raw = std::env::var(key).unwrap();
    let url = url::Url::parse(&raw).unwrap();
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    assert_eq!(url.path(), "/webdock_rls_test");
    Database::connect(raw).await.unwrap()
}
async fn execute(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .await
    .unwrap();
}
async fn cluster(
    owner: &DatabaseConnection,
    ids: &webdock_persistence::IdAllocator,
    agent: &str,
) -> (String, String) {
    let cluster = ids.allocate().await.unwrap().to_string();
    let installation = format!("wl-{}", &cluster[cluster.len() - 12..]);
    let hash = hex::encode(Sha256::digest(format!("local-fixture-{cluster}")));
    execute(owner, "INSERT INTO webdock_auth.hosting_cluster(id,name,provider,region,location_evidence,generation,verified,capacity,runtime_kind,runtime_installation) VALUES($1,$2,'fixture','local','owned workload fixture',1,true,'{}','kubernetes',$2)", vec![cluster.clone().into(), installation.clone().into()]).await;
    execute(owner, "INSERT INTO webdock_auth.hosting_agent(cluster_id,credential_hash,generation,last_sequence,last_seen,observation) VALUES($1,$2,1,0,NULL,'{}')", vec![cluster.clone().into(), hash.clone().into()]).await;
    let uid = rustix::process::geteuid().as_raw();
    execute(owner, "INSERT INTO webdock_auth.agent_transport_enrollment(installation,agent_id,generation,cluster_id,credential_hash,transport,unix_uid) VALUES($1,$2,1,$3,$4,'unix',$5)", vec![installation.clone().into(), agent.into(), cluster.clone().into(), hash.into(), i64::from(uid).into()]).await;
    (cluster, installation)
}

#[tokio::test]
#[ignore = "requires owned retained-schema local fixture; only unique test records are mutated"]
async fn agents_poll_their_cluster_and_report() {
    let owner = db("WEBDOCK_TEST_MIGRATION_DATABASE_URL").await;
    let auth = db("AUTH_DATABASE_URL").await;
    let ids = Arc::new(
        webdock_persistence::IdAllocator::connect(
            &std::env::var("AUTH_DATABASE_URL").unwrap(),
            webdock_ids::Identity::platform(0, 2).unwrap(),
        )
        .await
        .unwrap(),
    );
    let (mine, installation) = cluster(&owner, &ids, "local-agent").await;
    let (other, _) = cluster(&owner, &ids, "other-agent").await;
    let customer = ids.allocate().await.unwrap().to_string();
    let project = ids.allocate().await.unwrap().to_string();
    let plugin = format!("wl-{customer}");
    execute(
        &owner,
        "INSERT INTO webdock_admin.customers(id,name) VALUES($1,$2)",
        vec![
            customer.clone().into(),
            format!("workload {customer}").into(),
        ],
    )
    .await;
    execute(
        &owner,
        "INSERT INTO webdock_admin.projects(id,name,customer_id) VALUES($1,'Workloads',$2)",
        vec![project.clone().into(), customer.clone().into()],
    )
    .await;
    execute(&owner, "INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version,runtime,current_generation) VALUES($1,'external','enabled','enabled','1.0.0','ready',1)", vec![plugin.clone().into()]).await;
    let objects = json!([{"apiVersion":"v1","kind":"Service","metadata":{"name":"web","namespace":"wd-x-1"}}]);
    for (unit, cluster) in [("mine", &mine), ("theirs", &other)] {
        execute(&owner, "INSERT INTO webdock_auth.plugin_workload(plugin_id,unit,project_id,cluster_id,namespace,objects,revision) VALUES($1,$2,$3,$4,'wd-x-1',$5,2)", vec![plugin.clone().into(), unit.into(), project.clone().into(), cluster.clone().into(), objects.clone().into()]).await;
    }

    let hosting = Arc::new(
        webdock_hosting::HostingService::new(
            db("DATABASE_URL").await,
            auth.clone(),
            db("WEBDOCK_PLUGIN_WORKER_DATABASE_URL").await,
            std::env::var("DATABASE_RLS_SECRET").unwrap(),
            std::env::var("BETTER_AUTH_SECRET").unwrap(),
            ids.clone(),
        )
        .await
        .unwrap(),
    );
    let uid = rustix::process::geteuid().as_raw();
    let control = ControlPlane::new(
        auth.clone(),
        hosting,
        Some(LocalBinding {
            installation: installation.clone(),
            agent_id: "local-agent".into(),
            uid,
        }),
    )
    .await
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("control.sock");
    let socket = tokio::net::UnixListener::bind(&path).unwrap();
    let (stop, shutdown) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        Server::builder()
            .add_service(control.workload_service())
            .serve_with_incoming_shutdown(
                tokio_stream::wrappers::UnixListenerStream::new(socket),
                async {
                    let _ = shutdown.await;
                },
            )
            .await
            .unwrap();
    });
    let channel = Endpoint::from_shared(format!("unix://{}", path.display()))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let mut client = WorkloadControlClient::new(channel);
    let identity = Some(wire::AgentIdentity {
        installation: installation.clone(),
        agent_id: "local-agent".into(),
        generation: 1,
    });
    let poll = |client: &mut WorkloadControlClient<tonic::transport::Channel>| {
        let mut client = client.clone();
        let identity = identity.clone();
        async move {
            client
                .poll_workloads(wire::PollRequest { identity })
                .await
                .unwrap()
                .into_inner()
                .workloads
        }
    };
    let report = |revision: u64, removed: bool| wire::WorkloadReport {
        identity: identity.clone(),
        plugin_id: plugin.clone(),
        unit: "mine".into(),
        revision,
        observed_json: if removed {
            vec![]
        } else {
            br#"{"ready":true}"#.to_vec()
        },
        error: String::new(),
        removed,
    };

    // Only this cluster's unit is due, with its pinned objects.
    let due = poll(&mut client).await;
    let ours = due
        .iter()
        .filter(|w| w.plugin_id == plugin)
        .collect::<Vec<_>>();
    assert_eq!(ours.len(), 1);
    assert_eq!(
        (
            ours[0].unit.as_str(),
            ours[0].revision,
            ours[0].namespace.as_str()
        ),
        ("mine", 2, "wd-x-1")
    );
    // A report for a revision that does not exist yet is refused; a real one settles the unit.
    assert!(
        !client
            .report_workload(report(3, false))
            .await
            .unwrap()
            .into_inner()
            .accepted
    );
    assert!(
        client
            .report_workload(report(2, false))
            .await
            .unwrap()
            .into_inner()
            .accepted
    );
    assert!(
        poll(&mut client)
            .await
            .iter()
            .all(|w| w.plugin_id != plugin),
        "applied and freshly observed units are not due"
    );
    // Reports also carry log lines, usage and problems: Core appends only new lines, samples usage
    // and opens a notice.
    let at = chrono::Utc::now() - chrono::Duration::seconds(10);
    let stamp = |millis| {
        (at + chrono::Duration::milliseconds(millis))
            .to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
    };
    let mut detailed = report(2, false);
    detailed.observed_json = serde_json::to_vec(&json!({"ready": false,
        "logs": [format!("{} first", stamp(0)), format!("{} second", stamp(1000)), "not a log line"],
        "usage": {"cpu": 12, "memory": 40}, "problems": [{"reason": "CrashLoopBackOff"}]})).unwrap();
    assert!(
        client
            .report_workload(detailed.clone())
            .await
            .unwrap()
            .into_inner()
            .accepted
    );
    detailed.observed_json = serde_json::to_vec(&json!({"ready": false,
        "logs": [format!("{} second", stamp(1000)), format!("{} third", stamp(2500))],
        "usage": {"cpu": 20, "memory": 60}, "problems": [{"reason": "CrashLoopBackOff"}]}))
    .unwrap();
    assert!(
        client
            .report_workload(detailed)
            .await
            .unwrap()
            .into_inner()
            .accepted
    );
    let scalar = |sql: &'static str| {
        let (owner, plugin) = (owner.clone(), plugin.clone());
        async move {
            owner
                .query_one_raw(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    sql,
                    [plugin.into()],
                ))
                .await
                .unwrap()
                .unwrap()
                .try_get::<String>("", "v")
                .unwrap()
        }
    };
    assert_eq!(scalar("SELECT string_agg(line,',' ORDER BY at) AS v FROM webdock_auth.workload_log WHERE plugin_id=$1").await, "first,second,third");
    assert_eq!(scalar("SELECT samples||' '||cpu_max||' '||memory_max AS v FROM webdock_auth.workload_usage WHERE plugin_id=$1").await, "2 20 60");
    assert_eq!(scalar("SELECT string_agg(kind,',') AS v FROM webdock_auth.notification WHERE plugin_id=$1 AND resolved_at IS NULL").await, "crashing");
    // Another cluster's unit cannot be reported by this agent.
    let mut foreign = report(2, false);
    foreign.unit = "theirs".into();
    assert!(
        !client
            .report_workload(foreign)
            .await
            .unwrap()
            .into_inner()
            .accepted
    );
    // Removal ends only for the current revision.
    assert!(
        !client
            .report_workload(report(2, true))
            .await
            .unwrap()
            .into_inner()
            .accepted,
        "not removing yet"
    );
    execute(&owner, "UPDATE webdock_auth.plugin_workload SET removing=true,objects='[]',revision=3 WHERE plugin_id=$1 AND unit='mine'", vec![plugin.clone().into()]).await;
    assert!(
        poll(&mut client)
            .await
            .iter()
            .any(|w| w.plugin_id == plugin && w.removing)
    );
    assert!(
        client
            .report_workload(report(3, true))
            .await
            .unwrap()
            .into_inner()
            .accepted
    );
    let left: i64 = owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT count(*) AS n FROM webdock_auth.plugin_workload WHERE plugin_id=$1",
            [plugin.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "n")
        .unwrap();
    assert_eq!(left, 1, "only the other cluster's unit remains");
    assert_eq!(
        scalar("SELECT count(*)::text AS v FROM webdock_auth.notification WHERE plugin_id=$1 AND resolved_at IS NULL").await,
        "0",
        "a removed unit needs no attention"
    );
    for table in ["notification", "workload_usage", "workload_log"] {
        execute(
            &owner,
            &format!("DELETE FROM webdock_auth.{table} WHERE plugin_id=$1"),
            vec![plugin.clone().into()],
        )
        .await;
    }

    let _ = stop.send(());
    server.await.unwrap();
    execute(
        &owner,
        "DELETE FROM webdock_auth.plugin_workload WHERE plugin_id=$1",
        vec![plugin.clone().into()],
    )
    .await;
    execute(
        &owner,
        "DELETE FROM webdock_auth.plugin_installation WHERE id=$1",
        vec![plugin.into()],
    )
    .await;
    execute(
        &owner,
        "DELETE FROM webdock_admin.projects WHERE id=$1",
        vec![project.into()],
    )
    .await;
    execute(
        &owner,
        "DELETE FROM webdock_admin.customers WHERE id=$1",
        vec![customer.into()],
    )
    .await;
    for cluster in [mine, other] {
        execute(
            &owner,
            "DELETE FROM webdock_auth.agent_transport_enrollment WHERE cluster_id=$1",
            vec![cluster.clone().into()],
        )
        .await;
        execute(
            &owner,
            "DELETE FROM webdock_auth.hosting_agent WHERE cluster_id=$1",
            vec![cluster.clone().into()],
        )
        .await;
        execute(
            &owner,
            "DELETE FROM webdock_auth.hosting_cluster WHERE id=$1",
            vec![cluster.into()],
        )
        .await;
    }
}
