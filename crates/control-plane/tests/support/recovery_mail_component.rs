//! Matched Mail lane. Every tenant/installation/policy is admitted through the current Core.
//! Physical ceilings are explicit; an existing local CE volume is NOT called a per-volume quota.
use super::*;
use sea_orm::TransactionTrait;
use spitzli_agent::storage::recovery_pool::{ApprovedRecoveryVolume, RecoveryVolumeRole};
use spitzli_agent_protocol::mail::MailResources;
use std::{collections::BTreeMap, path::PathBuf};

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;
const PG_IMAGE: &str = "docker.io/library/postgres@sha256:4da1a4828be12604092fa55311276f08f9224a74a62dcb4708bd7439e2a03911";
const CE_IMAGE: &str = "webdock.local/mail-stalwart@sha256:09460db6473d8d7103836c806603096bc6d9b59ad3d90ce8276d553e88e2c1ad";

pub(crate) fn enabled() -> bool {
    std::env::var("SPITZLI_RECOVERY_MAIL_COMPONENT").as_deref() == Ok("1")
}
/// These are configuration ceilings, not evidence of PG initialization or data usage.
/// Named preflight must measure the pinned images and actual PGDATA/WAL/tmp mounts.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MailBudget {
    pub source_ce_bytes: u64,
    pub target_ce_bytes: u64,
    pub source_core_pg_bytes: u64,
    pub source_private_pg_bytes: u64,
    pub target_core_pg_bytes: u64,
    pub target_private_pg_bytes: u64,
    pub backup_staging_bytes: u64,
    pub restore_staging_bytes: u64,
    pub retained_reserve_bytes: u64,
    pub product_data_ceiling_bytes: u64,
    pub image_store_ceiling_bytes: u64,
    pub exec_tmpfs_bytes: u64,
    pub outer_memory_bytes: u64,
    pub host_reserve_bytes: u64,
}
impl MailBudget {
    pub fn current() -> Self {
        let source_ce_bytes = 2 * GIB; // Exact signed PrivateMail Engines::objects PVC contract.
        let value = Self {
            source_ce_bytes,
            target_ce_bytes: source_ce_bytes,
            source_core_pg_bytes: 512 * MIB,
            source_private_pg_bytes: 512 * MIB,
            target_core_pg_bytes: 512 * MIB,
            target_private_pg_bytes: 512 * MIB,
            backup_staging_bytes: 512 * MIB,
            restore_staging_bytes: 512 * MIB,
            retained_reserve_bytes: 256 * MIB,
            product_data_ceiling_bytes: 8 * GIB,
            image_store_ceiling_bytes: 8 * GIB,
            exec_tmpfs_bytes: 3 * GIB,
            outer_memory_bytes: 6 * GIB,
            host_reserve_bytes: 50 * GIB,
        };
        assert!(value.accounted_data_peak() <= value.product_data_ceiling_bytes);
        assert_eq!(
            value.product_data_ceiling_bytes + value.image_store_ceiling_bytes,
            16 * GIB
        );
        value
    }
    pub fn accounted_data_peak(&self) -> u64 {
        [
            self.source_ce_bytes,
            self.target_ce_bytes,
            self.source_core_pg_bytes,
            self.source_private_pg_bytes,
            self.target_core_pg_bytes,
            self.target_private_pg_bytes,
            self.backup_staging_bytes,
            self.restore_staging_bytes,
            self.retained_reserve_bytes,
        ]
        .into_iter()
        .try_fold(0u64, u64::checked_add)
        .unwrap()
    }
    pub fn target_volumes(&self, customer: &str) -> BTreeMap<String, ApprovedRecoveryVolume> {
        let customer =
            spitzli_core::id::RecordId::parse(customer).expect("real Registry customer ID");
        let pg = |role, bytes| ApprovedRecoveryVolume {
            role,
            bytes,
            image: PG_IMAGE.into(),
            uid: 70,
            gid: 70,
            cpu_millicores: 250,
            memory_bytes: 512 * MIB,
            tmp_bytes: 64 * MIB,
        };
        BTreeMap::from([
            (
                "core".into(),
                pg(
                    RecoveryVolumeRole::CoreLedgerPostgres {},
                    self.target_core_pg_bytes,
                ),
            ),
            (
                "private".into(),
                pg(
                    RecoveryVolumeRole::PrivateMailPostgres {},
                    self.target_private_pg_bytes,
                ),
            ),
            (
                "ce".into(),
                ApprovedRecoveryVolume {
                    role: RecoveryVolumeRole::IncomingCe {
                        original_customer_id: customer,
                    },
                    bytes: self.target_ce_bytes,
                    image: CE_IMAGE.into(),
                    uid: 65532,
                    gid: 65532,
                    cpu_millicores: MailResources::FIXED.cpu_millicores as u32,
                    memory_bytes: 512 * MIB,
                    tmp_bytes: 64 * MIB,
                },
            ),
        ])
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum MailInvokeRole {
    OutgoingDomain,
    SendKnown,
    SendUncertain,
    IncomingDomain,
    IncomingActivate,
    MailboxCreate,
    MailboxView,
    MailboxCredential,
}
impl MailInvokeRole {
    const ALL: [Self; 8] = [
        Self::OutgoingDomain,
        Self::SendKnown,
        Self::SendUncertain,
        Self::IncomingDomain,
        Self::IncomingActivate,
        Self::MailboxCreate,
        Self::MailboxView,
        Self::MailboxCredential,
    ];
    fn token(self) -> &'static str {
        match self {
            Self::OutgoingDomain => "outgoing_domain",
            Self::SendKnown => "send_known",
            Self::SendUncertain => "send_uncertain",
            Self::IncomingDomain => "incoming_domain",
            Self::IncomingActivate => "incoming_activate",
            Self::MailboxCreate => "mailbox_create",
            Self::MailboxView => "mailbox_view",
            Self::MailboxCredential => "mailbox_credential",
        }
    }
    fn action(self) -> &'static str {
        match self {
            Self::OutgoingDomain | Self::IncomingDomain => "domains.create",
            Self::SendKnown | Self::SendUncertain => "mail.send",
            Self::IncomingActivate => "incoming.activate",
            Self::MailboxCreate => "mailboxes.create",
            Self::MailboxView => "mailboxes.view",
            Self::MailboxCredential => "mailboxes.credentials",
        }
    }
}
fn mail_public_code(body: &Value) -> &'static str {
    match body.get("error").and_then(|v| v.get("code")) {
        None => "missing",
        Some(Value::String(code)) => match code.as_str() {
            "invalid_input" => "invalid_input",
            "not_supported" => "not_supported",
            "permission_denied" => "permission_denied",
            "capability_unavailable" => "capability_unavailable",
            "conflict" => "conflict",
            "unavailable" => "unavailable",
            "deadline_exceeded" => "deadline_exceeded",
            "not_found" => "not_found",
            _ => "unknown",
        },
        Some(_) => "unknown",
    }
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct MailInvokeCounter {
    role: &'static str,
    action: &'static str,
    entered: u32,
    completed: u32,
    http_status: Option<u16>,
    public_code: &'static str,
}
struct MailInvokeTrace {
    rows: BTreeMap<MailInvokeRole, MailInvokeCounter>,
    path: PathBuf,
    recorded: bool,
    forward: Option<ForwardChildCapture>,
}
impl MailInvokeTrace {
    fn new(path: PathBuf) -> Self {
        Self {
            rows: MailInvokeRole::ALL
                .into_iter()
                .map(|role| {
                    (
                        role,
                        MailInvokeCounter {
                            role: role.token(),
                            action: role.action(),
                            entered: 0,
                            completed: 0,
                            http_status: None,
                            public_code: "missing",
                        },
                    )
                })
                .collect(),
            path,
            recorded: false,
            forward: None,
        }
    }
    fn enter(&mut self, role: MailInvokeRole) {
        let row = self.rows.get_mut(&role).unwrap();
        row.entered = row.entered.saturating_add(1);
        row.http_status = None;
        row.public_code = "missing";
    }
    fn result(&mut self, role: MailInvokeRole, status: StatusCode, body: &Value) {
        let row = self.rows.get_mut(&role).unwrap();
        row.completed = row.completed.saturating_add(1);
        row.http_status = Some(status.as_u16());
        row.public_code = mail_public_code(body);
    }
    fn complete(&self) -> bool {
        self.rows.iter().all(|(role, r)| {
            r.entered > 0
                && r.entered == r.completed
                && r.http_status == Some(200)
                && (*role == MailInvokeRole::MailboxView || r.entered == 1)
        })
    }
    fn retain(&mut self) {
        if self.recorded {
            return;
        }
        self.recorded = true;
        if let Some(forward) = self.forward.as_mut() {
            forward.retain();
        }
        let value = json!({"scope":"genuine-signed-mail-actions-v1","roles":self.rows.values().collect::<Vec<_>>(),"allRolesCompleted200":self.complete()});
        if super::write_diagnostic_json(&self.path, &value).is_err() {
            eprintln!("recovery_component mail_invoke_capture category=unavailable");
        }
    }
}
tokio::task_local! {static MAIL_INVOKE_TRACE:std::cell::RefCell<MailInvokeTrace>;}
fn retain_mail_invocations() {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = MAIL_INVOKE_TRACE.try_with(|v| v.borrow_mut().retain());
    }));
}
struct MailPhaseCapture;
impl Drop for MailPhaseCapture {
    fn drop(&mut self) {
        retain_mail_invocations();
        let _ = MAIL_INVOKE_TRACE.try_with(|v| v.borrow_mut().forward.take());
    }
}
struct MailInvokeAttempt {
    completed: bool,
}
impl Drop for MailInvokeAttempt {
    fn drop(&mut self) {
        if !self.completed {
            retain_mail_invocations();
        }
    }
}
fn invoke_prefix_enabled() -> bool {
    enabled()
        && std::env::var_os("SPITZLI_RECOVERY_MAIL_INVOKE_PREFIX").as_deref()
            == Some(std::ffi::OsStr::new("1"))
}
fn prelude_enabled() -> bool {
    enabled()
        && std::env::var_os("SPITZLI_RECOVERY_MAIL_PRELUDE").as_deref()
            == Some(std::ffi::OsStr::new("1"))
}

async fn invoke_mail(
    app: &Router,
    console: &str,
    project: &str,
    role: MailInvokeRole,
    input: Value,
    key: &str,
) -> Value {
    let action = role.action();
    let _ = MAIL_INVOKE_TRACE.try_with(|v| v.borrow_mut().enter(role));
    let mut attempt = MailInvokeAttempt { completed: false };
    let reply=request(app,CONSOLE,"POST","/api/console/plugins/invoke",console,json!({"pluginID":"mail","projectID":project,"action":action,"input":input,"idempotencyKey":key})).await;
    let _ = MAIL_INVOKE_TRACE.try_with(|v| v.borrow_mut().result(role, reply.status, &reply.body));
    attempt.completed = true;
    if reply.status != StatusCode::OK {
        retain_mail_invocations();
    }
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "genuine signed Mail action failed; private body withheld"
    );
    reply.body
}
/// The selected PrivateMail source activates one customer engine through its real signed action.
/// It is not the separate CoreIncoming10GiB service and creates no ownership_verified rows.
pub(super) async fn activate_incoming(
    app: &Router,
    console: &str,
    project: &str,
    domain: &str,
) -> Value {
    invoke_mail(
        app,
        console,
        project,
        MailInvokeRole::IncomingActivate,
        json!({"resource_id":domain}),
        "matched-incoming-activate",
    )
    .await
}
pub(super) async fn assert_outgoing_only(owner: &DatabaseConnection, customer: &str) {
    let value=row(owner,"SELECT jsonb_build_object('workloads',(SELECT count(*) FROM webdock_auth.plugin_workload WHERE plugin_id='mail' AND customer_id=$1),'nativeServices',(SELECT count(*) FROM webdock_mail.service WHERE customer_id=$1 AND desired_enabled),'nativeInstances',(SELECT count(*) FROM webdock_mail.instance WHERE customer_id=$1)) AS value",vec![customer.into()]).await;
    assert_eq!(
        value,
        json!({"workloads":0,"nativeServices":0,"nativeInstances":0})
    );
}
/// Admit a reviewed real signed release and drive the existing production worker.
/// Agent runtime promotion, Describe/Health and the operation receipt remain required afterward.
const MAIL_INSTALL_DIAGNOSTIC_SQL: &str = "SELECT jsonb_build_object('status',o.status,'phase',o.phase,'condition',o.checkpoint->>'condition','generation',o.generation,'attempts',o.attempts,'runtimeGeneration',e.runtime_generation,'specPresent',e.spec IS NOT NULL,'bindingPresent',e.binding IS NOT NULL,'componentsPresent',e.private_components<>'{}'::jsonb,'catalogFeatureCount',jsonb_array_length(e.supported_features)) AS value FROM webdock_auth.plugin_install_execution e JOIN webdock_auth.plugin_operation o ON o.id=e.operation_id JOIN webdock_auth.plugin_installation p ON p.id=e.plugin_id AND p.generation=o.generation WHERE e.plugin_id='mail' AND ($1::text IS NULL OR o.id=$1)";
fn mail_install_projection(body: &Value) -> Value {
    let known = |value: Option<&str>, allowed: &'static [&'static str]| {
        allowed
            .iter()
            .copied()
            .find(|v| Some(*v) == value)
            .unwrap_or(if value.is_none() {
                "none"
            } else {
                "unrecognized"
            })
    };
    let number = |key: &str| {
        body[key]
            .as_u64()
            .or_else(|| body[key].as_str().and_then(|v| v.parse::<u64>().ok()))
            .filter(|v| i64::try_from(*v).is_ok())
    };
    json!({
        "status":known(body["status"].as_str(), &["pending","running","succeeded","failed","blocked"]),
        "phase":known(body["phase"].as_str(), &["requested","preparing_storage","preparing_configuration","deploying","suspending_runtime","verifying_runtime","complete","failed","blocked"]),
        "condition":known(body["condition"].as_str(), &["authority_changed","private_provisioning_trust_rejected","private_storage_ownership_mismatch","private_provisioning_invalid","private_provisioning_unavailable","installation_authority_changed","installation_contract_invalid","installation_service_unavailable","installation_reconciliation_required","runtime_spec_invalid","private_file_unavailable","private_profile_invalid","kernel_trust_profile_mismatch","private_material_incomplete","private_storage_target_unavailable","private_storage_not_separate","reserved_private_configuration"]),
        "generation":number("generation"),"attempts":number("attempts"),"runtimeGeneration":number("runtimeGeneration"),
        "specPresent":body["specPresent"].as_bool(),"bindingPresent":body["bindingPresent"].as_bool(),"componentsPresent":body["componentsPresent"].as_bool(),
        "catalogFeatureCount":number("catalogFeatureCount").filter(|n| *n<=256),
        "shape":if body["status"].is_string() && body["phase"].is_string() && body.get("generation").is_some() {"flat-operation"} else {"unrecognized"}
    })
}
async fn mail_install_owner_projection(
    owner: &DatabaseConnection,
    operation: Option<&str>,
) -> Value {
    match owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            MAIL_INSTALL_DIAGNOSTIC_SQL,
            [operation.map(str::to_owned).into()],
        ))
        .await
    {
        Ok(Some(row)) => match row.try_get::<Value>("", "value") {
            Ok(value) => mail_install_projection(&value),
            Err(_) => json!({"category":"projection_decode_failed"}),
        },
        Ok(None) => json!({"category":"current_execution_absent"}),
        Err(_) => json!({"category":"owner_query_failed"}),
    }
}
async fn retain_mail_install_observation(
    owner: &DatabaseConnection,
    operation: Option<&str>,
    api: &Value,
    counters: [u32; 3],
    reason: &'static str,
    index: u8,
) {
    if ![
        "before_advance",
        "after_advance",
        "blocked",
        "worker_error",
        "install_expiry",
        "runtime_expiry",
    ]
    .contains(&reason)
        || index > 31
    {
        return;
    }
    let owner = tokio::time::timeout(
        Duration::from_secs(2),
        mail_install_owner_projection(owner, operation),
    )
    .await
    .unwrap_or_else(|_| json!({"category":"owner_projection_timeout"}));
    let record = json!({"reason":reason,"api":mail_install_projection(api),"owner":owner,"advanceEntered":counters[0],"advanceCompleted":counters[1],"claimsReturned":counters[2]});
    let path = Path::new("/diagnostics").join(format!("mail-install-{reason}-{index:02}.json"));
    if super::write_diagnostic_json(&path, &record).is_err() {
        eprintln!("recovery_component mail_install_capture category=unavailable");
    }
}
#[test]
fn mail_install_projection_excludes_raw_material_and_classifies_only_known_values() {
    let raw = json!({"status":"blocked","phase":"blocked","condition":"private_profile_invalid","generation":"1","attempts":2,"runtimeGeneration":1,"specPresent":false,"bindingPresent":false,"componentsPresent":true,"catalogFeatureCount":3,"payload":{"Authorization":"poison"},"private_components":{"password":"poison"},"spec":{"url":"poison"}});
    let safe = mail_install_projection(&raw);
    assert_eq!(safe["condition"], "private_profile_invalid");
    assert_eq!(safe["generation"], 1);
    assert!(!safe.to_string().contains("poison"));
    assert!(safe.get("payload").is_none());
    for field in ["status", "phase", "condition"] {
        let mut value = raw.clone();
        value[field] = json!("poison-secret-url");
        let projection = mail_install_projection(&value);
        assert_eq!(projection[field], "unrecognized");
        assert!(!projection.to_string().contains("poison"));
    }
    let mut value = raw.clone();
    value["attempts"] = json!(true);
    value["generation"] = json!("18446744073709551615");
    value["specPresent"] = json!("true");
    let safe = mail_install_projection(&value);
    assert!(
        safe["attempts"].is_null() && safe["generation"].is_null() && safe["specPresent"].is_null()
    );
    assert!(mail_install_terminal_failure(&json!({"data":raw})));
    assert!(mail_install_terminal_failure(&Value::Null));
}
#[tokio::test]
async fn mail_install_owner_query_failure_is_static_and_secondary() {
    let value = mail_install_owner_projection(&DatabaseConnection::default(), None).await;
    assert_eq!(value, json!({"category":"owner_query_failed"}));
}

fn mail_install_terminal_failure(body: &Value) -> bool {
    !matches!(
        body["status"].as_str(),
        Some("pending" | "running" | "succeeded")
    )
}
#[test]
fn actual_install_operation_snapshot_blocked_is_terminal_in_the_fixture() {
    use spitzli_plugin_host::{OperationSnapshot, OperationStatus};
    for status in [OperationStatus::Failed, OperationStatus::Blocked] {
        let actual = OperationSnapshot {
            kind: "install",
            id: "123".into(),
            plugin_id: "mail".into(),
            status,
            phase: "blocked".into(),
            generation: "1".into(),
            cursor: "1".into(),
            condition: Some("private_profile_invalid".into()),
        };
        assert!(mail_install_terminal_failure(
            &serde_json::to_value(actual).unwrap()
        ));
    }
}

pub(super) async fn install_current_release(
    app: &Router,
    console: &str,
    endpoints: &spitzli_control_plane::plugins::PluginEndpoints,
    worker: &spitzli_control_plane::plugins::installation::worker::InstallationWorker,
    descriptor: &spitzli_plugin_host::CatalogDescriptor,
    owner: &DatabaseConnection,
) -> Value {
    let target = descriptor
        .targets
        .first()
        .expect("actual owned catalog target");
    let reply = request(
        app,
        CONSOLE,
        "POST",
        "/api/console/plugins/install",
        console,
        json!({
            "catalogID":"mail","version":descriptor.version,"reviewSha256":target.review_sha256,
            "targetID":target.id,"configuration":{},"approvedCapabilities":["plugin.workloads"],
            "idempotencyKey":"matched-mail-current-install"
        }),
    )
    .await;
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "signed Mail install admission failed; private reply withheld"
    );
    let receipt = reply.body;
    let operation_id = receipt["id"]
        .as_str()
        .expect("actual admitted install operation");
    let mut counters = [0u32; 3];
    let mut saved = 0u8;
    let mut last = Value::Null;
    let outcome = tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let before = request(
                app,
                CONSOLE,
                "GET",
                &format!("/api/console/plugins/operations/{operation_id}"),
                console,
                Value::Null,
            )
            .await;
            assert_eq!(
                before.status,
                StatusCode::OK,
                "real install operation API unavailable"
            );
            let projected = mail_install_projection(&before.body);
            if projected != last && saved < 16 {
                retain_mail_install_observation(
                    owner,
                    Some(operation_id),
                    &before.body,
                    counters,
                    "before_advance",
                    saved,
                )
                .await;
                saved += 1;
                last = projected;
            }
            if mail_install_terminal_failure(&before.body) {
                retain_mail_install_observation(
                    owner,
                    Some(operation_id),
                    &before.body,
                    counters,
                    "blocked",
                    30,
                )
                .await;
                panic!("actual installation terminal state; safe condition retained");
            }
            counters[0] = counters[0].saturating_add(1);
            eprintln!(
                "recovery_component mail_install_advance entered={} completed={} claims={}",
                counters[0], counters[1], counters[2]
            );
            let Ok(claimed) = worker.advance(endpoints).await else {
                retain_mail_install_observation(
                    owner,
                    Some(operation_id),
                    &before.body,
                    counters,
                    "worker_error",
                    31,
                )
                .await;
                panic!("actual installation worker error; safe state retained");
            };
            counters[1] = counters[1].saturating_add(1);
            if claimed {
                counters[2] = counters[2].saturating_add(1);
            }
            eprintln!(
                "recovery_component mail_install_advance entered={} completed={} claims={}",
                counters[0], counters[1], counters[2]
            );
            let operation = request(
                app,
                CONSOLE,
                "GET",
                &format!(
                    "/api/console/plugins/operations/{}",
                    receipt["id"].as_str().expect("actual install operation")
                ),
                console,
                Value::Null,
            )
            .await;
            assert_eq!(
                operation.status,
                StatusCode::OK,
                "real retained install receipt unavailable"
            );
            let projected = mail_install_projection(&operation.body);
            if projected != last && saved < 16 {
                retain_mail_install_observation(
                    owner,
                    Some(operation_id),
                    &operation.body,
                    counters,
                    "after_advance",
                    saved,
                )
                .await;
                saved += 1;
                last = projected;
            }
            if operation.body["status"] == "succeeded" {
                break;
            }
            if mail_install_terminal_failure(&operation.body) {
                retain_mail_install_observation(
                    owner,
                    Some(operation_id),
                    &operation.body,
                    counters,
                    "blocked",
                    30,
                )
                .await;
                panic!("actual installation terminal state; safe condition retained");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    if outcome.is_err() {
        let observed = tokio::time::timeout(
            Duration::from_secs(2),
            request(
                app,
                CONSOLE,
                "GET",
                &format!("/api/console/plugins/operations/{operation_id}"),
                console,
                Value::Null,
            ),
        )
        .await;
        let api = observed.map(|value| value.body).unwrap_or(Value::Null);
        retain_mail_install_observation(
            owner,
            Some(operation_id),
            &api,
            counters,
            "install_expiry",
            31,
        )
        .await;
    }
    outcome.expect("actual signed runtime promotion");
    receipt
}
/// The real installed generation + real configured Agent profile are prerequisites to this API.
fn backup_deployment_metadata(raw: &Value) -> Value {
    let fields=raw["metadata"]["managedFields"].as_array().map(|rows|rows.iter().take(16).filter(|v|v["fieldsV1"]["f:spec"].get("f:replicas").is_some()).map(|v|{let manager=v["manager"].as_str().filter(|s|!s.is_empty()&&s.len()<=64&&s.bytes().all(|b|b.is_ascii_lowercase()||b.is_ascii_digit()||b"_.-".contains(&b))).unwrap_or("other");json!({"manager":manager,"operation":if v["operation"]=="Apply"{"Apply"}else if v["operation"]=="Update"{"Update"}else{"other"}})}).collect::<Vec<_>>());
    json!({"uid":raw["metadata"]["uid"].as_str().filter(|v|uuid::Uuid::parse_str(v).is_ok()),"generation":raw["metadata"]["generation"].as_u64(),"deleting":!raw["metadata"]["deletionTimestamp"].is_null(),"replicas":raw["spec"]["replicas"].as_u64(),"replicaManagers":fields,"platformOwned":raw["metadata"]["labels"]["webdock.dev/platform"]=="owned-recovery","pluginMail":raw["metadata"]["labels"]["webdock.dev/plugin"]=="mail","managedByWebdock":raw["metadata"]["labels"]["webdock.dev/managed-by"]=="webdock","runtimeGeneration":raw["metadata"]["labels"]["webdock.dev/runtime-generation"].as_str().filter(|s|s.len()<=10&&s.bytes().all(|b|b.is_ascii_digit()))})
}
fn backup_source_metadata(plan: &Value) -> Value {
    let mut rows = Vec::new();
    for (kind, key) in [("runtime", "runtimes"), ("ce", "workloads")] {
        for entry in plan["source"][key]
            .as_array()
            .into_iter()
            .flatten()
            .take(16)
        {
            let ns = entry["namespace"].as_str().filter(|s| {
                !s.is_empty()
                    && s.len() <= 63
                    && s.bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            });
            let Some(ns) = ns else {
                rows.push(json!({"kind":kind,"category":"namespace_invalid"}));
                continue;
            };
            let mut command = Command::new("/usr/bin/timeout");
            command.args([
                "--signal=TERM",
                "--kill-after=1",
                "2",
                "/usr/local/bin/kubectl",
                "get",
                "deployment",
                "-n",
                ns,
            ]);
            if kind == "runtime" {
                command.arg("runtime");
            } else if let Some(unit) = entry["unit"].as_str().filter(|s| {
                s.len() <= 63
                    && s.bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            }) {
                command.args([
                    "-l",
                    &format!("webdock.dev/plugin=mail,webdock.dev/unit={unit}"),
                ]);
            } else {
                rows.push(json!({"kind":kind,"category":"unit_invalid"}));
                continue;
            }
            command.args(["--show-managed-fields", "-o", "json"]);
            let (status, raw, _) = super::byte_component::collect_command(command, None);
            let parsed = serde_json::from_slice::<Value>(&raw).ok();
            let objects = parsed.as_ref().map(|v| {
                if v["kind"] == "DeploymentList" || v["kind"] == "List" {
                    v["items"].as_array().map(|v| {
                        v.iter()
                            .take(16)
                            .map(backup_deployment_metadata)
                            .collect::<Vec<_>>()
                    })
                } else {
                    Some(vec![backup_deployment_metadata(v)])
                }
            });
            rows.push(json!({"kind":kind,"returnCode":status.code(),"sourceGenerationNumeric":entry["generation"].is_u64(),"objects":objects}));
        }
    }
    json!({"sourceParticipants":rows,"enrolledExecutorsCount":plan["sourceFence"]["enrolledExecutors"].as_array().map(Vec::len),"agentIdentityUIDPresent":plan["sourceFence"]["agentIdentity"]["uid"].is_string()})
}
#[test]
fn source_metadata_projection_keeps_replica_owner_but_drops_env_and_private_labels() {
    let raw = json!({"metadata":{"uid":"11111111-1111-1111-1111-111111111111","labels":{"private":"PRIVATE-CANARY"},"managedFields":[{"manager":"kube-client","operation":"Update","fieldsV1":{"f:spec":{"f:replicas":{}}}}]},"spec":{"replicas":1,"template":{"spec":{"containers":[{"env":[{"value":"PRIVATE-CANARY"}]}]}}}});
    let safe = backup_deployment_metadata(&raw);
    assert_eq!(safe["replicaManagers"][0]["manager"], "kube-client");
    assert!(!safe.to_string().contains("PRIVATE-CANARY"));
}

fn mail_backup_projection(raw: &Value) -> Value {
    let known = |key: &str, values: &[&str]| {
        raw[key]
            .as_str()
            .filter(|v| values.contains(v))
            .unwrap_or("other")
            .to_owned()
    };
    let callbacks=raw["callbackKeys"].as_array().map(|rows|rows.iter().take(16).map(|v|{let(boundary,phase)=v.as_str().and_then(|v|v.split_once(':')).unwrap_or(("other",""));json!({"boundary":if ["checkpoint","fencing","fenced","resume","source-admit","source-drained"].contains(&boundary){boundary}else{"other"},"phase":super::app_component::safe_phase(if phase.is_empty(){None}else{Some(phase)})})}).collect::<Vec<_>>());
    json!({"status":known("status",&["queued","running","succeeded","failed","needs-reconciliation"]),"condition":known("condition",&["recovery_authority_or_integrity_denied","recovery_deadline","recovery_execution_uncertain","recovery_policy_revoked"]),"checkpointPhase":super::app_component::safe_phase(raw["checkpointPhase"].as_str()),"setPresent":raw["setID"].is_string(),"callbacks":callbacks})
}
#[test]
fn backup_terminal_projection_retains_only_fixed_phase_codes_and_not_private_proof() {
    let raw = json!({"status":"needs-reconciliation","condition":"recovery_authority_or_integrity_denied","checkpointPhase":"preflight","proof":{"private":"PRIVATE-CANARY"},"callbackKeys":["checkpoint:preflight","source-admit:","PRIVATE-CANARY:PRIVATE-CANARY"]});
    let safe = mail_backup_projection(&raw);
    assert_eq!(safe["checkpointPhase"], "preflight");
    assert_eq!(safe["condition"], "recovery_authority_or_integrity_denied");
    assert!(!safe.to_string().contains("PRIVATE-CANARY"));
}

pub(super) async fn matched_backup(
    app: &Router,
    console: &str,
    owner: &DatabaseConnection,
    cluster: &str,
    private_logs: &Path,
) -> Value {
    let policy=okay(post(app,console,json!({"action":"recovery.policies.grant","clusterID":cluster,"profile":"mail-installation-v1","profileId":"mail-source","capacityBytes":128*MIB,"intervalSeconds":86400,"idempotencyKey":"matched-mail-policy"})).await,"matched Mail policy");
    let backup=okay(post(app,console,json!({"action":"recovery.backup","policyID":policy["policyID"],"revision":1,"idempotencyKey":"matched-mail-backup"})).await,"matched Mail backup");
    let operation = backup["operationID"]
        .as_str()
        .expect("Core allocated Mail operation");
    let set=tokio::time::timeout(Duration::from_secs(180),async {
        loop {
            let value=row(owner,"SELECT jsonb_build_object('status',o.status,'setID',s.id,'proof',s.proof,'condition',o.result->'error','checkpointPhase',j.checkpoint->'phase','callbackKeys',(SELECT COALESCE(jsonb_agg(c.phase_key ORDER BY c.created_at),'[]') FROM webdock_auth.hosting_recovery_callback c WHERE c.operation_id=o.id),'fencingPlan',(SELECT c.response FROM webdock_auth.hosting_recovery_callback c WHERE c.operation_id=o.id AND c.phase_key='fencing:canonical-fencing' ORDER BY c.generation DESC LIMIT 1)) AS value FROM webdock_auth.hosting_operation o LEFT JOIN webdock_auth.hosting_recovery_job j ON j.operation_id=o.id LEFT JOIN webdock_auth.hosting_recovery_set s ON s.operation_id=o.id AND s.referenced WHERE o.id=$1",vec![operation.into()]).await;
            match value["status"].as_str() {
                Some("succeeded")=>{assert!(value["setID"].as_str().is_some());break value;},
                Some("failed"|"needs-reconciliation")=>{
                    let _=super::write_diagnostic_json(Path::new("/diagnostics/mail-backup-terminal.json"),&mail_backup_projection(&value));
                    let _=super::write_diagnostic_json(Path::new("/diagnostics/mail-backup-reservation-markers.json"),&super::app_component::reservation_markers(private_logs));
                    let source=std::panic::catch_unwind(||backup_source_metadata(&value["fencingPlan"])).unwrap_or_else(|_|json!({"category":"secondary_capture_unavailable"}));
                    let _=super::write_diagnostic_json(Path::new("/diagnostics/mail-backup-source-metadata.json"),&source);
                    panic!("matched Mail backup failed; safe terminal phase retained");
                },
                _=>tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
    }).await.expect("actual matched Mail completed set");
    let exported = okay(
        post(
            app,
            console,
            json!({"action":"recovery.sets.export","recoverySetID":set["setID"]}),
        )
        .await,
        "matched Mail admitted operator export",
    );
    json!({"set":set,"export":exported})
}

#[test]
fn mail_resource_profile_preserves_fixed_incoming_and_explicit_target_roles() {
    let budget = MailBudget::current();
    assert_eq!(budget.source_ce_bytes, 2 * GIB);
    assert_eq!(budget.accounted_data_peak(), 7 * GIB + 256 * MIB);
    let roles = budget.target_volumes("123");
    assert_eq!(roles.len(), 3);
    assert!(matches!(
        roles["core"].role,
        RecoveryVolumeRole::CoreLedgerPostgres {}
    ));
    assert!(matches!(
        roles["private"].role,
        RecoveryVolumeRole::PrivateMailPostgres {}
    ));
    assert_eq!(roles["core"].uid, 70);
    assert_eq!(roles["private"].gid, 70);
    assert_eq!(roles["ce"].uid, 65532);
    let bytes = serde_json::to_value(roles).unwrap();
    assert!(bytes["core"]["role"].get("originalCustomerID").is_none());
    assert_eq!(bytes["ce"]["role"]["originalCustomerID"], "123");
}

/// Build the actual signed catalog only after the owned OCI image and real enrollment exist.
/// Policy/recipe are private operator inputs. Their review hash is computed by production Catalog.
pub(super) fn signed_catalog(
    cluster: &str,
    platform: &str,
    image: &str,
    policy: Value,
    recipe: Value,
    key: &ed25519_dalek::SigningKey,
) -> spitzli_plugin_host::Catalog {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
    use ed25519_dalek::Signer;
    spitzli_core::id::RecordId::parse(cluster).expect("actual enrolled cluster ID");
    let (name, digest) = image
        .split_once("@sha256:")
        .expect("actual current owned OCI root");
    assert!(
        name.starts_with("localhost/owned/")
            && digest.len() == 64
            && digest.bytes().all(|c| c.is_ascii_hexdigit())
    );
    let mut manifest: Value =
        serde_json::from_slice(include_bytes!("../../../../plugins/mail/manifest.json")).unwrap();
    manifest["image"] = json!(image);
    let raw = serde_json::to_string(&manifest).unwrap();
    let hash = format!("{:x}", Sha256::digest(raw.as_bytes()));
    let version = manifest["version"].as_str().unwrap();
    let payload = json!({"v":1,"registry":"https://registry.owned.test","plugin_slug":"mail","release_version":version,"kid":"owned-mail-current","manifest_sha256":hash,"assets":[{"name":"mail-universal.json","sha256":hash,"size":raw.len()}]});
    let body = format!(
        "{}.{}",
        B64.encode(br#"{"alg":"EdDSA","kid":"owned-mail-current"}"#),
        B64.encode(serde_json::to_vec(&payload).unwrap())
    );
    let signed = format!(
        "{body}.{}",
        B64.encode(key.sign(body.as_bytes()).to_bytes())
    );
    let catalog = json!({"version":1,"platformID":platform,
        "keys":[{"registryURL":"https://registry.owned.test","kid":"owned-mail-current","key":{"kty":"OKP","crv":"Ed25519","x":B64.encode(key.verifying_key().as_bytes())}}],
        "targets":[{"id":"matched-mail","label":"Owned matched Mail qualification","clusterID":cluster,"policy":policy}],
        "releases":[{"id":"mail","version":version,"registryURL":"https://registry.owned.test","kid":"owned-mail-current","jws":signed,"manifestRaw":raw,"descriptor":raw,"recipe":recipe,"supportedFeatures":["add-domains","create-mailboxes","manage-incoming"],"targetIDs":["matched-mail"]}]});
    let raw_catalog = serde_json::to_vec(&catalog).unwrap();
    private_bytes(Path::new("/owned/mail-fixture/catalog.json"), &raw_catalog);
    spitzli_plugin_host::Catalog::from_bytes(&raw_catalog)
        .expect("real signed current Mail release catalog")
}

/// Uses the current production generic installer, native control and canonical Mail ledger.
/// Every native actor/session/customer is supplied by the genuine existing foundation.
pub(super) struct MailCore {
    pub plugins: Arc<spitzli_control_plane::plugins::PluginEndpoints>,
    pub http: Arc<spitzli_control_plane::mail::MailEndpoints>,
    pub sender: Arc<spitzli_mail::sending::Sending>,
    pub managed: Arc<spitzli_mail::managed_control::ManagedControl>,
}
pub(super) async fn core_services(
    dependencies: spitzli_mail::MailDependencies,
    oauth: Arc<spitzli_oauth::OAuthService>,
    plugin_config: &spitzli_control_plane::plugin_config::PluginConfig,
    private_environment: &HashMap<String, String>,
    transports: Vec<spitzli_mail::transport_adoption::ConfiguredTransport>,
) -> MailCore {
    let plugins = Arc::new(
        spitzli_control_plane::plugins::PluginEndpoints::initialize(
            spitzli_control_plane::plugins::PluginDependencies {
                business: dependencies.business.clone(),
                worker: dependencies.worker.clone(),
                auth: dependencies.auth.clone(),
                identity: dependencies.identity.clone(),
                browser: dependencies.browser.clone(),
                oauth,
                ids: dependencies.ids.clone(),
                rls_secret: SECRET.into(),
                database_new_orders_enabled: false,
                neon_connections: None,
            },
            plugin_config,
        )
        .await
        .expect("actual current native plugin/installation services"),
    );
    let sender = spitzli_control_plane::mail::initialize_sender(
        dependencies.clone(),
        plugins.host.clone(),
        plugin_config
            .mail
            .as_ref()
            .expect("actual configured native Mail signing identity"),
        private_environment,
        None,
    )
    .await
    .expect("actual canonical Mail sender");
    let keys = Arc::new(
        spitzli_mail::keys::NativeKeys::new(dependencies.clone(), SECRET, true)
            .await
            .expect("actual native key service"),
    );
    let queue = Arc::new(
        spitzli_mail::submissions::Submissions::new(dependencies, SECRET, true)
            .await
            .expect("actual canonical Mail queue"),
    );
    let worker_secret = private_environment
        .get("MAIL_SEND_WORKER_SECRET")
        .expect("private generated worker credential");
    let http = Arc::new(
        spitzli_control_plane::mail::MailEndpoints::sending_only(keys, worker_secret)
            .unwrap()
            .with_queue(queue.clone())
            .with_sender(sender.clone())
            .with_operator_transports(transports)
            .unwrap(),
    );
    let managed = Arc::new(spitzli_mail::managed_control::ManagedControl::new(
        sender.clone(),
        queue,
        plugins.host.clone(),
    ));
    MailCore {
        plugins,
        http,
        sender,
        managed,
    }
}
/// The real callbacks retain TLS peer identity; no controlled/fabricated provider is substituted.
pub(super) async fn serve_callbacks(
    core: &MailCore,
    listener: tokio::net::TcpListener,
    certificate: Vec<u8>,
    key: Vec<u8>,
    authority: Vec<u8>,
    shutdown: tokio::sync::oneshot::Receiver<()>,
) -> tokio::task::JoinHandle<()> {
    let host = core.plugins.host.clone().rpc();
    let sender = core.sender.clone().service();
    let managed = core.managed.clone().service();
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .tls_config(
                tonic::transport::ServerTlsConfig::new()
                    .identity(tonic::transport::Identity::from_pem(certificate, key))
                    .client_ca_root(tonic::transport::Certificate::from_pem(authority)),
            )
            .unwrap()
            .concurrency_limit_per_connection(32)
            .timeout(Duration::from_secs(35))
            .add_service(host)
            .add_service(sender)
            .add_service(managed)
            .serve_with_incoming_shutdown(
                tokio_stream::wrappers::TcpListenerStream::new(listener),
                async {
                    let _ = shutdown.await;
                },
            )
            .await
            .expect("owned current Core Mail callback server");
    })
}

struct MailPki {
    root: PathBuf,
    certificate: PathBuf,
    private_key: PathBuf,
    authority: PathBuf,
}
fn private_bytes(path: &Path, bytes: &[u8]) {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}
fn make_pki(root: &Path) -> MailPki {
    let mut ca = rcgen::CertificateParams::new(vec!["owned-mail-ca".into()]).unwrap();
    ca.distinguished_name
        .push(rcgen::DnType::CommonName, "Owned Mail fixture CA");
    ca.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca.key_usages = vec![
        rcgen::KeyUsagePurpose::KeyCertSign,
        rcgen::KeyUsagePurpose::CrlSign,
        rcgen::KeyUsagePurpose::DigitalSignature,
    ];
    let ca_key = rcgen::KeyPair::generate().unwrap();
    let ca_cert = ca.self_signed(&ca_key).unwrap();
    let issuer_private_pem = ca_key.serialize_pem();
    let issuer = rcgen::Issuer::new(ca, ca_key);
    let mut leaf = rcgen::CertificateParams::new(vec![
        "localhost".into(),
        "core.owned.test".into(),
        "mail.owned.test".into(),
        "127.0.0.1".into(),
        "172.30.254.1".into(),
    ])
    .unwrap();
    leaf.distinguished_name
        .push(rcgen::DnType::CommonName, "Owned Mail fixture endpoint");
    leaf.extended_key_usages = vec![
        rcgen::ExtendedKeyUsagePurpose::ServerAuth,
        rcgen::ExtendedKeyUsagePurpose::ClientAuth,
    ];
    let leaf_key = rcgen::KeyPair::generate().unwrap();
    let leaf_cert = leaf.signed_by(&leaf_key, &issuer).unwrap();
    directory(root);
    let result = MailPki {
        root: root.into(),
        certificate: root.join("server.crt"),
        private_key: root.join("server.key"),
        authority: root.join("ca.crt"),
    };
    private_bytes(&result.certificate, leaf_cert.pem().as_bytes());
    private_bytes(&result.private_key, leaf_key.serialize_pem().as_bytes());
    private_bytes(&result.authority, ca_cert.pem().as_bytes());
    private_bytes(&root.join("issuer.key"), issuer_private_pem.as_bytes());
    result
}
/// Starts only inside the named own outer namespace, after the image UID/PGDATA/fs gates.
#[tokio::test]
async fn actual_mail_cold_gateway_tls_verifies_fixture_chain_with_python() {
    let temp = tempfile::tempdir().unwrap();
    let pki = make_pki(&temp.path().join("tls"));
    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_port = upstream.local_addr().unwrap().port();
    let upstream_task = tokio::spawn(async move {
        axum::serve(
            upstream,
            Router::new().fallback(|| async { axum::Json(json!({"ok":true})) }),
        )
        .await
        .unwrap()
    });
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let gateway_port = port.local_addr().unwrap().port();
    drop(port);
    let server = tokio::spawn(spitzli_agent::recovery::gateway::serve(
        spitzli_agent::recovery::gateway::Configuration {
            profile: spitzli_agent::recovery::gateway::Profile::Mail,
            certificate: pki.certificate.clone(),
            private_key: pki.private_key.clone(),
            port: gateway_port,
            upstream_port,
            deadline_seconds: 10,
        },
    ));
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            assert!(!server.is_finished());
            if tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, gateway_port))
                .await
                .is_ok()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let script = r#"import sys,json,ssl,socket,http.client
