//! Durable action ingress. No transport runs in an invocation future.
use crate::{Plugin, Result, invalid, store_error};
use serde_json::{Value, json};
use std::sync::Arc;
use webdock_mail_runtime::{PrivatePayloadStore, SendControl};
use webdock_plugin_protocol::{ErrorCode, InvocationClaims, PluginError};

pub struct Delivery {
    pub control: Arc<dyn SendControl>,
    pub payloads: Arc<dyn PrivatePayloadStore>,
    slots: Arc<tokio::sync::Semaphore>,
}
impl Delivery {
    pub fn new(control: Arc<dyn SendControl>, payloads: Arc<dyn PrivatePayloadStore>) -> Arc<Self> {
        Arc::new(Self {
            control,
            payloads,
            slots: Arc::new(tokio::sync::Semaphore::new(4)),
        })
    }
}
fn control_error(_: webdock_mail_runtime::ControlError) -> PluginError {
    PluginError::new(
        ErrorCode::Unavailable,
        "Mail request could not be completed",
    )
}
pub fn public_reply(value: &Value) -> Result<Value> {
    let id = value["id"]
        .as_str()
        .filter(|s| webdock_mail_runtime::proof::valid_id(s))
        .ok_or_else(|| control_error(webdock_mail_runtime::ControlError::unavailable()))?;
    let state = value["state"]
        .as_str()
        .filter(|s| {
            matches!(
                *s,
                "staging"
                    | "queued"
                    | "leased"
                    | "preparation_review"
                    | "handoff"
                    | "unknown"
                    | "accepted"
                    | "rejected"
                    | "cancelled"
            )
        })
        .ok_or_else(|| control_error(webdock_mail_runtime::ControlError::unavailable()))?;
    Ok(json!({"id":id,"state":state}))
}
impl Plugin {
    fn delivery(&self) -> Result<&Delivery> {
        self.delivery.as_deref().ok_or_else(|| {
            PluginError::new(
                ErrorCode::CapabilityUnavailable,
                "Durable mail sending is not configured",
            )
        })
    }
    pub(crate) async fn queued_send(
        &self,
        claims: &InvocationClaims,
        token: &str,
        input: Value,
    ) -> Result<Value> {
        let delivery = self.delivery()?;
        let _slot =
            delivery.slots.clone().try_acquire_owned().map_err(|_| {
                PluginError::new(ErrorCode::Unavailable, "Mail is busy. Try later.")
            })?;
        let _fence = self
            .store
            .customer_fence(&claims.customer_id)
            .await
            .map_err(|_| control_error(webdock_mail_runtime::ControlError::unavailable()))?;
        let resource = input["resource_id"]
            .as_str()
            .ok_or_else(|| invalid("Invalid mail request"))?;
        let domain = self.domain(claims, resource).await?;
        if !self
            .store
            .direct_allowed(&claims.customer_id)
            .await
            .map_err(store_error)?
        {
            return Err(PluginError::new(
                ErrorCode::Conflict,
                "Incoming mail remains retained; direct sending is unavailable",
            ));
        }
        if !domain.verified && !self.settings.trust_unverified_domains {
            return Err(PluginError::new(
                ErrorCode::Conflict,
                "Verify the domain before sending",
            ));
        }
        let compiled = webdock_mail_message::compile_action(input.clone(), &domain.domain)
            .map_err(|_| invalid("Invalid mail request"))?;
        let policy = self.private_policy(&claims.project_id).await?;
        let reference = uuid::Uuid::new_v4().to_string();
        let admission=delivery.control.call(json!({"action":"send.admitAction","submission":{"invocationToken":token,"input":input,"payloadRef":reference,"privatePolicySha256":policy}})).await.map_err(control_error)?;
        let reply = public_reply(&admission)?;
        let created = admission["created"]
            .as_bool()
            .ok_or_else(|| control_error(webdock_mail_runtime::ControlError::unavailable()))?;
        if !created && admission["state"] != "staging" {
            return Ok(reply);
        }
        let reference = if created {
            reference
        } else {
            admission["payloadRef"]
                .as_str()
                .filter(|s| valid_uuid(s))
                .ok_or_else(|| control_error(webdock_mail_runtime::ControlError::unavailable()))?
                .to_owned()
        };
        if admission["state"] != "staging"
            || admission["customerID"] != claims.customer_id
            || admission["payloadRef"] != reference
            || !admission["uploadToken"].as_str().is_some_and(valid_uuid)
        {
            return Err(control_error(
                webdock_mail_runtime::ControlError::unavailable(),
            ));
        }
        let lease = json!({"id":admission["id"],"customerID":claims.customer_id,"payloadRef":reference,"uploadToken":admission["uploadToken"]});
        let retained = delivery.payloads.get(&claims.customer_id, &reference).await;
        let saved = match retained {
            Ok(bytes) => bytes == compiled.payload,
            Err(_) => delivery
                .payloads
                .put(&claims.customer_id, &reference, &compiled.payload)
                .await
                .is_ok(),
        };
        if !saved {
            let _ = delivery
                .control
                .call(json!({"action":"send.abortPayload","lease":lease}))
                .await;
            return Err(control_error(
                webdock_mail_runtime::ControlError::unavailable(),
            ));
        }
        if delivery
            .control
            .call(json!({"action":"send.commitPayload","lease":lease}))
            .await
            .map_err(control_error)?["queued"]
            != true
        {
            return Err(control_error(
                webdock_mail_runtime::ControlError::unavailable(),
            ));
        }
        Ok(json!({"id":admission["id"],"state":"queued"}))
    }
    pub(crate) async fn outbox_status(
        &self,
        token: &str,
        input: Value,
        history: bool,
    ) -> Result<Value> {
        let result = self
            .delivery()?
            .control
            .call(json!({"action":"send.actionStatus","invocationToken":token,"input":input}))
            .await
            .map_err(control_error)?;
        if !history {
            return public_reply(&result);
        }
        let items = result["items"]
            .as_array()
            .filter(|i| i.len() <= 20)
            .ok_or_else(|| control_error(webdock_mail_runtime::ControlError::unavailable()))?;
        let mut safe = Vec::with_capacity(items.len());
        for item in items {
            let mut row = public_reply(item)?;
            for key in ["from", "to", "subject", "transport", "at"] {
                row[key] = item
                    .get(key)
                    .ok_or_else(
                        || control_error(webdock_mail_runtime::ControlError::unavailable()),
                    )?
                    .clone();
            }
            safe.push(row);
        }
        Ok(json!({"items":safe}))
    }
    pub(crate) async fn transition(&self, token: &str, input: &Value) -> Result<()> {
        if let Some(delivery) = &self.delivery
            && delivery
                .control
                .call(
                    json!({"action":"send.checkTransition","invocationToken":token,"input":input}),
                )
                .await
                .map_err(control_error)?["allowed"]
                != true
        {
            return Err(control_error(
                webdock_mail_runtime::ControlError::unavailable(),
            ));
        }
        Ok(())
    }
    pub async fn private_policy(&self, project: &str) -> Result<String> {
        let relay = self.store.relay(project).await.map_err(store_error)?;
        Ok(private_policy_digest(
            project,
            relay.as_ref().map(|r| r.id),
            &self.settings.platform,
        ))
    }
}
fn valid_uuid(s: &str) -> bool {
    uuid::Uuid::parse_str(s).is_ok_and(|u| {
        u.to_string() == s && u.get_version_num() == 4 && u.get_variant() == uuid::Variant::RFC4122
    })
}
pub fn private_policy_digest(
    project: &str,
    relay: Option<uuid::Uuid>,
    platform: &crate::transport::Platform,
) -> String {
    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Snapshot<'a> {
        version: u8,
        selection: &'a str,
        #[serde(rename = "projectID")]
        project_id: &'a str,
        #[serde(rename = "relayID")]
        relay_id: Option<uuid::Uuid>,
        transport: &'a str,
        platform_sha256: Option<String>,
    }
    let platform_sha256 = relay.is_none().then(|| {
        webdock_mail_protocol::sha256(&serde_json::to_vec(platform).expect("private platform JSON"))
    });
    webdock_mail_protocol::sha256(
        &serde_json::to_vec(&Snapshot {
            version: 1,
            selection: "project",
            project_id: project,
            relay_id: relay,
            transport: if relay.is_some() { "byok" } else { "platform" },
            platform_sha256,
        })
        .expect("static private policy JSON"),
    )
}
