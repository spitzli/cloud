//! Account-bound JMAP data and management clients. No dynamic endpoint discovery or retries.
use reqwest::{
    Client, Method,
    header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;
use url::Url;
pub const MAX_ATTACHMENT_BYTES: usize = 10 * 1024 * 1024;
const PROPERTIES: &[&str] = &[
    "id",
    "threadId",
    "mailboxIds",
    "keywords",
    "from",
    "to",
    "subject",
    "receivedAt",
    "preview",
    "hasAttachment",
    "size",
];
#[derive(Debug, thiserror::Error)]
#[error("Mail request unavailable; outcome uncertain={uncertain}")]
pub struct JmapError {
    pub uncertain: bool,
}
type Result<T> = std::result::Result<T, JmapError>;
fn fail(uncertain: bool) -> JmapError {
    JmapError { uncertain }
}
fn id(value: &str) -> Result<&str> {
    if value.is_empty()
        || value.len() > 255
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err(fail(false));
    }
    Ok(value)
}
fn js_len(value: &str) -> usize {
    value.encode_utf16().count()
}
fn media_type(value: &str) -> bool {
    let parts = value.split('/').collect::<Vec<_>>();
    value.len() <= 127
        && parts.len() == 2
        && parts.iter().all(|s| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&b))
        })
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MailAddress {
    pub email: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}