try:
 c=ssl.create_default_context(cafile=sys.argv[1]);s=c.wrap_socket(socket.create_connection(('127.0.0.1',int(sys.argv[2])),timeout=2),server_hostname='mail.owned.test')
 h=http.client.HTTPSConnection('mail.owned.test',int(sys.argv[2]),context=c,timeout=2);h.sock=s;h.request('GET','/jmap/session',headers={'Authorization':'Basic cHVibGljOmR1bW15'});r=h.getresponse();r.read(4096);h.close();print(json.dumps({'verified':True,'status':r.status}))
except Exception as e:print(json.dumps({'verified':False,'errorClass':type(e).__name__,'verifyCode':getattr(e,'verify_code',None)}))
"#;
    let result = tokio::process::Command::new("python3")
        .args(["-B", "-c", script])
        .arg(&pki.certificate)
        .arg(gateway_port.to_string())
        .output()
        .await
        .unwrap();
    server.abort();
    upstream_task.abort();
    let _ = server.await;
    let _ = upstream_task.await;
    assert!(result.status.success() && result.stdout.len() < 1024 && result.stderr.is_empty());
    let value: Value = serde_json::from_slice(&result.stdout).unwrap();
    eprintln!("mail_cold_python_tls {value}");
    assert_eq!(value, json!({"verified":true,"status":200}));
}

/// Core source data and WAL stay on the bounded Mail filesystem; no host publishing occurs.
async fn source_postgres(role: &str, database: &str, pki: &MailPki) -> (String, url::Url, PathBuf) {
    assert!(["core", "metadata"].contains(&role));
    assert!(
        database
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_')
    );
    let root = Path::new("/owned/mail-fixture");
    let data = source_filesystem(role, 512 * MIB, 70);
    // This is initialization of this fresh owned directory after UID70 was actually verified.
    std::os::unix::fs::chown(&data, Some(70), Some(70)).unwrap();
    let mount = Path::new("/owned/mail-data/pg-private").join(role);
    directory(&mount);
    for name in ["server.crt", "server.key"] {
        let path = mount.join(name);
        std::fs::copy(pki.root.join(name), &path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::chown(&path, Some(70), Some(70)).unwrap();
    }
    let password = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let password_file = root.join(format!("{role}-password"));
    private_bytes(&password_file, password.as_bytes());
    private_bytes(&mount.join("password"), password.as_bytes());
    std::os::unix::fs::chown(mount.join("password"), Some(70), Some(70)).unwrap();
    std::os::unix::fs::chown(&mount, Some(70), Some(70)).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let image = std::env::var("SPITZLI_RECOVERY_POSTGRES_IMAGE_ID").unwrap();
    let token = uuid::Uuid::new_v4().simple().to_string();
    let args = vec![
        "run".to_owned(),
        "--detach".into(),
        "--name".into(),
        format!("owned-mail-{role}-{}", &token[..12]),
        "--label".into(),
        format!("webdock.rust-operator={token}"),
        "--network".into(),
        "host".into(),
        "--user".into(),
        "70:70".into(),
        "--memory".into(),
        "512m".into(),
        "--memory-swap".into(),
        "512m".into(),
        "--cpus".into(),
        "0.5".into(),
        "--pids-limit".into(),
        "128".into(),
        "--mount".into(),
        format!(
            "type=bind,source={},target=/var/lib/postgresql",
            data.display()
        ),
        "--mount".into(),
        format!(
            "type=bind,source={},target=/run/owned,readonly",
            mount.display()
        ),
        "--env".into(),
        "POSTGRES_PASSWORD_FILE=/run/owned/password".into(),
        "--env".into(),
        format!("POSTGRES_DB={database}"),
        "--pull=never".into(),
        image,
        "postgres".into(),
        "-h".into(),
        "0.0.0.0".into(),
        "-p".into(),
        port.to_string(),
        "-c".into(),
        "ssl=on".into(),
        "-c".into(),
        "ssl_cert_file=/run/owned/server.crt".into(),
        "-c".into(),
        "ssl_key_file=/run/owned/server.key".into(),
        "-c".into(),
        "max_wal_size=64MB".into(),
        "-c".into(),
        "min_wal_size=32MB".into(),
    ];
    let mut command = Command::new("/usr/bin/timeout");
    command
        .args(["--signal=TERM", "--kill-after=1", "40", "/usr/bin/docker"])
        .args(args);
    let (status, stdout, _) = super::byte_component::collect_command(command, None);
    assert!(
        status.success(),
        "owned TLS PG startup; raw diagnostics withheld"
    );
    let id = String::from_utf8(stdout).unwrap().trim().to_owned();
    assert_eq!(id.len(), 64);
    let mut uri = url::Url::parse(&format!(
        "postgresql://postgres@127.0.0.1:{port}/{database}"
    ))
    .unwrap();
    uri.set_password(Some(&password)).unwrap();
    uri.query_pairs_mut()
        .append_pair("sslmode", "verify-full")
        .append_pair("sslrootcert", pki.authority.to_str().unwrap());
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if connect(uri.as_str()).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("actual TLS PG login");
    (id, uri, password_file)
}

fn mail_command(stage: &'static str, args: &[String]) -> Vec<u8> {
    let mut command = Command::new("/usr/bin/timeout");
    command
        .args(["--signal=TERM", "--kill-after=1", "60"])
        .args(args);
    let (status, out, _) = super::byte_component::collect_command(command, None);
    eprintln!(
        "recovery_component mail_command stage={stage} exitCode={}",
        status.code().unwrap_or(-1)
    );
    assert!(
        status.success(),
        "owned finite Mail infrastructure command failed; raw IO withheld"
    );
    out
}
/// Exact finite source infrastructure filesystem, not a fabricated recovery grant/RecordID.
fn write_source_filesystem_receipt(
    directory: &Path,
    role: &str,
    allocation: uuid::Uuid,
    phase: &str,
    record: &Value,
) -> io::Result<PathBuf> {
    if !matches!(phase, "initial" | "loop-attached" | "mounted-autoclear")
        || !(["core", "metadata"].contains(&role)
            || role
                .strip_prefix("ce-")
                .is_some_and(|c| spitzli_core::id::RecordId::parse(c).is_ok()))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid source receipt phase/role",
        ));
    }
    let path = directory.join(format!(
        "mail-source-pg-{role}-{}-{phase}.json",
        allocation.simple()
    ));
    super::write_diagnostic_json(&path, record)?;
    Ok(path)
}

