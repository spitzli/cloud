use serde_json::{Value, json};
use webdock_databases::ManagedCommand;
fn plan() -> Value {
    json!({"kind":"dedicated","postgresMajor":18,"cpuMillicores":500,"memoryBytes":1073741824,"volumeBytes":10737418240_u64,"logicalStorageAllowanceBytes":null,"name":"Dedicated","monthlyPriceCents":1200,"currency":"EUR","connectionLimit":10,"terms":"The customer accepts the retained fixed monthly database subscription terms."})
}
#[test]
fn managed_resources_and_sponsorship_are_not_erased_by_json_schema() {
    let value = json!({"action":"databases.plans.create","plan":plan(),"idempotencyKey":"fixture-plan-create-123"});
    ManagedCommand::parse(value.clone()).unwrap();
    let mut invalid = value.clone();
    invalid["plan"]["logicalStorageAllowanceBytes"] = json!(1073741824);
    assert!(ManagedCommand::parse(invalid).is_err());
    let mut free = value.clone();
    free["plan"]["monthlyPriceCents"] = json!(0);
    assert!(ManagedCommand::parse(free.clone()).is_err());
    free["plan"]["sponsoredCustomerID"] = json!("123");
    ManagedCommand::parse(free).unwrap();
    let mut priced = value;
    priced["plan"]["sponsoredCustomerID"] = json!("123");
    assert!(ManagedCommand::parse(priced).is_err());
}
#[test]
fn maintenance_requires_revision_idempotency_and_exact_confirmation_shape() {
    for value in [
        json!({"action":"databases.pause","databaseID":"123"}),
        json!({"action":"databases.delete","databaseID":"123","revision":1,"idempotencyKey":"fixture-delete-123","confirmName":"name","planHash":"bad"}),
        json!({"action":"databases.get","databaseID":"123","token":"injected"}),
    ] {
        assert!(ManagedCommand::parse(value).is_err());
    }
    ManagedCommand::parse(json!({"action":"databases.get","databaseID":"123"})).unwrap();
}
#[test]
fn database_checkpoint_retains_agent_lease_generation_and_rejects_unknown_fields() {
    let value = json!({"operationID":"123","generation":2,"leaseToken":"ab".repeat(32),"phase":"applied","cnpgUID":"owned-installation"});
    let checkpoint: webdock_databases::DatabaseCheckpoint =
        serde_json::from_value(value.clone()).unwrap();
    checkpoint.validate().unwrap();
    assert_eq!(checkpoint.lease.generation, 2);
    let mut extra = value;
    extra["applicationPassword"] = json!("not-a-checkpoint-field");
    assert!(serde_json::from_value::<webdock_databases::DatabaseCheckpoint>(extra).is_err());
    let completed: webdock_databases::DatabaseCompletion=serde_json::from_value(json!({"operationID":"123","generation":2,"leaseToken":"ab".repeat(32),"outcome":"succeeded"})).unwrap();
    assert!(completed.validate().is_err());
}
