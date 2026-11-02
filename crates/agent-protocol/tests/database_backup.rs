use serde_json::{Value, json};
use webdock_agent_protocol::database::*;
fn spec() -> Value {
    json!({"version":1,"operationID":"100","agentGeneration":3,"operationGeneration":2,"clusterID":"101","action":"configureBackup","phase":"apply","revision":1,"previousPodUID":null,"backend":{"id":"102","kind":"dedicated","namespace":"wd-db-102","node":"fixture-node","poolID":"00000000-0000-4000-8000-000000000001","revision":1,"cnpgUID":"source-uid","resources":{"cpuMillicores":500,"memoryBytes":1073741824,"volumeBytes":10737418240_u64},"host":"db-102.example.invalid","directPort":32000,"pooledPort":32001},"database":{"id":"103","kind":"dedicated","name":"db_103","revision":1,"credentialRevision":1,"connectionLimit":10},"members":[{"id":"103","projectID":"104","databaseName":"db_103","enabled":true}],"migration":null,"backup":{"id":"105","installationID":"102","customerID":"106","projectID":"104","sourceCnpgUID":"source-uid","revision":1,"resourceRevision":1,"destination":{"id":"107","endpointURL":"https://s3.example.invalid","region":"eu-central-1","bucket":"fixture-backup","archivePath":"postgresql/106/104/102/105","serverName":"wd-102-105","credentialID":"108","credentialRevision":1,"recoveryCredentialID":"109","recoveryCredentialRevision":1,"keyRecoveryID":"110"},"schedule":"0 0 1 * * *","suspended":false,"walEnabled":true,"retentionDays":30,"retentionDelete":false,"sidecar":{"cpuMillicores":100,"memoryBytes":134217728,"ephemeralBytes":268435456},"recoveryJob":{"cpuMillicores":500,"memoryBytes":536870912,"ephemeralBytes":536870912}}})
}
#[test]
fn policy_is_uid_owner_archive_and_revision_bound_without_secrets() {
    let value = spec();
    let job: DatabaseJobSpec =
        serde_json::from_value(value.clone()).expect("native backup policy contract");
    job.validate().unwrap();
    for (path, replacement) in [
        ("/backup/installationID", json!("999")),
        ("/backup/sourceCnpgUID", json!("foreign-uid")),
        (
            "/backup/destination/archivePath",
            json!("postgresql/foreign"),
        ),
        ("/backup/destination/serverName", json!("postgres")),
        ("/backup/projectID", json!("999")),
        ("/backup/resourceRevision", json!(0)),
        ("/backup/schedule", json!("@every 1s")),
        ("/backend/kind", json!("shared-pool")),
    ] {
        let mut bad = value.clone();
        *bad.pointer_mut(path).unwrap() = replacement;
        assert!(
            serde_json::from_value::<DatabaseJobSpec>(bad).map_or(true, |s| s.validate().is_err()),
            "{path}"
        );
    }
    let mut bad = value;
    bad["backup"]["destination"]["secretAccessKey"] = json!("must-not-persist");
    assert!(serde_json::from_value::<DatabaseJobSpec>(bad).is_err());
}
#[test]
fn suspension_is_independent_of_wal_and_retention_deletion() {
    let mut value = spec();
    value["backup"]["suspended"] = json!(true);
    let job: DatabaseJobSpec = serde_json::from_value(value).unwrap();
    job.validate().unwrap();
    let serialized = serde_json::to_value(job).unwrap();
    assert_eq!(serialized["backup"]["walEnabled"], true);
    assert_eq!(serialized["backup"]["retentionDelete"], false);
}
#[test]
fn backup_receipt_requires_separate_observed_controls_and_resource_versions() {
    let proof = json!({"policyID":"105","revision":1,"scheduledBackupUID":"schedule-uid","suspended":true,"backupUID":null,"baseBackupId":null,"completedAt":null});
    assert!(
        serde_json::from_value::<DatabaseBackupProof>(proof.clone()).is_err(),
        "a schedule alone cannot prove WAL or retention authority"
    );
    let mut complete = proof;
    complete["controls"] = json!({"schedule":"0 0 1 * * *","walEnabled":true,"retentionDelete":false,"retentionDays":30,"scheduledBackupResourceVersion":"123","clusterResourceVersion":"124","objectStoreUID":"objectstore-uid","objectStoreResourceVersion":"125"});
    serde_json::from_value::<DatabaseBackupProof>(complete.clone())
        .unwrap()
        .validate()
        .unwrap();
    complete["controls"]["objectStoreResourceVersion"] = json!("");
    assert!(
        serde_json::from_value::<DatabaseBackupProof>(complete)
            .unwrap()
            .validate()
            .is_err()
    );
}