#[test]
fn source_filesystem_receipts_keep_all_phases_immutable_for_both_pg_roles() {
    use std::os::unix::fs::MetadataExt;
    for role in ["core", "metadata"] {
        let directory = tempfile::tempdir().unwrap();
        let allocation = uuid::Uuid::new_v4();
        let mut paths = Vec::new();
        for phase in ["initial", "loop-attached", "mounted-autoclear"] {
            let path = write_source_filesystem_receipt(
                directory.path(),
                role,
                allocation,
                phase,
                &json!({"state":phase}),
            )
            .unwrap();
            let bytes = std::fs::read(&path).unwrap();
            let metadata = path.symlink_metadata().unwrap();
            assert!(metadata.is_file());
            assert_eq!(metadata.mode() & 0o777, 0o600);
            assert_eq!(metadata.nlink(), 1);
            assert_eq!(
                serde_json::from_slice::<Value>(&bytes).unwrap(),
                json!({"state":phase})
            );
            paths.push((path, bytes));
        }
        assert_eq!(
            paths
                .iter()
                .map(|v| &v.0)
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            3
        );
        for (path, original) in &paths {
            let phase = serde_json::from_slice::<Value>(original).unwrap()["state"]
                .as_str()
                .unwrap()
                .to_owned();
            assert_eq!(
                write_source_filesystem_receipt(
                    directory.path(),
                    role,
                    allocation,
                    &phase,
                    &json!({"state":"poison"})
                )
                .unwrap_err()
                .kind(),
                io::ErrorKind::AlreadyExists
            );
            assert_eq!(std::fs::read(path).unwrap(), *original);
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceLoopInventory {
    loopdevices: Vec<SourceLoopRow>,
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceLoopRow {
    name: String,
    #[serde(rename = "back-file")]
    _back_file: String,
    #[serde(rename = "back-ino")]
    backing_inode: u64,
    #[serde(rename = "back-maj:min")]
    backing_device: String,
    #[serde(rename = "maj:min")]
    device: String,
    autoclear: bool,
    offset: u64,
    sizelimit: u64,
    ro: bool,
}
struct ExpectedSourceLoop<'a> {
    name: &'a str,
    backing_device: u64,
    backing_inode: u64,
    device: u64,
    bytes: u64,
    autoclear: bool,
}
fn source_loop_matches(raw: &[u8], expected: &ExpectedSourceLoop<'_>, actual_bytes: u64) -> bool {
    if raw.len() > 16384 || actual_bytes != expected.bytes {
        return false;
    }
    let Ok(inventory) = serde_json::from_slice::<SourceLoopInventory>(raw) else {
        return false;
    };
    if inventory.loopdevices.len() > 64 {
        return false;
    }
    let mut names = std::collections::BTreeSet::new();
    if inventory.loopdevices.iter().any(|r| !names.insert(&r.name)) {
        return false;
    }
    let Some(row) = inventory
        .loopdevices
        .iter()
        .find(|r| r.name == expected.name)
    else {
        return false;
    };
    let dev = |n| format!("{}:{}", rustix::fs::major(n), rustix::fs::minor(n));
    row.backing_device == dev(expected.backing_device)
        && row.backing_inode == expected.backing_inode
        && row.device == dev(expected.device)
        && row.offset == 0
        && row.sizelimit == 0
        && row.autoclear == expected.autoclear
        && !row.ro
}
fn verify_source_loop(expected: &ExpectedSourceLoop<'_>) {
    let raw = mail_command(
        "source-loop-readonly-identity",
        &[
            "/usr/sbin/losetup".into(),
            "--list".into(),
            "--json".into(),
            "--output".into(),
            "NAME,BACK-FILE,BACK-INO,BACK-MAJ:MIN,AUTOCLEAR,MAJ:MIN,OFFSET,SIZELIMIT,RO".into(),
        ],
    );
    let sectors = std::fs::read_to_string(format!(
        "/sys/block/{}/size",
        expected.name.trim_start_matches("/dev/")
    ))
    .unwrap();
    let actual_bytes = sectors
        .trim()
        .parse::<u64>()
        .unwrap()
        .checked_mul(512)
        .unwrap();
    assert!(
        source_loop_matches(&raw, expected, actual_bytes),
        "owned source loop identity mismatch; no pathname fallback"
    );
}
#[test]
fn source_loop_projection_accepts_actual_relative_backing_only_with_exact_kernel_identity() {
    // Actual ff06 LOOP_GET_STATUS64 projection; this is a parser regression, not a new kernel run.
    let row = json!({"name":"/dev/loop2","back-file":"/source/core-668f7a3ca34f4dcda5a74ac8d1225593.ext4","back-ino":196610,"back-maj:min":"7:0","autoclear":false,"maj:min":"7:2","offset":0,"sizelimit":0,"ro":false});
    let expected = ExpectedSourceLoop {
        name: "/dev/loop2",
        backing_device: rustix::fs::makedev(7, 0),
        backing_inode: 196610,
        device: rustix::fs::makedev(7, 2),
        bytes: 512 * MIB,
        autoclear: false,
    };
    let check = |rows: Value, bytes| {
        source_loop_matches(
            &serde_json::to_vec(&json!({"loopdevices":rows})).unwrap(),
            &expected,
            bytes,
        )
    };
    assert!(check(json!([row.clone()]), 512 * MIB));
    for (field, value) in [
        ("back-ino", json!(196611)),
        ("back-maj:min", json!("7:1")),
        ("maj:min", json!("7:3")),
        ("autoclear", json!(true)),
        ("ro", json!(true)),
        ("offset", json!(1)),
        ("sizelimit", json!(1)),
        ("back-ino", Value::Null),
        ("back-maj:min", Value::Null),
        ("unknown", json!(0)),
    ] {
        let mut poison = row.clone();
        poison[field] = value;
        assert!(!check(json!([poison]), 512 * MIB));
    }
    assert!(!check(json!([row.clone(), row.clone()]), 512 * MIB));
    assert!(!check(json!([]), 512 * MIB));
    assert!(!check(json!([row.clone()]), 2 * GIB));
    let ce_expected = ExpectedSourceLoop {
        bytes: 2 * GIB,
        ..expected
    };
    assert!(source_loop_matches(
        &serde_json::to_vec(&json!({"loopdevices":[row.clone()]})).unwrap(),
        &ce_expected,
        2 * GIB
    ));
    let mut missing = row;
    missing.as_object_mut().unwrap().remove("autoclear");
    assert!(!check(json!([missing]), 512 * MIB));
}

/// Its loop is owned by this fresh file's dev/inode and marked autoclear while mounted.
fn source_filesystem(role: &str, bytes: u64, uid: u32) -> PathBuf {
    use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
    assert!(
        (["core", "metadata"].contains(&role) && bytes == 512 * MIB && uid == 70)
            || (role
                .strip_prefix("ce-")
                .is_some_and(|c| spitzli_core::id::RecordId::parse(c).is_ok())
                && bytes == 2 * GIB
                && uid == 65532)
    );
    let source = Path::new("/owned/mail-data/source");
    assert!(source.is_dir() && source.canonicalize().unwrap() == source);
    let allocation = uuid::Uuid::new_v4();
    let backing = source.join(format!("{role}-{}.ext4", allocation.simple()));
    let mount = source.join(role);
    directory(&mount);
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&backing)
        .unwrap();
    let identity = file.metadata().unwrap();
    assert_eq!(identity.nlink(), 1);
    let mut record = json!({"role":role,"scope":"owned source infrastructure, no claimed Core grant","backingFile":backing,"fileDev":identity.dev(),"fileInode":identity.ino(),"bytes":bytes,"mount":mount,"state":"pending-before-allocation"});
    write_source_filesystem_receipt(
        Path::new("/diagnostics"),
        role,
        allocation,
        "initial",
        &record,
    )
    .unwrap();
    mail_command(
        "source-pg-preallocate",
        &[
            "/usr/bin/fallocate".into(),
            "--length".into(),
            bytes.to_string(),
            backing.to_str().unwrap().into(),
        ],
    );
    file.sync_all().unwrap();
    mail_command(
        "source-pg-format",
        &[
            "/usr/sbin/mkfs.ext4".into(),
            "-q".into(),
            "-F".into(),
            "-m".into(),
            "0".into(),
            "-i".into(),
            "65536".into(),
            backing.to_str().unwrap().into(),
        ],
    );
    let device = String::from_utf8(mail_command(
        "source-pg-loop",
        &[
            "/usr/sbin/losetup".into(),
            "--find".into(),
            "--show".into(),
            "--nooverlap".into(),
            backing.to_str().unwrap().into(),
        ],
    ))
    .unwrap();
    let device = device.trim();
    assert!(
        device
            .strip_prefix("/dev/loop")
            .is_some_and(|v| !v.is_empty() && v.bytes().all(|c| c.is_ascii_digit()))
    );
    // Pin the exact block device before autoclear: diagnostics/mount failure closes the last
    // pre-mount descriptor; after mount, the owned namespace retains the autoclear reference.
    let loop_pin = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(device)
        .unwrap();
    let device_identity = loop_pin.metadata().unwrap();
    assert!(device_identity.file_type().is_block_device());
    assert_eq!(rustix::fs::major(device_identity.rdev()), 7);
    assert_eq!(
        device,
        format!("/dev/loop{}", rustix::fs::minor(device_identity.rdev()))
    );
    let mut expected = ExpectedSourceLoop {
        name: device,
        backing_device: identity.dev(),
        backing_inode: identity.ino(),
        device: device_identity.rdev(),
        bytes,
        autoclear: false,
    };
    assert_eq!(file.metadata().unwrap().len(), bytes);
    verify_source_loop(&expected);
    mail_command(
        "source-pg-autoclear",
        &["/usr/sbin/losetup".into(), "--detach".into(), device.into()],
    );
    expected.autoclear = true;
    verify_source_loop(&expected);
    record["autoclear"] = json!(true);
    record["loop"] = json!(device);
    record["state"] = json!("loop-attached");
    write_source_filesystem_receipt(
        Path::new("/diagnostics"),
        role,
        allocation,
        "loop-attached",
        &record,
    )
    .unwrap();
    mail_command(
        "source-pg-mount",
        &[
            "/usr/bin/mount".into(),
            "-o".into(),
            "rw,noexec,nodev,nosuid".into(),
            device.into(),
            mount.to_str().unwrap().into(),
        ],
    );
    verify_source_loop(&expected);
    let fresh = file.metadata().unwrap();
    assert_eq!(
        (fresh.dev(), fresh.ino(), fresh.len()),
        (identity.dev(), identity.ino(), bytes)
    );
    let actual_uuid = String::from_utf8(mail_command(
        "source-pg-uuid",
        &[
            "/usr/sbin/blkid".into(),
            "-s".into(),
            "UUID".into(),
            "-o".into(),
            "value".into(),
            backing.to_str().unwrap().into(),
        ],
    ))
    .unwrap();
    record["actualUUID"] = json!(actual_uuid.trim());
    record["state"] = json!("mounted-autoclear");
    write_source_filesystem_receipt(
        Path::new("/diagnostics"),
        role,
        allocation,
        "mounted-autoclear",
        &record,
    )
    .unwrap();
    drop(loop_pin);
    mount
}

// Runs ONLY after entering the exact verified Mail runtime Pod's held network namespace.
// First DATA receives a genuine250; second DATA is received and its connection closes before ACK.
const SMTP_SINK: &str = r#"
import asyncio,hashlib,json,os,signal,sys,time
async def main():
    request=json.load(sys.stdin)
    if set(request)!={'report','port'} or request['port']!=1025 or not request['report'].startswith('/owned/mail-fixture/'):raise ValueError('private sink input')
    counts={'connections':0,'received':0,'confirmed':0,'closedBeforeAck':0,'bytes':0,'dataSHA256':[],'failure':None}
    deadline=time.monotonic()+300;finished=asyncio.Event();slots=asyncio.Semaphore(2)
    def save():
        path=request['report'];temporary=path+'.new'
        with open(temporary,'w') as file:os.chmod(temporary,0o600);json.dump(counts,file);file.flush();os.fsync(file.fileno())
        os.replace(temporary,path)
    async def handle(reader,writer):
        async with slots:
            counts['connections']+=1
            try:
                if counts['connections']>16:raise ValueError('connection-bound')
                writer.write(b'220 owned.local ESMTP\r\n');await writer.drain()
                for _ in range(64):
                    line=await asyncio.wait_for(reader.readline(),5)
                    if not line:break
                    if len(line)>2048:raise ValueError('line-bound')
                    verb=line.split(b' ',1)[0].strip().upper()
                    if verb in (b'EHLO',b'HELO',b'MAIL',b'RCPT',b'NOOP',b'RSET'):writer.write(b'250 owned.local\r\n')
                    elif verb==b'QUIT':writer.write(b'221 bye\r\n');await writer.drain();break
                    elif verb==b'DATA':
                        writer.write(b'354 continue\r\n');await writer.drain();digest=hashlib.sha256();size=0
                        for _ in range(4096):
                            part=await asyncio.wait_for(reader.readline(),5)
                            if part==b'.\r\n':break
                            if not part or len(part)>16384:raise ValueError('data-framing')
                            size+=len(part)
                            if size>262144:raise ValueError('data-bound')
                            digest.update(part)
                        else:raise ValueError('data-lines')
                        counts['received']+=1;counts['bytes']+=size;counts['dataSHA256'].append(digest.hexdigest())
                        if counts['received']==1:
                            counts['confirmed']+=1;save();writer.write(b'250 accepted\r\n')
                        elif counts['received']==2:
                            counts['closedBeforeAck']+=1;save();break
                        else:raise ValueError('unexpected-replay')
                    else:writer.write(b'500 unsupported\r\n')
                    await writer.drain()
            except Exception:counts['failure']='bounded-smtp-failure';save()
            finally:writer.close();await writer.wait_closed()
    for sig in (signal.SIGTERM,signal.SIGINT):asyncio.get_running_loop().add_signal_handler(sig,finished.set)
    server=await asyncio.start_server(handle,'127.0.0.1',1025,limit=16385)
    save();print('{"stage":"smtp-ready"}',flush=True)
    try:await asyncio.wait_for(finished.wait(),max(1,deadline-time.monotonic()))
    except asyncio.TimeoutError:counts['failure']='deadline';save()
    finally:server.close();await server.wait_closed();save()
    return 0 if counts['failure'] is None else 1
if __name__=='__main__':sys.exit(asyncio.run(main()))
"#;

pub(super) async fn prepare_source_mailbox(
    app: &Router,
    console: &str,
    project: &str,
    run: &str,
) -> Value {
    let domain = invoke_mail(
        app,
        console,
        project,
        MailInvokeRole::IncomingDomain,
        json!({"domain":format!("incoming-{run}.example.test")}),
        "matched-incoming-domain",
    )
    .await;
    let domain_id = domain["resource"]["id"]
        .as_str()
        .expect("real private sending domain");
    let _ = activate_incoming(app, console, project, domain_id).await;
    let (mailbox,view)=tokio::time::timeout(Duration::from_secs(120),async {
    loop {
        let reply=request(app,CONSOLE,"POST","/api/console/plugins/invoke",console,json!({"pluginID":"mail","projectID":project,"action":"domains.view","input":{"resource_id":domain_id},"idempotencyKey":uuid::Uuid::new_v4().to_string()})).await;
        assert_eq!(reply.status,StatusCode::OK,"actual incoming readiness observation");
        let incoming=&reply.body["incoming"];
        assert_eq!(incoming["desiredEnabled"],true);
        if incoming["state"]=="ready" {break;}
        assert_eq!(incoming["state"],"provisioning","actual incoming initialization failed");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let created = invoke_mail(
        app,
        console,
        project,
        MailInvokeRole::MailboxCreate,
        json!({"domain_id":domain_id,"local_part":"recovery","name":"Owned matched mailbox"}),
        "matched-mailbox-create",
    )
    .await;
    let mailbox = created["resource"]["id"]
        .as_str()
        .expect("real private mailbox ID");
    let view = loop {
            let value = invoke_mail(
                app,
                console,
                project,
                MailInvokeRole::MailboxView,
                json!({"resource_id":mailbox}),
                &uuid::Uuid::new_v4().to_string(),
            )
            .await;
            if value["state"] == "ready" {
                break value;
            }
            assert!(
                value["state"] != "failed" && value["state"] != "migration-required",
                "actual customer Mailbox failed; private cause withheld"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
    };
    (mailbox.to_owned(),view)
    }).await.expect("real CE engine and mailbox convergence");
    let credential = invoke_mail(
        app,
        console,
        project,
        MailInvokeRole::MailboxCredential,
        json!({"resource_id":mailbox}),
        "matched-mailbox-credential",
    )
    .await;
    json!({"mailbox":mailbox,"domain":domain_id,"username":view["username"],"password":credential["value"]})
}
pub(super) async fn prepare_outgoing_project(app: &Router, console: &str) -> (String, String) {
    let customer = okay(
        request(
            app,
            CONSOLE,
            "POST",
            "/api/console/registry/customers",
            console,
            json!({"name":"Owned outgoing-only matched Mail","country":"de"}),
        )
        .await,
        "real outgoing-only customer",
    );
    let id = customer["id"].as_str().unwrap().to_owned();
    let project = okay(
        request(
            app,
            CONSOLE,
            "POST",
            "/api/console/registry/projects",
            console,
            json!({"name":"Owned outgoing-only Mail project","customer":id}),
        )
        .await,
        "real outgoing-only project",
    );
    (id, project["id"].as_str().unwrap().to_owned())
}
fn local_smtp_account(customer: &str) -> Value {
    spitzli_core::id::RecordId::parse(customer).expect("real customer ID");
    json!({"customerID":customer,"accountID":null,"mode":"managed","transportRevision":1,"bindingRevision":1,"byokRevision":null,"transport":{"kind":"smtp","host":"127.0.0.1","port":1025}})
}
pub(super) async fn confirmed_and_uncertain_send(
    app: &Router,
    console: &str,
    owner: &DatabaseConnection,
    project: &str,
    run: &str,
) -> Value {
    let domain = invoke_mail(
        app,
        console,
        project,
        MailInvokeRole::OutgoingDomain,
        json!({"domain":format!("outgoing-{run}.example.test")}),
        "matched-outgoing-domain",
    )
    .await;
    let mut receipts = Vec::new();
    for (key, expected, role) in [
        (
            "matched-known-confirmation",
            "accepted",
            MailInvokeRole::SendKnown,
        ),
        (
            "matched-uncertain-confirmation",
            "unknown",
            MailInvokeRole::SendUncertain,
        ),
    ] {
        let sent=invoke_mail(app,console,project,role,json!({"resource_id":domain["resource"]["id"],"from":"recovery","to":"fixture@recipient.example.test","subject":key,"text":format!("Owned matched recovery {key}")}),key).await;
        let id = sent["id"].as_str().expect("actual Core canonical send ID");
        let state=tokio::time::timeout(Duration::from_secs(40),async {loop {
            let current=row(owner,"SELECT jsonb_build_object('state',state) AS value FROM webdock_mail.send_job WHERE id=$1",vec![id.into()]).await;
            if current["state"]==expected {break current;}
            assert!(!["cancelled","failed","rejected"].contains(&current["state"].as_str().unwrap_or_default()),"actual send failed outside expected local SMTP outcome");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }}).await.expect("actual canonical provider outcome");
        receipts.push(json!({"id":id,"state":state["state"]}));
    }
    json!({"known":receipts[0],"uncertain":receipts[1]})
}

#[derive(Clone)]
pub(crate) struct FoundationDependencies {
    pub mail: spitzli_mail::MailDependencies,
    pub oauth: Arc<spitzli_oauth::OAuthService>,
    pub registry: Arc<spitzli_registry::RegistryService>,
    pub access: Arc<spitzli_access::AccessService>,
}
pub(super) struct Foundation {
    pub app: Router,
    pub core: MailCore,
    pub worker: spitzli_control_plane::plugins::installation::worker::InstallationWorker,
    pub metadata_container: String,
    pub metadata_password: PathBuf,
    pub authority: PathBuf,
    gateway_client: GatewayClientIdentity,
    outgoing: (String, String),
    pub callback_stop: Option<tokio::sync::oneshot::Sender<()>>,
    pub callback: tokio::task::JoinHandle<()>,
    pub root: PathBuf,
    pub descriptor: spitzli_plugin_host::CatalogDescriptor,
}
fn installer_admin_uri(metadata_admin: &url::Url) -> url::Url {
    let mut administrator = metadata_admin.clone();
    administrator.set_query(Some("sslmode=require"));
    administrator
}
#[test]
fn actual_installer_admin_driver_accepts_its_fixture_url_without_changing_sqlx_tls_mode() {
    let source=url::Url::parse("postgresql://postgres:public-test@127.0.0.1:55432/postgres?sslmode=verify-full&sslrootcert=%2Fpublic-unit%2Fca.crt").unwrap();
    let administrator = installer_admin_uri(&source);
    let parsed: tokio_postgres::Config = administrator
        .as_str()
        .parse()
        .expect("actual installed admin driver must accept the actual fixture producer");
    assert_eq!(
        parsed.get_ssl_mode(),
        tokio_postgres::config::SslMode::Require
    );
    let source_options: sea_orm::sqlx::postgres::PgConnectOptions =
        source.as_str().parse().unwrap();
    assert!(matches!(
        source_options.get_ssl_mode(),
        sea_orm::sqlx::postgres::PgSslMode::VerifyFull
    ));
    assert_eq!(
        source
            .query_pairs()
            .find(|(k, _)| k == "sslmode")
            .unwrap()
            .1,
        "verify-full"
    );
    let runtime: tokio_postgres::Config =
        "postgresql://wd_u_public:public-test@172.30.254.1:55433/wd_meta_public"
            .parse()
            .unwrap();
    assert_eq!(runtime.get_hosts().len(), 1);
}
#[test]
fn fixture_runtime_issuer_material_passes_actual_production_trust_validation() {
    let temporary = tempfile::tempdir().unwrap();
    let pki = make_pki(&temporary.path().join("pki"));
    let result = spitzli_plugin_host::provisioning::tls::issue_runtime_identity(
        &std::fs::read_to_string(pki.authority).unwrap(),
        &std::fs::read_to_string(pki.root.join("issuer.key")).unwrap(),
        "wd-owned-recovery-mail.mail-runtime.owned.test",
    );
    assert!(
        result.is_ok(),
        "fixture PKI rejected by existing native runtime issuer"
    );
}

struct TransportContractDeny;
#[tonic::async_trait]
impl spitzli_plugin_runtime::ActionHandler for TransportContractDeny {
    async fn invoke(
        &self,
        _: &spitzli_plugin_protocol::InvocationClaims,
        _: &str,
        _: Value,
        _: Value,
    ) -> Result<Value, spitzli_plugin_protocol::PluginError> {
        Err(spitzli_plugin_protocol::PluginError::denied())
    }
}

#[tokio::test]
async fn actual_fixture_runtime_connection_pki_describe_health_and_peer_negatives() {
    use spitzli_control_plane::plugin_config::{RuntimeConnection, RuntimeTransport};
    use spitzli_plugin_runtime::{CorePeer, MutualTls, RemoteAuthority, Runtime, wire};
    let temporary = tempfile::tempdir().unwrap();
    let pki = make_pki(&temporary.path().join("core"));
    let wrong_ca = make_pki(&temporary.path().join("wrong"));
    let gateway = gateway_client_identity(&pki);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let core_pem = std::fs::read(&pki.certificate).unwrap();
    let core_leaf = x509_parser::pem::parse_x509_pem(&core_pem).unwrap().1;
    let core_pin = hex::encode(Sha256::digest(&core_leaf.contents));
    let native = ed25519_dalek::SigningKey::from_bytes(&[23; 32]);
    let (policy, _) = foundation_runtime_profiles(
        &pki,
        address,
        55433,
        &core_pin,
        &gateway.certificate_sha256,
        &native,
    );
    let host = format!(
        "{}.{}",
        spitzli_agent_protocol::plugin::PluginRuntimeSpec::namespace_name("owned-recovery", "mail"),
        policy["runtimeDomain"].as_str().unwrap()
    );
    let issued = spitzli_plugin_host::provisioning::tls::issue_runtime_identity(
        &std::fs::read_to_string(&pki.authority).unwrap(),
        &std::fs::read_to_string(pki.root.join("issuer.key")).unwrap(),
        &host,
    )
    .unwrap();
    let runtime_cert = pki.root.join("transport-runtime.crt");
    let runtime_key = pki.root.join("transport-runtime.key");
    private_bytes(&runtime_cert, issued.certificate_pem.as_bytes());
    private_bytes(&runtime_key, issued.private_key_pem.as_bytes());
    let callbacks = spitzli_plugin_kit::transport::RuntimeTransport {
        listen: Some(pki.root.join("unused.sock")),
        core_tls: Some(serde_json::from_value(policy["core"].clone()).unwrap()),
        ..Default::default()
    };
    let lazy = callbacks.core_client_lazy().await;
    assert!(
        lazy.is_ok(),
        "actual fixture callback trust constructor denied"
    );
    let authority = RemoteAuthority::new(
        lazy.ok().unwrap(),
        "mail".into(),
        1,
        vec![CorePeer::MutualTls {
            certificate_sha256: core_pin,
        }],
    )
    .unwrap();
    let manifest = spitzli_plugin_protocol::Manifest::parse(include_bytes!(
        "../../../../plugins/mail/manifest.json"
    ))
    .unwrap();
    let expected_manifest = manifest.as_bytes().to_vec();
    let runtime = Runtime::new(
        manifest,
        1,
        native.verifying_key(),
        Arc::new(authority),
        Arc::new(TransportContractDeny),
    )
    .unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(runtime.serve_mtls(
        listener,
        MutualTls {
            identity: tonic::transport::Identity::from_pem(
                issued.certificate_pem,
                issued.private_key_pem,
            ),
            peer_ca: tonic::transport::Certificate::from_pem(
                std::fs::read(&pki.authority).unwrap(),
            ),
        },
        async {
            let _ = stopped.await;
        },
    ));
    let original = RuntimeConnection {
        generation: 1,
        transport: RuntimeTransport::MutualTls {
            origin: format!("https://{address}"),
            domain: host,
            certificate: serde_json::from_value(policy["core"]["certificate"].clone()).unwrap(),
            private_key: serde_json::from_value(policy["core"]["privateKey"].clone()).unwrap(),
            authority: serde_json::from_value(policy["runtimeIdentity"]["authority"].clone())
                .unwrap(),
            server_certificate_sha256: issued.certificate_sha256,
        },
        ui_origin: None,
    };
    // Actual public consumer serde and private file loader; no handmade TLS client or route row.
    let connection: RuntimeConnection =
        serde_json::from_slice(&serde_json::to_vec(&original).unwrap()).unwrap();
    assert!(connection == original);
    assert_eq!(
        forward_probe(&connection, "direct").await["stage"],
        "complete"
    );
    let positive =
        tokio::time::timeout(Duration::from_secs(5), connection.connect_authenticated()).await;
    assert!(positive.is_ok(), "transport contract connect deadline");
    let positive = positive.unwrap();
    assert!(
        positive.is_ok(),
        "transport contract positive pinned connection denied"
    );
    let (mut client, peer) = positive.ok().unwrap();
    assert!(matches!(
        peer,
        spitzli_plugin_runtime::PeerIdentity::MutualTls { .. }
    ));
    let described = client.describe(wire::DescribeRequest {}).await;
    assert!(described.is_ok(), "transport contract Describe denied");
    assert!(described.ok().unwrap().into_inner().manifest_json == expected_manifest);
    let health = client.health(wire::HealthRequest {}).await;
    assert!(health.is_ok(), "transport contract Health denied");
    let health = health.ok().unwrap().into_inner();
    assert!(
        health.plugin_id == "mail"
            && health.version == "0.3.0"
            && health.generation == 1
            && health.protocol_version == 1
    );
    for negative in ["wrong_ca", "wrong_hostname", "wrong_pin"] {
        let mut bad = connection.clone();
        let RuntimeTransport::MutualTls {
            authority,
            domain,
            server_certificate_sha256,
            ..
        } = &mut bad.transport
        else {
            unreachable!()
        };
        match negative {
            "wrong_ca" => *authority = wrong_ca.authority.clone(),
            "wrong_hostname" => *domain = "wrong.owned.test".into(),
            "wrong_pin" => *server_certificate_sha256 = "0".repeat(64),
            _ => unreachable!(),
        }
        let result =
            tokio::time::timeout(Duration::from_secs(5), bad.connect_authenticated()).await;
        assert!(result.is_ok(), "transport contract negative deadline");
        let denied = result.unwrap();
        assert!(
            denied.is_err(),
            "transport trust negative unexpectedly accepted"
        );
        let projected = forward_error_chain(denied.err().unwrap().as_ref());
        assert!(
            projected["segments"]
                .as_array()
                .unwrap()
                .iter()
                .any(|segment| segment["class"] == "rustls"),
            "actual TLS failure must retain its typed source"
        );
        if negative != "wrong_pin" {
            assert!(
                projected["segments"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|segment| segment["tlsCategory"] == "certificate")
            );
        }
    }
    let mut wrong_peer = connection.clone();
    let RuntimeTransport::MutualTls {
        certificate,
        private_key,
        ..
    } = &mut wrong_peer.transport
    else {
        unreachable!()
    };
    *certificate = gateway.certificate;
    *private_key = gateway.private_key;
    let gateway = wrong_peer.connect_authenticated().await;
    assert!(
        gateway.is_ok(),
        "same-CA gateway TLS must reach actual local CorePeer fence"
    );
    let (mut gateway, _) = gateway.ok().unwrap();
    let denied = gateway.describe(wire::DescribeRequest {}).await;
    assert!(denied.is_err());
    assert!(denied.err().unwrap().code() == tonic::Code::PermissionDenied);
    let denied = gateway.health(wire::HealthRequest {}).await;
    assert!(denied.is_err());
    assert!(denied.err().unwrap().code() == tonic::Code::PermissionDenied);
    drop(client);
    drop(gateway);
    let _ = stop.send(());
    let finished = tokio::time::timeout(Duration::from_secs(3), server).await;
    assert!(finished.is_ok(), "owned transport server shutdown deadline");
    assert!(finished.unwrap().is_ok());
    eprintln!(
        "recovery_transport_contract core_peer=pass pin=pass describe=pass health=pass wrong_ca=deny wrong_hostname=deny wrong_pin=deny gateway_peer=deny managed_route_lookup=unqualified"
    );
}

struct GatewayClientIdentity {
    certificate: PathBuf,
    private_key: PathBuf,
    certificate_sha256: String,
}
fn gateway_client_identity(pki: &MailPki) -> GatewayClientIdentity {
    let issued = spitzli_plugin_host::provisioning::tls::issue_runtime_identity(
        &std::fs::read_to_string(&pki.authority).unwrap(),
        &std::fs::read_to_string(pki.root.join("issuer.key")).unwrap(),
        "mail-gateway.owned.test",
    )
    .expect("actual operator gateway identity issuer");
    let identity = GatewayClientIdentity {
        certificate: pki.root.join("gateway-client.crt"),
        private_key: pki.root.join("gateway-client.key"),
        certificate_sha256: issued.certificate_sha256,
    };
    private_bytes(&identity.certificate, issued.certificate_pem.as_bytes());
    private_bytes(&identity.private_key, issued.private_key_pem.as_bytes());
    identity
}
fn foundation_runtime_profiles(
    pki: &MailPki,
    address: std::net::SocketAddr,
    metadata_port: u16,
    certificate_sha256: &str,
    gateway_sha256: &str,
    native_key: &ed25519_dalek::SigningKey,
) -> (Value, Value) {
    let policy = json!({"runtimeDomain":"mail-runtime.owned.test","runtimePort":8443,"runtimeOriginPort":19443,"runtimeIngressHost":"172.30.254.1","entryPoint":"websecure","storageID":"matched-private",
        "core":{"origin":format!("https://{address}"),"domain":address.ip().to_string(),"certificate":pki.certificate,"privateKey":pki.private_key,"authority":pki.authority,"serverCertificateSha256":certificate_sha256},
        "runtimeIdentity":{"authority":pki.authority,"privateKey":pki.root.join("issuer.key")},
        "resources":{"cpuMillicores":100,"memoryBytes":268435456},
        "egress":[{"cidr":"172.30.254.1/32","port":address.port()},{"cidr":"172.30.254.1/32","port":metadata_port},{"cidr":"172.30.254.1/32","port":19480}]});
    let recipe = json!({"storage":true,"sealingKey":true,"routeDomains":["mail.owned.test","mail-admin.owned.test"],"privateConfiguration":{
        "platformTransport":{"kind":"smtp","host":"127.0.0.1","port":1025},"trustUnverifiedDomains":true,
        "mailboxes":{"image":CE_IMAGE,"domain":"mail.owned.test","imapPort":1993,"submissionPort":2465,"admin":{"origin":"http://172.30.254.1:19480","domain":"mail-admin.owned.test","entryPoints":["web"]},"issuer":null},
        "delivery":{"nativeLeaseIssuerPublicKeyHex":hex::encode(native_key.verifying_key().as_bytes()),"gatewayPeers":[{"kind":"mutualTls","certificateSha256":gateway_sha256}],"nativeAccounts":[]}
    }});
    (policy, recipe)
}
#[test]
fn actual_mail_profile_separates_real_gateway_leaf_and_matches_core_tls_origin() {
    let temporary = tempfile::tempdir().unwrap();
    let pki = make_pki(&temporary.path().join("pki"));
    let gateway = gateway_client_identity(&pki);
    let core = x509_parser::pem::parse_x509_pem(&std::fs::read(&pki.certificate).unwrap())
        .unwrap()
        .1;
    let core_sha = format!("{:x}", Sha256::digest(&core.contents));
    let (policy, recipe) = foundation_runtime_profiles(
        &pki,
        "172.30.254.1:55443".parse().unwrap(),
        55444,
        &core_sha,
        &gateway.certificate_sha256,
        &ed25519_dalek::SigningKey::from_bytes(&[7; 32]),
    );
    let core_peer: spitzli_plugin_runtime::CorePeer =
        serde_json::from_value(json!({"kind":"mutualTls","certificateSha256":core_sha})).unwrap();
    let gateway_peer: spitzli_plugin_runtime::CorePeer = serde_json::from_value(
        recipe["privateConfiguration"]["delivery"]["gatewayPeers"][0].clone(),
    )
    .unwrap();
    assert!(
        serde_json::to_value(&core_peer).unwrap() != serde_json::to_value(&gateway_peer).unwrap(),
        "actual fixture serialized gateway and Core peers collide in the retained worker guard"
    );
    assert_eq!(
        url::Url::parse(policy["core"]["origin"].as_str().unwrap())
            .unwrap()
            .host_str(),
        policy["core"]["domain"].as_str()
    );
    // The retained production main compares these actual serialized CorePeer values.
    let old_gateway_peer = core_peer.clone();
    let core_identity = spitzli_plugin_runtime::PeerIdentity::MutualTls {
        certificates: vec![core.contents.clone()],
    };
    assert!(
        old_gateway_peer.permits(&core_identity),
        "old fixture erroneously accepted the Core identity as gateway"
    );
    assert!(core_peer.permits(&core_identity));
    assert!(!gateway_peer.permits(&core_identity));
    let cert = spitzli_plugin_kit::read_file(&gateway.certificate, false).unwrap();
    let leaf = x509_parser::pem::parse_x509_pem(&cert).unwrap().1;
    assert_eq!(
        format!("{:x}", Sha256::digest(&leaf.contents)),
        gateway.certificate_sha256
    );
    let authenticated = spitzli_plugin_runtime::PeerIdentity::MutualTls {
        certificates: vec![leaf.contents],
    };
    assert!(gateway_peer.permits(&authenticated));
    assert!(!core_peer.permits(&authenticated));
    assert!(
        std::fs::read(&gateway.private_key).unwrap() != std::fs::read(&pki.private_key).unwrap(),
        "gateway key must be a distinct operator role"
    );
    for path in [&gateway.certificate, &gateway.private_key] {
        use std::os::unix::fs::MetadataExt;
        let meta = path.symlink_metadata().unwrap();
        assert!(meta.is_file());
        assert_eq!(meta.mode() & 0o777, 0o600);
        assert_eq!(meta.nlink(), 1);
        assert_eq!(meta.uid(), rustix::process::geteuid().as_raw());
    }
    let verified = Command::new("/usr/bin/timeout")
        .args([
            "5",
            "/usr/bin/openssl",
            "verify",
            "-purpose",
            "sslclient",
            "-CAfile",
        ])
        .arg(&pki.authority)
        .arg(&gateway.certificate)
        .output()
        .unwrap();
    let ca_bytes = std::fs::read(&pki.authority).unwrap();
    let ca_pem = x509_parser::pem::parse_x509_pem(&ca_bytes).unwrap().1;
    let ca = x509_parser::parse_x509_certificate(&ca_pem.contents)
        .unwrap()
        .1;
    let gateway_pem = x509_parser::pem::parse_x509_pem(&cert).unwrap().1;
    let gateway_x509 = x509_parser::parse_x509_certificate(&gateway_pem.contents)
        .unwrap()
        .1;
    assert!(
        ca.subject() != gateway_x509.subject(),
        "fixture CA subject must differ from issued leaf"
    );
    assert_eq!(gateway_x509.issuer(), ca.subject());
    let error = String::from_utf8_lossy(&verified.stderr);
    let category = if error.contains("self-signed certificate") {
        "self_signed"
    } else if error.contains("unable to get local issuer") {
        "issuer_unavailable"
    } else if error.contains("not yet valid") {
        "not_yet_valid"
    } else if error.contains("certificate has expired") {
        "expired"
    } else if error.contains("unsupported certificate purpose") {
        "purpose"
    } else {
        "other"
    };
    assert!(
        verified.status.success(),
        "actual gateway leaf chain verification failed category={category}"
    );
    let transport = |core: Value| spitzli_plugin_kit::transport::RuntimeTransport {
        listen: Some(temporary.path().join("unused.sock")),
        core_tls: Some(serde_json::from_value(core).unwrap()),
        ..Default::default()
    };
    assert!(transport(policy["core"].clone()).validate().is_ok());
    let mut old_core = policy["core"].clone();
    old_core["domain"] = json!("localhost");
    assert!(transport(old_core).validate().is_err());
}

#[test]
fn actual_mail_fixture_pki_role_chains_and_hostname_negatives_are_verified() {
    let temporary = tempfile::tempdir().unwrap();
    let core = make_pki(&temporary.path().join("core"));
    let private_pg = make_pki(&temporary.path().join("private-pg"));
    let wrong = make_pki(&temporary.path().join("wrong"));
    let ca = std::fs::read_to_string(&core.authority).unwrap();
    let key = std::fs::read_to_string(core.root.join("issuer.key")).unwrap();
    let runtime_name = format!(
        "{}.mail-runtime.owned.test",
        spitzli_agent_protocol::plugin::PluginRuntimeSpec::namespace_name("owned-recovery", "mail")
    );
    let runtime =
        spitzli_plugin_host::provisioning::tls::issue_runtime_identity(&ca, &key, &runtime_name)
            .unwrap();
    let runtime_path = core.root.join("unit-runtime.crt");
    private_bytes(&runtime_path, runtime.certificate_pem.as_bytes());
    let gateway = gateway_client_identity(&core);
    let verify =
        |authority: &Path, certificate: &Path, purpose: &str, name: Option<(&str, &str)>| {
            let mut command = Command::new("/usr/bin/openssl");
            command
                .args(["verify", "-purpose", purpose, "-CAfile"])
                .arg(authority);
            if let Some((flag, value)) = name {
                command.args([flag, value]);
            }
            command.arg(certificate).output().unwrap().status.success()
        };
    assert!(verify(
        &core.authority,
        &core.certificate,
        "sslserver",
        Some(("-verify_ip", "172.30.254.1"))
    ));
    assert!(verify(
        &core.authority,
        &core.certificate,
        "sslclient",
        None
    ));
    assert!(verify(
        &core.authority,
        &runtime_path,
        "sslserver",
        Some(("-verify_hostname", &runtime_name))
    ));
    assert!(verify(&core.authority, &runtime_path, "sslclient", None));
    assert!(verify(
        &core.authority,
        &gateway.certificate,
        "sslclient",
        None
    ));
    assert!(verify(
        &core.authority,
        &gateway.certificate,
        "sslserver",
        Some(("-verify_hostname", "mail-gateway.owned.test"))
    ));
    assert!(verify(
        &private_pg.authority,
        &private_pg.certificate,
        "sslserver",
        Some(("-verify_ip", "127.0.0.1"))
    ));
    assert!(verify(
        &private_pg.authority,
        &private_pg.certificate,
        "sslserver",
        Some(("-verify_ip", "172.30.254.1"))
    ));
    assert!(!verify(
        &wrong.authority,
        &gateway.certificate,
        "sslclient",
        None
    ));
    assert!(!verify(
        &core.authority,
        &runtime_path,
        "sslserver",
        Some(("-verify_hostname", "wrong.owned.test"))
    ));
    assert!(!verify(
        &core.authority,
        &core.certificate,
        "sslserver",
        Some(("-verify_ip", "192.0.2.1"))
    ));
}

pub(super) async fn foundation(
    dependencies: FoundationDependencies,
    hosting: Arc<spitzli_hosting::HostingService>,
    cluster: &str,
    incoming_customer: &str,
    outgoing: (String, String),
) -> Foundation {
    use base64::Engine;
    use spitzli_control_plane::{
        http::{Services, build_app_with_domains},
        plugin_config::PluginConfig,
    };
    let root = PathBuf::from("/owned/mail-fixture");
    let pki = make_pki(&root.join("callback-tls"));
    let (metadata_container, metadata_admin, metadata_password) =
        source_postgres("metadata", "postgres", &pki).await;
    let listener = tokio::net::TcpListener::bind("172.30.254.1:0")
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let certificate = std::fs::read(&pki.certificate).unwrap();
    let pem = x509_parser::pem::parse_x509_pem(&certificate).unwrap().1;
    let certificate_sha256 = format!("{:x}", Sha256::digest(&pem.contents));
    let mut seed = [0u8; 32];
    getrandom::getrandom(&mut seed).unwrap();
    let catalog_key = ed25519_dalek::SigningKey::from_bytes(&seed);
    getrandom::getrandom(&mut seed).unwrap();
    let native_key = ed25519_dalek::SigningKey::from_bytes(&seed);
    getrandom::getrandom(&mut seed).unwrap();
    let invocation_key = ed25519_dalek::SigningKey::from_bytes(&seed);
    private_bytes(
        &root.join("native-signing.hex"),
        hex::encode(native_key.to_bytes()).as_bytes(),
    );
    private_bytes(
        &root.join("invocation.hex"),
        hex::encode(invocation_key.to_bytes()).as_bytes(),
    );
    private_bytes(
        &root.join("catalog-signing.hex"),
        hex::encode(catalog_key.to_bytes()).as_bytes(),
    );
    let image =
        std::env::var("SPITZLI_RECOVERY_MAIL_IMAGE").expect("verified current owned Mail OCI root");
    let gateway_client = gateway_client_identity(&pki);
    let (policy, mut recipe) = foundation_runtime_profiles(
        &pki,
        address,
        metadata_admin.port().unwrap(),
        &certificate_sha256,
        &gateway_client.certificate_sha256,
        &native_key,
    );
    assert_ne!(
        incoming_customer, outgoing.0,
        "separate incoming/outgoing customers"
    );
    let customers = [incoming_customer, outgoing.0.as_str()];
    recipe["privateConfiguration"]["delivery"]["nativeAccounts"] = json!(
        customers
            .iter()
            .map(|id| local_smtp_account(id))
            .collect::<Vec<_>>()
    );
    // Local SMTP has no provider password; seal its empty credential material in the native format.
    let fixture_cipher = spitzli_mail::secrets::MailCipher::new(
        &base64::engine::general_purpose::STANDARD.encode([0u8; 32]),
    )
    .unwrap();
    let transports = customers
        .iter()
        .map(|id| {
            let sealed = fixture_cipher
                .seal(&json!({}), &format!("send-transport:{id}:1"))
                .unwrap();
            spitzli_mail::transport_adoption::ConfiguredTransport::managed(
                (*id).into(),
                None,
                sealed,
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    let catalog = signed_catalog(
        cluster,
        "owned-recovery",
        &image,
        policy,
        recipe,
        &catalog_key,
    );
    // Serialize the original signed catalog itself, not its public descriptor projection.
    // The source input bytes are written by signed_catalog below and passed to the normal loader.
    let catalog_path = root.join("catalog.json");
    assert!(
        catalog_path.is_file(),
        "actual signed catalog input must be retained"
    );
    let bindings = root.join("bindings.json");
    private_bytes(&bindings, b"{\"runtimes\":{}}");
    let connections = root.join("connections.json");
    private_bytes(&connections, b"{\"runtimes\":{}}");
    let config_path = root.join("plugin.json");
    private(
        &config_path,
        &json!({"hostBindings":bindings,"connections":connections,"invocationSigningKey":root.join("invocation.hex"),"installationCatalog":catalog_path,
        "control":{"kind":"mutualTls","address":address.to_string(),"certificate":pki.certificate,"privateKey":pki.private_key,"authority":pki.authority},
        "mail":{"signingKey":root.join("native-signing.hex"),"workerId":"mail","workerGeneration":1}}),
    );
    let config = PluginConfig::load(&config_path).unwrap();
    let worker_secret = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    private_bytes(&root.join("send-worker-secret"), worker_secret.as_bytes());
    let environment = HashMap::from([
        ("DATABASE_RLS_SECRET".into(), SECRET.into()),
        ("MAIL_SEND_WORKER_SECRET".into(), worker_secret),
    ]);
    let core = core_services(
        dependencies.mail.clone(),
        dependencies.oauth.clone(),
        &config,
        &environment,
        transports,
    )
    .await;
    let app = build_app_with_domains(
        spitzli_core::config::PlatformConfig::from_map(&HashMap::from([
            ("SPITZLI_AUTH_ORIGIN".into(), AUTH.into()),
            ("SPITZLI_STUDIO_ORIGIN".into(), STUDIO.into()),
            ("SPITZLI_CONSOLE_ORIGIN".into(), CONSOLE.into()),
            (
                "SPITZLI_FEATURES".into(),
                "{\"hosting\":\"enabled\",\"mail\":\"enabled\"}".into(),
            ),
        ]))
        .unwrap(),
        dependencies.mail.auth.clone(),
        Services {
            identity: Some(dependencies.mail.identity.clone()),
            oauth: Some(dependencies.oauth),
            browser: dependencies.mail.browser.clone(),
            hosting: Some(hosting),
            registry: Some(dependencies.registry),
            access: Some(dependencies.access),
            plugins: Some(core.plugins.clone()),
            mail: Some(core.http.clone()),
            ..Default::default()
        },
    );
    let administrator = installer_admin_uri(&metadata_admin);
    let worker_path = root.join("installation-worker.json");
    private(
        &worker_path,
        &json!({"storageTargets":[{"id":"matched-private","adminUrl":administrator.as_str(),"authority":pki.authority,"runtimeHost":"172.30.254.1","runtimePort":metadata_admin.port().unwrap()}]}),
    );
    let worker = spitzli_control_plane::plugins::installation::worker::InstallationWorker::load(
        &worker_path,
        &dependencies.mail.worker,
        &config,
    )
    .await
    .expect("actual installation worker configuration");
    let (callback_stop, shutdown) = tokio::sync::oneshot::channel();
    let callback = serve_callbacks(
        &core,
        listener,
        certificate,
        std::fs::read(&pki.private_key).unwrap(),
        std::fs::read(&pki.authority).unwrap(),
        shutdown,
    )
    .await;
    assert_eq!(catalog.descriptors()[0].plugin_id, "mail");
    Foundation {
        app,
        core,
        worker,
        metadata_container,
        metadata_password,
        authority: pki.authority,
        gateway_client,
        outgoing,
        callback_stop: Some(callback_stop),
        callback,
        descriptor: catalog.descriptors()[0].clone(),
        root,
    }
}

pub(super) async fn core_fixture() -> Fixture {
    assert_eq!(rustix::process::geteuid().as_raw(), 0);
    assert!(Path::new("/.dockerenv").is_file());
    let root = Path::new("/owned/mail-fixture");
    directory(root);
    let pki = make_pki(&root.join("core-tls"));
    let (container, url, _) = source_postgres("core", "webdock_operator_test", &pki).await;
    let passwords = spitzli_persistence::roles::LOGIN_ROLES
        .iter()
        .copied()
        .chain(["webdock_offline_migrate"])
        .map(|role| {
            (
                role.to_owned(),
                format!(
                    "{}{}",
                    uuid::Uuid::new_v4().simple(),
                    uuid::Uuid::new_v4().simple()
                ),
            )
        })
        .collect();
    let fixture = Fixture {
        container,
        url,
        passwords,
    };
    let owner = connect(fixture.url.as_str()).await.unwrap();
    spitzli_persistence::roles::provision_roles(
        &owner,
        "webdock_operator_test",
        &fixture.passwords,
    )
    .await
    .unwrap();
    spitzli_persistence::migrations::bootstrap(&owner, "webdock_operator_test", SECRET)
        .await
        .unwrap();
    owner.close().await.unwrap();
    fixture
}

fn kube(args: &[&str]) -> Value {
    let mut command = Command::new("/usr/bin/timeout");
    command
        .args([
            "--signal=TERM",
            "--kill-after=1",
            "10",
            "/usr/local/bin/kubectl",
            "--kubeconfig",
            "/etc/rancher/k3s/k3s.yaml",
            "--context",
            "default",
        ])
        .args(args);
    let (status, out, _) = super::byte_component::collect_command(command, None);
    assert!(
        status.success(),
        "owned Kube metadata command failed; private IO withheld"
    );
    if out.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&out).expect("bounded Kube JSON metadata")
    }
}
fn create_kube(value: &Value) {
    let mut command = Command::new("/usr/bin/timeout");
    command.args([
        "--signal=TERM",
        "--kill-after=1",
        "10",
        "/usr/local/bin/kubectl",
        "--kubeconfig",
        "/etc/rancher/k3s/k3s.yaml",
        "--context",
        "default",
        "create",
        "-f",
        "-",
        "-o",
        "json",
    ]);
    let input = serde_json::to_vec(value).unwrap();
    let (status, _, _) = super::byte_component::collect_command(command, Some(&input));
    assert!(
        status.success(),
        "create exact own Kube infrastructure object failed; raw IO withheld"
    );
}
fn source_ce_filesystem(customer: &str) -> PathBuf {
    spitzli_core::id::RecordId::parse(customer).expect("real original Registry customer");
    source_filesystem(&format!("ce-{customer}"), 2 * GIB, 65532)
}
fn kubectl_forward_category(raw: &[u8]) -> &'static str {
    let text = String::from_utf8_lossy(raw).to_ascii_lowercase();
    if text.is_empty() {
        "empty"
    } else if text.contains("pod is not running") {
        "pod_not_running"
    } else if text.contains("does not have a service port") {
        "unsupported_service_port"
    } else if text.contains("address already in use")
        || text.contains("unable to listen on any of the requested ports")
    {
        "local_bind_unavailable"
    } else if text.contains("timed out waiting for the condition") {
        "pod_running_timeout"
    } else if text.contains("error upgrading connection")
        || text.contains("unable to upgrade connection")
    {
        "forward_channel_unavailable"
    } else if text.contains("forbidden") || text.contains("unauthorized") {
        "api_authority_denied"
    } else if text.contains("context") && text.contains("does not exist") {
        "kube_context_unavailable"
    } else if text.contains("no such file or directory") && text.contains("config") {
        "kube_config_unavailable"
    } else {
        "unclassified"
    }
}
fn forward_stream_receipt(path: &Path) -> Value {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let read = || -> io::Result<Value> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.nlink() != 1
            || metadata.mode() & 0o077 != 0
        {
            return Ok(json!({"category":"unsafe_file","hashComplete":false}));
        }
        if metadata.len() > 16384 {
            return Ok(
                json!({"category":"over_limit","bytes":metadata.len(),"hashComplete":false}),
            );
        }
        let mut raw = Vec::new();
        file.take(16385).read_to_end(&mut raw)?;
        if raw.len() > 16384 || raw.len() as u64 != metadata.len() {
            return Ok(json!({"category":"changed_or_over_limit","hashComplete":false}));
        }
        Ok(
            json!({"bytes":raw.len(),"sha256":format!("{:x}",Sha256::digest(&raw)),"hashComplete":true,"category":kubectl_forward_category(&raw)}),
        )
    };
    read().unwrap_or_else(|_| json!({"category":"file_unavailable","hashComplete":false}))
}
fn forward_pod_projection(service: &Value, pods: &Value) -> Value {
    let known = |value: Option<&str>, allowed: &'static [&'static str]| {
        allowed
            .iter()
            .copied()
            .find(|v| Some(*v) == value)
            .unwrap_or(if value.is_none() {
                "none"
            } else {
                "unrecognized"
            })
    };
    let Some(selector) = service["spec"]["selector"]
        .as_object()
        .filter(|v| !v.is_empty() && v.len() <= 32 && v.values().all(Value::is_string))
    else {
        return json!({"category":"invalid_service_selector"});
    };
    let Some(items) = pods["items"].as_array().filter(|v| v.len() <= 32) else {
        return json!({"category":"pod_query_unavailable"});
    };
    let selected = items
        .iter()
        .filter(|pod| {
            pod["metadata"]["namespace"] == service["metadata"]["namespace"]
                && selector
                    .iter()
                    .all(|(k, v)| pod["metadata"]["labels"][k] == *v)
        })
        .collect::<Vec<_>>();
    if selected.len() != 1 {
        return json!({"category":if selected.is_empty(){"owned_pod_absent"}else{"owned_pod_ambiguous"},"ownedPodCount":selected.len()});
    }
    let pod = selected[0];
    let status = &pod["status"];
    let reasons: &'static [&'static str] = &[
        "PodCompleted",
        "ContainersReady",
        "ContainersNotReady",
        "ContainersNotInitialized",
        "PodScheduled",
        "Unschedulable",
        "SchedulingGated",
        "UnexpectedAdmissionError",
        "PodInitializing",
        "ContainerCreating",
        "ErrImagePull",
        "ImagePullBackOff",
        "CrashLoopBackOff",
        "CreateContainerConfigError",
        "CreateContainerError",
        "RunContainerError",
        "InvalidImageName",
        "Completed",
        "Error",
        "OOMKilled",
        "ContainerCannotRun",
        "StartError",
    ];
    let conditions=status["conditions"].as_array().map(|rows|rows.iter().take(8).map(|row|json!({"type":known(row["type"].as_str(),&["Ready","Initialized","ContainersReady","PodScheduled","PodReadyToStartContainers"]),"status":known(row["status"].as_str(),&["True","False","Unknown"]),"reason":known(row["reason"].as_str(),reasons)})).collect::<Vec<_>>()).unwrap_or_default();
    let mut containers = Vec::new();
    for (field, kind) in [
        ("initContainerStatuses", "init"),
        ("containerStatuses", "runtime"),
    ] {
        if let Some(rows) = status[field].as_array() {
            for row in rows.iter().take(8) {
                let terminated = if row["state"]["terminated"].is_object() {
                    &row["state"]["terminated"]
                } else {
                    &row["lastState"]["terminated"]
                };
                containers.push(json!({"kind":kind,"restartCount":row["restartCount"].as_u64().filter(|n|u32::try_from(*n).is_ok()),"waitingReason":known(row["state"]["waiting"]["reason"].as_str(),reasons),"terminatedReason":known(terminated["reason"].as_str(),reasons),"exitCode":terminated["exitCode"].as_i64().filter(|n|i32::try_from(*n).is_ok())}));
            }
        }
    }
    json!({"category":"owned_pod_status","ownedPodCount":1,"phase":known(status["phase"].as_str(),&["Pending","Running","Succeeded","Failed","Unknown"]),"conditions":conditions,"containers":containers})
}
fn forward_query_args(namespace: &str, kind: &str, name: Option<&str>) -> Option<Vec<String>> {
    let dns = (1..=63).contains(&namespace.len())
        && !namespace.starts_with('-')
        && !namespace.ends_with('-')
        && namespace
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    let owned = namespace.strip_prefix("wd-plugin-").is_some_and(|v| {
        v.len() == 24
            && v.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }) || namespace
        .strip_prefix("wd-mail-c")
        .is_some_and(|v| spitzli_core::id::RecordId::parse(v).is_ok());
    if !dns || !owned {
        return None;
    }
    let mut args = [
        "--kubeconfig",
        "/etc/rancher/k3s/k3s.yaml",
        "--context",
        "default",
    ]
    .map(str::to_owned)
    .to_vec();
    match (kind, name) {
        ("pods", None) => args.extend([
            "get".to_owned(),
            format!("--raw=/api/v1/namespaces/{namespace}/pods"),
        ]),
        ("replicasets", None) => args.extend([
            "get".to_owned(),
            format!("--raw=/apis/apps/v1/namespaces/{namespace}/replicasets"),
        ]),
        ("service" | "deployment", Some(name)) => {
            if name.is_empty()
                || name.len() > 63
                || name.starts_with('-')
                || name.ends_with('-')
                || !name
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
            {
                return None;
            }
            args.extend(
                [
                    "-n",
                    namespace,
                    "get",
                    kind,
                    name,
                    "--ignore-not-found",
                    "-o",
                    "json",
                ]
                .map(str::to_owned),
            );
        }
        _ => return None,
    }
    Some(args)
}
fn forward_query_value(kind: &str, raw: &[u8]) -> Option<Value> {
    if raw.len() > 1048576 {
        return None;
    }
    let value: Value = serde_json::from_slice(raw).ok()?;
    if kind == "pods"
        && (!value.is_object()
            || value["apiVersion"] != "v1"
            || value["kind"] != "PodList"
            || !value["items"].is_array())
    {
        return None;
    }
    if kind == "replicasets"
        && (!value.is_object()
            || value["apiVersion"] != "apps/v1"
            || value["kind"] != "ReplicaSetList"
            || !value["items"].is_array())
    {
        return None;
    }
    Some(value)
}
#[test]
fn forward_raw_pods_arguments_are_namespaced_and_never_rendered() {
    let namespace = "wd-plugin-0123456789abcdef01234567";
    let args = forward_query_args(namespace, "pods", None).unwrap();
    assert_eq!(
        args,
        vec![
            "--kubeconfig",
            "/etc/rancher/k3s/k3s.yaml",
            "--context",
            "default",
            "get",
            "--raw=/api/v1/namespaces/wd-plugin-0123456789abcdef01234567/pods"
        ]
    );
    assert_eq!(
        forward_query_args(namespace, "service", Some("runtime")).unwrap(),
        vec![
            "--kubeconfig",
            "/etc/rancher/k3s/k3s.yaml",
            "--context",
            "default",
            "-n",
            namespace,
            "get",
            "service",
            "runtime",
            "--ignore-not-found",
            "-o",
            "json"
        ]
    );
    for bad in [
        "",
        "foreign",
        "default",
        "wd-plugin-/pods",
        "wd-plugin-%2f",
        "wd-plugin-abc?x=1",
        "wd-plugin-ABC",
        "-owned",
        "owned-",
        "/api/v1/pods",
    ] {
        assert!(forward_query_args(bad, "pods", None).is_none());
    }
    for (kind, name) in [
        ("pods", Some("named")),
        ("pod", None),
        ("nodes", None),
        ("pods?x=1", None),
        ("/pods", None),
    ] {
        assert!(forward_query_args(namespace, kind, name).is_none());
    }
}

#[test]
fn forward_raw_pods_list_rejects_empty_malformed_and_noncanonical_shapes() {
    assert!(
        forward_query_value(
            "pods",
            br#"{"apiVersion":"v1","kind":"PodList","items":[]}"#
        )
        .unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    for poison in [
        b"".as_slice(),
        b"null",
        b"not-json",
        br#"{"apiVersion":"v1","kind":"PodList"}"#,
        br#"{"apiVersion":"v1","kind":"PodList","items":null}"#,
        br#"{"apiVersion":"v1","kind":"List","items":[]}"#,
        br#"{"kind":"PodList","items":[]}"#,
        br#"{"apiVersion":"v1","kind":"PodList","items":{}}"#,
    ] {
        assert!(forward_query_value("pods", poison).is_none());
    }
    assert!(forward_query_value("pods", &vec![b'x'; 1048577]).is_none());
}
#[tokio::test]
async fn forward_raw_empty_podlist_waits_then_running_without_early_spawn() {
    let service = json!({"metadata":{"name":"runtime","namespace":"owned","uid":"service-1"},"spec":{"selector":{"owned":"yes"}}});
    let mut reads = 0;
    let result=wait_forward_pod_running(&service,None, None,tokio::time::Instant::now()+Duration::from_secs(1),|| {
        reads+=1;
        let raw=if reads==1 {br#"{"apiVersion":"v1","kind":"PodList","items":[]}"#.as_slice()}else{br#"{"apiVersion":"v1","kind":"PodList","items":[{"metadata":{"namespace":"owned","uid":"pod-1","labels":{"owned":"yes"}},"spec":{"containers":[{"name":"runtime"}]},"status":{"phase":"Running","conditions":[{"type":"Ready","status":"True"}],"containerStatuses":[{"name":"runtime","ready":true,"state":{"running":{}}}]}}]}"#.as_slice()};
        std::future::ready((service.clone(),forward_query_value("pods",raw).unwrap()))
    }).await;
    assert_eq!(reads, 2);
    assert_eq!(result.unwrap(), "pod-1");
    assert!(
        wait_forward_pod_running(
            &service,
            None,
            None,
            tokio::time::Instant::now() + Duration::from_secs(1),
            || std::future::ready((service.clone(), json!({"kind":"PodList"})))
        )
        .await
        .is_err()
    );
}

fn forward_kube_value(namespace: &str, kind: &str, name: Option<&str>) -> Option<Value> {
    let queried = std::panic::catch_unwind(|| {
        let mut command = Command::new("/usr/bin/timeout");
        command.args([
            "--signal=TERM",
            "--kill-after=1",
            "2",
            "/usr/local/bin/kubectl",
        ]);
        command.args(forward_query_args(namespace, kind, name)?);
        Some(super::byte_component::collect_command(command, None))
    });
    match queried {
        Ok(Some((status, out, _))) if status.success() && out.len() <= 1048576 => {
            if out.iter().all(u8::is_ascii_whitespace) {
                Some(Value::Null)
            } else {
                forward_query_value(kind, &out)
            }
        }
        _ => None,
    }
}
fn retain_forward_observation(
    namespace: &str,
    service: &str,
    expected: &str,
    name: &str,
    phase: &'static str,
    exit: Option<i32>,
) {
    if !["mail-runtime-forward", "mail-admin-forward"].contains(&name)
        || !["initial", "terminal", "ready", "timeout"].contains(&phase)
    {
        return;
    }
    let directory = Path::new("/owned/mail-fixture");
    let stdout = forward_stream_receipt(&directory.join(format!("{name}.stdout")));
    let stderr = forward_stream_receipt(&directory.join(format!("{name}.stderr")));
    let current = forward_kube_value(namespace, "service", Some(service));
    let pod = match current {
        Some(value)
            if value["metadata"]["uid"] == expected
                && value["metadata"]["name"] == service
                && value["metadata"]["namespace"] == namespace =>
        {
            forward_pod_projection(
                &value,
                &forward_kube_value(namespace, "pods", None).unwrap_or(Value::Null),
            )
        }
        Some(Value::Null) => json!({"category":"service_absent"}),
        Some(_) => json!({"category":"service_uid_mismatch"}),
        None => json!({"category":"service_query_unavailable"}),
    };
    let record = json!({"phase":phase,"serviceUID":expected,"childExitCode":exit,"stdout":stdout,"stderr":stderr,"pod":pod});
    if super::write_diagnostic_json(
        &Path::new("/diagnostics").join(format!("{name}-{phase}-observation.json")),
        &record,
    )
    .is_err()
    {
        eprintln!("recovery_component mail_forward_capture category=unavailable");
    }
}
#[test]
fn forward_stream_capture_uses_real_bounded_files_without_leaking_child_text() {
    use std::os::unix::fs::PermissionsExt;
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("child.stderr");
    let raw=b"error: Service runtime does not have a service port 8443 https://poison.invalid/credential";
    std::fs::write(&path, raw).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let projected = forward_stream_receipt(&path);
    assert_eq!(projected["category"], "unsupported_service_port");
    assert_eq!(projected["bytes"], raw.len());
    assert_eq!(projected["hashComplete"], true);
    assert!(!projected.to_string().contains("poison"));
    std::fs::write(&path, vec![b'x'; 16385]).unwrap();
    let projected = forward_stream_receipt(&path);
    assert_eq!(projected["category"], "over_limit");
    assert_eq!(projected["hashComplete"], false);
}
#[test]
fn forward_pod_projection_ignores_env_args_secrets_and_unrecognized_reasons() {
    let service = json!({"metadata":{"namespace":"owned","uid":"own-service"},"spec":{"selector":{"owned":"yes"}}});
    let pod = json!({"metadata":{"namespace":"owned","labels":{"owned":"yes"}},"spec":{"containers":[{"env":[{"value":"poison-secret"}],"args":["poison-arg"]}]},"status":{"phase":"Pending","conditions":[{"type":"PodScheduled","status":"False","reason":"Unschedulable","message":"poison-message"}],"initContainerStatuses":[{"restartCount":2,"state":{"waiting":{"reason":"ImagePullBackOff","message":"poison-registry"}},"lastState":{"terminated":{"exitCode":13,"reason":"Error","message":"poison-termination"}}}]}});
    let projection = forward_pod_projection(&service, &json!({"items":[pod.clone()]}));
    assert_eq!(projection["phase"], "Pending");
    assert_eq!(
        projection["containers"][0]["waitingReason"],
        "ImagePullBackOff"
    );
    assert_eq!(projection["containers"][0]["exitCode"], 13);
    assert!(!projection.to_string().contains("poison"));
    let mut bad = pod.clone();
    bad["status"]["phase"] = json!("poison-phase");
    bad["status"]["conditions"][0]["reason"] = json!("poison-reason");
    let projection = forward_pod_projection(&service, &json!({"items":[bad]}));
    assert_eq!(projection["phase"], "unrecognized");
    assert!(!projection.to_string().contains("poison"));
    assert_eq!(
        forward_pod_projection(&service, &json!({"items":[pod.clone(),pod]}))["category"],
        "owned_pod_ambiguous"
    );
}

#[tokio::test]
async fn forward_running_requires_real_ready_for_runtime_and_ce_container_names() {
    let service = json!({"metadata":{"name":"runtime","namespace":"owned","uid":"service-1"},"spec":{"selector":{"owned":"yes"}}});
    for name in ["runtime", "stalwart"] {
        let mut reads = 0;
        let mut spawns = 0;
        let result=wait_forward_pod_running(&service,None, None,tokio::time::Instant::now()+Duration::from_secs(1),|| {
            assert_eq!(spawns,0);reads+=1;let ready=reads>1;
            std::future::ready((service.clone(),json!({"items":[{"metadata":{"namespace":"owned","uid":"pod-1","labels":{"owned":"yes"}},"spec":{"containers":[{"name":name}]},"status":{"phase":"Running","conditions":[{"type":"Ready","status":if ready {"True"}else{"False"}}],"containerStatuses":[{"name":name,"ready":ready,"state":{"running":{}}}]}}]})))
        }).await;
        if result.is_ok() {
            spawns += 1;
        }
        assert_eq!(
            reads, 2,
            "Running without actual Ready launched forward child"
        );
        assert_eq!(spawns, 1);
        assert_eq!(result.unwrap(), "pod-1");
    }
}

#[tokio::test]
async fn forward_readiness_missing_false_and_unknown_wait_under_same_deadline() {
    let service = json!({"metadata":{"name":"runtime","namespace":"owned","uid":"s"},"spec":{"selector":{"owned":"yes"}}});
    let ready = json!({"metadata":{"namespace":"owned","uid":"p","labels":{"owned":"yes"}},"spec":{"containers":[{"name":"stalwart"},{"name":"sidecar"}]},"status":{"phase":"Running","conditions":[{"type":"Ready","status":"True"}],"containerStatuses":[{"name":"stalwart","ready":true,"state":{"running":{}}},{"name":"sidecar","ready":true,"state":{"running":{}}}]}});
    for variant in [
        "missing_condition",
        "false",
        "unknown",
        "missing_status",
        "incomplete",
        "not_running",
    ] {
        let mut first = ready.clone();
        match variant {
            "missing_condition" => first["status"]["conditions"] = Value::Null,
            "false" => first["status"]["conditions"][0]["status"] = json!("False"),
            "unknown" => first["status"]["conditions"][0]["status"] = json!("Unknown"),
            "missing_status" => first["status"]["containerStatuses"] = Value::Null,
            "incomplete" => {
                first["status"]["containerStatuses"]
                    .as_array_mut()
                    .unwrap()
                    .pop();
            }
            "not_running" => {
                first["status"]["containerStatuses"][0]["state"] = json!({"waiting":{}})
            }
            _ => unreachable!(),
        }
        let mut reads = 0;
        let result = wait_forward_pod_running(
            &service,
            None,
            None,
            tokio::time::Instant::now() + Duration::from_secs(1),
            || {
                reads += 1;
                std::future::ready((
                    service.clone(),
                    json!({"items":[if reads==1 {first.clone()}else{ready.clone()}]}),
                ))
            },
        )
        .await;
        assert_eq!(
            reads, 2,
            "readiness unknown/false returned before real Ready"
        );
        assert_eq!(result.unwrap(), "p");
    }
    let mut pending = ready.clone();
    pending["status"]["conditions"][0]["status"] = json!("False");
    let result = wait_forward_pod_running(
        &service,
        None,
        None,
        tokio::time::Instant::now() + Duration::from_millis(20),
        || std::future::ready((service.clone(), json!({"items":[pending.clone()]}))),
    )
    .await;
    assert_eq!(result.unwrap_err(), "forward_deadline");
}
#[tokio::test]
async fn forward_readiness_rejects_malformed_duplicate_and_terminal_status() {
    let service = json!({"metadata":{"name":"runtime","namespace":"owned","uid":"s"},"spec":{"selector":{"owned":"yes"}}});
    let ready = json!({"metadata":{"namespace":"owned","uid":"p","labels":{"owned":"yes"}},"spec":{"containers":[{"name":"stalwart"}]},"status":{"phase":"Running","conditions":[{"type":"Ready","status":"True"}],"containerStatuses":[{"name":"stalwart","ready":true,"state":{"running":{}}}]}});
    for variant in [
        "deleting",
        "duplicate_condition",
        "bad_condition",
        "bad_conditions",
        "duplicate_declared",
        "bad_declared",
        "duplicate_status",
        "foreign_status",
        "bad_ready",
        "bad_state",
        "terminal",
    ] {
        let mut pod = ready.clone();
        match variant {
            "deleting" => pod["metadata"]["deletionTimestamp"] = json!("owned-fixed"),
            "duplicate_condition" => pod["status"]["conditions"]
                .as_array_mut()
                .unwrap()
                .push(json!({"type":"Ready","status":"True"})),
            "bad_condition" => pod["status"]["conditions"][0]["status"] = json!(true),
            "bad_conditions" => pod["status"]["conditions"] = json!({}),
            "duplicate_declared" => pod["spec"]["containers"]
                .as_array_mut()
                .unwrap()
                .push(json!({"name":"stalwart"})),
            "bad_declared" => pod["spec"]["containers"] = Value::Null,
            "duplicate_status" => pod["status"]["containerStatuses"]
                .as_array_mut()
                .unwrap()
                .push(json!({"name":"stalwart","ready":true,"state":{"running":{}}})),
            "foreign_status" => pod["status"]["containerStatuses"][0]["name"] = json!("foreign"),
            "bad_ready" => pod["status"]["containerStatuses"][0]["ready"] = json!("True"),
            "bad_state" => {
                pod["status"]["containerStatuses"][0]["state"] = json!({"running":{},"waiting":{}})
            }
            "terminal" => {
                pod["status"]["containerStatuses"][0]["state"] =
                    json!({"terminated":{"exitCode":1}})
            }
            _ => unreachable!(),
        }
        let result = wait_forward_pod_running(
            &service,
            None,
            None,
            tokio::time::Instant::now() + Duration::from_secs(1),
            || std::future::ready((service.clone(), json!({"items":[pod.clone()]}))),
        )
        .await;
        assert!(result.is_err(), "malformed readiness accepted");
    }
}

fn forward_pod_ready(pod: &Value) -> Result<bool, &'static str> {
    let declared = pod["spec"]["containers"]
        .as_array()
        .filter(|v| !v.is_empty() && v.len() <= 8)
        .ok_or("pod_containers_invalid")?;
    let mut names = std::collections::BTreeSet::new();
    for container in declared {
        let name = container["name"]
            .as_str()
            .filter(|v| !v.is_empty() && v.len() <= 63)
            .ok_or("pod_containers_invalid")?;
        if !names.insert(name) {
            return Err("pod_containers_invalid");
        }
    }
    let mut pod_ready = false;
    if let Some(value) = pod["status"].get("conditions").filter(|v| !v.is_null()) {
        let rows = value
            .as_array()
            .filter(|v| v.len() <= 16)
            .ok_or("pod_readiness_invalid")?;
        let mut seen = false;
        for row in rows {
            if !row.is_object() {
                return Err("pod_readiness_invalid");
            }
            if row["type"] == "Ready" {
                if seen {
                    return Err("pod_readiness_invalid");
                }
                seen = true;
                match row.get("status") {
                    None | Some(Value::Null) => {}
                    Some(Value::String(status)) => match status.as_str() {
                        "True" => pod_ready = true,
                        "False" | "Unknown" => {}
                        _ => return Err("pod_readiness_invalid"),
                    },
                    _ => return Err("pod_readiness_invalid"),
                }
            }
        }
    }
    let Some(value) = pod["status"]
        .get("containerStatuses")
        .filter(|v| !v.is_null())
    else {
        return Ok(false);
    };
    let rows = value
        .as_array()
        .filter(|v| v.len() <= 8)
        .ok_or("pod_container_status_invalid")?;
    let mut seen = std::collections::BTreeSet::new();
    let mut containers_ready = true;
    for row in rows {
        let name = row["name"].as_str().ok_or("pod_container_status_invalid")?;
        if !names.contains(name) || !seen.insert(name) {
            return Err("pod_container_status_invalid");
        }
        let ready = match row.get("ready") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(value)) => *value,
            _ => return Err("pod_container_status_invalid"),
        };
        let running = match row.get("state").filter(|v| !v.is_null()) {
            None => false,
            Some(value) => {
                let states = value
                    .as_object()
                    .filter(|v| v.len() <= 1)
                    .ok_or("pod_container_status_invalid")?;
                if states.is_empty() {
                    false
                } else {
                    let (kind, state) =
                        states.iter().next().ok_or("pod_container_status_invalid")?;
                    if !state.is_object() {
                        return Err("pod_container_status_invalid");
                    }
                    match kind.as_str() {
                        "running" => true,
                        "waiting" => false,
                        "terminated" => return Err("pod_container_terminal"),
                        _ => return Err("pod_container_status_invalid"),
                    }
                }
            }
        };
        containers_ready &= ready && running;
    }
    Ok(pod_ready && containers_ready && seen == names)
}