impl MailAddress {
    fn validate(&self) -> Result<()> {
        let (local, domain) = self.email.split_once('@').ok_or_else(|| fail(false))?;
        if js_len(&self.email) > 320
            || local.is_empty()
            || !domain.contains('.')
            || domain.starts_with('.')
            || domain.ends_with('.')
            || domain.contains('@')
            || self
                .email
                .chars()
                .any(|c| c.is_whitespace() || c <= '\u{1f}' || c == '\u{7f}' || "<>,;".contains(c))
            || self
                .name
                .as_ref()
                .is_some_and(|s| js_len(s) > 200 || s.contains(['\r', '\n', '\0']))
        {
            return Err(fail(false));
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MailAttachment {
    pub blob_id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub media_type: String,
    pub size: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MailDraft {
    pub identity_id: String,
    pub to: Vec<MailAddress>,
    #[serde(default)]
    pub cc: Vec<MailAddress>,
    #[serde(default)]
    pub bcc: Vec<MailAddress>,
    pub subject: String,
    pub text: String,
    pub in_reply_to: Option<Vec<String>>,
    pub references: Option<Vec<String>>,
    #[serde(default)]
    pub attachments: Vec<MailAttachment>,
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EmailChange {
    pub seen: Option<bool>,
    pub flagged: Option<bool>,
    pub mailbox_ids: Option<Vec<String>>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EmailQuery {
    pub mailbox_id: Option<String>,
    pub text: Option<String>,
    #[serde(default)]
    pub position: u32,
    #[serde(default = "default_limit")]
    pub limit: u32,
}
fn default_limit() -> u32 {
    50
}
impl Default for EmailQuery {
    fn default() -> Self {
        Self {
            mailbox_id: None,
            text: None,
            position: 0,
            limit: 50,
        }
    }
}
pub struct ExportedDraft {
    pub sender: MailAddress,
    pub to: Vec<String>,
    pub raw: Vec<u8>,
    pub blob_id: String,
}
pub struct DownloadedAttachment {
    pub bytes: Vec<u8>,
    pub name: String,
}

struct Http {
    client: Client,
    origin: Url,
}
impl Http {
    fn new(
        origin: &str,
        authorization: &str,
        timeout: Duration,
        trust: &[reqwest::Certificate],
        local: bool,
    ) -> Result<Self> {
        let url = Url::parse(origin).map_err(|_| fail(false))?;
        if !(url.scheme() == "https"
            || (local
                && url.scheme() == "http"
                && (url.host_str() == Some("127.0.0.1") || assigned_origin(&url))))
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || url.query().is_some()
            || url.path() != "/"
            || authorization.is_empty()
            || authorization.len() > 8192
            || timeout.is_zero()
            || timeout > Duration::from_secs(60)
        {
            return Err(fail(false));
        }
        let mut auth = HeaderValue::from_str(authorization).map_err(|_| fail(false))?;
        auth.set_sensitive(true);
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, auth);
        let mut builder = Client::builder()
            .default_headers(headers)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .timeout(timeout)
            .connect_timeout(timeout.min(Duration::from_secs(10)));
        for ca in trust {
            builder = builder.add_root_certificate(ca.clone());
        }
        Ok(Self {
            client: builder.build().map_err(|_| fail(false))?,
            origin: url,
        })
    }
    async fn request(
        &self,
        path: &str,
        method: Method,
        content_type: Option<&str>,
        body: Option<Vec<u8>>,
        max: usize,
        mutation: bool,
    ) -> Result<Vec<u8>> {
        let url = self.origin.join(path).map_err(|_| fail(false))?;
        if url.origin() != self.origin.origin() {
            return Err(fail(false));
        }
        let mut request = self.client.request(method, url);
        if let Some(ty) = content_type {
            request = request.header(CONTENT_TYPE, ty);
        }
        if let Some(body) = body {
            request = request.body(body);
        }
        let mut response = request.send().await.map_err(|_| fail(mutation))?;
        if !response.status().is_success()
            || response.content_length().is_some_and(|n| n > max as u64)
        {
            return Err(fail(mutation));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| fail(mutation))? {
            if bytes.len() + chunk.len() > max {
                return Err(fail(mutation));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
    async fn call(&self, method: &str, args: Value, using: &[&str], max: usize) -> Result<Value> {
        let mutation = method.ends_with("/set");
        let request =
            serde_json::to_vec(&json!({"using":using,"methodCalls":[[method,args,"webdock"]]}))
                .map_err(|_| fail(false))?;
        if request.len() > 2_097_152 {
            return Err(fail(false));
        }
        let bytes = self
            .request(
                "/jmap",
                Method::POST,
                Some("application/json"),
                Some(request),
                max,
                mutation,
            )
            .await?;
        let body: Value = serde_json::from_slice(&bytes).map_err(|_| fail(mutation))?;
        let responses = body["methodResponses"]
            .as_array()
            .filter(|a| a.len() == 1)
            .ok_or_else(|| fail(mutation))?;
        let response = responses[0]
            .as_array()
            .filter(|a| a.len() == 3 && a[2] == "webdock" && a[1].is_object())
            .ok_or_else(|| fail(mutation))?;
        if response[0] == "error" {
            let known = [
                "invalidArguments",
                "unknownMethod",
                "unknownCapability",
                "accountNotFound",
                "accountNotSupportedByMethod",
                "forbidden",
                "stateMismatch",
            ]
            .contains(&response[1]["type"].as_str().unwrap_or(""));
            return Err(fail(mutation && !known));
        }
        if response[0] != method {
            return Err(fail(mutation));
        }
        Ok(response[1].clone())
    }
}
fn assigned_origin(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    let Some((customer, tail)) = host
        .strip_prefix("app-")
        .and_then(|s| s.split_once(".wd-mail-"))
    else {
        return false;
    };
    crate::proof::valid_id(customer)
        && tail == format!("{customer}.svc.cluster.local")
        && url.port() == Some(8080)
}
pub struct JmapClient {
    http: Http,
    account: String,
    sender: Option<MailAddress>,
}
impl JmapClient {
    /// Retained private cluster endpoint; the broker must authorize customer and credential first.
    pub fn assigned(
        customer: &str,
        authorization: &str,
        account: &str,
        sender: Option<MailAddress>,
    ) -> Result<Self> {
        if !crate::proof::valid_id(customer) {
            return Err(fail(false));
        }
        id(account)?;
        if let Some(s) = &sender {
            s.validate()?;
        }
        let origin = format!("http://app-{customer}.wd-mail-{customer}.svc.cluster.local:8080/");
        Ok(Self {
            http: Http::new(&origin, authorization, Duration::from_secs(10), &[], true)?,
            account: account.into(),
            sender,
        })
    }

    pub fn new(
        origin: &str,
        authorization: &str,
        account: &str,
        sender: Option<MailAddress>,
        timeout: Duration,
        trust: &[reqwest::Certificate],
    ) -> Result<Self> {
        id(account)?;
        if let Some(sender) = &sender {
            sender.validate()?;
        }
        Ok(Self {
            http: Http::new(origin, authorization, timeout, trust, false)?,
            account: account.into(),
            sender,
        })
    }
    #[cfg(feature = "test-support")]
    pub fn local_fixture(
        origin: &str,
        authorization: &str,
        account: &str,
        sender: Option<MailAddress>,
    ) -> Result<Self> {
        id(account)?;
        if let Some(s) = &sender {
            s.validate()?;
        }
        Ok(Self {
            http: Http::new(origin, authorization, Duration::from_secs(2), &[], true)?,
            account: account.into(),
            sender,
        })
    }
    async fn call(&self, method: &str, mut args: Value) -> Result<Value> {
        args["accountId"] = json!(self.account);
        let result = self
            .http
            .call(
                method,
                args,
                &[
                    "urn:ietf:params:jmap:core",
                    "urn:ietf:params:jmap:mail",
                    "urn:ietf:params:jmap:submission",
                ],
                4_194_304,
            )
            .await?;
        let mutation = method.ends_with("/set");
        if result["accountId"] != self.account {
            return Err(fail(mutation));
        }
        for key in ["notCreated", "notUpdated", "notDestroyed"] {
            if let Some(errors) = result[key].as_object().filter(|e| !e.is_empty()) {
                let partial = ["created", "updated"]
                    .iter()
                    .any(|k| result[k].as_object().is_some_and(|o| !o.is_empty()))
                    || result["destroyed"]
                        .as_array()
                        .is_some_and(|a| !a.is_empty());
                let uncertain = errors.values().any(|e| {
                    ![
                        "invalidProperties",
                        "notFound",
                        "forbidden",
                        "overQuota",
                        "tooLarge",
                        "tooManyRecipients",
                        "invalidEmail",
                        "invalidRecipients",
                    ]
                    .contains(&e["type"].as_str().unwrap_or(""))
                });
                return Err(fail(partial || uncertain));
            }
        }
        Ok(result)
    }
    fn list(result: &Value, expected: Option<&[&str]>) -> Result<Vec<Value>> {
        let rows = result["list"]
            .as_array()
            .filter(|a| a.len() <= 100)
            .ok_or_else(|| fail(false))?;
        for row in rows {
            let value = row["id"].as_str().ok_or_else(|| fail(false))?;
            id(value)?;
            if expected.is_some_and(|e| !e.contains(&value)) {
                return Err(fail(false));
            }
        }
        Ok(rows.clone())
    }
    pub async fn mailboxes(&self) -> Result<Value> {
        let result=self.call("Mailbox/get",json!({"ids":null,"properties":["id","name","role","parentId","sortOrder","totalEmails","unreadEmails","myRights"]})).await?;
        Ok(json!({"list":Self::list(&result,None)?,"state":result["state"]}))
    }
    pub async fn identities(&self) -> Result<Vec<Value>> {
        Self::list(&self.call("Identity/get", json!({"ids":null})).await?, None)
    }
    pub async fn emails(&self, input: EmailQuery) -> Result<Value> {
        if !(1..=100).contains(&input.limit)
            || input.position > 1_000_000
            || input.text.as_ref().is_some_and(|s| js_len(s) > 500)
        {
            return Err(fail(false));
        }
        let mut filter = json!({});
        if let Some(mailbox) = input.mailbox_id.filter(|s| !s.is_empty()) {
            filter["inMailbox"] = json!(id(&mailbox)?);
        }
        if let Some(text) = input.text.filter(|s| !s.is_empty()) {
            filter["text"] = json!(text);
        }
        let query=self.call("Email/query",json!({"filter":filter,"position":input.position,"limit":input.limit,"calculateTotal":true,"sort":[{"property":"receivedAt","isAscending":false}]})).await?;
        let ids = query["ids"]
            .as_array()
            .filter(|a| a.len() <= input.limit as usize)
            .ok_or_else(|| fail(false))?
            .iter()
            .map(|v| id(v.as_str().ok_or_else(|| fail(false))?))
            .collect::<Result<Vec<_>>>()?;
        let list = if ids.is_empty() {
            vec![]
        } else {
            Self::list(
                &self
                    .call("Email/get", json!({"ids":ids,"properties":PROPERTIES}))
                    .await?,
                Some(&ids),
            )?
        };
        Ok(
            json!({"list":list,"total":query["total"],"queryState":query["queryState"],"position":input.position}),
        )
    }
    pub async fn email(&self, email: &str) -> Result<Value> {
        id(email)?;
        let properties = PROPERTIES
            .iter()
            .copied()
            .chain([
                "blobId",
                "cc",
                "bcc",
                "replyTo",
                "sentAt",
                "messageId",
                "inReplyTo",
                "references",
                "textBody",
                "htmlBody",
                "attachments",
                "bodyValues",
            ])
            .collect::<Vec<_>>();
        let result=self.call("Email/get",json!({"ids":[email],"properties":properties,"fetchAllBodyValues":true,"maxBodyValueBytes":262144})).await?;
        let rows = Self::list(&result, Some(&[email]))?;
        if rows.len() != 1 {
            return Err(fail(false));
        }
        Ok(rows[0].clone())
    }
    pub async fn update_email(
        &self,
        email: &str,
        change: EmailChange,
        state: Option<&str>,
    ) -> Result<Value> {
        id(email)?;
        let mut patch = json!({});
        for (key, value) in [
            ("keywords/$seen", change.seen),
            ("keywords/$flagged", change.flagged),
        ] {
            if let Some(value) = value {
                patch[key] = if value { json!(true) } else { Value::Null };
            }
        }
        if let Some(mailboxes) = change.mailbox_ids {
            if mailboxes.is_empty() || mailboxes.len() > 16 {
                return Err(fail(false));
            }
            let mut folders = json!({});
            for mailbox in mailboxes {
                folders[id(&mailbox)?] = json!(true);
            }
            patch["mailboxIds"] = folders;
        }
        if patch.as_object().unwrap().is_empty() {
            return Err(fail(false));
        }
        let mut args = json!({"update":{email:patch}});
        if let Some(state) = state.filter(|s| !s.is_empty()) {
            args["ifInState"] = json!(state);
        }
        let result = self.call("Email/set", args).await?;
        if !result["updated"]
            .as_object()
            .is_some_and(|o| o.contains_key(email))
        {
            return Err(fail(true));
        }
        Ok(json!({"state":result["newState"]}))
    }
    async fn identity(&self, identity: &str) -> Result<MailAddress> {
        id(identity)?;
        let row = self
            .identities()
            .await?
            .into_iter()
            .find(|v| v["id"] == identity)
            .ok_or_else(|| fail(false))?;
        let email = row["email"].as_str().ok_or_else(|| fail(false))?;
        if let Some(sender) = &self.sender {
            if sender.email != email {
                return Err(fail(false));
            }
            return Ok(sender.clone());
        }
        let sender = MailAddress {
            email: email.into(),
            name: row["name"].as_str().map(str::to_owned),
        };
        sender.validate()?;
        Ok(sender)
    }
    pub async fn create_draft(&self, input: MailDraft, previous: Option<&str>) -> Result<Value> {
        if js_len(&input.subject) > 998
            || input.subject.contains(['\r', '\n', '\0'])
            || input.text.len() > 262144
            || input.to.len() + input.cc.len() + input.bcc.len() > 100
            || input.attachments.len() > 10
        {
            return Err(fail(false));
        }
        for address in input.to.iter().chain(&input.cc).chain(&input.bcc) {
            address.validate()?;
        }
        let mut total = 0u64;
        for attachment in &input.attachments {
            id(&attachment.blob_id)?;
            if attachment.name.is_empty()
                || js_len(&attachment.name) > 200
                || attachment.name.contains(['\r', '\n', '\0'])
                || !media_type(&attachment.media_type)
                || attachment.size > MAX_ATTACHMENT_BYTES as u64
            {
                return Err(fail(false));
            }
            total += attachment.size;
        }
        if total > MAX_ATTACHMENT_BYTES as u64 {
            return Err(fail(false));
        }
        for values in [&input.in_reply_to, &input.references]
            .into_iter()
            .flatten()
        {
            if values.len() > 50
                || values.iter().any(|v| {
                    js_len(v) > 998
                        || v.chars().any(|c| c.is_whitespace() || "<>".contains(c))
                        || !v
                            .split_once('@')
                            .is_some_and(|(a, b)| !a.is_empty() && !b.is_empty())
                })
            {
                return Err(fail(false));
            }
        }
        let sender = self.identity(&input.identity_id).await?;
        let mut previous_state = None;
        if let Some(previous) = previous {
            id(previous)?;
            let result = self
                .call(
                    "Email/get",
                    json!({"ids":[previous],"properties":["id","keywords","from"]}),
                )
                .await?;
            let rows = Self::list(&result, Some(&[previous]))?;
            let row = rows.first().ok_or_else(|| fail(false))?;
            if !is_draft(row, &sender.email) {
                return Err(fail(false));
            }
            previous_state = Some(
                result["state"]
                    .as_str()
                    .ok_or_else(|| fail(false))?
                    .to_owned(),
            );
        }
        let mailboxes = self.mailboxes().await?;
        let drafts = mailboxes["list"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["role"] == "drafts")
            .ok_or_else(|| fail(false))?;
        let drafts = drafts["id"].as_str().ok_or_else(|| fail(false))?;
        let mut draft = json!({"mailboxIds":{drafts:true},"keywords":{"$draft":true},"from":[sender],"to":input.to,"cc":input.cc,"bcc":input.bcc,"subject":input.subject,"textBody":[{"partId":"body","type":"text/plain"}],"bodyValues":{"body":{"value":input.text}}});
        if !input.attachments.is_empty() {
            draft["attachments"]=json!(input.attachments.iter().map(|a|json!({"blobId":a.blob_id,"name":a.name,"type":a.media_type,"disposition":"attachment"})).collect::<Vec<_>>());
        }
        if let Some(value) = input.in_reply_to {
            draft["inReplyTo"] = json!(value);
        }
        if let Some(value) = input.references {
            draft["references"] = json!(value);
        }
        let mut args = json!({"create":{"draft":draft}});
        if let Some(state) = previous_state {
            args["ifInState"] = json!(state);
        }
        let mut result = self.call("Email/set", args).await?;
        let created = result["created"]["draft"]["id"]
            .as_str()
            .ok_or_else(|| fail(true))?
            .to_owned();
        id(&created).map_err(|_| fail(true))?;
        if let Some(previous) = previous {
            let state = result["newState"].as_str().ok_or_else(|| fail(true))?;
            let cleanup = self
                .call("Email/set", json!({"ifInState":state,"destroy":[previous]}))
                .await
                .map_err(|_| fail(true))?;
            if !cleanup["destroyed"]
                .as_array()
                .is_some_and(|a| a.iter().any(|v| v == previous))
            {
                return Err(fail(true));
            }
            result["newState"] = cleanup["newState"].clone();
        }
        Ok(json!({"id":created,"state":result["newState"]}))
    }
    /// Call only after durable submission admission. No automatic retry, including malformed receipts.
    pub async fn submit_draft(&self, email: &str, identity: &str) -> Result<Value> {
        id(email)?;
        let sender = self.identity(identity).await?;
        let result = self
            .call(
                "Email/get",
                json!({"ids":[email],"properties":["id","from","keywords"]}),
            )
            .await?;
        let rows = Self::list(&result, Some(&[email]))?;
        if !rows.first().is_some_and(|r| is_draft(r, &sender.email)) {
            return Err(fail(false));
        }
        let result = self
            .call(
                "EmailSubmission/set",
                json!({"create":{"submission":{"emailId":email,"identityId":identity}}}),
            )
            .await?;
        let created = result["created"]["submission"]["id"]
            .as_str()
            .ok_or_else(|| fail(true))?;
        id(created).map_err(|_| fail(true))?;
        Ok(json!({"id":created,"state":result["newState"]}))
    }
    pub async fn file_sent(&self, email: &str) -> Result<Value> {
        id(email)?;
        let mailboxes = self.mailboxes().await?;
        let sent = mailboxes["list"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["role"] == "sent")
            .ok_or_else(|| fail(false))?;
        let sent = sent["id"].as_str().ok_or_else(|| fail(false))?;
        let result = self
            .call(
                "Email/set",
                json!({"update":{email:{"keywords/$draft":null,"mailboxIds":{sent:true}}}}),
            )
            .await?;
        if !result["updated"]
            .as_object()
            .is_some_and(|o| o.contains_key(email))
        {
            return Err(fail(true));
        }
        Ok(json!({"state":result["newState"]}))
    }
    pub async fn upload(&self, bytes: Vec<u8>, media_type_value: &str) -> Result<Value> {
        if bytes.len() > MAX_ATTACHMENT_BYTES || !media_type(media_type_value) {
            return Err(fail(false));
        }
        let size = bytes.len();
        let raw = self
            .http
            .request(
                &format!("/jmap/upload/{}/", self.account),
                Method::POST,
                Some(media_type_value),
                Some(bytes),
                65536,
                false,
            )
            .await?;
        let result: Value = serde_json::from_slice(&raw).map_err(|_| fail(false))?;
        if result["accountId"] != self.account
            || result["size"].as_u64() != Some(size as u64)
            || !result["type"].is_string()
        {
            return Err(fail(false));
        }
        let blob = id(result["blobId"].as_str().ok_or_else(|| fail(false))?)?;
        Ok(json!({"blobId":blob,"size":size,"type":media_type_value}))
    }
    pub async fn export_draft(&self, email: &str, identity: &str) -> Result<ExportedDraft> {
        id(email)?;
        let sender = self.identity(identity).await?;
        let result=self.call("Email/get",json!({"ids":[email],"properties":["id","blobId","size","keywords","from","to","cc","bcc"]})).await?;
        let rows = Self::list(&result, Some(&[email]))?;
        let row = rows.first().ok_or_else(|| fail(false))?;
        if !is_draft(row, &sender.email)
            || !row["size"].as_u64().is_some_and(|n| n <= 25 * 1024 * 1024)
        {
            return Err(fail(false));
        }
        let mut to = Vec::new();
        for key in ["to", "cc", "bcc"] {
            if !row[key].is_null() {
                for recipient in row[key].as_array().ok_or_else(|| fail(false))? {
                    let address = MailAddress {
                        email: recipient["email"]
                            .as_str()
                            .ok_or_else(|| fail(false))?
                            .into(),
                        name: None,
                    };
                    address.validate()?;
                    to.push(address.email);
                }
            }
        }
        if to.is_empty() || to.len() > 100 {
            return Err(fail(false));
        }
        let blob = id(row["blobId"].as_str().ok_or_else(|| fail(false))?)?;
        let raw = self
            .http
            .request(
                &format!(
                    "/jmap/download/{}/{blob}/message.eml?accept=message%2Frfc822",
                    self.account
                ),
                Method::GET,
                None,
                None,
                25 * 1024 * 1024,
                false,
            )
            .await?;
        Ok(ExportedDraft {
            sender,
            to,
            raw,
            blob_id: blob.into(),
        })
    }
    pub async fn download_attachment(
        &self,
        email: &str,
        blob: &str,
    ) -> Result<DownloadedAttachment> {
        id(blob)?;
        let email = self.email(email).await?;
        let attachment = email["attachments"]
            .as_array()
            .and_then(|a| a.iter().find(|v| v["blobId"] == blob))
            .ok_or_else(|| fail(false))?;
        let name = attachment["name"].as_str().ok_or_else(|| fail(false))?;
        if !attachment["size"]
            .as_u64()
            .is_some_and(|n| n <= MAX_ATTACHMENT_BYTES as u64)
        {
            return Err(fail(false));
        }
        let bytes = self
            .http
            .request(
                &format!(
                    "/jmap/download/{}/{blob}/attachment?accept=application%2Foctet-stream",
                    self.account
                ),
                Method::GET,
                None,
                None,
                MAX_ATTACHMENT_BYTES,
                false,
            )
            .await?;
        Ok(DownloadedAttachment {
            bytes,
            name: name.into(),
        })
    }
}
fn is_draft(row: &Value, email: &str) -> bool {
    row["keywords"]["$draft"] == true
        && row["from"]
            .as_array()
            .is_some_and(|a| a.len() == 1 && a[0]["email"] == email)
}

/// Management credentials never enter the user data client or browser response.
pub struct ManagementClient {
    http: Http,
}
impl ManagementClient {
    #[cfg(feature = "test-support")]
    pub fn local_fixture(origin: &str, authorization: &str) -> Result<Self> {
        Ok(Self {
            http: Http::new(origin, authorization, Duration::from_secs(2), &[], true)?,
        })
    }
    pub fn assigned(customer: &str, authorization: &str) -> Result<Self> {
        if !crate::proof::valid_id(customer) {
            return Err(fail(false));
        }
        Ok(Self {
            http: Http::new(
                &format!("http://app-{customer}.wd-mail-{customer}.svc.cluster.local:8080/"),
                authorization,
                Duration::from_secs(10),
                &[],
                true,
            )?,
        })
    }

    pub fn new(
        origin: &str,
        authorization: &str,
        timeout: Duration,
        trust: &[reqwest::Certificate],
    ) -> Result<Self> {
        Ok(Self {
            http: Http::new(origin, authorization, timeout, trust, false)?,
        })
    }
    pub async fn call(&self, method: &str, args: Value) -> Result<Value> {
        let Some((name, verb)) = method.strip_prefix("x:").and_then(|s| s.split_once('/')) else {
            return Err(fail(false));
        };
        if name.is_empty()
            || !name.bytes().all(|b| b.is_ascii_alphabetic())
            || !["get", "query", "set"].contains(&verb)
            || !args.is_object()
        {
            return Err(fail(false));
        }
        let result = self
            .http
            .call(
                method,
                args,
                &["urn:ietf:params:jmap:core", "urn:stalwart:jmap"],
                1_048_576,
            )
            .await?;
        if ["notCreated", "notUpdated", "notDestroyed"]
            .iter()
            .any(|k| result[k].as_object().is_some_and(|v| !v.is_empty()))
        {
            return Err(fail(verb == "set"));
        }
        Ok(result)
    }
}
