use k8s_openapi::api::core::v1::Service;
use kube::{Api, Client};
use serde_json::{Value, json};
use std::time::Duration;
use webdock_agent_protocol::mail::{MailClaim, MailMaterial};
pub struct RetainedDraft {
    username: String,
    password: String,
    account_id: String,
    email_id: String,
}
async fn origin(client: &Client, claim: &MailClaim) -> String {
    let namespace = format!("wd-mail-{}", claim.spec.customer_id);
    let service = Api::<Service>::namespaced(client.clone(), &namespace)
        .get(&format!("app-{}", claim.spec.customer_id))
        .await
        .unwrap();
    assert_eq!(
        service.metadata.labels.as_ref().unwrap()["webdock.dev/app-id"],
        claim.spec.customer_id
    );
    let address = service
        .spec
        .unwrap()
        .cluster_ip
        .unwrap()
        .parse::<std::net::Ipv4Addr>()
        .unwrap();
    assert!(address.is_private());
    format!("http://{address}:8080/jmap")
}
async fn call(
    origin: &str,
    username: &str,
    password: &str,
    method: &str,
    args: Value,
    management: bool,
) -> Value {
    let capability = if management {
        "urn:stalwart:jmap"
    } else {
        "urn:ietf:params:jmap:mail"
    };
    let response:Value=reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(5)).build().unwrap().post(origin).basic_auth(username,Some(password)).json(&json!({"using":["urn:ietf:params:jmap:core",capability],"methodCalls":[[method,args,"fixture"]]})).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
    assert_eq!(response["methodResponses"][0][0], method);
    response["methodResponses"][0][1].clone()
}
impl RetainedDraft {
    pub async fn create(client: &Client, claim: &MailClaim) -> Self {
        let origin = origin(client, claim).await;
        let MailMaterial::Credentials {
            username, password, ..
        } = &claim.material
        else {
            panic!("fixture material required")
        };
        let domains = call(
            &origin,
            username,
            password,
            "x:Domain/get",
            json!({"ids":null}),
            true,
        )
        .await;
        let domain = domains["list"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["name"] == claim.spec.hostname)
            .unwrap()["id"]
            .as_str()
            .unwrap();
        let mailbox_password = uuid::Uuid::new_v4().to_string();
        let mailbox_username = format!("recipient@{}", claim.spec.hostname);
        let created=call(&origin,username,password,"x:Account/set",json!({"create":{"mailbox":{"@type":"User","name":"recipient","domainId":domain,"credentials":{"0":{"@type":"Password","secret":mailbox_password}},"roles":{"@type":"User"},"permissions":{"@type":"Inherit"},"encryptionAtRest":{"@type":"Disabled"},"quotas":{"maxDiskQuota":10485760}}}}),true).await;
        let account = created["created"]["mailbox"]["id"].as_str().unwrap();
        let folders = call(
            &origin,
            &mailbox_username,
            &mailbox_password,
            "Mailbox/get",
            json!({"accountId":account,"ids":null}),
            false,
        )
        .await;
        let drafts = folders["list"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["role"] == "drafts")
            .unwrap()["id"]
            .as_str()
            .unwrap();
        let created=call(&origin,&mailbox_username,&mailbox_password,"Email/set",json!({"accountId":account,"create":{"draft":{"mailboxIds":{drafts:true},"keywords":{"$draft":true},"from":[{"email":mailbox_username}],"subject":"Owned Kubernetes retained draft","textBody":[{"partId":"body","type":"text/plain"}],"bodyValues":{"body":{"value":"A real mailbox body survives suspension and migration."}}}}}),false).await;
        Self {
            username: mailbox_username,
            password: mailbox_password,
            account_id: account.into(),
            email_id: created["created"]["draft"]["id"].as_str().unwrap().into(),
        }
    }
    pub async fn verify(&self, client: &Client, claim: &MailClaim) {
        let result = call(
            &origin(client, claim).await,
            &self.username,
            &self.password,
            "Email/get",
            json!({"accountId":self.account_id,"ids":[self.email_id],"fetchTextBodyValues":true}),
            false,
        )
        .await;
        assert_eq!(
            result["list"][0]["subject"],
            "Owned Kubernetes retained draft"
        );
        assert!(
            result["list"][0]["bodyValues"]
                .as_object()
                .unwrap()
                .values()
                .any(|body| body["value"]
                    .as_str()
                    .is_some_and(|body| body
                        .contains("A real mailbox body survives suspension and migration.")))
        );
    }
}