// Expected values come only from the admitted Core runtime spec or stored Core workload objects.
fn forward_producer_matches(actual: &Value, expected: &Value) -> bool {
    match expected {
        Value::Object(fields) => actual.as_object().is_some_and(|object| {
            fields.iter().all(|(key, value)| {
                object.get(key).map_or_else(
                    || key == "envFrom" && value.as_array().is_some_and(Vec::is_empty),
                    |actual| forward_producer_matches(actual, value),
                )
            })
        }),
        Value::Array(rows) => actual.as_array().is_some_and(|actual| {
            actual.len() == rows.len()
                && actual
                    .iter()
                    .zip(rows)
                    .all(|(a, e)| forward_producer_matches(a, e))
        }),
        _ => actual == expected,
    }
}
fn forward_uid(value: &Value) -> Option<&str> {
    value.as_str().filter(|v| uuid::Uuid::parse_str(v).is_ok())
}
fn forward_controller(object: &Value, kind: &str, parent: &Value) -> Result<(), &'static str> {
    let owners = object["metadata"]["ownerReferences"]
        .as_array()
        .filter(|v| v.len() == 1)
        .ok_or("owner_chain_invalid")?;
    let owner = &owners[0];
    if owner["controller"] != true
        || owner["kind"] != kind
        || owner["uid"] != parent["metadata"]["uid"]
        || owner["name"] != parent["metadata"]["name"]
    {
        return Err("owner_chain_invalid");
    }
    Ok(())
}
// TypeMeta is omitempty on nested items; the strict canonical collection fixes their type.
fn forward_list_item_type(value: &Value, version: &str, kind: &str) -> bool {
    [("apiVersion", version), ("kind", kind)]
        .iter()
        .all(|(key, expected)| match value.get(*key) {
            None => true,
            Some(Value::String(v)) => v.is_empty() || v == expected,
            _ => false,
        })
}
fn forward_owner_binding(
    expected: &Value,
    service: &Value,
    deployment: &Value,
    sets: &Value,
    pods: &Value,
    pinned: &mut Option<Value>,
) -> Result<(), &'static str> {
    if pods["apiVersion"] != "v1"
        || pods["kind"] != "PodList"
        || sets["apiVersion"] != "apps/v1"
        || sets["kind"] != "ReplicaSetList"
    {
        return Err("owner_list_invalid");
    }
    for list in [pods, sets] {
        if list["metadata"]
            .get("continue")
            .is_some_and(|v| v.as_str() != Some(""))
        {
            return Err("owner_list_incomplete");
        }
    }
    let namespace = expected["namespace"].as_str().ok_or("producer_invalid")?;
    if expected["sourceGeneration"].as_u64().is_none_or(|v| v == 0) {
        return Err("producer_invalid");
    }
    for object in [service, deployment] {
        if !object["metadata"]["ownerReferences"].is_null()
            && object["metadata"]["ownerReferences"]
                .as_array()
                .is_none_or(|v| !v.is_empty())
        {
            return Err("owner_chain_invalid");
        }
    }
    let desired_service = &expected["service"];
    let desired_deployment = &expected["deployment"];
    if desired_service["kind"] != "Service"
        || desired_deployment["kind"] != "Deployment"
        || desired_deployment["spec"]["replicas"] != 1
        || desired_service["metadata"]["namespace"] != namespace
        || desired_deployment["metadata"]["namespace"] != namespace
    {
        return Err("producer_invalid");
    }
    let items = pods["items"]
        .as_array()
        .filter(|v| v.len() <= 32)
        .ok_or("pod_query_unavailable")?;
    let selector = desired_service["spec"]["selector"]
        .as_object()
        .filter(|v| !v.is_empty() && v.len() <= 32)
        .ok_or("producer_invalid")?;
    if forward_uid(&service["metadata"]["uid"]).is_none()
        || service["metadata"]["name"] != desired_service["metadata"]["name"]
        || service["metadata"]["namespace"] != namespace
        || !service["metadata"]["deletionTimestamp"].is_null()
        || !forward_producer_matches(service, desired_service)
    {
        return Err("service_producer_changed");
    }
    if deployment.is_null() && items.is_empty() && pinned.is_none() {
        return Ok(());
    }
    if deployment["apiVersion"] != "apps/v1"
        || deployment["kind"] != "Deployment"
        || forward_uid(&deployment["metadata"]["uid"]).is_none()
        || deployment["metadata"]["namespace"] != namespace
        || deployment["metadata"]["name"] != desired_deployment["metadata"]["name"]
        || deployment["metadata"]["generation"]
            .as_u64()
            .is_none_or(|v| v == 0)
        || !deployment["metadata"]["deletionTimestamp"].is_null()
        || !forward_producer_matches(deployment, desired_deployment)
    {
        return Err("deployment_producer_changed");
    }
    let sets = sets["items"]
        .as_array()
        .filter(|v| v.len() <= 32)
        .ok_or("replicaset_query_unavailable")?;
    let mut own_sets = Vec::new();
    for set in sets {
        if set["metadata"]["ownerReferences"]
            .as_array()
            .is_some_and(|owners| {
                owners
                    .iter()
                    .any(|owner| owner["uid"] == deployment["metadata"]["uid"])
            })
        {
            forward_controller(set, "Deployment", deployment)?;
            if !forward_list_item_type(set, "apps/v1", "ReplicaSet")
                || set["metadata"]["namespace"] != namespace
                || forward_uid(&set["metadata"]["uid"]).is_none()
                || set["metadata"]["generation"]
                    .as_u64()
                    .is_none_or(|v| v == 0)
                || !set["metadata"]["deletionTimestamp"].is_null()
                || !forward_producer_matches(
                    &set["spec"]["template"],
                    &desired_deployment["spec"]["template"],
                )
            {
                return Err("replicaset_producer_changed");
            }
            own_sets.push(set);
        }
    }
    if own_sets.is_empty() && items.is_empty() && pinned.is_none() {
        return Ok(());
    }
    if own_sets.len() != 1 {
        return Err("replicaset_ambiguous");
    }
    let set = own_sets[0];
    let binding = json!({"serviceUID":service["metadata"]["uid"],"deploymentUID":deployment["metadata"]["uid"],"deploymentGeneration":deployment["metadata"]["generation"],"replicaSetUID":set["metadata"]["uid"],"replicaSetGeneration":set["metadata"]["generation"],"sourceGeneration":expected["sourceGeneration"],"templateSHA256":format!("{:x}",Sha256::digest(serde_json::to_vec(&desired_deployment["spec"]["template"]).map_err(|_|"producer_invalid")?))});
    if pinned.as_ref().is_some_and(|old| old != &binding) {
        return Err("producer_binding_changed");
    }
    let template = &desired_deployment["spec"]["template"];
    let mut ids = std::collections::BTreeSet::new();
    for pod in items {
        let uid = forward_uid(&pod["metadata"]["uid"]).ok_or("pod_uid_missing")?;
        if !ids.insert(uid) {
            return Err("pod_uid_duplicate");
        }
        if !forward_list_item_type(pod, "v1", "Pod")
            || pod["metadata"]["namespace"] != namespace
            || !selector
                .iter()
                .all(|(k, v)| pod["metadata"]["labels"][k] == *v)
            || !forward_producer_matches(pod, template)
            || pod["spec"]["ephemeralContainers"]
                .as_array()
                .is_some_and(|v| !v.is_empty())
        {
            return Err("pod_producer_changed");
        }
        forward_controller(pod, "ReplicaSet", set)?;
    }
    *pinned = Some(binding);
    Ok(())
}
fn forward_runtime_producer(spec: &spitzli_agent_protocol::plugin::PluginRuntimeSpec) -> Value {
    let labels = spec.labels();
    let annotations = json!({"webdock.dev/spec-sha256":spec.fingerprint().unwrap()});
    json!({"namespace":spec.namespace,"sourceGeneration":spec.runtime_generation,
        "service":{"apiVersion":"v1","kind":"Service","metadata":{"namespace":spec.namespace,"name":spec.service,"labels":labels,"annotations":annotations},"spec":{"selector":labels,"ports":[{"name":"runtime","port":spec.port,"targetPort":"runtime","protocol":"TCP"}]}},
        "deployment":{"apiVersion":"apps/v1","kind":"Deployment","metadata":{"namespace":spec.namespace,"name":spec.service,"labels":labels,"annotations":annotations},"spec":{"replicas":1,"strategy":{"type":"Recreate"},"selector":{"matchLabels":labels},"template":{"metadata":{"labels":labels,"annotations":annotations},"spec":{"initContainers":[{"name":"private-config","image":spec.image}],"containers":[{"name":"runtime","image":spec.image}]}}}}})
}
async fn forward_read_owned(
    namespace: &str,
    service: &str,
    expected: &Value,
    deadline: tokio::time::Instant,
    trace: &ForwardQueryTrace,
    pinned: &std::sync::Mutex<Option<Value>>,
) -> (Value, Value) {
    let current =
        forward_gate_kube(namespace, "service", Some(service), deadline, trace, None).await;
    let deployment_name = expected["deployment"]["metadata"]["name"]
        .as_str()
        .expect("actual producer Deployment name");
    let pods = forward_gate_kube(
        namespace,
        "pods",
        None,
        deadline,
        trace,
        Some(&expected["service"]["spec"]["selector"]),
    )
    .await;
    let (deployment, sets) = tokio::join!(
        forward_gate_kube(
            namespace,
            "deployment",
            Some(deployment_name),
            deadline,
            trace,
            None
        ),
        forward_gate_kube(namespace, "replicasets", None, deadline, trace, None),
    );
    let mut pin = pinned.lock().unwrap();
    let result = forward_owner_binding(expected, &current, &deployment, &sets, &pods, &mut pin);
    let mut projection = forward_decision_metadata(&current, &pods);
    projection["binding"] = json!(pin.as_ref());
    projection["deploymentUID"] = json!(forward_uid(&deployment["metadata"]["uid"]));
    projection["deploymentGeneration"] = json!(deployment["metadata"]["generation"].as_u64());
    projection["deploymentMatches"] = json!(forward_producer_matches(
        &deployment,
        &expected["deployment"]
    ));
    projection["replicaSets"] = json!(sets["items"].as_array().filter(|v|v.len()<=32).map(|rows| rows.iter().map(|row|json!({"uid":forward_uid(&row["metadata"]["uid"]),"generation":row["metadata"]["generation"].as_u64(),"controllerMatches":forward_controller(row,"Deployment",&deployment).is_ok(),"templateMatches":forward_producer_matches(&row["spec"]["template"],&expected["deployment"]["spec"]["template"])})).collect::<Vec<_>>()));
    if let (Some(projected), Some(items)) =
        (projection["pods"].as_array_mut(), pods["items"].as_array())
    {
        for (row, pod) in projected.iter_mut().zip(items) {
            row["producerMatches"] = json!(forward_producer_matches(
                pod,
                &expected["deployment"]["spec"]["template"]
            ));
        }
    }
    projection["podQuerySHA256"] = trace
        .last
        .lock()
        .unwrap()
        .get("Pods")
        .map(|v| v["stdoutSHA256"].clone())
        .unwrap_or(Value::Null);
    projection["ownerBound"] = json!(pin.is_some());
    projection["producerMatches"] = json!(result.is_ok() && pin.is_some());
    projection["category"] = json!(result.as_ref().err().copied().unwrap_or(if pin.is_some() {
        "matched"
    } else {
        "pending_owner"
    }));
    trace.record("Owner", projection);
    result.expect("owned forward producer binding denied");
    (current, pods)
}

fn forward_terminal_complete(pod: &Value) -> Result<(), &'static str> {
    if !pod["metadata"]["deletionTimestamp"].is_null()
        && pod["metadata"]["deletionTimestamp"].as_str().is_none()
    {
        return Err("pod_history_incomplete");
    }
    for (declarations, statuses) in [
        ("initContainers", "initContainerStatuses"),
        ("containers", "containerStatuses"),
    ] {
        let empty = Vec::new();
        let declared = match pod["spec"].get(declarations) {
            None | Some(Value::Null) if declarations == "initContainers" => &empty,
            Some(Value::Array(rows))
                if rows.len() <= 8 && (!rows.is_empty() || declarations == "initContainers") =>
            {
                rows
            }
            _ => return Err("pod_history_incomplete"),
        };
        let names = declared
            .iter()
            .map(|row| row["name"].as_str().filter(|v| !v.is_empty()))
            .collect::<Option<std::collections::BTreeSet<_>>>()
            .ok_or("pod_history_incomplete")?;
        if names.len() != declared.len() {
            return Err("pod_history_incomplete");
        }
        let rows = match pod["status"].get(statuses) {
            None | Some(Value::Null) if names.is_empty() => &empty,
            Some(Value::Array(rows)) if rows.len() == names.len() => rows,
            _ => return Err("pod_history_incomplete"),
        };
        let mut seen = std::collections::BTreeSet::new();
        for row in rows {
            let name = row["name"].as_str().ok_or("pod_history_incomplete")?;
            let state = row["state"].as_object().ok_or("pod_history_incomplete")?;
            if !names.contains(name)
                || !seen.insert(name)
                || state.len() != 1
                || !state.get("terminated").is_some_and(Value::is_object)
                || row["restartCount"]
                    .as_u64()
                    .is_none_or(|v| u32::try_from(v).is_err())
                || row["state"]["terminated"]["exitCode"]
                    .as_i64()
                    .is_none_or(|v| i32::try_from(v).is_err())
            {
                return Err("pod_history_incomplete");
            }
        }
        if seen != names {
            return Err("pod_history_incomplete");
        }
    }
    Ok(())
}
fn forward_active_pod(
    before: &Value,
    pods: &Value,
    bound: &mut Option<String>,
) -> Result<Option<String>, &'static str> {
    let selector = before["spec"]["selector"]
        .as_object()
        .filter(|v| !v.is_empty() && v.len() <= 32 && v.values().all(Value::is_string))
        .ok_or("service_selector_invalid")?;
    let items = pods["items"]
        .as_array()
        .filter(|v| v.len() <= 32)
        .ok_or("pod_query_unavailable")?;
    let mut active = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for pod in items {
        if pod["metadata"]["namespace"] != before["metadata"]["namespace"]
            || !selector
                .iter()
                .all(|(k, v)| pod["metadata"]["labels"][k] == *v)
        {
            return Err("foreign_pod");
        }
        let uid = pod["metadata"]["uid"]
            .as_str()
            .filter(|v| !v.is_empty() && v.len() <= 128)
            .ok_or("pod_uid_missing")?;
        if !seen.insert(uid) {
            return Err("pod_uid_duplicate");
        }
        match pod["status"]["phase"].as_str() {
            Some("Succeeded" | "Failed") => {
                forward_terminal_complete(pod)?;
                if bound.as_deref() == Some(uid) {
                    return Err("pod_terminal");
                }
            }
            Some("Pending" | "Running") => active.push(pod),
            _ => return Err("pod_phase_unknown"),
        }
    }
    if active.len() > 1 {
        return Err("pod_ambiguous");
    }
    let Some(pod) = active.first() else {
        return if bound.is_some() {
            Err("bound_pod_missing")
        } else {
            Ok(None)
        };
    };
    let uid = pod["metadata"]["uid"].as_str().ok_or("pod_uid_missing")?;
    if bound.as_deref().is_some_and(|v| v != uid) {
        return Err("pod_uid_changed");
    }
    if !pod["metadata"]["deletionTimestamp"].is_null() {
        return Err("pod_deleting");
    }
    *bound = Some(uid.to_owned());
    if pod["status"]["phase"] == "Running" && forward_pod_ready(pod)? {
        Ok(Some(uid.to_owned()))
    } else {
        Ok(None)
    }
}
fn forward_decision_metadata(service: &Value, pods: &Value) -> Value {
    let uid = |v: &Value| {
        v.as_str()
            .filter(|v| uuid::Uuid::parse_str(v).is_ok())
            .map(str::to_owned)
    };
    let rows = pods["items"].as_array().filter(|v| v.len() <= 32).map(|rows| rows.iter().map(|pod| {
        let mut status = forward_pod_projection(service, &json!({"items":[pod]}));
        let actual=pod["status"]["initContainerStatuses"].as_array().into_iter().flatten().take(8).chain(pod["status"]["containerStatuses"].as_array().into_iter().flatten().take(8));
        if let Some(rows)=status["containers"].as_array_mut() {
            for (safe,row) in rows.iter_mut().zip(actual) {
                let state=row["state"].as_object().filter(|v|v.len()==1).and_then(|v|v.keys().next()).map(String::as_str);
                let state=match state {Some("running")=>"running",Some("waiting")=>"waiting",Some("terminated")=>"terminated",_=>"invalid"};
                safe["state"]=json!(state);safe["ready"]=json!(row["ready"].as_bool());
                safe["exitCode"]=json!(row["state"]["terminated"]["exitCode"].as_i64().filter(|v|i32::try_from(*v).is_ok()));
                if state!="terminated" {safe["terminatedReason"]=json!("none");}
            }
        }
        let owners = pod["metadata"]["ownerReferences"].as_array().filter(|v| v.len() <= 4).map(|rows| rows.iter().map(|row| json!({"uid":uid(&row["uid"]),"replicaSet":row["kind"]=="ReplicaSet","controller":row["controller"]==true})).collect::<Vec<_>>());
        let mut value=json!({"uid":uid(&pod["metadata"]["uid"]),"deleting":!pod["metadata"]["deletionTimestamp"].is_null(),"owners":owners,"status":status});
        if prelude_enabled() {value["lifetime"]=prelude_pod_projection(pod).unwrap_or_else(|category|json!({"category":category}));}
        value
    }).collect::<Vec<_>>());
    json!({"serviceUID":uid(&service["metadata"]["uid"]),"pods":rows,"matchingCount":pods["items"].as_array().map(Vec::len)})
}
async fn wait_forward_pod_running<R, F>(
    before: &Value,
    pinned: Option<&str>,
    trace: Option<&ForwardQueryTrace>,
    deadline: tokio::time::Instant,
    mut read: R,
) -> Result<String, &'static str>
where
    R: FnMut() -> F,
    F: std::future::Future<Output = (Value, Value)>,
{
    let mut bound = pinned.map(str::to_owned);
    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err("forward_deadline");
        }
        let (service, pods) = tokio::time::timeout_at(deadline, read())
            .await
            .map_err(|_| "forward_deadline")?;
        if tokio::time::Instant::now() >= deadline {
            return Err("forward_deadline");
        }
        let before_pin = bound.clone();
        let decision = if service["metadata"]["uid"] != before["metadata"]["uid"]
            || service["metadata"]["namespace"] != before["metadata"]["namespace"]
            || service["metadata"]["name"] != before["metadata"]["name"]
            || service["spec"]["selector"] != before["spec"]["selector"]
        {
            Err("service_binding_changed")
        } else {
            forward_active_pod(before, &pods, &mut bound)
        };
        if let Some(trace) = trace {
            let mut projection = forward_decision_metadata(&service, &pods);
            let safe_uid = |v: &Option<String>| {
                v.as_deref()
                    .filter(|v| uuid::Uuid::parse_str(v).is_ok())
                    .map(str::to_owned)
            };
            projection["podQuerySHA256"] = trace
                .last
                .lock()
                .unwrap()
                .get("Pods")
                .map(|v| v["stdoutSHA256"].clone())
                .unwrap_or(Value::Null);
            projection["pinnedUIDBefore"] = json!(safe_uid(&before_pin));
            projection["pinnedUIDAfter"] = json!(safe_uid(&bound));
            projection["decision"] = json!(match &decision {
                Ok(Some(_)) => "ready",
                Ok(None) => "waiting",
                Err(category) => category,
            });
            trace.record("Decision", projection);
        }
        if let Some(uid) = decision? {
            return Ok(uid);
        }
        tokio::time::sleep_until(
            (tokio::time::Instant::now() + Duration::from_millis(100)).min(deadline),
        )
        .await;
    }
}

fn forward_query_projection(
    stage: &'static str,
    code: Option<i32>,
    out: &[u8],
    err: &[u8],
    namespace: &str,
    selector: Option<&Value>,
) -> Value {
    let limited = out.len() > 1048576 || err.len() > 16384;
    let parsed = if limited {
        None
    } else {
        serde_json::from_slice::<Value>(out).ok()
    };
    let kind = |value: &Value| match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    };
    let category = if limited {
        "over_limit"
    } else if code.is_some_and(|n| n != 0) {
        "exit_failed"
    } else if out.is_empty() {
        "empty_output"
    } else if parsed.is_none() {
        "invalid_json"
    } else {
        "valid_json"
    };
    let top_keys = parsed
        .as_ref()
        .and_then(Value::as_object)
        .map(|value| {
            value
                .keys()
                .map(|key| match key.as_str() {
                    "apiVersion" => "apiVersion",
                    "kind" => "kind",
                    "metadata" => "metadata",
                    "items" => "items",
                    "status" => "status",
                    "reason" => "reason",
                    "code" => "code",
                    "message" => "message",
                    "details" => "details",
                    _ => "other",
                })
                .collect::<std::collections::BTreeSet<_>>()
        })
        .unwrap_or_default();
    let items = parsed.as_ref().and_then(|v| v.get("items"));
    let rows = items.and_then(Value::as_array);
    let matched = rows.and_then(|rows| {
        selector
            .and_then(Value::as_object)
            .filter(|v| !v.is_empty())
            .map(|selector| {
                rows.iter()
                    .filter(|pod| {
                        pod["metadata"]["namespace"] == namespace
                            && selector
                                .iter()
                                .all(|(k, v)| pod["metadata"]["labels"][k] == *v)
                    })
                    .count()
            })
    });
    json!({"stage":stage,"returnCode":code,"category":category,"stdoutCapturedBytes":out.len(),"stdoutSHA256":format!("{:x}",Sha256::digest(out)),"stderrCapturedBytes":err.len(),"stderrSHA256":format!("{:x}",Sha256::digest(err)),"hashComplete":!limited,"errorCategory":kubectl_forward_category(err),"topType":parsed.as_ref().map(kind).unwrap_or(if out.is_empty(){"empty"}else{"invalid"}),"topKeys":top_keys,"itemsType":items.map(kind).unwrap_or("missing"),"itemsCount":rows.map(Vec::len),"namespaceCount":rows.map(|rows|rows.iter().filter(|pod|pod["metadata"]["namespace"]==namespace).count()),"selectionCount":matched})
}
struct ForwardQueryTrace {
    name: String,
    last: std::sync::Mutex<BTreeMap<&'static str, Value>>,
}
impl ForwardQueryTrace {
    fn allowed_name(&self) -> bool {
        [
            "mail-runtime-forward",
            "mail-admin-forward",
            "mail-admin-service",
            "mail-admin-connection",
            "mail-runtime-namespace",
            "mail-prelude-installed",
            "mail-prelude-gateway401",
            "mail-prelude-enabled",
            "mail-prelude-before_namespace_entry",
            "mail-prelude-smtp_ready",
        ]
        .contains(&self.name.as_str())
    }
    fn record(&self, stage: &'static str, value: Value) {
        let first = match self.last.lock() {
            Ok(mut last) => last.insert(stage, value.clone()).is_none(),
            Err(_) => return,
        };
        if first {
            self.retain(stage, "initial", &value);
        }
    }
    fn retain(&self, stage: &'static str, phase: &'static str, value: &Value) {
        if !self.allowed_name()
            || ![
                "Service",
                "Pods",
                "Deployment",
                "ReplicaSets",
                "Owner",
                "Decision",
            ]
            .contains(&stage)
        {
            return;
        }
        let fallback;
        let value = if let Ok(raw) = serde_json::to_vec(value) {
            if raw.len() > 65536 {
                fallback = json!({"category":"projection_over_limit","bytes":raw.len(),"sha256":format!("{:x}",Sha256::digest(&raw))});
                &fallback
            } else {
                value
            }
        } else {
            return;
        };
        let path =
            Path::new("/diagnostics").join(format!("{}-{stage}-query-{phase}.json", self.name));
        if super::write_diagnostic_json(&path, value).is_err() {
            eprintln!("recovery_component mail_query_capture category=unavailable");
        }
    }
}
impl Drop for ForwardQueryTrace {
    fn drop(&mut self) {
        if let Ok(last) = self.last.lock() {
            for (stage, value) in last.iter() {
                self.retain(stage, "terminal", value);
            }
        }
    }
}
#[test]
fn forward_query_shapes_and_failures_are_distinct_without_exposing_payloads() {
    let project = |code, out: &[u8]| {
        forward_query_projection(
            "Pods",
            Some(code),
            out,
            b"",
            "owned",
            Some(&json!({"owned":"yes"})),
        )
    };
    assert_eq!(project(1, b"{} ")["category"], "exit_failed");
    assert_eq!(project(0, b"not-json")["category"], "invalid_json");
    let empty = project(0, br#"{"apiVersion":"v1","kind":"List","items":[]}"#);
    assert_eq!(empty["itemsType"], "array");
    assert_eq!(empty["itemsCount"], 0);
    let null = project(0, br#"{"apiVersion":"v1","kind":"List","items":null}"#);
    assert_eq!(null["itemsType"], "null");
    assert!(null["itemsCount"].is_null());
    let oversized = project(0, &vec![b'x'; 1048577]);
    assert_eq!(oversized["category"], "over_limit");
    assert_eq!(oversized["hashComplete"], false);
    let poison=project(0,br#"{"apiVersion":"v1","kind":"List","Authorization":"poison-key","items":[{"metadata":{"namespace":"owned","labels":{"owned":"yes"}},"spec":{"env":[{"value":"poison-secret"}]}}]}"#);
    assert_eq!(poison["selectionCount"], 1);
    assert!(!poison.to_string().contains("poison"));
    assert!(!poison.to_string().contains("Authorization"));
}

async fn forward_gate_kube(
    namespace: &str,
    kind: &str,
    name: Option<&str>,
    deadline: tokio::time::Instant,
    trace: &ForwardQueryTrace,
    selector: Option<&Value>,
) -> Value {
    use tokio::io::AsyncReadExt;
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut code = None;
    let mut execution = "spawn_failed";
    let query = async {
        let mut command = tokio::process::Command::new("/usr/local/bin/kubectl");
        command.args(forward_query_args(namespace, kind, name)?);
        command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().ok()?;
        execution = "read_failed";
        let mut stdout = child.stdout.take()?.take(1048577);
        let mut stderr = child.stderr.take()?.take(16385);
        tokio::try_join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err)).ok()?;
        if out.len() > 1048576 || err.len() > 16384 {
            execution = "output_limit";
            return None;
        }
        execution = "wait_failed";
        let status = child.wait().await.ok()?;
        code = status.code();
        execution = if status.success() {
            "completed"
        } else {
            "exit_failed"
        };
        if !status.success() {
            return None;
        }
        forward_query_value(kind, &out)
    };
    let result = tokio::time::timeout_at(deadline, query).await;
    if result.is_err() {
        execution = "deadline";
    }
    let stage = match kind {
        "service" => "Service",
        "deployment" => "Deployment",
        "replicasets" => "ReplicaSets",
        _ => "Pods",
    };
    let mut receipt = forward_query_projection(stage, code, &out, &err, namespace, selector);
    receipt["executionCategory"] = json!(execution);
    if !["completed", "exit_failed"].contains(&execution) {
        receipt["hashComplete"] = json!(false);
    }
    trace.record(stage, receipt);
    result.ok().flatten().unwrap_or(Value::Null)
}

#[tokio::test]
async fn forward_pending_waits_before_one_spawn_and_rejects_unsafe_transitions() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let service = json!({"metadata":{"name":"runtime","namespace":"owned","uid":"service-1"},"spec":{"selector":{"owned":"yes"}}});
    let pod = |uid: &str, phase: &str| json!({"items":[{"metadata":{"namespace":"owned","uid":uid,"labels":{"owned":"yes"}},"spec":{"containers":[{"name":"runtime"}]},"status":{"phase":phase,"conditions":[{"type":"Ready","status":"True"}],"containerStatuses":[{"name":"runtime","ready":true,"state":{"running":{}}}]}}]});
    let spawns = AtomicUsize::new(0);
    let mut reads = 0;
    let result = wait_forward_pod_running(
        &service,
        None,
        None,
        tokio::time::Instant::now() + Duration::from_secs(1),
        || {
            assert_eq!(spawns.load(Ordering::SeqCst), 0);
            reads += 1;
            std::future::ready((
                service.clone(),
                pod("pod-1", if reads == 1 { "Pending" } else { "Running" }),
            ))
        },
    )
    .await;
    if result.is_ok() {
        spawns.fetch_add(1, Ordering::SeqCst);
    }
    assert_eq!(
        reads, 2,
        "old forward boundary launched while selected Pod was Pending"
    );
    assert_eq!(result.unwrap(), "pod-1");
    assert_eq!(spawns.load(Ordering::SeqCst), 1);
    for variant in ["service", "selector", "foreign", "ambiguous"] {
        let mut current = service.clone();
        let mut pods = pod("pod-1", "Running");
        match variant {
            "service" => current["metadata"]["uid"] = json!("replacement"),
            "selector" => current["spec"]["selector"] = json!({}),
            "foreign" => pods["items"][0]["metadata"]["namespace"] = json!("foreign"),
            "ambiguous" => pods["items"]
                .as_array_mut()
                .unwrap()
                .push(pod("pod-2", "Running")["items"][0].clone()),
            _ => unreachable!(),
        }
        let mut spawned = 0;
        let result = wait_forward_pod_running(
            &service,
            None,
            None,
            tokio::time::Instant::now() + Duration::from_secs(1),
            || std::future::ready((current.clone(), pods.clone())),
        )
        .await;
        if result.is_ok() {
            spawned += 1;
        }
        assert!(result.is_err());
        assert_eq!(spawned, 0);
    }
    for phase in ["Failed", "Succeeded"] {
        let mut spawned = 0;
        let result = wait_forward_pod_running(
            &service,
            None,
            None,
            tokio::time::Instant::now() + Duration::from_secs(1),
            || std::future::ready((service.clone(), pod("pod-1", phase))),
        )
        .await;
        if result.is_ok() {
            spawned += 1;
        }
        assert!(result.is_err());
        assert_eq!(spawned, 0);
    }
    let mut reads = 0;
    let result = wait_forward_pod_running(
        &service,
        None,
        None,
        tokio::time::Instant::now() + Duration::from_secs(1),
        || {
            reads += 1;
            std::future::ready((
                service.clone(),
                pod(
                    if reads == 1 {
                        "pod-1"
                    } else {
                        "pod-replacement"
                    },
                    if reads == 1 { "Pending" } else { "Running" },
                ),
            ))
        },
    )
    .await;
    assert!(result.is_err());
    let result = wait_forward_pod_running(
        &service,
        None,
        None,
        tokio::time::Instant::now() + Duration::from_millis(20),
        || std::future::ready((service.clone(), pod("pod-1", "Pending"))),
    )
    .await;
    assert_eq!(result.unwrap_err(), "forward_deadline");
}

struct ForwardChildCapture {
    child: Option<tokio::process::Child>,
    pid: Option<u32>,
    streams: PathBuf,
    receipt: PathBuf,
    recorded: bool,
}
impl ForwardChildCapture {
    fn new(name: &str, phase: &str, streams: &Path, diagnostics: &Path) -> Self {
        assert!(["mail-runtime-forward", "mail-admin-forward"].contains(&name));
        assert!(["setup", "installer", "actions"].contains(&phase));
        Self {
            child: None,
            pid: None,
            streams: streams.join(name),
            receipt: diagnostics.join(format!("{name}-{phase}-final.json")),
            recorded: false,
        }
    }
    fn adopt(&mut self, child: tokio::process::Child) {
        self.pid = child.id();
        self.child = Some(child);
    }
    fn retain(&mut self) {
        if self.recorded || self.child.is_none() {
            return;
        }
        self.recorded = true;
        let status = self.child.as_mut().unwrap().try_wait();
        let (exit, category) = match status {
            Ok(Some(status)) => (Some(status.code().unwrap_or(-1)), "exited"),
            Ok(None) => (None, "not_exited"),
            Err(_) => (None, "try_wait_unavailable"),
        };
        if self
            .receipt
            .file_name()
            .is_some_and(|v| v == "mail-runtime-forward-actions-final.json")
            && Path::new("/owned/mail-state.py").is_file()
        {
            let mut command = Command::new("/usr/bin/timeout");
            command.args([
                "--signal=TERM",
                "--kill-after=1",
                "10",
                "python3",
                "-B",
                "/owned/mail-state.py",
                "actions",
            ]);
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                super::byte_component::collect_command(command, None)
            }));
        }
        let value = json!({"pid":self.pid,"childExitCode":exit,"category":category,
            "stdout":forward_stream_receipt(&self.streams.with_extension("stdout")),
            "stderr":forward_stream_receipt(&self.streams.with_extension("stderr"))});
        if super::write_diagnostic_json(&self.receipt, &value).is_err() {
            eprintln!("recovery_component mail_forward_capture category=unavailable");
        }
    }
}
impl Drop for ForwardChildCapture {
    fn drop(&mut self) {
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.retain())).is_err() {
            eprintln!("recovery_component mail_forward_capture category=secondary_failure");
        }
    }
}

