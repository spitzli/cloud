use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceState {
    Disabled,
    Pending,
    Provisioning,
    Ready,
    Suspended,
    NeedsReview,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigurationState {
    Pending,
    Running,
    Succeeded,
    Superseded,
    NeedsReview,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceState {
    Pending,
    Ready,
    NeedsReview,
}
#[derive(Clone, Deserialize)]
#[serde(tag = "action", rename_all = "lowercase", deny_unknown_fields)]
pub enum ServiceCommand {
    Activate { revision: String },
    Suspend { revision: String },
    Reconcile { revision: String },
}
#[derive(Clone, Deserialize)]
#[serde(tag = "action", deny_unknown_fields)]
pub enum MailboxCommand {
    #[serde(rename = "configuration.retry")]
    ConfigurationRetry { revision: String },
    #[serde(rename = "domain.attach")]
    DomainAttach {
        #[serde(rename = "senderDomainID")]
        sender_domain_id: String,
    },
    #[serde(rename = "mailbox.create")]
    MailboxCreate {
        #[serde(rename = "domainID")]
        domain_id: String,
        #[serde(rename = "localPart")]
        local_part: String,
        name: String,
        #[serde(rename = "quotaBytes", deserialize_with = "quota")]
        quota_bytes: u64,
    },
    #[serde(rename = "grant")]
    Grant {
        #[serde(rename = "mailboxID")]
        mailbox_id: String,
        subject: String,
    },
    #[serde(rename = "revoke")]
    Revoke {
        #[serde(rename = "mailboxID")]
        mailbox_id: String,
        subject: String,
    },
}
fn quota<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Input {
        Number(u64),
        Form(String),
    }
    match Input::deserialize(deserializer)? {
        Input::Number(value) => Ok(value),
        Input::Form(value) => {
            if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(serde::de::Error::custom("expected decimal quota"));
            }
            value.parse().map_err(serde::de::Error::custom)
        }
    }
}
#[derive(Serialize)]
pub struct MailboxChange {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}
#[derive(Serialize)]
pub struct Service {
    #[serde(rename = "customerID")]
    pub customer_id: String,
    pub enabled: bool,
    pub state: ServiceState,
    pub revision: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceDirectory {
    pub service: Service,
    pub mailboxes_available: bool,
    pub sending_available: bool,
    pub can_reconcile: bool,
    pub can_activate: bool,
    pub can_suspend: bool,
    pub pending_review: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MailboxDirectory {
    #[serde(rename = "customerID")]
    pub customer_id: String,
    pub operator: bool,
    pub configuration: Option<Configuration>,
    pub can_manage: bool,
    pub can_configure: bool,
    pub can_retry_configuration: bool,
    pub mailboxes: Vec<Mailbox>,
    pub domains: Vec<Domain>,
    pub verified_domains: Vec<VerifiedDomain>,
    pub people: Vec<Person>,
    pub grants: Vec<Grant>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Configuration {
    pub revision: String,
    pub applied_revision: String,
    pub state: Option<ConfigurationState>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Mailbox {
    pub id: String,
    pub name: String,
    pub address: String,
    pub state: ResourceState,
    pub enabled: bool,
    pub quota_bytes: String,
    pub can_open: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Domain {
    pub id: String,
    pub name: String,
    pub state: ResourceState,
    pub dkim_keys: Vec<DkimKey>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DkimKey {
    pub id: String,
    pub selector: String,
    pub public_key: String,
}
#[derive(Serialize)]
pub struct VerifiedDomain {
    pub id: String,
    pub domain: String,
}
#[derive(Serialize)]
pub struct Person {
    pub id: String,
    pub name: String,
    pub email: String,
}
#[derive(Serialize)]
pub struct Grant {
    #[serde(rename = "mailboxID")]
    pub mailbox_id: String,
    pub subject: String,
    pub enabled: bool,
    pub revision: String,
}
