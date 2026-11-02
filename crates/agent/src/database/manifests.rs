use super::*;
use serde_json::{Value, json};
pub fn backend_manifests(
    job: &DatabaseJobSpec,
    config: &DatabaseRuntimeConfig,
) -> Result<Vec<Value>> {
    config.admit(job)?;
    let b = &job.backend;
    let r = &b.resources;
    let ns = &b.namespace;
    let labels =
        json!({"app.kubernetes.io/managed-by":"webdock","webdock.dev/database-installation":b.id});
    let meta = |name: &str| json!({"name":name,"namespace":ns,"labels":labels});
    let resources = json!({"cpu":format!("{}m",r.cpu_millicores),"memory":r.memory_bytes.to_string(),"ephemeral-storage":"268435456"});
    let projects = job
        .members
        .iter()
        .map(|m| m.project_id.as_str())
        .chain(config.auxiliary(job)?.iter().map(|e| e.project_id.as_str()))
        .collect::<BTreeSet<_>>();
    let mut ingress = vec![];
    if !job.members.is_empty() {
        ingress.push(json!({"from":projects.iter().map(|p|json!({"namespaceSelector":{"matchLabels":{"webdock.dev/project-id":p}}})).collect::<Vec<_>>(),"ports":[{"protocol":"TCP","port":5432}]}));
    }
    ingress.extend([json!({"from":[{"podSelector":{}}]}),json!({"from":[{"namespaceSelector":{"matchLabels":{"kubernetes.io/metadata.name":"cnpg-system"}}}],"ports":[{"protocol":"TCP","port":8000}]}),json!({"from":[{"ipBlock":{"cidr":"0.0.0.0/0","except":config.pod_cidrs}}],"ports":[{"protocol":"TCP","port":5432}]})]);
    let pool_size = if job
        .members
        .iter()
        .any(|m| config.platform_databases.contains_key(&m.id))
    {
        "2"
    } else {
        "5"
    };
    let mut objects = vec![
        json!({"apiVersion":"v1","kind":"Namespace","metadata":{"name":ns,"labels":{"app.kubernetes.io/managed-by":"webdock","webdock.dev/database-installation":b.id,"webdock.dev/workload-class":"database","pod-security.kubernetes.io/enforce":"restricted"}}}),
        json!({"apiVersion":"networking.k8s.io/v1","kind":"NetworkPolicy","metadata":meta("default-deny"),"spec":{"podSelector":{},"policyTypes":["Ingress","Egress"]}}),
        json!({"apiVersion":"networking.k8s.io/v1","kind":"NetworkPolicy","metadata":meta("database-infrastructure"),"spec":{"podSelector":{},"policyTypes":["Ingress","Egress"],"ingress":ingress,"egress":[{"to":[{"podSelector":{}}]},{"to":[{"namespaceSelector":{"matchLabels":{"kubernetes.io/metadata.name":"kube-system"}}}],"ports":[{"protocol":"UDP","port":53},{"protocol":"TCP","port":53}]},{"to":config.api_cidrs.iter().map(|cidr|json!({"ipBlock":{"cidr":cidr}})).collect::<Vec<_>>(),"ports":[{"protocol":"TCP","port":443},{"protocol":"TCP","port":6443}]}]}}),
        json!({"apiVersion":"v1","kind":"ResourceQuota","metadata":meta("database-budget"),"spec":{"hard":{"requests.cpu":format!("{}m",r.cpu_millicores+200),"limits.cpu":format!("{}m",r.cpu_millicores+200),"requests.memory":(r.memory_bytes+536870912).to_string(),"limits.memory":(r.memory_bytes+536870912).to_string(),"requests.storage":r.volume_bytes.to_string(),"persistentvolumeclaims":"1","pods":"4"}}}),
        json!({"apiVersion":"cert-manager.io/v1","kind":"Certificate","metadata":meta("server-tls"),"spec":{"secretName":"server-tls","dnsNames":[b.host,"postgres-rw",format!("postgres-rw.{ns}"),format!("postgres-rw.{ns}.svc"),format!("postgres-rw.{ns}.svc.cluster.local")],"issuerRef":{"name":config.issuer,"kind":"ClusterIssuer"},"privateKey":{"rotationPolicy":"Always"}}}),
        json!({"apiVersion":"postgresql.cnpg.io/v1","kind":"Cluster","metadata":meta("postgres"),"spec":{"instances":1,"smartShutdownTimeout":10,"imageName":config.image,"postgresUID":26,"postgresGID":26,"enableSuperuserAccess":false,"resources":{"requests":resources,"limits":resources},"storage":{"size":r.volume_bytes.to_string(),"storageClass":"","pvcTemplate":{"volumeName":format!("wd-db-{}",b.id),"accessModes":["ReadWriteOnce"]}},"certificates":{"serverTLSSecret":"server-tls","serverCASecret":"server-ca"},"bootstrap":{"initdb":{"postInitSQL":["REVOKE ALL ON DATABASE postgres, template0, template1 FROM PUBLIC"]}},"postgresql":{"enableAlterSystem":false,"parameters":{"password_encryption":"scram-sha-256","shared_buffers":format!("{}MB",32.max(r.memory_bytes/4194304)),"max_connections":"100","max_wal_size":"256MB","min_wal_size":"64MB","log_statement":"none"},"pg_hba":config.hba(job)?}}}),
        json!({"apiVersion":"postgresql.cnpg.io/v1","kind":"Pooler","metadata":meta("pooler"),"spec":{"cluster":{"name":"postgres"},"instances":1,"type":"rw","pgbouncer":{"poolMode":"transaction","parameters":{"max_client_conn":"80","default_pool_size":pool_size,"max_db_connections":"10","max_user_connections":"10","server_connect_timeout":"10","client_tls_sslmode":"require","server_tls_sslmode":"verify-full"}},"template":{"spec":{"nodeSelector":{"kubernetes.io/hostname":b.node},"initContainers":[{"name":"bootstrap-controller","resources":{"requests":{"cpu":"100m","memory":"268435456","ephemeral-storage":"134217728"},"limits":{"cpu":"100m","memory":"268435456","ephemeral-storage":"268435456"}}}],"containers":[{"name":"pgbouncer","image":config.pooler_image,"resources":{"requests":{"cpu":"100m","memory":"134217728","ephemeral-storage":"134217728"},"limits":{"cpu":"100m","memory":"134217728","ephemeral-storage":"268435456"}}}]}}}}),
    ];
    if let Some(recovery) = &job.recovery {
        let cluster = objects
            .iter_mut()
            .find(|o| o["kind"] == "Cluster")
            .ok_or(ExecutionError::Runtime)?;
        cluster["spec"]["bootstrap"] = json!({"recovery":{"source":"backup-source","recoveryTarget":{"backupID":recovery.base_backup_id,"targetTime":recovery.target_time.to_rfc3339(),"targetTLI":recovery.timeline.to_string()}}});
        cluster["spec"]["externalClusters"] = json!([{"name":"backup-source","plugin":{"name":"barman-cloud.cloudnative-pg.io","parameters":{"barmanObjectName":format!("backup-{}",recovery.source.id),"serverName":recovery.source.destination.server_name}}}]);
    }
    Ok(objects)
}