async fn join_install_with_forward_capture(
    installation: impl std::future::Future<Output = Value>,
    runtime: impl std::future::Future<Output = tokio::process::Child>,
    mut capture: ForwardChildCapture,
) -> (Value, tokio::process::Child) {
    let (installed, ()) = tokio::join!(installation, async {
        capture.adopt(runtime.await);
    });
    capture.retain();
    (
        installed,
        capture.child.take().expect("actual forward returned"),
    )
}

#[tokio::test]
async fn installer_panic_retains_actual_forward_child_status_and_redacted_streams_before_drop() {
    let temporary = tempfile::tempdir().unwrap();
    let stdout = temporary.path().join("mail-runtime-forward.stdout");
    let stderr = temporary.path().join("mail-runtime-forward.stderr");
    private_bytes(&stdout, b"");
    private_bytes(&stderr, b"");
    let (ready, installed) = tokio::sync::oneshot::channel();
    let output = temporary
        .path()
        .join("mail-runtime-forward-installer-final.json");
    let capture = ForwardChildCapture::new(
        "mail-runtime-forward",
        "installer",
        temporary.path(),
        temporary.path(),
    );
    let task = tokio::spawn(join_install_with_forward_capture(
        async {
            let _ = installed.await;
            tokio::task::yield_now().await;
            panic!("controlled installer failure");
        },
        async move {
            let mut command = tokio::process::Command::new("/bin/sh");
            command
                .args([
                    "-c",
                    "printf 'private-forward-canary https://private.invalid' >&2; exit 7",
                ])
                .kill_on_drop(true)
                .stdout(Stdio::from(
                    File::options().write(true).open(stdout).unwrap(),
                ))
                .stderr(Stdio::from(
                    File::options().write(true).open(stderr).unwrap(),
                ));
            let mut child = command.spawn().unwrap();
            child.wait().await.unwrap();
            let _ = ready.send(());
            child
        },
        capture,
    ));
    assert!(task.await.unwrap_err().is_panic());
    let body = std::fs::read(&output).unwrap();
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["childExitCode"], 7);
    assert_eq!(value["stderr"]["hashComplete"], true);
    assert!(
        !String::from_utf8(body)
            .unwrap()
            .contains("private-forward-canary")
    );
}

async fn port_forward(
    namespace: &str,
    service: &str,
    port: u16,
    remote: u16,
    name: &str,
    producer: &Value,
) -> tokio::process::Child {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(40);
    let queries = ForwardQueryTrace {
        name: name.to_owned(),
        last: std::sync::Mutex::new(BTreeMap::new()),
    };
    let owner_pin = std::sync::Mutex::new(None);
    let directory = Path::new("/owned/mail-fixture");
    let before = forward_gate_kube(
        namespace,
        "service",
        Some(service),
        deadline,
        &queries,
        None,
    )
    .await;
    assert_eq!(before["metadata"]["namespace"], namespace);
    assert_eq!(before["metadata"]["name"], service);
    let expected = before["metadata"]["uid"]
        .as_str()
        .expect("real own Service UID")
        .to_owned();
    retain_forward_observation(namespace, service, &expected, name, "initial", None);
    let selected = wait_forward_pod_running(&before, None, Some(&queries), deadline, || {
        forward_read_owned(namespace, service, producer, deadline, &queries, &owner_pin)
    })
    .await;
    let pod_uid = match selected {
        Ok(uid) => uid,
        Err(category) => {
            retain_forward_observation(
                namespace,
                service,
                &expected,
                name,
                if category == "forward_deadline" {
                    "timeout"
                } else {
                    "terminal"
                },
                None,
            );
            panic!("owned forward Pod gate denied: {category}");
        }
    };
    assert!(
        tokio::time::Instant::now() < deadline,
        "owned forward deadline elapsed before spawn"
    );
    let args = [
        "--kubeconfig",
        "/etc/rancher/k3s/k3s.yaml",
        "--context",
        "default",
        "-n",
        namespace,
        "port-forward",
        "--address",
        "172.30.254.1",
        "--pod-running-timeout=40s",
    ];
    let resource = format!("service/{service}");
    let mapping = format!("{port}:{remote}");
    let mut command = tokio::process::Command::new("/usr/local/bin/kubectl");
    command
        .args(args)
        .arg(&resource)
        .arg(&mapping)
        .kill_on_drop(true)
        .stdout(Stdio::from(
            File::create(directory.join(format!("{name}.stdout"))).unwrap(),
        ))
        .stderr(Stdio::from(
            File::create(directory.join(format!("{name}.stderr"))).unwrap(),
        ));
    let mut capture = ForwardChildCapture::new(name, "setup", directory, Path::new("/diagnostics"));
    capture.adopt(command.spawn().unwrap());
    let child = capture.child.as_mut().unwrap();
    let pid = child.id().unwrap();
    let start = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let start = start
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .nth(19)
        .unwrap();
    super::write_diagnostic_json(&Path::new("/diagnostics").join(format!("{name}-custody.json")),&json!({"pid":pid,"startTime":start,"namespace":namespace,"service":service,"serviceUID":expected,"podUID":pod_uid,"bind":"172.30.254.1","port":port,"remotePort":remote})).unwrap();
    let outcome = tokio::time::timeout_at(deadline, async {
        loop {
            let exit = child.try_wait().unwrap();
            if let Some(status) = exit {
                retain_forward_observation(
                    namespace,
                    service,
                    &expected,
                    name,
                    "terminal",
                    Some(status.code().unwrap_or(-1)),
                );
            }
            assert!(exit.is_none(), "exact owned port-forward exited");
            if tokio::net::TcpStream::connect(("172.30.254.1", port))
                .await
                .is_ok()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    if outcome.is_err() {
        retain_forward_observation(
            namespace,
            service,
            &expected,
            name,
            "timeout",
            child
                .try_wait()
                .ok()
                .flatten()
                .map(|v| v.code().unwrap_or(-1)),
        );
    }
    outcome.expect("owned actual Service port-forward readiness");
    let revalidated =
        wait_forward_pod_running(&before, Some(&pod_uid), Some(&queries), deadline, || {
            forward_read_owned(namespace, service, producer, deadline, &queries, &owner_pin)
        })
        .await;
    if revalidated.is_err() {
        retain_forward_observation(
            namespace,
            service,
            &expected,
            name,
            "terminal",
            child
                .try_wait()
                .ok()
                .flatten()
                .map(|v| v.code().unwrap_or(-1)),
        );
    }
    assert_eq!(
        revalidated.as_deref(),
        Ok(pod_uid.as_str()),
        "owned forward Pod/Service binding changed"
    );
    capture.child.take().unwrap()
}
// CE deliberately rolls setup -> bootstrapped -> ready. A port-forward pins the
// first Pod and cannot implement that stable Service route across those rolls.
struct CeAdminRoute {
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}
impl CeAdminRoute {
    async fn stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        tokio::time::timeout(Duration::from_secs(2), &mut self.task)
            .await
            .unwrap()
            .unwrap();
    }
}
impl Drop for CeAdminRoute {
    fn drop(&mut self) {
        self.task.abort();
    }
}
fn ce_service_address(
    service: &Value,
    namespace: &str,
    name: &str,
    uid: &str,
) -> Result<std::net::SocketAddr, &'static str> {
    if service["apiVersion"] != "v1"
        || service["kind"] != "Service"
        || service["metadata"]["namespace"] != namespace
        || service["metadata"]["name"] != name
        || service["metadata"]["uid"] != uid
        || !service["metadata"]["deletionTimestamp"].is_null()
        || service["spec"]["selector"] != json!({"app":"engine"})
        || service["spec"]["type"] != "ClusterIP"
        || !service["spec"]["externalName"].is_null()
        || service["spec"]["externalIPs"]
            .as_array()
            .is_some_and(|v| !v.is_empty())
    {
        return Err("CE Service identity");
    }
    let ports = service["spec"]["ports"].as_array().ok_or("CE ports")?;
    if ports.len() != 3
        || ports
            .iter()
            .filter(|v| {
                v["name"] == "http"
                    && v["port"] == 8080
                    && v["targetPort"] == 8080
                    && v["protocol"] == "TCP"
            })
            .count()
            != 1
    {
        return Err("CE HTTP port");
    }
    let ip = service["spec"]["clusterIP"]
        .as_str()
        .ok_or("CE IP")?
        .parse::<std::net::Ipv4Addr>()
        .map_err(|_| "CE IP")?;
    if ip.octets()[..2] != [10, 43] || service["spec"]["clusterIPs"] != json!([ip.to_string()]) {
        return Err("CE own cluster IP");
    }
    Ok((ip, 8080).into())
}
fn ce_admin_route<F, Fut>(listener: tokio::net::TcpListener, backend: F) -> CeAdminRoute
where
    F: Fn() -> Fut + Send + Sync + Clone + 'static,
    Fut: std::future::Future<Output = Result<(std::net::SocketAddr, String), &'static str>>
        + Send
        + 'static,
{
    let (stop, mut stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                _ = &mut stopped => break,
                result=listener.accept() => {
                    let (mut client,_)=result.unwrap();
                    if connections.len()>=32 { drop(client); continue; }
                    let target=backend();let verify=backend.clone();
                    connections.spawn(async move {
                        let _=tokio::time::timeout(Duration::from_secs(15),async {
                            let (target,uid)=target.await.map_err(std::io::Error::other)?;
                            let connected=tokio::net::TcpStream::connect(target).await;
                            if enabled() {let _=super::write_diagnostic_json(Path::new("/diagnostics/mail-admin-first-dial.json"),&json!({"targetOwnedPodIP":target.ip().to_string(),"port":target.port(),"connected":connected.is_ok(),"ioErrno":connected.as_ref().err().and_then(std::io::Error::raw_os_error)}));}
                            let mut server=connected?;
                            if verify().await.map_err(std::io::Error::other)? != (target,uid) {return Err(std::io::Error::other("CE backend changed before forwarding"));}
                            tokio::io::copy_bidirectional(&mut client,&mut server).await
                        }).await;
                    });
                }
                Some(_) = connections.join_next(), if !connections.is_empty() => {}
            }
        }
        drop(listener);
        connections.abort_all();
        while connections.join_next().await.is_some() {}
        drop(backend);
    });
    CeAdminRoute {
        stop: Some(stop),
        task,
    }
}
#[test]
fn ce_admin_service_guard_rejects_foreign_or_external_targets() {
    let service = json!({"apiVersion":"v1","kind":"Service","metadata":{"name":"engine","namespace":"own","uid":"own-service"},"spec":{"type":"ClusterIP","selector":{"app":"engine"},"ports":[{"name":"http","port":8080,"targetPort":8080,"protocol":"TCP"},{"name":"imaps"},{"name":"submissions"}],"clusterIP":"10.43.1.2","clusterIPs":["10.43.1.2"]}});
    assert_eq!(
        ce_service_address(&service, "own", "engine", "own-service").unwrap(),
        "10.43.1.2:8080".parse().unwrap()
    );
    for pointer in [
        "/metadata/uid",
        "/metadata/namespace",
        "/spec/selector/app",
        "/spec/clusterIP",
        "/spec/ports/0/targetPort",
        "/spec/externalName",
    ] {
        let mut bad = service.clone();
        if let Some(v) = bad.pointer_mut(pointer) {
            *v = json!("foreign");
        }
        if pointer == "/spec/externalName" {
            bad["spec"]["externalName"] = json!("foreign");
        }
        assert!(
            ce_service_address(&bad, "own", "engine", "own-service").is_err(),
            "{pointer}"
        );
    }
}
#[tokio::test]
async fn ce_admin_route_survives_backend_listener_restart_and_stops_cleanly() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let first = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = first.local_addr().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local = listener.local_addr().unwrap();
    let own_socket = std::fs::read_link(format!(
        "/proc/self/fd/{}",
        std::os::fd::AsRawFd::as_raw_fd(&listener)
    ))
    .unwrap();
    let mut route = ce_admin_route(listener, move || {
        std::future::ready(Ok((target, "local-test".to_owned())))
    });
    for (listener, byte) in [
        (first, b'A'),
        (
            tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(),
            b'B',
        ),
    ] {
        // Rebind the same Service address after the previous backend has closed.
        let backend = if byte == b'A' {
            listener
        } else {
            drop(listener);
            tokio::net::TcpListener::bind(target).await.unwrap()
        };
        let serve = tokio::spawn(async move {
            let (mut s, _) = backend.accept().await.unwrap();
            s.write_all(&[byte]).await.unwrap();
        });
        let mut client = tokio::net::TcpStream::connect(local).await.unwrap();
        let mut got = [0];
        tokio::time::timeout(Duration::from_secs(1), client.read_exact(&mut got))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got, [byte]);
        drop(client);
        serve.await.unwrap();
    }
    route.stop().await;
    assert!(
        !std::fs::read_dir("/proc/self/fd")
            .unwrap()
            .filter_map(Result::ok)
            .any(|fd| std::fs::read_link(fd.path()).ok().as_ref() == Some(&own_socket)),
        "owned listener FD survived stop"
    );
}

#[tokio::test]
async fn ce_backend_uid_change_before_forwarding_never_sends_request_bytes() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let server = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = server.local_addr().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local = listener.local_addr().unwrap();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut route = ce_admin_route(listener, move || {
        let n = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        std::future::ready(Ok((
            target,
            if n == 0 { "before" } else { "changed" }.into(),
        )))
    });
    let peer = tokio::spawn(async move {
        let (mut c, _) = server.accept().await.unwrap();
        let mut body = [0; 64];
        c.read(&mut body).await.unwrap()
    });
    let mut client = tokio::net::TcpStream::connect(local).await.unwrap();
    client.write_all(b"private-test-request").await.unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), peer)
            .await
            .unwrap()
            .unwrap(),
        0
    );
    route.stop().await;
}

fn ce_pod_address(pods: &Value, selected: &str) -> Result<std::net::SocketAddr, &'static str> {
    let items = pods["items"].as_array().ok_or("CE PodList")?;
    let rows = items
        .iter()
        .filter(|v| v["metadata"]["uid"] == selected)
        .collect::<Vec<_>>();
    if rows.len() != 1
        || rows[0]["status"]["phase"] != "Running"
        || !rows[0]["metadata"]["deletionTimestamp"].is_null()
    {
        return Err("CE exact backend");
    }
    let ip = rows[0]["status"]["podIP"]
        .as_str()
        .ok_or("CE PodIP")?
        .parse::<std::net::Ipv4Addr>()
        .map_err(|_| "CE PodIP")?;
    if ip.octets()[..2] != [10, 42] {
        return Err("CE own Pod network");
    }
    Ok((ip, 8080).into())
}
fn ce_retired_replica_sets(
    expected: &Value,
    deployment: &Value,
    sets: &Value,
) -> Result<(Value, Vec<String>), &'static str> {
    if sets["apiVersion"] != "apps/v1"
        || sets["kind"] != "ReplicaSetList"
        || sets["metadata"]
            .get("continue")
            .is_some_and(|v| v.as_str() != Some(""))
    {
        return Err("CE ReplicaSetList");
    }
    let rows = sets["items"]
        .as_array()
        .filter(|v| v.len() <= 32)
        .ok_or("CE sets")?;
    let mut current = Vec::new();
    let mut history = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for set in rows {
        let uid = forward_uid(&set["metadata"]["uid"]).ok_or("CE set UID")?;
        if set["metadata"]["generation"]
            .as_u64()
            .is_none_or(|v| v == 0)
            || !seen.insert(uid)
            || !forward_list_item_type(set, "apps/v1", "ReplicaSet")
            || set["metadata"]["namespace"] != expected["namespace"]
            || !set["metadata"]["deletionTimestamp"].is_null()
        {
            return Err("CE set identity");
        }
        forward_controller(set, "Deployment", deployment)?;
        if forward_producer_matches(
            &set["spec"]["template"],
            &expected["deployment"]["spec"]["template"],
        ) {
            current.push(set.clone());
            continue;
        }
        // Only dormant history may differ from the current Core plan. No Pod is filtered out.
        if set["spec"]["replicas"] != 0
            || [
                "replicas",
                "readyReplicas",
                "availableReplicas",
                "terminatingReplicas",
            ]
            .iter()
            .any(|k| set["status"].get(*k).is_some_and(|v| v.as_u64() != Some(0)))
            || !matches!(
                set["spec"]["template"]["metadata"]["annotations"]["restarted"].as_str(),
                Some("setup" | "bootstrapped" | "ready")
            )
        {
            return Err("CE live or unknown history");
        }
        let spec = &set["spec"]["template"]["spec"];
        let containers = spec["containers"]
            .as_array()
            .ok_or("CE history containers")?;
        if containers.len() != 1
            || containers[0]["name"] != "stalwart"
            || containers[0]["image"] != CE_IMAGE
            || ["initContainers", "ephemeralContainers"].iter().any(|k| {
                spec.get(*k)
                    .is_some_and(|v| v.as_array().is_none_or(|v| !v.is_empty()))
            })
        {
            return Err("CE history image");
        }
        history.push(uid.to_owned());
    }
    let mut filtered = sets.clone();
    filtered["items"] = json!(current);
    Ok((filtered, history))
}
#[test]
fn ce_planned_roll_accepts_only_dormant_owned_same_image_history_and_never_a_history_pod() {
    let (mut expected, mut service, mut dep, mut sets, mut pods) = forward_owner_test_objects();
    for template in [
        &mut expected["deployment"]["spec"]["template"],
        &mut dep["spec"]["template"],
        &mut sets["items"][0]["spec"]["template"],
        &mut pods["items"][0],
    ] {
        template["spec"]
            .as_object_mut()
            .unwrap()
            .remove("initContainers");
        template["spec"]["containers"] = json!([{"name":"stalwart","image":CE_IMAGE}]);
        template["metadata"]["annotations"]["restarted"] = json!("ready");
    }
    pods["items"][0]["status"]["initContainerStatuses"] = json!([]);
    pods["items"][0]["status"]["containerStatuses"] =
        json!([{"name":"stalwart","restartCount":0,"ready":true,"state":{"running":{}}}]);
    let mut old = sets["items"][0].clone();
    old["metadata"]["uid"] = json!("55555555-5555-5555-5555-555555555555");
    old["metadata"]["name"] = json!("old-setup");
    old["spec"]["template"]["metadata"]["annotations"]["restarted"] = json!("setup");
    old["spec"]["replicas"] = json!(0);
    old["status"] = json!({"replicas":0});
    sets["items"].as_array_mut().unwrap().push(old.clone());
    assert!(
        forward_owner_binding(&expected, &service, &dep, &sets, &pods, &mut None).is_err(),
        "Mail runtime remains strict"
    );
    let (filtered, history) = ce_retired_replica_sets(&expected, &dep, &sets).unwrap();
    assert_eq!(history.len(), 1);
    assert!(forward_owner_binding(&expected, &service, &dep, &filtered, &pods, &mut None).is_ok());
    for pointer in [
        "/spec/replicas",
        "/status/replicas",
        "/spec/template/spec/containers/0/image",
        "/metadata/ownerReferences/0/uid",
        "/metadata/namespace",
    ] {
        let mut bad = sets.clone();
        *bad["items"][1].pointer_mut(pointer).unwrap() = if pointer.ends_with("replicas") {
            json!(1)
        } else {
            json!("foreign")
        };
        assert!(
            ce_retired_replica_sets(&expected, &dep, &bad).is_err(),
            "{pointer}"
        );
    }
    let mut old_pod = pods["items"][0].clone();
    old_pod["metadata"]["uid"] = json!("66666666-6666-6666-6666-666666666666");
    old_pod["metadata"]["ownerReferences"][0]["uid"] = old["metadata"]["uid"].clone();
    old_pod["metadata"]["ownerReferences"][0]["name"] = old["metadata"]["name"].clone();
    pods["items"].as_array_mut().unwrap().push(old_pod);
    assert!(forward_owner_binding(&expected, &service, &dep, &filtered, &pods, &mut None).is_err());
    service["metadata"]["uid"] = json!("foreign");
    assert!(forward_owner_binding(&expected, &service, &dep, &filtered, &pods, &mut None).is_err());
}

fn retain_ce_backend_receipt(path: &Path, value: &Value) {
    // Exclusive first evidence stays immutable; subsequent capture cannot block the request.
    let _ = super::write_diagnostic_json(path, value);
}
#[test]
fn ce_repeated_connection_observation_never_masks_the_real_request() {
    let own = tempfile::tempdir().unwrap();
    let path = own.path().join("first.json");
    let first = json!({"connected":true});
    super::write_diagnostic_json(&path, &first).unwrap();
    assert_eq!(
        super::write_diagnostic_json(&path, &json!({"second":true}))
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::AlreadyExists,
        "old unwrap would abort second validation"
    );
    retain_ce_backend_receipt(&path, &json!({"second":true}));
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(&path).unwrap()).unwrap(),
        first
    );
}

async fn ce_admin_backend(
    owner: &DatabaseConnection,
    customer: &str,
    expected_uid: &str,
    generation: u64,
    queries: &ForwardQueryTrace,
) -> Result<(std::net::SocketAddr, String), &'static str> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    let units=row(owner,"SELECT COALESCE(jsonb_agg(jsonb_build_object('namespace',namespace,'objects',objects)),'[]') AS value FROM webdock_auth.plugin_workload WHERE plugin_id='mail' AND customer_id=$1 AND NOT removing",vec![customer.into()]).await;
    let units = units.as_array().ok_or("CE actual producer")?;
    if units.len() != 1 {
        return Err("CE unit ownership");
    }
    let unit = &units[0];
    let namespace = unit["namespace"].as_str().ok_or("CE namespace")?;
    if namespace != format!("wd-mail-c{customer}") {
        return Err("CE customer namespace");
    }
    let objects = unit["objects"].as_array().ok_or("CE stored objects")?;
    let dep = objects
        .iter()
        .filter(|v| v["kind"] == "Deployment")
        .collect::<Vec<_>>();
    let svc = objects
        .iter()
        .filter(|v| v["kind"] == "Service")
        .collect::<Vec<_>>();
    if dep.len() != 1 || svc.len() != 1 || svc[0]["metadata"]["name"] != "engine" {
        return Err("CE producer selection");
    }
    let producer = json!({"namespace":namespace,"sourceGeneration":generation,"deployment":dep[0],"service":svc[0]});
    let owner_pin = std::sync::Mutex::new(None);
    let actual = forward_gate_kube(
        namespace,
        "service",
        Some("engine"),
        deadline,
        queries,
        None,
    )
    .await;
    let pods = forward_gate_kube(
        namespace,
        "pods",
        None,
        deadline,
        queries,
        Some(&producer["service"]["spec"]["selector"]),
    )
    .await;
    let (deployment, sets) = tokio::join!(
        forward_gate_kube(
            namespace,
            "deployment",
            Some(producer["deployment"]["metadata"]["name"].as_str().unwrap()),
            deadline,
            queries,
            None
        ),
        forward_gate_kube(namespace, "replicasets", None, deadline, queries, None)
    );
    let (current_sets, history) = ce_retired_replica_sets(&producer, &deployment, &sets)?;
    let owned = forward_owner_binding(
        &producer,
        &actual,
        &deployment,
        &current_sets,
        &pods,
        &mut owner_pin.lock().unwrap(),
    );
    let mut observation = forward_decision_metadata(&actual, &pods);
    observation["binding"] = json!(owner_pin.lock().unwrap().as_ref());
    observation["dormantReplicaSetUIDs"] = json!(history);
    observation["category"] = json!(owned.as_ref().err().copied().unwrap_or("matched"));
    queries.record("Owner", observation);
    owned?;
    ce_service_address(&actual, namespace, "engine", expected_uid)?;
    // Each connection binds the current Core-authorized CE generation; Mail runtime UID pins stay unchanged.
    let selected = wait_forward_pod_running(&actual, None, Some(queries), deadline, || {
        std::future::ready((actual.clone(), pods.clone()))
    })
    .await?;
    let target = ce_pod_address(&pods, &selected)?;
    retain_ce_backend_receipt(
        Path::new("/diagnostics/mail-admin-first-connection.json"),
        &json!({"ServiceUID":expected_uid,"PodUID":selected,"currentCoreProducerValidated":true,"backend":"owned-PodIP","sameService":true}),
    );
    Ok((target, selected))
}
#[test]
fn ce_backend_address_requires_the_exact_validated_pod_and_private_network() {
    let p = json!({"items":[{"metadata":{"uid":"owned"},"status":{"phase":"Running","podIP":"10.42.0.2"}}]});
    assert_eq!(
        ce_pod_address(&p, "owned").unwrap(),
        "10.42.0.2:8080".parse().unwrap()
    );
    assert!(ce_pod_address(&p, "foreign").is_err());
    for ip in ["127.0.0.1", "172.30.254.1", "10.43.1.2", "example.test"] {
        let mut bad = p.clone();
        bad["items"][0]["status"]["podIP"] = json!(ip);
        assert!(ce_pod_address(&bad, "owned").is_err());
    }
    let mut duplicate = p.clone();
    duplicate["items"]
        .as_array_mut()
        .unwrap()
        .push(p["items"][0].clone());
    assert!(ce_pod_address(&duplicate, "owned").is_err());
}

async fn provision_actual_customer_pvc(
    owner: DatabaseConnection,
    customer: String,
) -> (PathBuf, CeAdminRoute, Value) {
    let (namespace,pvc,service,producer_deployment,producer_service)=tokio::time::timeout(Duration::from_secs(120),async {loop {
        let units=row(&owner,"SELECT COALESCE(jsonb_agg(jsonb_build_object('namespace',namespace,'objects',objects)),'[]') AS value FROM webdock_auth.plugin_workload WHERE plugin_id='mail' AND customer_id=$1 AND NOT removing",vec![customer.clone().into()]).await;
        assert!(units.as_array().unwrap().len()<=1,"one real customer CE unit");
        if let Some(unit)=units.as_array().unwrap().first() {
            let namespace=unit["namespace"].as_str().unwrap();assert_eq!(namespace,format!("wd-mail-c{customer}"));
            let objects=unit["objects"].as_array().unwrap();
            let pvc=objects.iter().find(|o|o["kind"]=="PersistentVolumeClaim").unwrap();assert_eq!(pvc["spec"]["resources"]["requests"]["storage"],"2Gi");
            let name=pvc["metadata"]["name"].as_str().unwrap();
            let actual=kube(&["get","pvc",name,"-n",namespace,"--ignore-not-found","-o","json"]);
            let service=objects.iter().find(|o|o["kind"]=="Service").unwrap()["metadata"]["name"].as_str().unwrap();
            if !actual.is_null(){let deployment=objects.iter().find(|o|o["kind"]=="Deployment").unwrap().clone();let service_object=objects.iter().find(|o|o["kind"]=="Service").unwrap().clone();break (namespace.to_owned(),actual,service.to_owned(),deployment,service_object);}
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }}).await.expect("real Core-created customer PVC");
    let pvc_name = pvc["metadata"]["name"].as_str().unwrap();
    let pvc_uid = pvc["metadata"]["uid"].as_str().unwrap();
    assert!(pvc["spec"]["volumeName"].is_null());
    assert!(pvc["spec"]["storageClassName"].is_null() || pvc["spec"]["storageClassName"] == "");
    assert_eq!(
        pvc["metadata"]["labels"]["webdock.dev/customer-id"],
        customer
    );
    let mount = source_ce_filesystem(&customer);
    let data = mount.join("data");
    directory(&data);
    std::os::unix::fs::chown(&data, Some(65532), Some(65532)).unwrap();
    let pv_name = format!("wd-own-mail-{}", uuid::Uuid::new_v4().simple());
    assert!(kube(&["get", "pv", &pv_name, "--ignore-not-found", "-o", "json"]).is_null());
    let node =
        std::env::var("SPITZLI_RECOVERY_NODE_NAME").expect("actual measured owned Node name");
    let pv = json!({"apiVersion":"v1","kind":"PersistentVolume","metadata":{"name":pv_name,"labels":{"webdock.recovery-fixture":"matched-mail","webdock.dev/customer-id":customer}},"spec":{"capacity":{"storage":"2Gi"},"accessModes":["ReadWriteOnce"],"volumeMode":"Filesystem","persistentVolumeReclaimPolicy":"Retain","storageClassName":"","claimRef":{"namespace":namespace,"name":pvc_name,"uid":pvc_uid},"local":{"path":data},"nodeAffinity":{"required":{"nodeSelectorTerms":[{"matchExpressions":[{"key":"kubernetes.io/hostname","operator":"In","values":[node]}]}]}}}});
    super::write_diagnostic_json(Path::new("/diagnostics/mail-static-pv.json"),&json!({"state":"pending-before-create","pv":pv_name,"pvc":pvc_name,"pvcUID":pvc_uid,"namespace":namespace,"node":node,"data":data,"bytes":2*GIB,"coverage":"operator static provisioning; not WorkloadExecutor native-storage integration"})).unwrap();
    create_kube(&pv);
    tokio::time::timeout(Duration::from_secs(40), async {
        loop {
            let bound = kube(&["get", "pvc", pvc_name, "-n", &namespace, "-o", "json"]);
            assert_eq!(bound["metadata"]["uid"], pvc_uid);
            if bound["status"]["phase"] == "Bound" {
                assert_eq!(bound["spec"]["volumeName"], pv_name);
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("actual PV controller binding");
    let bound = kube(&["get", "pv", &pv_name, "-o", "json"]);
    assert_eq!(bound["spec"]["claimRef"]["uid"], pvc_uid);
    assert_eq!(bound["status"]["phase"], "Bound");
    super::write_diagnostic_json(Path::new("/diagnostics/mail-static-pv-bound.json"),&json!({"pv":pv_name,"pvUID":bound["metadata"]["uid"],"pvc":pvc_name,"pvcUID":pvc_uid,"namespace":namespace,"node":node,"data":data,"bytes":2*GIB,"reclaimPolicy":"Retain"})).unwrap();
    let generation = runtime_spec(&owner).await.runtime_generation;
    let producer = json!({"namespace":namespace,"sourceGeneration":generation,"deployment":producer_deployment,"service":producer_service});
    let deadline = tokio::time::Instant::now() + Duration::from_secs(40);
    let queries = ForwardQueryTrace {
        name: "mail-admin-service".into(),
        last: std::sync::Mutex::new(BTreeMap::new()),
    };
    let pin = std::sync::Mutex::new(None);
    let actual = forward_gate_kube(
        &namespace,
        "service",
        Some(&service),
        deadline,
        &queries,
        None,
    )
    .await;
    let uid = actual["metadata"]["uid"].as_str().unwrap().to_owned();
    wait_forward_pod_running(&actual, None, Some(&queries), deadline, || {
        forward_read_owned(&namespace, &service, &producer, deadline, &queries, &pin)
    })
    .await
    .expect("actual CE initial ownership/readiness");
    let current = forward_gate_kube(
        &namespace,
        "service",
        Some(&service),
        deadline,
        &queries,
        None,
    )
    .await;
    let target = ce_service_address(&current, &namespace, &service, &uid).unwrap();
    let listener = tokio::net::TcpListener::bind("172.30.254.1:19480")
        .await
        .unwrap();
    super::write_diagnostic_json(Path::new("/diagnostics/mail-admin-service-route.json"),&json!({"serviceUID":uid,"initialOwnerValidated":true,"route":"owned-Service-current-authorized-backend","port":8080,"authorizedCEGenerationPerConnection":true})).unwrap();
    let _ = target;
    let pinned_uid = uid.clone();
    let own_customer = customer.clone();
    let route_trace = Arc::new(ForwardQueryTrace {
        name: "mail-admin-connection".into(),
        last: std::sync::Mutex::new(BTreeMap::new()),
    });
    let relay = ce_admin_route(listener, move || {
        let owner = owner.clone();
        let customer = own_customer.clone();
        let uid = pinned_uid.clone();
        let trace = route_trace.clone();
        async move { ce_admin_backend(&owner, &customer, &uid, generation, &trace).await }
    });
    (mount, relay, current)
}

const MAIL_RUNTIME_SPEC_SQL: &str = "SELECT COALESCE((SELECT spec FROM webdock_auth.plugin_install_execution WHERE plugin_id='mail' AND spec IS NOT NULL ORDER BY runtime_generation DESC LIMIT 1),'null'::jsonb) AS value";

async fn runtime_spec(
    owner: &DatabaseConnection,
) -> spitzli_agent_protocol::plugin::PluginRuntimeSpec {
    let outcome = tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let value = row(owner, MAIL_RUNTIME_SPEC_SQL, vec![]).await;
            if !value.is_null() {
                let spec: spitzli_agent_protocol::plugin::PluginRuntimeSpec =
                    serde_json::from_value(value).unwrap();
                spec.validate().unwrap();
                return spec;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    if outcome.is_err() {
        retain_mail_install_observation(owner, None, &Value::Null, [0; 3], "runtime_expiry", 31)
            .await;
    }
    outcome.expect("real prepared Mail runtime spec")
}
pub(super) async fn forward_current_runtime(owner: DatabaseConnection) -> tokio::process::Child {
    let spec = runtime_spec(&owner).await;
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let service = kube(&[
                "get",
                "service",
                &spec.service,
                "-n",
                &spec.namespace,
                "--ignore-not-found",
                "-o",
                "json",
            ]);
            if !service.is_null() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("real candidate Mail Service");
    port_forward(
        &spec.namespace,
        &spec.service,
        19443,
        spec.port,
        "mail-runtime-forward",
        &forward_runtime_producer(&spec),
    )
    .await
}
async fn runtime_pod_namespace(owner: &DatabaseConnection) -> File {
    use std::os::unix::fs::MetadataExt;
    let spec = runtime_spec(owner).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let producer = forward_runtime_producer(&spec);
    let queries = ForwardQueryTrace {
        name: "mail-runtime-namespace".into(),
        last: std::sync::Mutex::new(BTreeMap::new()),
    };
    let owner_pin = std::sync::Mutex::new(None);
    let before = forward_gate_kube(
        &spec.namespace,
        "service",
        Some(&spec.service),
        deadline,
        &queries,
        None,
    )
    .await;
    let selected_uid = wait_forward_pod_running(&before, None, Some(&queries), deadline, || {
        forward_read_owned(
            &spec.namespace,
            &spec.service,
            &producer,
            deadline,
            &queries,
            &owner_pin,
        )
    })
    .await
    .expect("actual current owned Mail Pod");
    let (current, pods) = forward_read_owned(
        &spec.namespace,
        &spec.service,
        &producer,
        deadline,
        &queries,
        &owner_pin,
    )
    .await;
    assert_eq!(
        forward_active_pod(&current, &pods, &mut Some(selected_uid.clone()))
            .as_ref()
            .map(|v| v.as_deref()),
        Ok(Some(selected_uid.as_str())),
        "same Ready Mail Pod before CRI"
    );
    let pod = pods["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|pod| pod["metadata"]["uid"] == selected_uid)
        .expect("validated unique current Mail Pod UID");
    let uid = pod["metadata"]["uid"].as_str().unwrap();
    let container = pod["status"]["containerStatuses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "runtime")
        .unwrap();
    let id = container["containerID"]
        .as_str()
        .unwrap()
        .strip_prefix("containerd://")
        .expect("actual owned CRI container");
    assert!(id.len() == 64 && id.bytes().all(|c| c.is_ascii_hexdigit()));
    let raw = mail_command(
        "owned-runtime-CRI-metadata",
        &[
            "/usr/local/bin/crictl".into(),
            "--runtime-endpoint=unix:///run/k3s/containerd/containerd.sock".into(),
            "inspect".into(),
            id.into(),
        ],
    );
    let actual: Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(actual["status"]["id"], id);
    assert_eq!(actual["status"]["labels"]["io.kubernetes.pod.uid"], uid);
    let pid = actual["info"]["pid"].as_u64().expect("actual CRI task PID");
    assert!(pid > 1 && pid != u64::from(std::process::id()));
    let path = PathBuf::from(format!("/proc/{pid}"));
    let before = std::fs::read_to_string(path.join("stat")).unwrap();
    let namespace = File::open(path.join("ns/net")).unwrap();
    let inode = namespace.metadata().unwrap().ino();
    assert_ne!(inode, std::fs::metadata("/proc/self/ns/net").unwrap().ino());
    assert_ne!(
        inode,
        std::env::var("OWNED_HOST_NETWORK_NS")
            .unwrap()
            .parse::<u64>()
            .unwrap()
    );
    let after = std::fs::read_to_string(path.join("stat")).unwrap();
    assert_eq!(
        before
            .rsplit_once(')')
            .unwrap()
            .1
            .split_whitespace()
            .nth(19),
        after.rsplit_once(')').unwrap().1.split_whitespace().nth(19)
    );
    let current = kube(&[
        "get",
        "pod",
        pod["metadata"]["name"].as_str().unwrap(),
        "-n",
        &spec.namespace,
        "-o",
        "json",
    ]);
    assert_eq!(current["metadata"]["uid"], uid);
    super::write_diagnostic_json(Path::new("/diagnostics/mail-source-runtime-network.json"),&json!({"namespace":spec.namespace,"podUID":uid,"containerID":id,"pid":pid,"networkNamespaceInode":inode,"image":spec.image})).unwrap();
    namespace
}
pub(super) async fn start_smtp_sink(owner: &DatabaseConnection) -> tokio::process::Child {
    let namespace = runtime_pod_namespace(owner).await;
    start_smtp_sink_in_namespace(namespace).await
}
async fn start_smtp_sink_in_namespace(namespace: File) -> tokio::process::Child {
    use std::os::fd::AsRawFd;
    let argument = format!(
        "--net=/proc/{}/fd/{}",
        std::process::id(),
        namespace.as_raw_fd()
    );
    let path = Path::new("/owned/mail-fixture");
    let report = path.join("smtp-counts.json");
    let mut command = tokio::process::Command::new("/usr/bin/nsenter");
    command
        .arg(argument)
        .args(["--", "python3", "-B", "-c", SMTP_SINK])
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::from(
            File::create(path.join("smtp-sink.stdout")).unwrap(),
        ))
        .stderr(Stdio::from(
            File::create(path.join("smtp-sink.stderr")).unwrap(),
        ));
    let mut child = command.spawn().unwrap();
    use tokio::io::AsyncWriteExt;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&json!({"report":report,"port":1025})).unwrap())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            assert!(
                child.try_wait().unwrap().is_none(),
                "owned SMTP sink exited before readiness"
            );
            if report.is_file() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("actual namespace-local SMTP sink");
    child
}

fn prelude_pod_projection(pod: &Value) -> Result<Value, &'static str> {
    let policy = |v: &Value| match v.as_str() {
        Some("Always") => "Always",
        Some("OnFailure") => "OnFailure",
        Some("Never") => "Never",
        None if v.is_null() => "absent",
        _ => "invalid",
    };
    let time = |v: &Value| -> Result<Option<i64>, &'static str> {
        if v.is_null() {
            return Ok(None);
        }
        let value = v
            .as_str()
            .filter(|v| v.len() <= 40)
            .ok_or("prelude_time_invalid")?;
        sea_orm::prelude::DateTimeWithTimeZone::parse_from_rfc3339(value)
            .map(|v| Some(v.timestamp_millis()))
            .map_err(|_| "prelude_time_invalid")
    };
    let mut rows = Vec::new();
    for (kind, declared, statuses) in [
        ("init", "initContainers", "initContainerStatuses"),
        ("runtime", "containers", "containerStatuses"),
    ] {
        let declared = pod["spec"][declared]
            .as_array()
            .filter(|v| !v.is_empty() && v.len() <= 8)
            .ok_or("prelude_containers_invalid")?;
        let statuses = pod["status"][statuses]
            .as_array()
            .filter(|v| v.len() <= 8)
            .ok_or("prelude_statuses_invalid")?;
        let mut names = std::collections::BTreeSet::new();
        for (index, container) in declared.iter().enumerate() {
            let name = container["name"]
                .as_str()
                .filter(|v| v.len() <= 63 && !v.is_empty())
                .ok_or("prelude_container_name")?;
            if !names.insert(name) {
                return Err("prelude_duplicate_container");
            }
            let matched = statuses
                .iter()
                .filter(|v| v["name"] == name)
                .collect::<Vec<_>>();
            if matched.len() != 1 {
                return Err("prelude_status_correspondence");
            }
            let status = matched[0];
            let state = status["state"]
                .as_object()
                .filter(|v| v.len() == 1)
                .ok_or("prelude_state_invalid")?;
            let state_kind = match state.keys().next().map(String::as_str) {
                Some("running") => "running",
                Some("terminated") => "terminated",
                Some("waiting") => "waiting",
                _ => return Err("prelude_state_invalid"),
            };
            let rules = &container["restartPolicyRules"];
            let rule_count = if rules.is_null() {
                None
            } else {
                Some(
                    rules
                        .as_array()
                        .filter(|v| v.len() <= 8)
                        .ok_or("prelude_rules_invalid")?
                        .len(),
                )
            };
            let exit = status["state"]["terminated"]["exitCode"]
                .as_i64()
                .filter(|v| i32::try_from(*v).is_ok());
            let signal = status["state"]["terminated"]["signal"]
                .as_u64()
                .filter(|v| *v <= 128);
            let restarts = status["restartCount"]
                .as_u64()
                .filter(|v| u32::try_from(*v).is_ok())
                .ok_or("prelude_restarts_invalid")?;
            rows.push(json!({"kind":kind,"index":index,"restartPolicy":policy(&container["restartPolicy"]),"restartRulesPresent":!rules.is_null(),"restartRulesCount":rule_count,"state":state_kind,"ready":status["ready"].as_bool(),"restarts":restarts,"exitCode":exit,"signal":signal,"startedAtMs":time(status["state"]["running"].get("startedAt").unwrap_or(&status["state"]["terminated"]["startedAt"]))?,"finishedAtMs":time(&status["state"]["terminated"]["finishedAt"])?}));
        }
        if statuses.len() != names.len() {
            return Err("prelude_status_correspondence");
        }
    }
    let conditions = pod["status"]["conditions"]
        .as_array()
        .filter(|v| v.len() <= 16)
        .ok_or("prelude_conditions_invalid")?;
    let disruption = conditions
        .iter()
        .filter(|v| v["type"] == "DisruptionTarget")
        .collect::<Vec<_>>();
    if disruption.len() > 1 {
        return Err("prelude_disruption_ambiguous");
    }
    let disruption = disruption.first().map(|v| {
        let status = match v["status"].as_str() {
            Some("True") => "True",
            Some("False") => "False",
            Some("Unknown") => "Unknown",
            _ => "invalid",
        };
        let reason = match v["reason"].as_str() {
            Some("PreemptionByScheduler") => "PreemptionByScheduler",
            Some("DeletionByTaintManager") => "DeletionByTaintManager",
            Some("EvictionByEvictionAPI") => "EvictionByEvictionAPI",
            Some("DeletionByPodGC") => "DeletionByPodGC",
            Some("TerminationByKubelet") => "TerminationByKubelet",
            _ => "unknown",
        };
        json!({"status":status,"reason":reason})
    });
    let phase = match pod["status"]["phase"].as_str() {
        Some("Pending") => "Pending",
        Some("Running") => "Running",
        Some("Succeeded") => "Succeeded",
        Some("Failed") => "Failed",
        _ => "unknown",
    };
    Ok(
        json!({"category":"actual_pod_lifetime","restartPolicy":policy(&pod["spec"]["restartPolicy"]),"deleting":!pod["metadata"]["deletionTimestamp"].is_null(),"deletionAtMs":time(&pod["metadata"]["deletionTimestamp"])? ,"phase":phase,"disruptionTarget":disruption,"containers":rows}),
    )
}

fn prelude_process(pid: u64) -> Value {
    use std::os::unix::fs::MetadataExt;
    let root = PathBuf::from(format!("/proc/{pid}"));
    let start = std::fs::read_to_string(root.join("stat"))
        .ok()
        .and_then(|v| v.rsplit_once(')').map(|v| v.1.to_owned()))
        .and_then(|v| {
            v.split_whitespace()
                .nth(19)
                .and_then(|v| v.parse::<u64>().ok())
        });
    let uid = std::fs::metadata(&root).ok().map(|v| v.uid());
    let namespace = std::fs::metadata(root.join("ns/net")).ok().map(|v| v.ino());
    let cgroup = std::fs::read(root.join("cgroup"))
        .ok()
        .filter(|v| v.len() <= 4096)
        .map(|v| format!("{:x}", Sha256::digest(v)));
    json!({"pid":pid,"alive":start.is_some()&&uid.is_some()&&namespace.is_some()&&cgroup.is_some(),"startTime":start,"uid":uid,"networkNamespaceInode":namespace,"cgroupSHA256":cgroup})
}
fn prelude_lifetime_expected(value: &Value) -> bool {
    value["category"] == "actual_pod_lifetime"
        && value["restartPolicy"] == "Always"
        && value["phase"] == "Running"
        && value["deleting"] == false
        && value["disruptionTarget"].is_null()
        && value["containers"].as_array().is_some_and(|rows| {
            rows.iter().all(|v| {
                v["restartPolicy"] == "absent"
                    && v["restartRulesPresent"] == false
                    && v["restarts"] == 0
            })
        })
}
fn prelude_forward_status() -> Value {
    MAIL_INVOKE_TRACE.with(|trace| {
        let mut trace=trace.borrow_mut();
        let forward=trace.forward.as_mut().expect("same owned prelude forward");
        let status=forward.child.as_mut().unwrap().try_wait();
        let (category,exit)=match status {Ok(None)=>("not_exited",None),Ok(Some(v))=>("exited",Some(v.code().unwrap_or(-1))),Err(_)=>("unavailable",None)};
        json!({"pid":forward.pid,"category":category,"exitCode":exit,"stdout":forward_stream_receipt(&forward.streams.with_extension("stdout")),"stderr":forward_stream_receipt(&forward.streams.with_extension("stderr"))})
    })
}
struct MailPrelude {
    spec: spitzli_agent_protocol::plugin::PluginRuntimeSpec,
    uid: String,
    owner: std::sync::Mutex<Option<Value>>,
    namespace: File,
    process: Value,
    cid: String,
    deadline: tokio::time::Instant,
    completed: usize,
}
impl MailPrelude {
    const STAGES: [&'static str; 5] = [
        "installed",
        "gateway401",
        "enabled",
        "before_namespace_entry",
        "smtp_ready",
    ];
    async fn start(owner: &DatabaseConnection) -> Self {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(40);
        let spec = runtime_spec(owner).await;
        let custody: Value = serde_json::from_slice(&super::app_component::private_input(
            Path::new("/diagnostics/mail-runtime-forward-custody.json"),
            4096,
        ))
        .unwrap();
        let binding: Value = serde_json::from_slice(&super::app_component::private_input(
            Path::new("/diagnostics/mail-runtime-forward-Owner-query-terminal.json"),
            65536,
        ))
        .unwrap();
        let namespace = tokio::time::timeout_at(deadline, runtime_pod_namespace(owner))
            .await
            .expect("bounded prelude namespace pin");
        let proof: Value = serde_json::from_slice(&super::app_component::private_input(
            Path::new("/diagnostics/mail-source-runtime-network.json"),
            4096,
        ))
        .unwrap();
        let process = prelude_process(proof["pid"].as_u64().unwrap());
        super::write_diagnostic_json(Path::new("/diagnostics/mail-prelude-custody.json"),&json!({"expectedPodUID":custody["podUID"],"actualPodUID":proof["podUID"],"sameNamespace":proof["namespace"]==spec.namespace,"sameImage":proof["image"]==spec.image,"process":process,"ownerBinding":binding["binding"]})).unwrap();
        assert!(
            custody["podUID"] == proof["podUID"]
                && proof["namespace"] == spec.namespace
                && proof["image"] == spec.image,
            "prelude original pinned UID mismatch"
        );
        Self {
            spec,
            uid: proof["podUID"].as_str().unwrap().into(),
            owner: std::sync::Mutex::new(Some(binding["binding"].clone())),
            namespace,
            process,
            cid: proof["containerID"].as_str().unwrap().into(),
            deadline,
            completed: 0,
        }
    }
    async fn checkpoint(&mut self, stage: &'static str) {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            Self::STAGES.get(self.completed),
            Some(&stage),
            "prelude fixed checkpoint order"
        );
        let trace = ForwardQueryTrace {
            name: format!("mail-prelude-{stage}"),
            last: std::sync::Mutex::new(BTreeMap::new()),
        };
        let before = prelude_process(self.process["pid"].as_u64().unwrap());
        let (service, pods) = forward_read_owned(
            &self.spec.namespace,
            &self.spec.service,
            &forward_runtime_producer(&self.spec),
            self.deadline,
            &trace,
            &self.owner,
        )
        .await;
        let decision = forward_active_pod(&service, &pods, &mut Some(self.uid.clone()));
        let matched = pods["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|v| v["metadata"]["uid"] == self.uid)
            .collect::<Vec<_>>();
        let lifetime = if matched.len() == 1 {
            prelude_pod_projection(matched[0])
                .unwrap_or_else(|category| json!({"category":category}))
        } else {
            json!({"category":"pinned_pod_missing"})
        };
        let actual_cid = matched
            .first()
            .and_then(|v| v["status"]["containerStatuses"].as_array())
            .and_then(|rows| rows.iter().find(|v| v["name"] == "runtime"))
            .and_then(|v| v["containerID"].as_str())
            .and_then(|v| v.strip_prefix("containerd://"));
        let after = prelude_process(self.process["pid"].as_u64().unwrap());
        let forward = prelude_forward_status();
        let stable = before == self.process
            && after == self.process
            && after["alive"] == true
            && after["uid"] == 65532
            && after["networkNamespaceInode"] == self.namespace.metadata().unwrap().ino()
            && actual_cid == Some(self.cid.as_str());
        let passed = decision.as_ref().ok().and_then(|v| v.as_deref()) == Some(self.uid.as_str())
            && stable
            && prelude_lifetime_expected(&lifetime)
            && forward["category"] == "not_exited"
            && forward["stdout"]["hashComplete"] == true
            && forward["stderr"]["hashComplete"] == true;
        let digest = trace.last.lock().unwrap().get("Pods").unwrap()["stdoutSHA256"].clone();
        let record = json!({"scope":"genuine-mail-prelude-checkpoint-v1","stage":stage,"podUID":self.uid,"containerID":self.cid,"podQuerySHA256":digest,"sameProcess":stable,"beforeProcess":before,"afterProcess":after,"lifetime":lifetime,"forward":forward,"passed":passed});
        super::write_diagnostic_json(
            &Path::new("/diagnostics").join(format!("mail-prelude-{stage}.json")),
            &record,
        )
        .unwrap();
        assert!(
            passed,
            "same owned Mail prelude checkpoint denied at {stage}"
        );
        self.completed += 1;
    }
}

pub(super) struct Context<'a> {
    pub foundation: Foundation,
    pub app: &'a Router,
    pub console: &'a str,
    pub fixture: &'a Fixture,
    pub owner: &'a DatabaseConnection,
    pub path: &'a Path,
    pub cluster: &'a str,
    pub customer: &'a str,
    pub project: &'a str,
    pub agent: &'a mut tokio::process::Child,
    pub helper: &'a mut tokio::process::Child,
    pub agent_config: &'a mut Value,
    pub agent_config_path: &'a Path,
    pub storage_config: &'a Path,
}
async fn installed_material(owner: &DatabaseConnection) -> Value {
    use base64::Engine;
    let spec = runtime_spec(owner).await;
    let secret = kube(&[
        "get",
        "secret",
        "runtime-config",
        "-n",
        &spec.namespace,
        "-o",
        "json",
    ]);
    for (key, value) in spec.labels() {
        assert_eq!(secret["metadata"]["labels"][&key], value);
    }
    let raw = base64::engine::general_purpose::STANDARD
        .decode(secret["data"]["runtime.json"].as_str().unwrap())
        .unwrap();
    assert!(raw.len() <= 65536);
    let config: Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(config["generation"], spec.runtime_generation);
    assert_eq!(config["issuerPublicKeyHex"].as_str().unwrap().len(), 64);
    config
}
async fn verify_gateway_client_role(foundation: &Foundation, owner: &DatabaseConnection) {
    use base64::Engine;
    use tower::ServiceExt;
    let spec = runtime_spec(owner).await;
    let secret = kube(&[
        "get",
        "secret",
        "runtime-config",
        "-n",
        &spec.namespace,
        "-o",
        "json",
    ]);
    for (key, value) in spec.labels() {
        assert_eq!(secret["metadata"]["labels"][&key], value);
    }
    let files = spitzli_agent_protocol::plugin::MATERIAL_FILES
        .iter()
        .map(|name| {
            let raw = base64::engine::general_purpose::STANDARD
                .decode(
                    secret["data"][*name]
                        .as_str()
                        .expect("actual runtime material member"),
                )
                .unwrap();
            (name.to_string(), String::from_utf8(raw).unwrap())
        })
        .collect();
    let material = spitzli_agent_protocol::plugin::PluginRuntimeMaterial { files };
    assert_eq!(
        material.fingerprint().unwrap(),
        spec.material_sha256,
        "actual Core-admitted runtime material binding"
    );
    let leaf = x509_parser::pem::parse_x509_pem(material.files["runtime.crt"].as_bytes())
        .unwrap()
        .1;
    let client = &foundation.gateway_client;
    let certificate = spitzli_plugin_kit::read_file(&client.certificate, false).unwrap();
    let client_leaf = x509_parser::pem::parse_x509_pem(&certificate).unwrap().1;
    assert_eq!(
        format!("{:x}", Sha256::digest(&client_leaf.contents)),
        client.certificate_sha256
    );
    let tls = spitzli_plugin_runtime::PinnedMutualTls {
        certificate,
        private_key: spitzli_plugin_kit::read_file(&client.private_key, true).unwrap(),
        authority: spitzli_plugin_kit::read_file(&foundation.authority, false).unwrap(),
        server_certificate_sha256: format!("{:x}", Sha256::digest(&leaf.contents)),
    };
    let result=tokio::time::timeout(Duration::from_secs(10),async {
        let (channel,_)=spitzli_plugin_runtime::pinned_mtls_authenticated("https://172.30.254.1:19443".into(),spec.runtime_host,tls).await.expect("actual operator gateway pinned mutual TLS");
        let request=tonic::codegen::http::Request::builder().method("GET").uri("/v1/emails/1").body(tonic::body::Body::empty()).unwrap();
        let reply=channel.oneshot(request).await.expect("actual gateway role request");
        assert_eq!(reply.status(),StatusCode::UNAUTHORIZED,"gateway TLS role must reach actual native token authentication, without a send credential");
    }).await;
    result.expect("bounded actual gateway client-role probe");
    eprintln!("recovery_component phase=actual_gateway_client_role_observed");
}

fn helper_staging_path(agent_stage: &Path) -> PathBuf {
    let mut name = agent_stage.file_name().unwrap().to_os_string();
    name.push("-helper");
    agent_stage.with_file_name(name)
}
#[test]
fn mail_agent_and_helper_staging_are_distinct_private_siblings() {
    for path in [
        Path::new("/owned/mail-data/recovery-stage"),
        Path::new("/owned/mail-cold/stage"),
    ] {
        let helper = helper_staging_path(path);
        assert_ne!(
            helper, path,
            "Agent and helper cannot share conflicting UID ownership"
        );
        assert_eq!(helper.parent(), path.parent());
    }
}

async fn install_mail_profile(c: &mut Context<'_>, runtime: &Value) -> (Value, String) {
    use spitzli_agent::recovery::config::{
        KubernetesSource, PostgresEndpoint, RecoveryMode, RecoveryProfileConfig,
        RecoveryRuntimeConfig, RecoverySource, fingerprint,
    };
    let operator: Value = serde_json::from_slice(&super::app_component::private_input(
        Path::new("/owned/operator-recovery/inputs.json"),
        16384,
    ))
    .unwrap();
    let core = PostgresEndpoint {
        host_address: "127.0.0.1".into(),
        server_name: "localhost".into(),
        port: c.fixture.url.port().unwrap(),
        database: "webdock_operator_test".into(),
        username: "postgres".into(),
        password_file: PathBuf::from("/owned/mail-fixture/core-password"),
        authority: PathBuf::from("/owned/mail-fixture/core-tls/ca.crt"),
    };
    let metadata_uri = url::Url::parse(runtime["storageUrl"].as_str().unwrap()).unwrap();
    let metadata = PostgresEndpoint {
        host_address: "127.0.0.1".into(),
        server_name: "localhost".into(),
        port: metadata_uri.port().unwrap(),
        database: metadata_uri.path().trim_start_matches('/').into(),
        username: "postgres".into(),
        password_file: c.foundation.metadata_password.clone(),
        authority: c.foundation.authority.clone(),
    };
    let files = c.foundation.root.join("master-files");
    directory(&files);
    private_bytes(
        &files.join("database-roles"),
        &serde_json::to_vec(&c.fixture.passwords).unwrap(),
    );
    private_bytes(&files.join("core-rls-secret"), SECRET.as_bytes());
    private_bytes(
        &files.join("mail-secret-key"),
        runtime["secretKeyHex"].as_str().unwrap().as_bytes(),
    );
    private_bytes(
        &files.join("runtime-configuration"),
        &serde_json::to_vec(runtime).unwrap(),
    );
    let mut independent = BTreeMap::new();
    for name in [
        "database-roles",
        "core-rls-secret",
        "mail-secret-key",
        "runtime-configuration",
    ] {
        independent.insert(name.to_owned(), files.join(name));
    }
    // Preserve the actual retained installation encryption secret separately, never expose it publicly.
    let installation_secret = spitzli_control_plane::plugin_config::installation_secret().unwrap();
    private_bytes(
        &files.join("installation-key"),
        installation_secret.as_bytes(),
    );
    independent.insert("installation-key".into(), files.join("installation-key"));
    let custody = PathBuf::from("/owned/independent-mail-custody");
    directory(&custody);
    let staging = PathBuf::from("/owned/mail-data/recovery-stage");
    directory(&staging);
    let replay = PathBuf::from("/owned/mail-fixture/replay");
    directory(&replay);
    let verify = c.foundation.root.join("verify");
    directory(&verify);
    let actor=row(c.owner,"SELECT to_jsonb(id) AS value FROM webdock_auth.\"user\" WHERE email='operator@example.test' AND role='operator'",vec![]).await.as_str().unwrap().to_owned();
    let core_public = std::fs::read_to_string(c.path.join("recovery-signing.pub")).unwrap();
    let operator_public = String::from_utf8(super::app_component::private_input(
        Path::new("/owned/operator-recovery/operator.pub"),
        16384,
    ))
    .unwrap();
    let core_signers = c.foundation.root.join("core-signers");
    let operator_signers = c.foundation.root.join("operator-signers");
    private_bytes(
        &core_signers,
        format!(
            "webdock-core-recovery-reservation {}\nwebdock-core-recovery {}\n",
            core_public.trim(),
            core_public.trim()
        )
        .as_bytes(),
    );
    private_bytes(
        &operator_signers,
        format!("{actor} {}\n", operator_public.trim()).as_bytes(),
    );
    let profile = RecoveryProfileConfig {
        id: "mail-source".into(),
        profile: spitzli_agent_protocol::recovery::RecoveryProfile::MailInstallationV1,
        key_custody_id: operator["keyCustodyID"].as_str().unwrap().into(),
        destination: serde_json::from_value(operator["destination"].clone()).unwrap(),
        write_credentials: "/owned/operator-recovery/writer.json".into(),
        restore_credentials: "/owned/operator-recovery/reader.json".into(),
        age_recipient: operator["ageRecipient"].as_str().unwrap().into(),
        age_identity: "/owned/operator-recovery/identity.agekey".into(),
        independent_files: independent,
        source: RecoverySource::Mail {
            core: Box::new(core),
            metadata,
            runtime_labels: BTreeMap::from([("webdock.dev/plugin-id".into(), "mail".into())]),
            ce_labels: BTreeMap::from([("webdock.dev/customer-id".into(), c.customer.into())]),
        },
    };
    let helper_image = std::env::var("SPITZLI_RECOVERY_MAIL_IMAGE").unwrap();
    let mut config = serde_json::to_value(RecoveryRuntimeConfig {
        mode: RecoveryMode::BackupAndRestore,
        version: 1,
        installation: "owned-recovery".into(),
        docker_socket: PathBuf::from("/run/owned-engine.sock"),
        storage_socket: c.agent_config["runtime"]["storageSocket"]
            .as_str()
            .unwrap()
            .into(),
        staging_directory: staging.clone(),
        custody_directory: custody,
        replay_directory: replay,
        core_signers: core_signers.clone(),
        operator_signers: operator_signers.clone(),
        profiles: vec![profile],
        targets: vec![],
        kubernetes_source: Some(KubernetesSource {
            kubeconfig: c.agent_config["runtime"]["kubeconfig"]
                .as_str()
                .unwrap()
                .into(),
            context: c.agent_config["runtime"]["context"]
                .as_str()
                .unwrap()
                .into(),
            platform_id: "owned-recovery".into(),
            helper_image,
        }),
    })
    .unwrap();
    let expected = spitzli_agent::storage::StorageClient::new(
        c.agent_config["runtime"]["storageSocket"]
            .as_str()
            .unwrap()
            .into(),
    )
    .recovery_pool_identity()
    .await
    .unwrap();
    stop(c.agent).await;
    stop(c.helper).await;
    let peer = Path::new("/owned").join(format!(
        "mail-recovery-peer-{}",
        uuid::Uuid::new_v4().simple()
    ));
    directory(&peer);
    super::app_component::record_socket_directory(&peer, "before");
    let helper_staging = helper_staging_path(&staging);
    directory(&helper_staging);
    std::os::unix::fs::chown(&helper_staging, Some(65532), Some(65532)).unwrap();
    spitzli_agent::config::private_directory(&staging)
        .expect("Agent keeps its own private staging");
    assert_eq!(
        std::fs::symlink_metadata(&helper_staging).unwrap().uid(),
        65532
    );
    let socket = super::app_component::bind_trusted_socket(&peer, c.agent_config, &mut config);
    let profile: RecoveryProfileConfig =
        serde_json::from_value(config["profiles"][0].clone()).unwrap();
    let trust = c.foundation.root.join("recovery-trust.json");
    private(
        &trust,
        &json!({"version":1,"installationID":"owned-recovery","coreSigners":core_signers,"operatorSigners":operator_signers,"postgresTlsCertificate":null,"postgresTlsKey":null,"verificationDirectory":verify,"agentStagingDirectory":helper_staging,"socketDirectory":peer,"targets":[{"id":"mail-source","sha256":fingerprint(&profile).unwrap(),"nonce":null,"action":"backup","profile":"mail-installation-v1","capacityBytes":128*MIB,"volumes":{},"applicationTarget":null}]}),
    );
    let path = c.foundation.root.join("recovery.json");
    private(&path, &config);
    c.agent_config["recoveryRuntime"] = json!(path);
    private(c.agent_config_path, c.agent_config);
    *c.helper = spawn(
        "/usr/local/bin/spitzli-storage",
        &[
            "--config".as_ref(),
            c.storage_config.as_os_str(),
            "--recovery-trust".as_ref(),
            trust.as_os_str(),
            "serve".as_ref(),
            "--socket".as_ref(),
            socket.as_os_str(),
        ],
        c.path,
        "storage-trusted",
    );
    super::app_component::wait_trusted_helper(c.helper, c.path, &socket, &expected).await;
    super::app_component::record_socket_directory(&peer, "after");
    super::app_component::cli_validate(c.agent_config_path);
    let sequence=row(c.owner,"SELECT to_jsonb(last_sequence) AS value FROM webdock_auth.hosting_agent WHERE cluster_id=$1",vec![c.cluster.into()]).await.as_i64().unwrap();
    *c.agent = spawn(
        "/usr/local/bin/spitzli-agent",
        &[
            "--config".as_ref(),
            c.agent_config_path.as_os_str(),
            "run".as_ref(),
        ],
        c.path,
        "agent-profile-trusted",
    );
    let required_profile = json!({"id":profile.id,"profile":profile.profile,"sha256":fingerprint(&profile).unwrap(),"keyCustodyID":profile.key_custody_id,"productionQualified":false,"storageIdentity":expected});
    let observed = super::app_component::observation_after(
        c.owner,
        c.cluster,
        sequence,
        c.agent,
        c.helper,
        c.path,
        super::app_component::RecoveryHeartbeat::Mail(&required_profile),
    )
    .await;
    let cap = &observed["observation"]["capabilities"]["nativeRecovery"];
    assert_eq!(cap["profiles"][0]["profile"], "mail-installation-v1");
    assert_eq!(
        cap["profiles"][0]["storageIdentity"],
        serde_json::to_value(&expected).unwrap()
    );
    (config, actor)
}

const MAIL_JMAP_PROBE: &str = r#"
import base64,hashlib,http.client,json,socket,ssl,sys
phase='read_input';last_http=None
def failure(kind,error,tb):
    name=kind.__name__
    if name not in ('AssertionError','SSLCertVerificationError','SSLError','TimeoutError','ConnectionRefusedError','ConnectionResetError','OSError','KeyError','JSONDecodeError','FileNotFoundError'):name='other'
    def numeric(name):
        value=getattr(error,name,None)
        return value if type(value)is int and 0<=value<=4095 else None
    sys.stderr.write('mail_jmap_failure '+json.dumps({'phase':phase,'errorClass':name,'httpStatus':last_http,'errno':numeric('errno'),'verifyCode':numeric('verify_code')},sort_keys=True)+'\n')
sys.excepthook=failure
x=json.load(sys.stdin)
assert x['mode'] in ('create','verify')
authorization='Basic '+base64.b64encode((x['username']+':'+x['password']).encode()).decode()
def call(path,body=None,authenticated=True):
    global last_http
    last_http=None
    if x['mode']=='create':c=http.client.HTTPConnection('172.30.254.1',19480,timeout=5)
    else:
        context=ssl.create_default_context(cafile=x['ca'])
        c=http.client.HTTPSConnection('mail.owned.test',8443,context=context,timeout=5)
        c.sock=context.wrap_socket(socket.create_connection(('127.0.0.1',8443),timeout=5),server_hostname='mail.owned.test')
    headers={'Content-Type':'application/json'}
    if authenticated:headers['Authorization']=authorization
    c.request('POST' if body is not None else 'GET',path,body=None if body is None else json.dumps(body).encode(),headers=headers)
    response=c.getresponse();data=response.read(1048577);assert len(data)<=1048576;status=response.status;last_http=status;c.close()
    return status,json.loads(data) if status==200 and data else None
def jmap(method,args):
    status,response=call('/jmap',{'using':['urn:ietf:params:jmap:core','urn:ietf:params:jmap:mail'],'methodCalls':[[method,args,'m']]})
    assert status==200 and len(response['methodResponses'])==1
    method_response=response['methodResponses'][0];assert method_response[0]==method and method_response[2]=='m'
    return method_response[1]
phase='session'
status,session=call('/jmap/session');assert status==200
account=session['primaryAccounts']['urn:ietf:params:jmap:mail']
if x['mode']=='create':
    phase='mailbox_list'
    boxes=jmap('Mailbox/get',{'accountId':account})['list']
    draft=next(m for m in boxes if m.get('role')=='drafts' or m.get('name')=='Drafts')
    text='Owned matched Mail recovery body'
    phase='sentinel_create'
    created=jmap('Email/set',{'accountId':account,'create':{'sentinel':{'mailboxIds':{draft['id']:True},'keywords':{'$draft':True},'from':[{'email':x['username']}],'subject':'Owned matched Mail snapshot','textBody':[{'partId':'body','type':'text/plain'}],'bodyValues':{'body':{'value':text}}}}})
    assert created.get('created',{}).get('sentinel',{}).get('id')
    print(json.dumps({'accountID':account,'emailID':created['created']['sentinel']['id'],'bodySHA256':hashlib.sha256(text.encode()).hexdigest(),'subject':'Owned matched Mail snapshot'}))
else:
    phase='identity_check'
    assert account==x['sentinel']['accountID']
    phase='message_read'
    restored=jmap('Email/get',{'accountId':account,'ids':[x['sentinel']['emailID']],'fetchAllBodyValues':True})['list']
    phase='message_check'
    assert len(restored)==1 and restored[0]['subject']==x['sentinel']['subject']
    assert any(hashlib.sha256(v['value'].rstrip('\r\n').encode()).hexdigest()==x['sentinel']['bodySHA256'] for v in restored[0].get('bodyValues',{}).values())
    phase='anonymous_deny'
    status,_=call('/jmap/session',authenticated=False);assert status in (401,403)
    phase='write_deny'
    status,_=call('/jmap',{'using':['urn:ietf:params:jmap:mail'],'methodCalls':[['Email/set',{'accountId':account,'update':{x['sentinel']['emailID']:{'subject':'forbidden'}}},'m']]});assert status==403
    print(json.dumps({'mailboxAuthenticated':True,'bodyMatched':True,'unauthenticatedDenied':True,'writeDenied':True}))
"#;
fn source_mailbox_sentinel(mailbox: &Value) -> Value {
    let input = serde_json::to_vec(
        &json!({"mode":"create","username":mailbox["username"],"password":mailbox["password"]}),
    )
    .unwrap();
    let raw = super::byte_component::run_private(
        "python3",
        &["-c".as_ref(), MAIL_JMAP_PROBE.as_ref()],
        Some(&input),
    );
    serde_json::from_slice(&raw).expect("bounded own JMAP sentinel proof")
}

fn cli(config: &Path, args: &[&std::ffi::OsStr]) -> Vec<u8> {
    let mut values = vec!["--config".as_ref(), config.as_os_str()];
    values.extend_from_slice(args);
    super::byte_component::run_private("/usr/local/bin/spitzli-recovery", &values, None)
}
fn endpoint_uri(endpoint: &spitzli_agent::recovery::config::PostgresEndpoint) -> url::Url {
    let mut uri = url::Url::parse(&format!(
        "postgresql://postgres@localhost:{}/{}",
        endpoint.port, endpoint.database
    ))
    .unwrap();
    let password = String::from_utf8(super::app_component::private_input(
        &endpoint.password_file,
        4096,
    ))
    .unwrap();
    uri.set_password(Some(password.trim_end())).unwrap();
    uri.query_pairs_mut()
        .append_pair("sslmode", "verify-full")
        .append_pair("sslrootcert", endpoint.authority.to_str().unwrap());
    uri
}
async fn cold_restore(
    c: &mut Context<'_>,
    (source_config, actor): (&Value, &str),
    captured: &Value,
    mailbox: &Value,
    sentinel: &Value,
    sends: &Value,
    source_mount: &Path,
) {
    use spitzli_agent::recovery::config::{
        PostgresEndpoint, RecoveryMode, RecoveryProfileConfig, RecoveryRuntimeConfig,
        RecoverySource, RecoveryTarget, TargetConfiguration, fingerprint,
    };
    use spitzli_agent::storage::recovery_pool::RecoveryStorageTrust;
    use spitzli_agent::storage::{StorageClient, StorageConfig};
    let set = &captured["set"];
    let proof = &set["proof"];
    assert_eq!(
        proof["sourceResumed"], false,
        "uncertain canonical delivery must keep source closed"
    );
    assert_eq!(proof["ledger"]["accepted"], 1);
    assert_eq!(proof["ledger"]["unknown"], 1);
    let source_profile: RecoveryProfileConfig =
        serde_json::from_value(source_config["profiles"][0].clone()).unwrap();
    let (metadata_database, source_passwords) = match &source_profile.source {
        RecoverySource::Mail { core, metadata, .. } => (
            metadata.database.clone(),
            vec![core.password_file.clone(), metadata.password_file.clone()],
        ),
        RecoverySource::Ntfy { .. } => unreachable!(),
    };
    let independent = PathBuf::from("/owned/independent-mail-cold");
    directory(&independent);
    let envelope_name = proof["keyMaterial"]["name"].as_str().unwrap();
    assert!(!envelope_name.contains('/'));
    let envelope = source_config["custodyDirectory"].as_str().unwrap();
    let source_envelope = Path::new(envelope).join(envelope_name);
    let independent_envelope = independent.join(envelope_name);
    std::fs::copy(&source_envelope, &independent_envelope).unwrap();
    std::fs::set_permissions(
        &independent_envelope,
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let bytes = super::app_component::private_input(&independent_envelope, 8 * 1024 * 1024);
    assert_eq!(
        bytes.len() as u64,
        proof["keyMaterial"]["bytes"].as_u64().unwrap()
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(&bytes)),
        proof["keyMaterial"]["sha256"]
    );
    let exported = independent.join("admitted-set.json");
    private(&exported, &captured["export"]);
    stop(c.agent).await;
    stop(c.helper).await;
    if let Some(shutdown) = c.foundation.callback_stop.take() {
        let _ = shutdown.send(());
    }
    (&mut c.foundation.callback).await.unwrap();
    mail_command(
        "source-core-stop",
        &[
            "/usr/bin/docker".into(),
            "stop".into(),
            "--time".into(),
            "10".into(),
            c.fixture.container.clone(),
        ],
    );
    mail_command(
        "source-private-stop",
        &[
            "/usr/bin/docker".into(),
            "stop".into(),
            "--time".into(),
            "10".into(),
            c.foundation.metadata_container.clone(),
        ],
    );
    // Application writers were stopped by the actual Mail recovery backend before its dumps.
    let claims = kube(&[
        "get",
        "pods",
        "-n",
        &format!("wd-mail-c{}", c.customer),
        "-o",
        "json",
    ]);
    assert!(claims["items"].as_array().unwrap().is_empty());
    for mount in [
        PathBuf::from("/owned/mail-data/source/core"),
        PathBuf::from("/owned/mail-data/source/metadata"),
        source_mount.to_path_buf(),
    ] {
        mail_command(
            "source-data-unmount",
            &["/usr/bin/umount".into(), mount.to_str().unwrap().into()],
        );
    }
    for path in source_profile.independent_files.values() {
        let m = std::fs::symlink_metadata(path).unwrap();
        assert!(m.is_file() && !m.file_type().is_symlink() && m.uid() == 0 && m.nlink() == 1);
        std::fs::remove_file(path).unwrap();
    }
    for path in source_passwords {
        let m = std::fs::symlink_metadata(&path).unwrap();
        assert!(m.is_file() && !m.file_type().is_symlink() && m.uid() == 0 && m.nlink() == 1);
        std::fs::remove_file(path).unwrap();
    }
    for path in [
        PathBuf::from("/owned/operator-recovery/writer.json"),
        PathBuf::from("/operator-recovery-inputs/writer.json"),
    ] {
        let m = std::fs::symlink_metadata(&path).unwrap();
        assert!(
            m.is_file()
                && !m.file_type().is_symlink()
                && m.nlink() == 1
                && (m.uid() == 0 || m.uid() == 1000)
        );
        std::fs::remove_file(&path).unwrap();
        assert!(!path.exists());
    }
    let outage = std::time::Instant::now();
    let cold = PathBuf::from("/owned/mail-data/cold");
    directory(&cold);
    let pool = cold.join("pool");
    directory(&pool);
    let node = StorageConfig::load(c.storage_config).unwrap().node;
    let storage = cold.join("storage.json");
    private(
        &storage,
        &json!({"root":pool,"node":node,"clusterID":c.cluster,"poolID":uuid::Uuid::new_v4(),"capacityBytes":8*GIB,"reserveBytes":256*MIB}),
    );
    super::byte_component::run_private(
        "/usr/local/bin/spitzli-storage",
        &[
            "--config".as_ref(),
            storage.as_os_str(),
            "initialize".as_ref(),
        ],
        None,
    );
    let bootstrap_socket = cold.join("bootstrap.sock");
    let mut helper = spawn(
        "/usr/local/bin/spitzli-storage",
        &[
            "--config".as_ref(),
            storage.as_os_str(),
            "serve".as_ref(),
            "--socket".as_ref(),
            bootstrap_socket.as_os_str(),
        ],
        c.path,
        "storage-cold-bootstrap",
    );
    let identity = tokio::time::timeout(Duration::from_secs(40), async {
        loop {
            assert!(helper.try_wait().unwrap().is_none(), "cold helper exited");
            let client = StorageClient::new(bootstrap_socket.clone());
            if let Ok(identity) = client.recovery_pool_identity().await {
                assert_eq!(client.information().await.unwrap(), identity.configuration);
                break identity;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("actual cold Mail pool identity");
    stop(&mut helper).await;
    let tls = make_pki(&cold.join("tls"));
    let endpoint = |database: &str, name: &str| {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let password_file = cold.join(format!("{name}-password"));
        private_bytes(
            &password_file,
            format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            )
            .as_bytes(),
        );
        PostgresEndpoint {
            host_address: "127.0.0.1".into(),
            server_name: "localhost".into(),
            port,
            database: database.into(),
            username: "postgres".into(),
            password_file,
            authority: tls.authority.clone(),
        }
    };
    let core_endpoint = endpoint("webdock_operator_test", "core");
    let metadata_endpoint = endpoint(&metadata_database, "metadata");
    let target = RecoveryTarget {
        id: "mail-cold".into(),
        profile: spitzli_agent_protocol::recovery::RecoveryProfile::MailInstallationV1,
        capacity_bytes: 128 * MIB,
        nonce: uuid::Uuid::new_v4().to_string(),
        configuration: TargetConfiguration::Mail {
            core: core_endpoint.clone(),
            metadata: Box::new(metadata_endpoint.clone()),
            ce_image: CE_IMAGE.into(),
            volume_plan: MailBudget::current().target_volumes(c.customer),
        },
        tls_certificate: tls.certificate.clone(),
        tls_key: tls.private_key.clone(),
        gateway_image: std::env::var("SPITZLI_RECOVERY_GATEWAY_IMAGE").unwrap(),
    };
    let stage = cold.join("stage");
    let replay = cold.join("replay");
    let verify = cold.join("verify");
    let custody = independent.join("custody");
    for directory_path in [&stage, &replay, &verify, &custody] {
        directory(directory_path);
    }
    std::fs::copy(&independent_envelope, custody.join(envelope_name)).unwrap();
    std::fs::set_permissions(
        custody.join(envelope_name),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let mut profile = source_profile;
    profile.write_credentials = cold.join("writer-must-not-exist");
    assert!(!profile.write_credentials.exists());
    let peer =
        Path::new("/owned").join(format!("mail-cold-peer-{}", uuid::Uuid::new_v4().simple()));
    directory(&peer);
    let socket = peer.join("storage.sock");
    let config = RecoveryRuntimeConfig {
        mode: RecoveryMode::RestoreOnly,
        version: 1,
        installation: "owned-recovery".into(),
        docker_socket: "/run/owned-engine.sock".into(),
        storage_socket: socket.clone(),
        staging_directory: stage.clone(),
        custody_directory: custody,
        replay_directory: replay,
        core_signers: source_config["coreSigners"].as_str().unwrap().into(),
        operator_signers: source_config["operatorSigners"].as_str().unwrap().into(),
        profiles: vec![profile],
        targets: vec![target.clone()],
        kubernetes_source: None,
    };
    let config_path = cold.join("recovery.json");
    private(&config_path, &serde_json::to_value(&config).unwrap());
    let helper_stage = helper_staging_path(&stage);
    directory(&helper_stage);
    std::os::unix::fs::chown(&helper_stage, Some(65532), Some(65532)).unwrap();
    spitzli_agent::config::private_directory(&stage)
        .expect("Restore CLI keeps its own private staging");
    assert_eq!(
        std::fs::symlink_metadata(&helper_stage).unwrap().uid(),
        65532
    );
    let trust = RecoveryStorageTrust {
        version: 1,
        installation_id: "owned-recovery".into(),
        core_signers: config.core_signers.clone(),
        operator_signers: config.operator_signers.clone(),
        postgres_tls_certificate: Some(tls.certificate.clone()),
        postgres_tls_key: Some(tls.private_key.clone()),
        verification_directory: verify,
        agent_staging_directory: helper_stage,
        socket_directory: peer,
        targets: vec![
            spitzli_agent::storage::recovery_pool::RecoveryStorageTarget {
                id: target.id.clone(),
                sha256: fingerprint(&(target.clone(), identity.clone())).unwrap(),
                nonce: Some(target.nonce.clone()),
                action: spitzli_agent_protocol::recovery::RecoveryAction::Restore,
                profile: target.profile,
                capacity_bytes: target.capacity_bytes,
                volumes: MailBudget::current().target_volumes(c.customer),
                application_target: None,
            },
        ],
    };
    let trust_path = cold.join("trust.json");
    private(&trust_path, &serde_json::to_value(trust).unwrap());
    let metadata = |path: &Path| match std::fs::symlink_metadata(path) {
        Ok(m) => {
            json!({"present":true,"uid":m.uid(),"gid":m.gid(),"mode":m.mode()&0o777,"nlink":m.nlink(),"directory":m.is_dir(),"regular":m.is_file(),"symlink":m.file_type().is_symlink(),"canonical":std::fs::canonicalize(path).is_ok_and(|p|p==path)})
        }
        Err(_) => json!({"present":false}),
    };
    let trust_value: RecoveryStorageTrust =
        serde_json::from_slice(&super::app_component::private_input(&trust_path, 1048576)).unwrap();
    let check = spitzli_agent::storage::StoragePool::open(&storage)
        .is_ok_and(|pool| pool.with_recovery_trust(&trust_path).is_ok());
    let _ = super::write_diagnostic_json(
        Path::new("/diagnostics/mail-cold-trust-startup.json"),
        &json!({"readOnlyCurrentTrustValidation":check,"coreSigners":metadata(&trust_value.core_signers),"operatorSigners":metadata(&trust_value.operator_signers),"verification":metadata(&trust_value.verification_directory),"helperStage":metadata(&trust_value.agent_staging_directory),"socketDirectory":metadata(&trust_value.socket_directory),"trustFile":metadata(&trust_path),"targetCount":trust_value.targets.len(),"targets":trust_value.targets.iter().map(|t|json!({"capacityBytes":t.capacity_bytes,"noncePresent":t.nonce.is_some(),"volumeCount":t.volumes.len(),"volumes":t.volumes.values().map(|v|json!({"bytes":v.bytes,"uid":v.uid,"gid":v.gid,"cpuMillis":v.cpu_millicores,"memoryBytes":v.memory_bytes,"tmpBytes":v.tmp_bytes,"imageExactlyKnown":([PG_IMAGE,CE_IMAGE].contains(&v.image.as_str()))})).collect::<Vec<_>>() })).collect::<Vec<_>>() }),
    );
    helper = spawn(
        "/usr/local/bin/spitzli-storage",
        &[
            "--config".as_ref(),
            storage.as_os_str(),
            "--recovery-trust".as_ref(),
            trust_path.as_os_str(),
            "serve".as_ref(),
            "--socket".as_ref(),
            socket.as_os_str(),
        ],
        c.path,
        "storage-cold",
    );
    super::byte_component::wait_cold_helper(&config_path, &mut helper, &identity, c.path).await;
    let mut missing = serde_json::to_value(&config).unwrap();
    missing["profiles"][0]["ageIdentity"] = json!(cold.join("missing-age-identity"));
    let missing_path = cold.join("missing-key.json");
    private(&missing_path, &missing);
    super::byte_component::failed_cli(&missing_path, &["validate".as_ref()]);
    assert!(!pool.join("_pool/recovery-custody.json").exists());
    let wrong = cold.join("wrong.agekey");
    super::byte_component::run_private(
        "/usr/bin/age-keygen",
        &["-o".as_ref(), wrong.as_os_str()],
        None,
    );
    let mut bad = Command::new("/usr/bin/timeout");
    bad.args(["10", "/usr/bin/age", "--decrypt", "--identity"])
        .arg(&wrong)
        .arg("--output")
        .arg(cold.join("wrong-material"))
        .arg(&independent_envelope);
    let (bad_status, _, _) = super::byte_component::collect_command(bad, None);
    assert!(
        !bad_status.success(),
        "wrong independent operator key decrypted matched material"
    );
    assert!(!pool.join("_pool/recovery-custody.json").exists());
    cli(&config_path, &["validate".as_ref()]);
    let capsule = cold.join("capsule.json");
    cli(
        &config_path,
        &[
            "issue".as_ref(),
            "--admitted-set".as_ref(),
            exported.as_os_str(),
            "--actor".as_ref(),
            actor.as_ref(),
            "--target".as_ref(),
            target.id.as_ref(),
            "--operator-key".as_ref(),
            "/owned/operator-recovery/operator.key".as_ref(),
            "--seconds".as_ref(),
            "300".as_ref(),
            "--output".as_ref(),
            capsule.as_os_str(),
        ],
    );
    let receipt = cold.join("receipt.json");
    let arguments = [
        "restore".as_ref(),
        "--capsule".as_ref(),
        capsule.as_os_str(),
        "--target".as_ref(),
        target.id.as_ref(),
        "--receipt".as_ref(),
        receipt.as_os_str(),
    ];
    cli(&config_path, &arguments);
    let result: Value =
        serde_json::from_slice(&super::app_component::private_input(&receipt, 1048576)).unwrap();
    assert_eq!(result["dispatch"], "quarantined");
    assert_ne!(result["epoch"], result["sourceEpoch"]);
    assert_eq!(
        result["restoredLedger"],
        json!({"accepted":1,"unknown":1,"queued":0})
    );
    let local: Value = serde_json::from_slice(&super::app_component::private_input(
        &pool.join("_pool/recovery-custody.json"),
        1048576,
    ))
    .unwrap();
    let reservations: BTreeMap<
        String,
        spitzli_agent::storage::recovery_pool::RecoveryReservationReceipt,
    > = serde_json::from_value(local["reservations"].clone()).unwrap();
    assert_eq!(reservations.len(), 1);
    let reserved = reservations.values().next().unwrap();
    assert_eq!(reserved.target_nonce, target.nonce);
    assert_eq!(reserved.pool, identity);
    assert_eq!(reserved.volumes.len(), 3);
    let mut physical = Vec::new();
    for record in reserved.volumes.values() {
        let expected = MailBudget::current()
            .target_volumes(c.customer)
            .into_values()
            .find(|r| r.role == record.role)
            .unwrap();
        assert_eq!(
            (record.uid, record.gid, record.bytes),
            (expected.uid, expected.gid, expected.bytes)
        );
        let directory = std::fs::symlink_metadata(&record.data_directory).unwrap();
        assert!(
            directory.is_dir()
                && !directory.file_type().is_symlink()
                && directory.uid() == record.uid
                && directory.mode() & 0o077 == 0
        );
        let backing = record
            .data_directory
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("volume.ext4");
        let uuid = String::from_utf8(super::byte_component::run_private(
            "/usr/sbin/blkid",
            &[
                "-s".as_ref(),
                "UUID".as_ref(),
                "-o".as_ref(),
                "value".as_ref(),
                backing.as_os_str(),
            ],
            None,
        ))
        .unwrap();
        assert_eq!(uuid.trim(), record.uuid);
        physical.push(json!({"role":record.role,"resourceID":record.resource_id,"uuid":record.uuid,"bytes":record.bytes,"uid":record.uid,"gid":record.gid,"dataDirectory":record.data_directory,"backing":backing}));
    }
    let mut postgres_proofs = Vec::new();
    for record in reserved.volumes.values() {
        let endpoint = match record.role {
            RecoveryVolumeRole::CoreLedgerPostgres {} => Some(&core_endpoint),
            RecoveryVolumeRole::PrivateMailPostgres {} => Some(&metadata_endpoint),
            RecoveryVolumeRole::IncomingCe { .. } => None,
        };
        if let Some(endpoint) = endpoint {
            postgres_proofs.push(verify_pg_role_filesystem(endpoint, record, &target.nonce).await);
        }
    }
    super::write_diagnostic_json(
        Path::new("/diagnostics/mail-target-pgdata-wal-temp.json"),
        &json!({"roles":postgres_proofs}),
    )
    .unwrap();
    super::write_diagnostic_json(
        Path::new("/diagnostics/mail-target-native-volumes.json"),
        &json!({"reservationID":reserved.reservation_id,"pool":identity,"roles":physical}),
    )
    .unwrap();
    let target_core = connect(endpoint_uri(&core_endpoint).as_str())
        .await
        .unwrap();
    let epoch=row(&target_core,"SELECT jsonb_build_object('state',state,'epoch',epoch::text,'enrollments',(SELECT count(*) FROM webdock_auth.agent_transport_enrollment WHERE NOT revoked)) AS value FROM webdock_auth.native_recovery_epoch WHERE singleton",vec![]).await;
    assert_eq!(epoch["state"], "quarantined");
    assert_eq!(epoch["enrollments"], 0);
    for (id, state) in [
        (sends["known"]["id"].as_str().unwrap(), "accepted"),
        (sends["uncertain"]["id"].as_str().unwrap(), "unknown"),
    ] {
        let job=row(&target_core,"SELECT jsonb_build_object('state',state) AS value FROM webdock_mail.send_job WHERE id=$1",vec![id.into()]).await;
        assert_eq!(job["state"], state);
    }
    let ce = result["ce"].as_array().unwrap();
    assert_eq!(ce.len(), 1);
    assert_eq!(ce[0]["customerID"], c.customer);
    let probe = super::byte_component::TargetProbe {
        gateway: ce[0]["gatewayRuntimeID"].as_str().unwrap(),
        target: ce[0]["runtimeID"].as_str().unwrap(),
        image: &target.gateway_image,
        nonce: &target.nonce,
        certificate: &target.tls_certificate,
    };
    let input=serde_json::to_vec(&json!({"mode":"verify","ca":target.tls_certificate,"username":mailbox["username"],"password":mailbox["password"],"sentinel":sentinel})).unwrap();
    let verified: Value = serde_json::from_slice(&super::byte_component::in_target_network(
        &probe,
        MAIL_JMAP_PROBE,
        Some(&input),
    ))
    .unwrap();
    assert_eq!(verified["bodyMatched"], true);
    assert_eq!(verified["writeDenied"], true);
    super::byte_component::verify_target_network(&probe);
    super::byte_component::failed_cli(&config_path, &arguments);
    super::write_diagnostic_json(Path::new("/diagnostics/matched-mail-result.json"),&json!({"scope":"actual owned matched Mail workload only","completedSet":true,"sourceUnavailable":true,"corePrivateCEQuarantined":true,"oldEnrollmentDenied":true,"knownConfirmationRetained":true,"unknownDeliveryRetainedClosed":true,"mailboxAuthContentVerified":true,"readonlyNoEgress":true,"replayDenied":true,"restoreElapsedMillis":outage.elapsed().as_millis(),"nativeCoreIncoming10GiQualified":false,"staticPVSourceBytes":2*GIB,"targetCEBytes":2*GIB})).unwrap();
    target_core.close().await.unwrap();
    stop(&mut helper).await;
}

async fn install_with_runtime_forward(
    installation: impl std::future::Future<Output = Value>,
    runtime: impl std::future::Future<Output = tokio::process::Child>,
) -> (Value, tokio::process::Child) {
    // The actual watcher stays in this future: a query/forward panic cancels installation immediately.
    join_install_with_forward_capture(
        installation,
        runtime,
        ForwardChildCapture::new(
            "mail-runtime-forward",
            "installer",
            Path::new("/owned/mail-fixture"),
            Path::new("/diagnostics"),
        ),
    )
    .await
}

#[test]
fn mail_runtime_spec_query_uses_actual_frozen_execution_columns() {
    let schema = include_str!("../../../persistence/migrations/0042_plugin_installation.sql");
    let table = schema
        .split("CREATE TABLE webdock_auth.plugin_install_execution (")
        .nth(1)
        .unwrap()
        .split("\n);")
        .next()
        .unwrap();
    let order = MAIL_RUNTIME_SPEC_SQL
        .split("ORDER BY ")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap();
    for column in ["spec", "plugin_id", order] {
        assert!(
            table
                .lines()
                .any(|line| line.trim_start().starts_with(&format!("{column} "))),
            "query references a column absent from actual frozen execution DDL"
        );
    }
}

#[tokio::test]
async fn mail_runtime_watcher_query_failure_interrupts_pending_installation() {
    // A real negative query through the existing watcher; no database/identity rows or provider calls.
    let mut task = tokio::spawn(install_with_runtime_forward(
        std::future::pending::<Value>(),
        forward_current_runtime(DatabaseConnection::default()),
    ));
    let result = tokio::time::timeout(Duration::from_millis(250), &mut task).await;
    if result.is_err() {
        task.abort();
        let _ = task.await;
        panic!("actual watcher failure was hidden behind pending installer");
    }
    assert!(result.unwrap().unwrap_err().is_panic());
}

pub(super) async fn check(c: Context<'_>) {
    let trace = MailInvokeTrace::new(PathBuf::from("/diagnostics/mail-invoke-actions.json"));
    // Keep the nested integration-test state off the caller's test-thread stack.
    MAIL_INVOKE_TRACE
        .scope(std::cell::RefCell::new(trace), async {
            Box::pin(check_inner(c)).await;
            retain_mail_invocations();
        })
        .await;
}
#[test]
fn mail_fixture_entry_future_has_bounded_inline_size() {
    fn size<F, T>(_: F) -> usize
    where
        F: FnOnce(Context<'static>) -> T,
    {
        std::mem::size_of::<T>()
    }
    let bytes = size(check);
    println!(
        "Mail fixture entry={bytes}, body={} bytes",
        size(check_inner)
    );
    assert!(
        bytes < 16 * 1024,
        "Mail fixture inline future is {bytes} bytes"
    );
}
async fn check_inner(mut c: Context<'_>) {
    assert!(enabled());
    assert_eq!(c.agent_config["runtime"]["kind"], "kubernetes");
    let source_started = std::time::Instant::now();
    super::byte_component::readonly_provider_probes();
    let (_installed, runtime_forward) = install_with_runtime_forward(
        install_current_release(
            &c.foundation.app,
            c.console,
            &c.foundation.core.plugins,
            &c.foundation.worker,
            &c.foundation.descriptor,
            c.owner,
        ),
        forward_current_runtime(c.owner.clone()),
    )
    .await;
    let mut runtime_capture = ForwardChildCapture::new(
        "mail-runtime-forward",
        "actions",
        Path::new("/owned/mail-fixture"),
        Path::new("/diagnostics"),
    );
    runtime_capture.adopt(runtime_forward);
    MAIL_INVOKE_TRACE.with(|v| v.borrow_mut().forward = Some(runtime_capture));
    let _action_phase = MailPhaseCapture;
    let mut prelude = if prelude_enabled() {
        Some(MailPrelude::start(c.owner).await)
    } else {
        None
    };
    if let Some(prelude) = prelude.as_mut() {
        prelude.checkpoint("installed").await;
    }
    verify_gateway_client_role(&c.foundation, c.owner).await;
    if let Some(prelude) = prelude.as_mut() {
        prelude.checkpoint("gateway401").await;
    }
    let current=row(c.owner,"SELECT jsonb_build_object('revision',revision::text,'state',state) AS value FROM webdock_auth.plugin_installation WHERE id='mail'",vec![]).await;
    let enable=request(&c.foundation.app,CONSOLE,"POST","/api/console/plugins/installed",c.console,json!({"action":"state","pluginID":"mail","desired":"enabled","expectedRevision":current["revision"],"idempotencyKey":"matched-mail-enable"})).await;
    assert_eq!(
        enable.status,
        StatusCode::OK,
        "genuine current Mail enable admission"
    );
    tokio::time::timeout(Duration::from_secs(40),async {loop {
        if let Some(lease)=c.foundation.core.plugins.host.claim_state_operation(Some("mail")).await.unwrap(){c.foundation.core.plugins.host.advance_state_operation(&lease,c.foundation.core.plugins.as_ref()).await.unwrap();}
        let current=row(c.owner,"SELECT jsonb_build_object('state',state) AS value FROM webdock_auth.plugin_installation WHERE id='mail'",vec![]).await;
        if current["state"]=="enabled"{break;}
        tokio::time::sleep(Duration::from_millis(100)).await;
    }}).await.expect("actual signed Mail availability transition");
    if let Some(prelude) = prelude.as_mut() {
        prelude.checkpoint("enabled").await;
    }
    let offered = request(
        &c.foundation.app,
        CONSOLE,
        "PUT",
        &format!("/api/console/clusters/{}", c.cluster),
        c.console,
        json!({"offered":true}),
    )
    .await;
    assert_eq!(
        offered.status,
        StatusCode::OK,
        "actual plugin workload cluster offer"
    );
    let outgoing_owned = c.foundation.outgoing.0.clone();
    let project_owned = c.foundation.outgoing.1.clone();
    let outgoing = outgoing_owned.as_str();
    let outgoing_project = project_owned.as_str();
    let capacity = json!({"apps":4,"cpuMillicores":2000,"memoryBytes":2147483648u64,"ephemeralBytes":1073741824u64,"volumeBytes":8*GIB,"replicasPerApp":1,"concurrentDeployments":2});
    okay(request(c.app,CONSOLE,"POST",&format!("/api/console/access/customers/{outgoing}/allowances"),c.console,json!({"expectedRevision":0,"idempotencyKey":"matched-outgoing-allowance","hosting":capacity,"mailMessages":5})).await,"real finite outgoing allowance");
    for project in [c.project, outgoing_project] {
        let placed = request(
            &c.foundation.app,
            CONSOLE,
            "PUT",
            &format!("/api/console/projects/{project}/placement"),
            c.console,
            json!({"clusterId":c.cluster}),
        )
        .await;
        assert_eq!(
            placed.status,
            StatusCode::OK,
            "actual private Mail project placement"
        );
    }
    let unauthenticated = request(
        &c.foundation.app,
        CONSOLE,
        "POST",
        "/api/console/mail",
        "",
        json!({"action":"adoptConfiguredNativeSending","customerID":c.customer}),
    )
    .await;
    assert_eq!(
        unauthenticated.status,
        StatusCode::UNAUTHORIZED,
        "operator transport adoption requires actual login"
    );
    let unconfigured = request(
        &c.foundation.app,
        CONSOLE,
        "POST",
        "/api/console/mail",
        c.console,
        json!({"action":"adoptConfiguredNativeSending","customerID":"1"}),
    )
    .await;
    assert_eq!(
        unconfigured.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "operator cannot select an unconfigured account"
    );
    for customer in [c.customer, outgoing] {
        let adopted = request(
            &c.foundation.app,
            CONSOLE,
            "POST",
            "/api/console/mail",
            c.console,
            json!({"action":"adoptConfiguredNativeSending","customerID":customer}),
        )
        .await;
        assert_eq!(
            adopted.status,
            StatusCode::OK,
            "actual operator-configured transport adoption"
        );
        let repeated = request(
            &c.foundation.app,
            CONSOLE,
            "POST",
            "/api/console/mail",
            c.console,
            json!({"action":"adoptConfiguredNativeSending","customerID":customer}),
        )
        .await;
        assert_eq!(
            repeated.status,
            StatusCode::OK,
            "same configured transport adoption is idempotent"
        );
    }
    assert_outgoing_only(c.owner, outgoing).await;
    if let Some(prelude) = prelude.as_mut() {
        prelude.checkpoint("before_namespace_entry").await;
    }
    let mut smtp = if let Some(prelude) = prelude.as_ref() {
        start_smtp_sink_in_namespace(prelude.namespace.try_clone().unwrap()).await
    } else {
        start_smtp_sink(c.owner).await
    };
    if let Some(prelude) = prelude.as_mut() {
        prelude.checkpoint("smtp_ready").await;
        assert_eq!(prelude.completed, 5);
        assert!(
            MAIL_INVOKE_TRACE.with(|v| v
                .borrow()
                .rows
                .values()
                .all(|row| row.entered == 0 && row.completed == 0)),
            "prelude must precede every Mail action"
        );
        retain_mail_invocations();
        stop(&mut smtp).await;
        let mut forward = MAIL_INVOKE_TRACE
            .with(|v| v.borrow_mut().forward.take())
            .unwrap();
        stop(forward.child.as_mut().unwrap()).await;
        super::write_diagnostic_json(Path::new("/diagnostics/matched-mail-prelude.json"),&json!({"version":1,"scope":"genuine-mail-prelude-only-v1","preludeOnly":true,"passed":true,"checkpoints":5,"samePinnedUIDAndProcess":true,"mailActions":0,"actionPrefixQualified":false,"wholeMailQualified":false,"backup":false,"sourceUnavailable":false,"coldRestore":false})).unwrap();
        return;
    }
    let run = uuid::Uuid::new_v4().simple().to_string();
    let sends = confirmed_and_uncertain_send(
        &c.foundation.app,
        c.console,
        c.owner,
        outgoing_project,
        &run,
    )
    .await;
    assert_outgoing_only(c.owner, outgoing).await;
    let pvc_task = tokio::spawn(provision_actual_customer_pvc(
        c.owner.clone(),
        c.customer.into(),
    ));
    let mailbox = prepare_source_mailbox(&c.foundation.app, c.console, c.project, &run).await;
    let (source_mount, mut admin_forward, service_before) = pvc_task.await.unwrap();
    let ce_namespace = format!("wd-mail-c{}", c.customer);
    let ce_uid = service_before["metadata"]["uid"].as_str().unwrap();
    let service_after = kube(&[
        "get",
        "service",
        "engine",
        "-n",
        &ce_namespace,
        "-o",
        "json",
    ]);
    assert_eq!(
        ce_service_address(&service_after, &ce_namespace, "engine", ce_uid),
        ce_service_address(&service_before, &ce_namespace, "engine", ce_uid),
        "CE Service route changed during authorized backend rolls"
    );
    super::write_diagnostic_json(Path::new("/diagnostics/mail-admin-service-converged.json"),&json!({"sameServiceUID":true,"sameClusterIPAddress":true,"actualMailboxCredentialReturned":true,"route":"owned-Service-current-authorized-backend"})).unwrap();
    if invoke_prefix_enabled() {
        assert!(
            mailbox["username"].as_str().is_some_and(|v| !v.is_empty())
                && mailbox["password"].as_str().is_some_and(|v| !v.is_empty()),
            "actual mailbox credential returned"
        );
        let sink: Value = serde_json::from_slice(&super::app_component::private_input(
            Path::new("/owned/mail-fixture/smtp-counts.json"),
            16384,
        ))
        .unwrap();
        assert_eq!(sink["received"], 2);
        assert_eq!(sink["confirmed"], 1);
        assert_eq!(sink["closedBeforeAck"], 1);
        assert!(sink["failure"].is_null());
        let complete = MAIL_INVOKE_TRACE.with(|v| v.borrow().complete());
        assert!(complete, "all genuine signed action roles completed");
        retain_mail_invocations();
        stop(&mut smtp).await;
        admin_forward.stop().await;
        let mut runtime_capture = MAIL_INVOKE_TRACE
            .with(|v| v.borrow_mut().forward.take())
            .unwrap();
        runtime_capture.retain();
        stop(runtime_capture.child.as_mut().unwrap()).await;
        super::write_diagnostic_json(Path::new("/diagnostics/matched-mail-invoke-prefix.json"),&json!({"version":1,"scope":"genuine-signed-mail-action-prefix-v1","prefixOnly":true,"wholeMailQualified":false,"allRolesCompleted200":true,"logicalRoles":8,"providerReceived":2,"providerConfirmed":1,"providerUncertainClosed":1,"cePVCBytes":2*GIB,"credentialReturned":true,"backup":false,"sourceUnavailable":false,"coldRestore":false})).unwrap();
        return;
    }
    let sentinel = source_mailbox_sentinel(&mailbox);
    let mut sink: Value = serde_json::from_slice(&super::app_component::private_input(
        Path::new("/owned/mail-fixture/smtp-counts.json"),
        16384,
    ))
    .unwrap();
    assert_eq!(sink["received"], 2);
    assert_eq!(sink["confirmed"], 1);
    assert_eq!(sink["closedBeforeAck"], 1);
    assert!(sink["failure"].is_null());
    // These actual provider effects are complete; keeping canonical unknown does not depend on PID absence.
    stop(&mut smtp).await;
    sink = serde_json::from_slice(&super::app_component::private_input(
        Path::new("/owned/mail-fixture/smtp-counts.json"),
        16384,
    ))
    .unwrap();
    assert_eq!(sink["received"], 2);
    assert!(sink["failure"].is_null());
    let runtime = installed_material(c.owner).await;
    let (profile, actor) = install_mail_profile(&mut c, &runtime).await;
    let captured = matched_backup(&c.foundation.app, c.console, c.owner, c.cluster, c.path).await;
    assert_eq!(captured["set"]["proof"]["ce"].as_array().unwrap().len(), 1);
    assert_eq!(captured["set"]["proof"]["ce"][0]["customerID"], c.customer);
    super::write_diagnostic_json(
        Path::new("/diagnostics/ntfy-byte-objects.json"),
        &json!({"version":1,"objects":captured["set"]["proof"]["objects"]}),
    )
    .unwrap();
    super::write_diagnostic_json(Path::new("/diagnostics/mail-source-provider-counts.json"),&json!({"received":sink["received"],"confirmed":sink["confirmed"],"uncertainAcceptedClosed":sink["closedBeforeAck"],"knownID":sends["known"]["id"],"unknownID":sends["uncertain"]["id"]})).unwrap();
    admin_forward.stop().await;
    let mut runtime_capture = MAIL_INVOKE_TRACE
        .with(|v| v.borrow_mut().forward.take())
        .unwrap();
    runtime_capture.retain();
    stop(runtime_capture.child.as_mut().unwrap()).await;
    cold_restore(
        &mut c,
        (&profile, &actor),
        &captured,
        &mailbox,
        &sentinel,
        &sends,
        &source_mount,
    )
    .await;
    let after: Value = serde_json::from_slice(&super::app_component::private_input(
        Path::new("/owned/mail-fixture/smtp-counts.json"),
        16384,
    ))
    .unwrap();
    assert_eq!(
        after["received"], 2,
        "known confirmation and uncertain delivery must not replay"
    );
    super::write_diagnostic_json(Path::new("/diagnostics/matched-mail-scope.json"),&json!({"elapsedMillis":source_started.elapsed().as_millis(),"sourceLane":"signed PrivateMail2GiB PVC/operator static provisioning","nativeCoreIncoming10GiQualified":false,"publicIngressDeliveryQualified":false,"sourceCustomer":c.customer,"outgoingOnlyCustomer":outgoing,"providerReceived":2,"providerConfirmed":1,"providerUncertainClosed":1})).unwrap();
}

fn postgres_scratch_matches(image: &Value, budget: u64) -> bool {
    let Some(tmpfs) = image["HostConfig"]["Tmpfs"].as_object() else {
        return false;
    };
    if tmpfs.len() != 2
        || budget == 0
        || !budget.is_multiple_of(4)
        || image["HostConfig"]["ShmSize"].as_u64() != Some(budget / 2)
    {
        return false;
    }
    let wanted = format!(
        "rw,nodev,nosuid,noexec,size={},mode=0700,uid=70,gid=70",
        budget / 4
    );
    let wanted: std::collections::BTreeSet<_> = wanted.split(',').collect();
    ["/tmp", "/var/run/postgresql"].iter().all(|path| {
        tmpfs
            .get(*path)
            .and_then(Value::as_str)
            .is_some_and(|value| {
                let parts = value.split(',').collect::<Vec<_>>();
                let unique = parts
                    .iter()
                    .copied()
                    .collect::<std::collections::BTreeSet<_>>();
                unique.len() == parts.len() && unique == wanted
            })
    })
}
#[test]
fn postgres_scratch_checks_the_approved_aggregate_instead_of_assigning_it_to_each_mount() {
    let options = "rw,nodev,nosuid,noexec,size=16777216,mode=0700,uid=70,gid=70";
    let good = json!({"HostConfig":{"Tmpfs":{"/tmp":options,"/var/run/postgresql":options},"ShmSize":33554432}});
    assert!(
        !options.contains("size=67108864"),
        "old per-mount assertion rejects the real producer split"
    );
    assert!(postgres_scratch_matches(&good, 64 * MIB));
    for bad in [
        json!({"HostConfig":{"Tmpfs":{"/tmp":options,"/var/run/postgresql":options},"ShmSize":67108864}}),
        json!({"HostConfig":{"Tmpfs":{"/tmp":"rw,nodev,nosuid,noexec,size=67108864,mode=0700,uid=70,gid=70","/var/run/postgresql":options},"ShmSize":33554432}}),
        json!({"HostConfig":{"Tmpfs":{"/tmp":format!("{options},rw"),"/var/run/postgresql":options},"ShmSize":33554432}}),
    ] {
        assert!(!postgres_scratch_matches(&bad, 64 * MIB));
    }
}

async fn verify_pg_role_filesystem(
    endpoint: &spitzli_agent::recovery::config::PostgresEndpoint,
    record: &spitzli_agent::storage::recovery_pool::RecoveryVolumeRecord,
    nonce: &str,
) -> Value {
    let name = format!("wd-recovery-pg-{nonce}-{}", record.resource_id);
    let raw = mail_command(
        "target-pg-exact-mount-metadata",
        &["/usr/bin/docker".into(), "inspect".into(), name.clone()],
    );
    let image: Value = serde_json::from_slice(&raw).unwrap();
    let image = &image[0];
    assert_eq!(image["Name"], format!("/{name}"));
    assert_eq!(image["Config"]["User"], "70:70");
    let db = connect(endpoint_uri(endpoint).as_str()).await.unwrap();
    let tx = db.begin().await.unwrap();
    tx.execute_unprepared("CREATE TEMP TABLE webdock_recovery_fs_probe AS SELECT repeat('x',1024) AS body FROM generate_series(1,8192);").await.unwrap();
    let paths=row(&tx,"SELECT jsonb_build_object('data',current_setting('data_directory'),'temp',pg_relation_filepath('pg_temp.webdock_recovery_fs_probe'),'tempTablespaces',current_setting('temp_tablespaces'),'walFiles',(SELECT count(*) FROM pg_ls_waldir()),'externalTablespaces',(SELECT count(*) FROM pg_tablespace WHERE pg_tablespace_location(oid)<>'')) AS value",vec![]).await;
    assert_eq!(paths["tempTablespaces"], "");
    assert_eq!(paths["externalTablespaces"], 0);
    assert!(paths["walFiles"].as_u64().unwrap() > 0);
    let pgdata = Path::new(paths["data"].as_str().unwrap());
    let mounted = image["Mounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| {
            Path::new(m["Destination"].as_str().unwrap()).starts_with("/var/lib/postgresql")
                && pgdata.starts_with(m["Destination"].as_str().unwrap())
        })
        .unwrap();
    assert_eq!(mounted["Source"], record.data_directory.to_str().unwrap());
    let actual = record.data_directory.join(
        pgdata
            .strip_prefix(mounted["Destination"].as_str().unwrap())
            .unwrap(),
    );
    let actual = actual.canonicalize().unwrap();
    assert!(actual.starts_with(&record.data_directory));
    let wal = actual.join("pg_wal").canonicalize().unwrap();
    let temp = actual
        .join(paths["temp"].as_str().unwrap())
        .canonicalize()
        .unwrap();
    assert!(wal.starts_with(&record.data_directory) && temp.starts_with(&record.data_directory));
    let scratch = 64 * MIB;
    assert!(
        postgres_scratch_matches(image, scratch),
        "actual PostgreSQL scratch split differs from its approved aggregate"
    );
    let result = json!({"resourceID":record.resource_id,"filesystemUUID":record.uuid,"uid":70,"gid":70,"pgdataInsideRole":true,"walInsideRole":true,"actualTempRelationInsideRole":true,"externalTablespaces":0,"tmpfsBytes":scratch/4,"runTmpfsBytes":scratch/4,"sharedMemoryBytes":scratch/2,"scratchBudgetBytes":scratch});
    tx.rollback().await.unwrap();
    db.close().await.unwrap();
    result
}

// Build-only causal probe. Not a production CLI or an additional runtime deployment binary.
const FORWARD_TOOL_ROOT: &str = "/owned/forward-runtime";
const FORWARD_SERVER_CONFIG: &str = "/run/webdock/runtime/runtime.json";
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ForwardServerConfig {
    transport: spitzli_plugin_kit::transport::RuntimeTransport,
    core_pin: String,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ForwardProbeTarget {
    route: String,
    address: std::net::SocketAddr,
}
fn forward_tool_prepare(root: &Path) {
    use spitzli_control_plane::plugin_config::{RuntimeConnection, RuntimeTransport};
    directory(root);
    let pki = make_pki(&root.join("client"));
    let core_leaf = x509_parser::pem::parse_x509_pem(&std::fs::read(&pki.certificate).unwrap())
        .unwrap()
        .1;
    let core_pin = hex::encode(Sha256::digest(core_leaf.contents));
    let gateway = gateway_client_identity(&pki);
    let native = ed25519_dalek::SigningKey::from_bytes(&[23; 32]);
    let (policy, _) = foundation_runtime_profiles(
        &pki,
        "172.30.254.1:19444".parse().unwrap(),
        55433,
        &core_pin,
        &gateway.certificate_sha256,
        &native,
    );
    let host = format!(
        "{}.{}",
        spitzli_agent_protocol::plugin::PluginRuntimeSpec::namespace_name("owned-recovery", "mail"),
        policy["runtimeDomain"].as_str().unwrap()
    );
    let issued = spitzli_plugin_host::provisioning::tls::issue_runtime_identity(
        &std::fs::read_to_string(&pki.authority).unwrap(),
        &std::fs::read_to_string(pki.root.join("issuer.key")).unwrap(),
        &host,
    )
    .expect("actual fixture issuer");
    let staged = root.join("source");
    directory(&staged);
    let paths = Path::new("/run/webdock/runtime");
    let transport_value = json!({
        "listenTls":{"address":"0.0.0.0:8443","certificate":paths.join("runtime.crt"),"privateKey":paths.join("runtime.key"),"authority":paths.join("core-ca.crt")},
        "coreTls":{"origin":"https://172.30.254.1:19444","domain":"172.30.254.1","certificate":paths.join("runtime.crt"),"privateKey":paths.join("runtime.key"),"authority":paths.join("core-ca.crt"),"serverCertificateSha256":core_pin}
    });
    let transport: spitzli_plugin_kit::transport::RuntimeTransport =
        serde_json::from_value(transport_value.clone()).unwrap();
    transport
        .validate()
        .expect("actual callback/listener contract");
    // Serialize real transport and consumer DTOs, not handwritten wire casing.
    private_bytes(
        &staged.join("runtime.json"),
        &serde_json::to_vec(&json!({"transport":transport_value,"corePin":core_pin})).unwrap(),
    );
    private_bytes(
        &staged.join("manifest.json"),
        include_bytes!("../../../../plugins/mail/manifest.json"),
    );
    private_bytes(
        &staged.join("runtime.crt"),
        issued.certificate_pem.as_bytes(),
    );
    private_bytes(
        &staged.join("runtime.key"),
        issued.private_key_pem.as_bytes(),
    );
    for name in ["core-ca.crt", "storage-ca.crt"] {
        private_bytes(&staged.join(name), &std::fs::read(&pki.authority).unwrap());
    }
    let connection = RuntimeConnection {
        generation: 1,
        transport: RuntimeTransport::MutualTls {
            origin: "https://172.30.254.1:19443".into(),
            domain: host,
            certificate: pki.certificate,
            private_key: pki.private_key,
            authority: pki.authority,
            server_certificate_sha256: issued.certificate_sha256,
        },
        ui_origin: None,
    };
    private_bytes(
        &root.join("connection.json"),
        &serde_json::to_vec(&connection).unwrap(),
    );
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ForwardChainSegment {
    class: &'static str,
    io_errno: Option<i32>,
    tonic_code: Option<i32>,
    tls_category: Option<&'static str>,
}
fn forward_error_chain(mut error: &(dyn std::error::Error + 'static)) -> Value {
    // Never call Display/Debug, even for unknown wrappers. Keep the original typed chain in memory.
    let mut segments = Vec::new();
    loop {
        let segment = if let Some(io) = error.downcast_ref::<std::io::Error>() {
            ForwardChainSegment {
                class: "io",
                io_errno: io.raw_os_error(),
                tonic_code: None,
                tls_category: None,
            }
        } else if let Some(status) = error.downcast_ref::<tonic::Status>() {
            ForwardChainSegment {
                class: "tonic_status",
                io_errno: None,
                tonic_code: Some(status.code() as i32),
                tls_category: None,
            }
        } else if let Some(tls) = error.downcast_ref::<rustls::Error>() {
            let category = match tls {
                rustls::Error::InvalidCertificate(_) | rustls::Error::NoCertificatesPresented => {
                    "certificate"
                }
                rustls::Error::PeerIncompatible(_) | rustls::Error::NoApplicationProtocol => {
                    "protocol"
                }
                rustls::Error::PeerMisbehaved(_)
                | rustls::Error::InappropriateMessage { .. }
                | rustls::Error::InappropriateHandshakeMessage { .. }
                | rustls::Error::AlertReceived(_) => "handshake",
                _ => "unknown",
            };
            ForwardChainSegment {
                class: "rustls",
                io_errno: None,
                tonic_code: None,
                tls_category: Some(category),
            }
        } else if error.is::<tonic::transport::Error>() {
            ForwardChainSegment {
                class: "tonic_transport",
                io_errno: None,
                tonic_code: None,
                tls_category: None,
            }
        } else {
            ForwardChainSegment {
                class: "opaque",
                io_errno: None,
                tonic_code: None,
                tls_category: None,
            }
        };
        segments.push(segment);
        // io::Error::source delegates to the inner error's source; get_ref preserves
        // a terminal rustls error instead of silently skipping its typed variant.
        let next = error
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::get_ref)
            .map(|inner| inner as &(dyn std::error::Error + 'static))
            .or_else(|| error.source());
        match next {
            Some(next) if segments.len() < 8 => error = next,
            Some(_) => return json!({"segments":segments,"truncated":true}),
            None => return json!({"segments":segments,"truncated":false}),
        }
    }
}
async fn forward_probe(
    connection: &spitzli_control_plane::plugin_config::RuntimeConnection,
    route: &str,
) -> Value {
    use spitzli_plugin_runtime::wire;
    let mut report = json!({"route":route,"stage":"connect","connected":false,"describe":false,"health":false,"error":null});
    let connected =
        tokio::time::timeout(Duration::from_secs(5), connection.connect_authenticated()).await;
    let (mut client, _) = match connected {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => {
            report["error"] = forward_error_chain(error.as_ref());
            return report;
        }
        Err(_) => {
            report["error"] = json!({"deadline":true});
            return report;
        }
    };
    report["connected"] = json!(true);
    report["stage"] = json!("describe");
    match tokio::time::timeout(
        Duration::from_secs(5),
        client.describe(wire::DescribeRequest {}),
    )
    .await
    {
        Ok(Ok(response)) => {
            let manifest = spitzli_plugin_protocol::Manifest::parse(include_bytes!(
                "../../../../plugins/mail/manifest.json"
            ))
            .unwrap();
            if response.into_inner().manifest_json != manifest.as_bytes() {
                report["error"] = json!({"contract":"manifest_mismatch"});
                return report;
            }
        }
        Ok(Err(error)) => {
            report["error"] = forward_error_chain(&error);
            return report;
        }
        Err(_) => {
            report["error"] = json!({"deadline":true});
            return report;
        }
    }
    report["describe"] = json!(true);
    report["stage"] = json!("health");
    match tokio::time::timeout(
        Duration::from_secs(5),
        client.health(wire::HealthRequest {}),
    )
    .await
    {
        Ok(Ok(response)) => {
            let value = response.into_inner();
            if value.plugin_id != "mail"
                || value.version != "0.3.0"
                || value.generation != 1
                || value.protocol_version != 1
            {
                report["error"] = json!({"contract":"health_mismatch"});
                return report;
            }
        }
        Ok(Err(error)) => {
            report["error"] = forward_error_chain(&error);
            return report;
        }
        Err(_) => {
            report["error"] = json!({"deadline":true});
            return report;
        }
    }
    report["health"] = json!(true);
    report["stage"] = json!("complete");
    report
}
#[tokio::test]
#[ignore = "build-only owned forwarding comparison; named runtime window required"]
async fn forward_runtime_tool() {
    assert_eq!(
        std::env::var_os("SPITZLI_RECOVERY_COMPONENT_TEST").as_deref(),
        Some(std::ffi::OsStr::new("1"))
    );
    match std::env::var("SPITZLI_FORWARD_TOOL_MODE").as_deref() {
        Ok("prepare") => {
            assert!(!Path::new(FORWARD_TOOL_ROOT).exists());
            forward_tool_prepare(Path::new(FORWARD_TOOL_ROOT));
            println!("forward_runtime_prepared");
        }
        Ok("serve") => {
            use spitzli_plugin_runtime::{CorePeer, RemoteAuthority, Runtime};
            let bytes = spitzli_plugin_kit::read_file(Path::new(FORWARD_SERVER_CONFIG), true)
                .expect("actual staged private server configuration");
            let config: ForwardServerConfig =
                serde_json::from_slice(&bytes).expect("typed server configuration");
            config
                .transport
                .validate()
                .expect("actual transport validation");
            let authority = RemoteAuthority::new(
                config
                    .transport
                    .core_client_lazy()
                    .await
                    .expect("actual lazy callback client"),
                "mail".into(),
                1,
                vec![CorePeer::MutualTls {
                    certificate_sha256: config.core_pin,
                }],
            )
            .expect("actual CorePeer authority");
            let material = config
                .transport
                .listen_tls
                .as_ref()
                .unwrap()
                .material()
                .expect("actual listener material");
            let manifest = spitzli_plugin_protocol::Manifest::parse(include_bytes!(
                "../../../../plugins/mail/manifest.json"
            ))
            .unwrap();
            let runtime = Runtime::new(
                manifest,
                1,
                ed25519_dalek::SigningKey::from_bytes(&[23; 32]).verifying_key(),
                Arc::new(authority),
                Arc::new(TransportContractDeny),
            )
            .unwrap();
            let listener =
                tokio::net::TcpListener::bind(config.transport.listen_tls.unwrap().address)
                    .await
                    .expect("owned TLS listener");
            println!("forward_runtime_listening");
            runtime
                .serve_mtls(listener, material, async {
                    let _ = tokio::signal::ctrl_c().await;
                })
                .await
                .expect("owned standard Runtime");
        }
        Ok("probe") => {
            use spitzli_control_plane::plugin_config::{RuntimeConnection, RuntimeTransport};
            let root = Path::new(FORWARD_TOOL_ROOT);
            let mut connection: RuntimeConnection = serde_json::from_slice(
                &spitzli_plugin_kit::read_file(&root.join("connection.json"), true)
                    .expect("actual private consumer config"),
            )
            .expect("actual consumer DTO");
            let target: ForwardProbeTarget = serde_json::from_slice(
                &spitzli_plugin_kit::read_file(&root.join("target.json"), true)
                    .expect("owned bound target"),
            )
            .expect("typed target");
            assert!(
                (target.route == "direct"
                    && target.address.ip().is_ipv4()
                    && !target.address.ip().is_loopback()
                    && target.address.port() == 8443)
                    || (target.route == "forward"
                        && target.address == "172.30.254.1:19443".parse().unwrap())
            );
            let RuntimeTransport::MutualTls { origin, .. } = &mut connection.transport else {
                unreachable!()
            };
            *origin = format!("https://{}", target.address);
            // Observation is emitted regardless of connection success; lifecycle status is separate.
            println!(
                "forward_runtime_result {}",
                forward_probe(&connection, &target.route).await
            );
        }
        _ => panic!("fixed build-only tool mode required"),
    }
}
#[test]
fn forward_chain_projection_never_formats_private_errors_and_preserves_nested_errno() {
    #[derive(Debug)]
    struct Poison(std::io::Error);
    impl std::fmt::Display for Poison {
        fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            panic!("must not format private error")
        }
    }
    impl std::error::Error for Poison {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }
    let value = forward_error_chain(&Poison(std::io::Error::from_raw_os_error(111)));
    assert_eq!(value["segments"][1]["ioErrno"], 111);
    assert_eq!(value["segments"][0]["class"], "opaque");
    let denied = forward_error_chain(&tonic::Status::permission_denied("private-key-canary"));
    assert_eq!(denied["segments"][0]["tonicCode"], 7);
    assert!(!denied.to_string().contains("canary"));
    let nested = std::io::Error::other(rustls::Error::InvalidCertificate(
        rustls::CertificateError::UnknownIssuer,
    ));
    let tls = forward_error_chain(&nested);
    assert_eq!(tls["segments"][1]["tlsCategory"], "certificate");
    let unknown = forward_error_chain(&rustls::Error::General("PRIVATE_CANARY".into()));
    assert_eq!(unknown["segments"][0]["tlsCategory"], "unknown");
    assert!(!unknown.to_string().contains("PRIVATE_CANARY"));
}
#[test]
fn forward_prepare_uses_actual_consumer_serde_and_exact_six_staged_files() {
    let temporary = tempfile::tempdir().unwrap();
    forward_tool_prepare(&temporary.path().join("fixture"));
    let root = temporary.path().join("fixture");
    let names: std::collections::BTreeSet<_> = std::fs::read_dir(root.join("source"))
        .unwrap()
        .map(|v| v.unwrap().file_name())
        .collect();
    assert_eq!(
        names,
        [
            "runtime.json",
            "manifest.json",
            "runtime.crt",
            "runtime.key",
            "core-ca.crt",
            "storage-ca.crt"
        ]
        .into_iter()
        .map(std::ffi::OsString::from)
        .collect()
    );
    let config: ForwardServerConfig =
        serde_json::from_slice(&std::fs::read(root.join("source/runtime.json")).unwrap()).unwrap();
    config.transport.validate().unwrap();
    let consumer: spitzli_control_plane::plugin_config::RuntimeConnection =
        serde_json::from_slice(&std::fs::read(root.join("connection.json")).unwrap()).unwrap();
    assert_eq!(consumer.generation, 1);
    for item in std::fs::read_dir(root.join("source")).unwrap() {
        let meta = item.unwrap().metadata().unwrap();
        assert_eq!(meta.mode() & 0o777, 0o600);
        assert_eq!(meta.nlink(), 1);
    }
}

#[tokio::test]
async fn forward_terminal_history_waits_for_same_active_uid_ready() {
    let service = json!({"metadata":{"name":"runtime","namespace":"owned","uid":"s"},"spec":{"selector":{"owned":"yes"}}});
    let ready = json!({"metadata":{"namespace":"owned","uid":"active","labels":{"owned":"yes"}},"spec":{"initContainers":[{"name":"stage"}],"containers":[{"name":"runtime"}]},"status":{"phase":"Running","conditions":[{"type":"Ready","status":"True"}],"containerStatuses":[{"name":"runtime","ready":true,"restartCount":0,"state":{"running":{}}}]}});
    let mut pending = ready.clone();
    pending["status"]["phase"] = json!("Pending");
    pending["status"]["conditions"][0]["status"] = json!("False");
    let mut history = ready.clone();
    history["metadata"]["uid"] = json!("history");
    history["status"] = json!({"phase":"Succeeded","initContainerStatuses":[{"name":"stage","restartCount":0,"state":{"terminated":{"exitCode":0}}}],"containerStatuses":[{"name":"runtime","restartCount":0,"state":{"terminated":{"exitCode":0}}}]});
    let mut reads = 0;
    let result = wait_forward_pod_running(
        &service,
        None,
        None,
        tokio::time::Instant::now() + Duration::from_secs(1),
        || {
            reads += 1;
            std::future::ready((
                service.clone(),
                json!({"items":[history.clone(),if reads==1 {pending.clone()}else{ready.clone()}]}),
            ))
        },
    )
    .await;
    assert_eq!(result.as_deref(), Ok("active"));
    assert_eq!(reads, 2);
}

fn forward_owner_test_objects() -> (Value, Value, Value, Value, Value) {
    let namespace = "owned";
    let image = "localhost/owned/recovery-component@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let labels = json!({"owned":"yes","webdock.dev/runtime-generation":"7"});
    let annotations = json!({"webdock.dev/spec-sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"});
    let template = json!({"metadata":{"labels":labels,"annotations":annotations},"spec":{"initContainers":[{"name":"stage","image":image}],"containers":[{"name":"runtime","image":image}]}});
    let expected = json!({"namespace":namespace,"sourceGeneration":7,"service":{"apiVersion":"v1","kind":"Service","metadata":{"namespace":namespace,"name":"runtime","labels":labels},"spec":{"selector":labels}},"deployment":{"apiVersion":"apps/v1","kind":"Deployment","metadata":{"namespace":namespace,"name":"runtime","labels":labels,"annotations":annotations},"spec":{"replicas":1,"strategy":{"type":"Recreate"},"selector":{"matchLabels":labels},"template":template}}});
    let mut service = expected["service"].clone();
    service["metadata"]["uid"] = json!("11111111-1111-1111-1111-111111111111");
    let mut deployment = expected["deployment"].clone();
    deployment["metadata"]["uid"] = json!("22222222-2222-2222-2222-222222222222");
    deployment["metadata"]["generation"] = json!(5);
    let set = json!({"apiVersion":"apps/v1","kind":"ReplicaSet","metadata":{"namespace":namespace,"name":"runtime-hash","uid":"33333333-3333-3333-3333-333333333333","generation":1,"ownerReferences":[{"kind":"Deployment","name":"runtime","uid":deployment["metadata"]["uid"],"controller":true}]},"spec":{"template":template}});
    let mut pod = template;
    pod["apiVersion"] = json!("v1");
    pod["kind"] = json!("Pod");
    pod["metadata"]["namespace"] = json!(namespace);
    pod["metadata"]["name"] = json!("runtime-hash-active");
    pod["metadata"]["uid"] = json!("44444444-4444-4444-4444-444444444444");
    pod["metadata"]["ownerReferences"] = json!([{"kind":"ReplicaSet","name":"runtime-hash","uid":set["metadata"]["uid"],"controller":true}]);
    pod["status"] = json!({"phase":"Running","conditions":[{"type":"Ready","status":"True"}],"initContainerStatuses":[{"name":"stage","restartCount":0,"state":{"terminated":{"exitCode":0}}}],"containerStatuses":[{"name":"runtime","restartCount":0,"ready":true,"state":{"running":{}}}]});
    (
        expected,
        service,
        deployment,
        json!({"apiVersion":"apps/v1","kind":"ReplicaSetList","items":[set]}),
        json!({"apiVersion":"v1","kind":"PodList","items":[pod]}),
    )
}
#[test]
fn forward_authoritative_owner_binding_rejects_chain_image_generation_and_source_poisons() {
    let (expected, service, deployment, sets, pods) = forward_owner_test_objects();
    let mut pin = None;
    forward_owner_binding(&expected, &service, &deployment, &sets, &pods, &mut pin).unwrap();
    assert_eq!(pin.as_ref().unwrap()["sourceGeneration"], 7);
    assert_eq!(pin.as_ref().unwrap()["deploymentGeneration"], 5);
    for variant in [
        "service_uid",
        "service_selector",
        "deployment_uid",
        "deployment_generation",
        "source_generation",
        "deployment_image",
        "spec_hash",
        "rs_controller",
        "rs_image",
        "rs_uid",
        "pod_owner",
        "pod_image",
        "pod_namespace",
        "pod_duplicate",
        "pod_uid",
        "pod_owner_duplicate",
    ] {
        let (mut e, mut s, mut d, mut r, mut p) = (
            expected.clone(),
            service.clone(),
            deployment.clone(),
            sets.clone(),
            pods.clone(),
        );
        match variant {
            "service_uid" => s["metadata"]["uid"] = json!("55555555-5555-5555-5555-555555555555"),
            "service_selector" => s["spec"]["selector"] = json!({"foreign":"yes"}),
            "deployment_uid" => {
                d["metadata"]["uid"] = json!("55555555-5555-5555-5555-555555555555")
            }
            "deployment_generation" => d["metadata"]["generation"] = json!(6),
            "source_generation" => e["sourceGeneration"] = json!(8),
            "deployment_image" => {
                d["spec"]["template"]["spec"]["containers"][0]["image"] = json!("private-poison")
            }
            "spec_hash" => {
                d["spec"]["template"]["metadata"]["annotations"]["webdock.dev/spec-sha256"] =
                    json!("private-poison")
            }
            "rs_controller" => {
                r["items"][0]["metadata"]["ownerReferences"][0]["uid"] =
                    json!("55555555-5555-5555-5555-555555555555")
            }
            "rs_image" => {
                r["items"][0]["spec"]["template"]["spec"]["containers"][0]["image"] =
                    json!("private-poison")
            }
            "rs_uid" => {
                r["items"][0]["metadata"]["uid"] = json!("55555555-5555-5555-5555-555555555555")
            }
            "pod_owner" => {
                p["items"][0]["metadata"]["ownerReferences"][0]["uid"] =
                    json!("55555555-5555-5555-5555-555555555555")
            }
            "pod_image" => {
                p["items"][0]["spec"]["containers"][0]["image"] = json!("private-poison")
            }
            "pod_namespace" => p["items"][0]["metadata"]["namespace"] = json!("foreign"),
            "pod_duplicate" => {
                let duplicate = p["items"][0].clone();
                p["items"].as_array_mut().unwrap().push(duplicate);
            }
            "pod_uid" => p["items"][0]["metadata"]["uid"] = json!("private-poison"),
            "pod_owner_duplicate" => {
                let duplicate = p["items"][0]["metadata"]["ownerReferences"][0].clone();
                p["items"][0]["metadata"]["ownerReferences"]
                    .as_array_mut()
                    .unwrap()
                    .push(duplicate);
            }
            _ => unreachable!(),
        }
        assert!(
            forward_owner_binding(&e, &s, &d, &r, &p, &mut pin.clone()).is_err(),
            "owner poison accepted: {variant}"
        );
    }
}
#[test]
fn forward_history_rejects_incomplete_unknown_deleting_and_stale_bound_transitions() {
    let (_, service, _, _, pods) = forward_owner_test_objects();
    let active = pods["items"][0].clone();
    let old = "66666666-6666-6666-6666-666666666666";
    let mut history = active.clone();
    history["metadata"]["uid"] = json!(old);
    history["status"]["phase"] = json!("Succeeded");
    history["status"]["containerStatuses"][0]["state"] = json!({"terminated":{"exitCode":0}});
    assert!(forward_terminal_complete(&history).is_ok());
    for variant in [
        "unknown_phase",
        "missing_init",
        "missing_regular",
        "duplicate_status",
        "wrong_status_name",
        "not_terminated",
        "bool_exit",
        "negative_restarts",
        "bad_deletion",
    ] {
        let mut h = history.clone();
        match variant {
            "unknown_phase" => h["status"]["phase"] = json!("private-poison"),
            "missing_init" => h["status"]["initContainerStatuses"] = Value::Null,
            "missing_regular" => h["status"]["containerStatuses"] = Value::Null,
            "duplicate_status" => {
                let row = h["status"]["containerStatuses"][0].clone();
                h["status"]["containerStatuses"]
                    .as_array_mut()
                    .unwrap()
                    .push(row);
            }
            "wrong_status_name" => h["status"]["containerStatuses"][0]["name"] = json!("foreign"),
            "not_terminated" => {
                h["status"]["containerStatuses"][0]["state"] = json!({"running":{}})
            }
            "bool_exit" => {
                h["status"]["containerStatuses"][0]["state"]["terminated"]["exitCode"] = json!(true)
            }
            "negative_restarts" => h["status"]["containerStatuses"][0]["restartCount"] = json!(-1),
            "bad_deletion" => h["metadata"]["deletionTimestamp"] = json!(true),
            _ => unreachable!(),
        }
        assert!(
            forward_active_pod(&service, &json!({"items":[h,active.clone()]}), &mut None).is_err(),
            "invalid history accepted: {variant}"
        );
    }
    let pair = json!({"items":[history.clone(),active.clone()]});
    let mut pinned = Some(old.to_owned());
    assert_eq!(
        forward_active_pod(&service, &pair, &mut pinned),
        Err("pod_terminal")
    );
    assert_eq!(pinned.as_deref(), Some(old));
    let mut second = active.clone();
    second["metadata"]["uid"] = json!("77777777-7777-7777-7777-777777777777");
    assert_eq!(
        forward_active_pod(
            &service,
            &json!({"items":[active.clone(),second]}),
            &mut None
        ),
        Err("pod_ambiguous")
    );
    let mut deleting = active.clone();
    deleting["metadata"]["deletionTimestamp"] = json!("2026-10-28T00:00:00Z");
    assert_eq!(
        forward_active_pod(
            &service,
            &json!({"items":[history.clone(),deleting]}),
            &mut None
        ),
        Err("pod_deleting")
    );
    assert_eq!(
        forward_active_pod(&service, &json!({"items":[history.clone()]}), &mut None),
        Ok(None)
    );
    assert_eq!(
        forward_active_pod(
            &service,
            &json!({"items":[history]}),
            &mut Some(active["metadata"]["uid"].as_str().unwrap().to_owned())
        ),
        Err("bound_pod_missing")
    );
}
#[test]
fn forward_decision_snapshot_is_exact_bounded_and_discards_private_spec() {
    let (_, service, _, _, mut pods) = forward_owner_test_objects();
    pods["items"][0]["spec"]["containers"][0]["env"] =
        json!([{"name":"SECRET","value":"private-poison"}]);
    pods["items"][0]["status"]["containerStatuses"][0]["lastState"] =
        json!({"terminated":{"message":"private-poison"}});
    let projection = forward_decision_metadata(&service, &pods);
    let raw = serde_json::to_vec(&projection).unwrap();
    assert!(!String::from_utf8_lossy(&raw).contains("private-poison"));
    assert_eq!(projection["matchingCount"], 1);
    let trace = ForwardQueryTrace {
        name: "unit-no-files".into(),
        last: std::sync::Mutex::new(BTreeMap::new()),
    };
    trace.record("Decision", projection.clone());
    assert_eq!(trace.last.lock().unwrap()["Decision"], projection);
    let mut rows = Vec::new();
    for n in 0..32 {
        let mut p = pods["items"][0].clone();
        p["metadata"]["uid"] = json!(uuid::Uuid::from_u128(n + 1).to_string());
        rows.push(p);
    }
    let bounded = forward_decision_metadata(&service, &json!({"items":rows}));
    assert!(serde_json::to_vec(&bounded).unwrap().len() < 65536);
    let initial = json!({"phase":"initial","serviceUID":service["metadata"]["uid"],"childExitCode":null,"stdout":{"category":"file_unavailable","hashComplete":false},"stderr":{"category":"file_unavailable","hashComplete":false},"pod":forward_pod_projection(&service,&json!({"items":[]}))});
    assert!(serde_json::to_vec(&initial).unwrap().len() < 4096);
}
#[tokio::test]
async fn forward_history_deadline_never_reads_or_spawns_after_expiry() {
    let (_, service, _, _, _) = forward_owner_test_objects();
    let mut reads = 0;
    let result =
        wait_forward_pod_running(&service, None, None, tokio::time::Instant::now(), || {
            reads += 1;
            std::future::ready((Value::Null, Value::Null))
        })
        .await;
    assert_eq!(result, Err("forward_deadline"));
    assert_eq!(reads, 0);
}
#[test]
fn forward_owner_queries_use_only_exact_namespaced_raw_replicaset_contract() {
    let ns = "wd-plugin-0123456789abcdef01234567";
    let args = forward_query_args(ns, "replicasets", None).unwrap();
    assert_eq!(
        args.last().unwrap(),
        &format!("--raw=/apis/apps/v1/namespaces/{ns}/replicasets")
    );
    assert!(
        forward_query_value(
            "replicasets",
            br#"{"apiVersion":"apps/v1","kind":"ReplicaSetList","items":[]}"#
        )
        .is_some()
    );
    for raw in [
        br#"{"apiVersion":"v1","kind":"List","items":[]}"#.as_slice(),
        br#"{"apiVersion":"apps/v1","kind":"ReplicaSetList","items":null}"#,
        br#"{"apiVersion":"v1","kind":"ReplicaSetList","items":[]}"#,
    ] {
        assert!(forward_query_value("replicasets", raw).is_none());
    }
    for kind in ["secrets", "pods/../secrets"] {
        assert!(forward_query_args(ns, kind, None).is_none());
    }
    assert!(forward_query_args(ns, "deployment", Some("../foreign")).is_none());
}

#[test]
fn forward_typed_lists_allow_only_documented_nested_typemeta_omission() {
    let (expected, service, deployment, mut sets, mut pods) = forward_owner_test_objects();
    for row in [&mut sets["items"][0], &mut pods["items"][0]] {
        row.as_object_mut().unwrap().remove("apiVersion");
        row.as_object_mut().unwrap().remove("kind");
    }
    forward_owner_binding(&expected, &service, &deployment, &sets, &pods, &mut None).unwrap();
    for variant in [
        "pod_kind",
        "rs_version",
        "null_kind",
        "wrong_root",
        "partial_page",
    ] {
        let (mut r, mut p) = (sets.clone(), pods.clone());
        match variant {
            "pod_kind" => p["items"][0]["kind"] = json!("Secret"),
            "rs_version" => r["items"][0]["apiVersion"] = json!("v1"),
            "null_kind" => p["items"][0]["kind"] = Value::Null,
            "wrong_root" => p["kind"] = json!("List"),
            "partial_page" => p["metadata"]["continue"] = json!("private-page-token"),
            _ => unreachable!(),
        }
        assert!(
            forward_owner_binding(&expected, &service, &deployment, &r, &p, &mut None).is_err()
        );
    }
}
#[tokio::test]
async fn forward_exact_decision_retains_history_and_pinned_uid_before_ready() {
    let (expected, service, deployment, sets, pods) = forward_owner_test_objects();
    let active = pods["items"][0].clone();
    let mut history = active.clone();
    history["metadata"]["uid"] = json!("66666666-6666-6666-6666-666666666666");
    history["status"]["phase"] = json!("Succeeded");
    history["status"]["containerStatuses"][0]["state"] = json!({"terminated":{"exitCode":0}});
    let trace = ForwardQueryTrace {
        name: "unit-no-files".into(),
        last: std::sync::Mutex::new(BTreeMap::new()),
    };
    let mut reads = 0;
    let mut owner_pin = None;
    let uid = wait_forward_pod_running(
        &service,
        None,
        Some(&trace),
        tokio::time::Instant::now() + Duration::from_secs(1),
        || {
            reads += 1;
            let mut current = active.clone();
            if reads == 1 {
                current["status"]["phase"] = json!("Pending");
            }
            let list =
                json!({"apiVersion":"v1","kind":"PodList","items":[history.clone(),current]});
            forward_owner_binding(
                &expected,
                &service,
                &deployment,
                &sets,
                &list,
                &mut owner_pin,
            )
            .unwrap();
            std::future::ready((service.clone(), list))
        },
    )
    .await
    .unwrap();
    assert_eq!(reads, 2);
    assert_eq!(uid, active["metadata"]["uid"]);
    let snapshot = trace.last.lock().unwrap()["Decision"].clone();
    assert_eq!(snapshot["decision"], "ready");
    assert_eq!(snapshot["pinnedUIDBefore"], uid);
    assert_eq!(snapshot["pinnedUIDAfter"], uid);
    assert_eq!(snapshot["matchingCount"], 2);
    assert_eq!(snapshot["pods"][0]["status"]["phase"], "Succeeded");
    assert_eq!(snapshot["pods"][1]["status"]["phase"], "Running");
    assert_eq!(
        snapshot["pods"][0]["status"]["containers"][1]["state"],
        "terminated"
    );
    assert_eq!(
        snapshot["pods"][1]["status"]["containers"][1]["state"],
        "running"
    );
}

#[test]
fn forward_ce_empty_envfrom_omission_has_no_generic_field_fallback() {
    assert!(forward_producer_matches(&json!({}), &json!({"envFrom":[]})));
    assert!(!forward_producer_matches(
        &json!({"envFrom":[{"secretRef":{"name":"foreign"}}]}),
        &json!({"envFrom":[]})
    ));
    for expected in [
        json!({"image":"owned"}),
        json!({"args":[]}),
        json!({"securityContext":{"allowPrivilegeEscalation":false}}),
    ] {
        assert!(!forward_producer_matches(&json!({}), &expected));
    }
}

#[tokio::test]
async fn action_phase_capture_retains_child_before_unwind_and_redacts_private_streams() {
    let root = tempfile::tempdir().unwrap();
    let streams = root.path().join("mail-runtime-forward");
    private_bytes(&streams.with_extension("stdout"), b"private-action-secret");
    private_bytes(&streams.with_extension("stderr"), b"private-action-error");
    let mut command = tokio::process::Command::new("/bin/sh");
    command.args(["-c", "exit 7"]).kill_on_drop(true);
    let mut child = command.spawn().unwrap();
    child.wait().await.unwrap();
    let mut capture =
        ForwardChildCapture::new("mail-runtime-forward", "actions", root.path(), root.path());
    capture.adopt(child);
    drop(capture);
    let raw = std::fs::read(root.path().join("mail-runtime-forward-actions-final.json")).unwrap();
    let v: Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(v["childExitCode"], 7);
    assert_eq!(v["stdout"]["hashComplete"], true);
    assert!(!String::from_utf8_lossy(&raw).contains("private-action"));
}

#[test]
fn invoke_trace_covers_all_roles_codes_and_bounded_polling_without_private_body() {
    let root = tempfile::tempdir().unwrap();
    let mut trace = MailInvokeTrace::new(root.path().join("trace.json"));
    for role in MailInvokeRole::ALL {
        trace.enter(role);
        trace.result(
            role,
            StatusCode::OK,
            &json!({"password":"private-poison","resource":{"id":"private-id"}}),
        );
    }
    for _ in 0..1200 {
        trace.enter(MailInvokeRole::MailboxView);
        trace.result(MailInvokeRole::MailboxView, StatusCode::OK, &json!({}));
    }
    assert!(trace.complete());
    assert_eq!(trace.rows.len(), 8);
    trace.retain();
    let raw = std::fs::read(&trace.path).unwrap();
    assert!(raw.len() < 4096);
    assert!(!String::from_utf8_lossy(&raw).contains("private"));
    assert_eq!(trace.rows[&MailInvokeRole::MailboxView].entered, 1201);
    for code in [
        "invalid_input",
        "not_supported",
        "permission_denied",
        "capability_unavailable",
        "conflict",
        "unavailable",
        "deadline_exceeded",
        "not_found",
    ] {
        assert_eq!(
            mail_public_code(&json!({"error":{"code":code,"message":"private-poison"}})),
            code
        );
    }
    assert_eq!(mail_public_code(&json!({})), "missing");
    for code in [json!("private-poison"), json!(true), json!({}), Value::Null] {
        assert_eq!(mail_public_code(&json!({"error":{"code":code}})), "unknown");
    }
    trace.enter(MailInvokeRole::SendKnown);
    trace.result(MailInvokeRole::SendKnown, StatusCode::OK, &json!({}));
    assert!(!trace.complete());
    trace
        .rows
        .get_mut(&MailInvokeRole::MailboxView)
        .unwrap()
        .entered = u32::MAX;
    trace.enter(MailInvokeRole::MailboxView);
    assert_eq!(trace.rows[&MailInvokeRole::MailboxView].entered, u32::MAX);
}
#[tokio::test]
async fn invoke_failure_capture_happens_before_unwind_and_capture_failure_cannot_mask_primary() {
    for fail in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let path = if fail {
            root.path().to_path_buf()
        } else {
            root.path().join("trace.json")
        };
        let body = json!({"error":{"code":"unavailable","message":"private-poison"}});
        let task = tokio::spawn(MAIL_INVOKE_TRACE.scope(
            std::cell::RefCell::new(MailInvokeTrace::new(path.clone())),
            async move {
                MAIL_INVOKE_TRACE.with(|v| {
                    v.borrow_mut().enter(MailInvokeRole::OutgoingDomain);
                    v.borrow_mut().result(
                        MailInvokeRole::OutgoingDomain,
                        StatusCode::SERVICE_UNAVAILABLE,
                        &body,
                    );
                });
                retain_mail_invocations();
                panic!("original fixed HTTP assertion");
            },
        ));
        assert!(task.await.unwrap_err().is_panic());
        if !fail {
            let raw = std::fs::read(path).unwrap();
            let value: Value = serde_json::from_slice(&raw).unwrap();
            assert_eq!(value["roles"][0]["httpStatus"], 503);
            assert_eq!(value["roles"][0]["publicCode"], "unavailable");
            assert!(!String::from_utf8_lossy(&raw).contains("poison"));
        }
    }
}

#[tokio::test]
async fn actual_invoke_helper_captures_original_http_failure_once_without_body_or_capture_mask() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    for capture_fails in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let path = if capture_fails {
            root.path().to_path_buf()
        } else {
            root.path().join("actual.json")
        };
        let calls = std::sync::Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let app=Router::new().route("/api/console/plugins/invoke",axum::routing::post(move||{counter.fetch_add(1,Ordering::SeqCst);async{(StatusCode::SERVICE_UNAVAILABLE,axum::Json(json!({"error":{"code":"unavailable","message":"private-response-poison"}})))}}));
        let task = tokio::spawn(MAIL_INVOKE_TRACE.scope(
            std::cell::RefCell::new(MailInvokeTrace::new(path.clone())),
            async move {
                invoke_mail(
                    &app,
                    "private-console-poison",
                    "private-project-poison",
                    MailInvokeRole::OutgoingDomain,
                    json!({"domain":"public.example.test"}),
                    "private-key-poison",
                )
                .await
            },
        ));
        assert!(task.await.unwrap_err().is_panic());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        if !capture_fails {
            let raw = std::fs::read(path).unwrap();
            let v: Value = serde_json::from_slice(&raw).unwrap();
            assert_eq!(v["roles"][0]["httpStatus"], 503);
            assert_eq!(v["roles"][0]["publicCode"], "unavailable");
            assert!(!String::from_utf8_lossy(&raw).contains("poison"));
        }
    }
}

#[tokio::test]
async fn actual_action_http_panic_retains_forward_child_before_other_unwind_drops() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    struct Witness(PathBuf, Arc<AtomicBool>);
    impl Drop for Witness {
        fn drop(&mut self) {
            self.1.store(self.0.is_file(), Ordering::SeqCst);
        }
    }
    let root = tempfile::tempdir().unwrap();
    let receipt = root.path().join("mail-runtime-forward-actions-final.json");
    let observed = Arc::new(AtomicBool::new(false));
    let witness = observed.clone();
    let path = root.path().join("trace.json");
    let directory = root.path().to_path_buf();
    private_bytes(
        &directory.join("mail-runtime-forward.stdout"),
        b"private-forward-poison",
    );
    private_bytes(&directory.join("mail-runtime-forward.stderr"), b"");
    let mut command = tokio::process::Command::new("/bin/sh");
    command.args(["-c", "exit 7"]).kill_on_drop(true);
    let mut child = command.spawn().unwrap();
    child.wait().await.unwrap();
    let app = Router::new().route(
        "/api/console/plugins/invoke",
        axum::routing::post(|| async {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                axum::Json(json!({"error":{"code":"unavailable","message":"private-poison"}})),
            )
        }),
    );
    let expected = receipt.clone();
    let task = tokio::spawn(MAIL_INVOKE_TRACE.scope(
        std::cell::RefCell::new(MailInvokeTrace::new(path)),
        async move {
            let _witness = Witness(expected, witness);
            let _phase = MailPhaseCapture;
            let mut capture =
                ForwardChildCapture::new("mail-runtime-forward", "actions", &directory, &directory);
            capture.adopt(child);
            MAIL_INVOKE_TRACE.with(|v| v.borrow_mut().forward = Some(capture));
            invoke_mail(
                &app,
                "private-token",
                "private-project",
                MailInvokeRole::OutgoingDomain,
                json!({}),
                "private-key",
            )
            .await
        },
    ));
    assert!(task.await.unwrap_err().is_panic());
    assert!(observed.load(Ordering::SeqCst));
    let raw = std::fs::read(receipt).unwrap();
    let v: Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(v["childExitCode"], 7);
    assert_eq!(v["stdout"]["hashComplete"], true);
    assert!(!String::from_utf8_lossy(&raw).contains("poison"));
}

#[test]
fn prelude_lifetime_projects_actual_restart_and_termination_without_private_spec() {
    let (_, _, _, _, pods) = forward_owner_test_objects();
    let mut pod = pods["items"][0].clone();
    pod["spec"]["restartPolicy"] = json!("Always");
    pod["spec"]["containers"][0]["env"] =
        json!([{"name":"private-poison","value":"private-poison"}]);
    let initial = prelude_pod_projection(&pod).unwrap();
    assert!(prelude_lifetime_expected(&initial));
    assert!(
        !serde_json::to_string(&initial)
            .unwrap()
            .contains("private-poison")
    );
    pod["status"]["phase"] = json!("Succeeded");
    pod["status"]["containerStatuses"][0]["state"] = json!({"terminated":{"exitCode":0,"signal":15,"startedAt":"2026-10-28T19:00:00Z","finishedAt":"2026-10-28T19:00:01Z","message":"private-poison"}});
    pod["status"]["conditions"].as_array_mut().unwrap().push(json!({"type":"DisruptionTarget","status":"True","reason":"TerminationByKubelet","message":"private-poison"}));
    let terminal = prelude_pod_projection(&pod).unwrap();
    assert!(!prelude_lifetime_expected(&terminal));
    assert_eq!(terminal["containers"][1]["signal"], 15);
    assert_eq!(
        terminal["containers"][1]["finishedAtMs"].as_i64().unwrap()
            - terminal["containers"][1]["startedAtMs"].as_i64().unwrap(),
        1000
    );
    assert_eq!(
        terminal["disruptionTarget"]["reason"],
        "TerminationByKubelet"
    );
    assert!(
        !serde_json::to_string(&terminal)
            .unwrap()
            .contains("private-poison")
    );
}
#[test]
fn prelude_lifetime_denies_policy_rule_correspondence_and_time_poison() {
    let (_, _, _, _, pods) = forward_owner_test_objects();
    let mut original = pods["items"][0].clone();
    original["spec"]["restartPolicy"] = json!("Always");
    for variant in [
        "missing_policy",
        "non_always",
        "container_policy",
        "restart_rules",
        "duplicate_status",
        "missing_status",
        "bad_time",
        "duplicate_disruption",
    ] {
        let mut pod = original.clone();
        match variant {
            "missing_policy" => pod["spec"]["restartPolicy"] = Value::Null,
            "non_always" => pod["spec"]["restartPolicy"] = json!("Never"),
            "container_policy" => pod["spec"]["containers"][0]["restartPolicy"] = json!("Never"),
            "restart_rules" => pod["spec"]["containers"][0]["restartPolicyRules"] = json!([]),
            "duplicate_status" => {
                let row = pod["status"]["containerStatuses"][0].clone();
                pod["status"]["containerStatuses"]
                    .as_array_mut()
                    .unwrap()
                    .push(row);
            }
            "missing_status" => pod["status"]["containerStatuses"] = json!([]),
            "bad_time" => {
                pod["status"]["containerStatuses"][0]["state"]["running"]["startedAt"] =
                    json!("private-poison")
            }
            "duplicate_disruption" => {
                pod["status"]["conditions"] =
                    json!([{"type":"DisruptionTarget"},{"type":"DisruptionTarget"}])
            }
            _ => unreachable!(),
        }
        assert!(
            !prelude_pod_projection(&pod)
                .as_ref()
                .is_ok_and(prelude_lifetime_expected),
            "unsafe prelude context accepted: {variant}"
        );
    }
    assert_eq!(
        MailPrelude::STAGES,
        [
            "installed",
            "gateway401",
            "enabled",
            "before_namespace_entry",
            "smtp_ready"
        ]
    );
}
#[test]
fn prelude_trace_authorizes_only_five_fixed_checkpoint_names() {
    for stage in MailPrelude::STAGES {
        let trace = ForwardQueryTrace {
            name: format!("mail-prelude-{stage}"),
            last: std::sync::Mutex::new(BTreeMap::new()),
        };
        assert!(trace.allowed_name());
    }
    for name in [
        "mail-prelude-unknown",
        "mail-prelude-../private",
        "mail-prelude-installed/extra",
        "foreign",
    ] {
        let trace = ForwardQueryTrace {
            name: name.into(),
            last: std::sync::Mutex::new(BTreeMap::new()),
        };
        assert!(!trace.allowed_name());
    }
}
