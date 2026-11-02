use crate::{Error, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use mail_builder::{MessageBuilder, mime::MimePart};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use unicode_normalization::UnicodeNormalization;

pub const MAX_MESSAGE: usize = 25 * 1024 * 1024;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sender {
    pub email: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}
pub struct PreparedMessage {
    pub from: String,
    pub to: Vec<String>,
    pub raw: Vec<u8>,
    pub hash: String,
}
pub struct DispatchMessage {
    pub sender: Sender,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub raw: Vec<u8>,
}

fn js_len(s: &str) -> usize {
    s.encode_utf16().count()
}
fn control(s: &str) -> bool {
    s.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}')
}
pub fn envelope_address(value: &str) -> Result<String> {
    if js_len(value) > 320
        || value
            .chars()
            .any(|c| c.is_whitespace() || c == '\u{feff}' || "<>()[]\\\",;:".contains(c))
        || control(value)
    {
        return Err(Error::Invalid);
    }
    let (local, domain) = value.split_once('@').ok_or(Error::Invalid)?;
    let local = local.nfc().collect::<String>().to_lowercase();
    let domain = idna::domain_to_ascii(domain)
        .map_err(|_| Error::Invalid)?
        .to_ascii_lowercase();
    if local.is_empty()
        || local.len() > 64
        || local.split('.').any(str::is_empty)
        || !domain.contains('.')
        || domain.len() > 253
        || !domain.split('.').all(|p| {
            !p.is_empty()
                && p.len() <= 63
                && p.as_bytes()[0].is_ascii_alphanumeric()
                && p.as_bytes()[p.len() - 1].is_ascii_alphanumeric()
                && p.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
    {
        return Err(Error::Invalid);
    }
    let address = format!("{local}@{domain}");
    if address.len() > 254 {
        return Err(Error::Invalid);
    }
    Ok(address)
}
pub fn header_name(name: &[u8]) -> bool {
    !name.is_empty()
        && name
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+.^_`|~-".contains(b))
}
#[derive(Clone, Copy)]
enum ValidationProfile {
    Native,
    Action,
}
impl ValidationProfile {
    fn valid_name(self, name: &str) -> bool {
        match self {
            Self::Native => js_len(name) <= 80 && !control(name),
            Self::Action => name.chars().count() <= 80 && !name.chars().any(char::is_control),
        }
    }
}
fn display_name(name: &str, profile: ValidationProfile) -> Result<String> {
    if !profile.valid_name(name) {
        return Err(Error::Invalid);
    }
    let mut words = Vec::new();
    let mut word = String::new();
    for c in name.chars() {
        if word.len() + c.len_utf8() > 42 {
            words.push(std::mem::take(&mut word));
        }
        word.push(c);
    }
    if !word.is_empty() {
        words.push(word);
    }
    Ok(words
        .iter()
        .map(|s| format!("=?UTF-8?B?{}?=", STANDARD.encode(s)))
        .collect::<Vec<_>>()
        .join("\r\n "))
}
/// Rewrite only authority-bearing headers; preserve all remaining MIME bytes verbatim.
pub fn prepare_message(
    raw: &[u8],
    sender: &Sender,
    recipients: &[String],
) -> Result<PreparedMessage> {
    prepare_with_profile(raw, sender, recipients, ValidationProfile::Native)
}
/// Action-only MIME preparation; caller must derive action authority from its trusted lease.
pub fn prepare_action_message(
    raw: &[u8],
    sender: &Sender,
    recipients: &[String],
) -> Result<PreparedMessage> {
    if recipients.len() > 10 {
        return Err(Error::Invalid);
    }
    prepare_with_profile(raw, sender, recipients, ValidationProfile::Action)
}
fn prepare_with_profile(
    raw: &[u8],
    sender: &Sender,
    recipients: &[String],
    profile: ValidationProfile,
) -> Result<PreparedMessage> {
    if raw.len() > MAX_MESSAGE
        || raw.contains(&0)
        || recipients.is_empty()
        || recipients.len() > 100
    {
        return Err(Error::Invalid);
    }
    let from = envelope_address(&sender.email)?;
    let mut seen = HashSet::new();
    let mut to = Vec::new();
    for value in recipients {
        let value = envelope_address(value)?;
        if seen.insert(value.clone()) {
            to.push(value);
        }
    }
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or(Error::Invalid)?;
    if split > 131072 {
        return Err(Error::Invalid);
    }
    let header = &raw[..split];
    if header.iter().enumerate().any(|(i, b)| {
        (*b == b'\n' && (i == 0 || header[i - 1] != b'\r'))
            || (*b == b'\r' && header.get(i + 1) != Some(&b'\n'))
    }) {
        return Err(Error::Invalid);
    }
    let mut headers: Vec<(String, Vec<u8>)> = Vec::new();
    for line in header.split(|b| *b == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if matches!(line.first(), Some(b' ' | b'\t')) {
            let previous = headers.last_mut().ok_or(Error::Invalid)?;
            previous.1.extend_from_slice(b"\r\n");
            previous.1.extend_from_slice(line);
            continue;
        }
        let colon = line.iter().position(|b| *b == b':').ok_or(Error::Invalid)?;
        if !header_name(&line[..colon]) || headers.len() >= 1000 {
            return Err(Error::Invalid);
        }
        headers.push((
            String::from_utf8(line[..colon].to_ascii_lowercase()).map_err(|_| Error::Invalid)?,
            line.to_vec(),
        ));
    }
    let name = sender
        .name
        .as_deref()
        .map(|name| display_name(name, profile))
        .transpose()?
        .unwrap_or_default();
    let mut prepared = if name.is_empty() {
        format!("From: <{from}>\r\n")
    } else {
        format!("From: {name}\r\n <{from}>\r\n")
    }
    .into_bytes();
    for (name, line) in headers {
        if [
            "from",
            "sender",
            "return-path",
            "bcc",
            "dkim-signature",
            "authentication-results",
            "delivered-to",
            "x-original-to",
        ]
        .contains(&name.as_str())
            || ["x-webdock-", "resent-", "arc-"]
                .iter()
                .any(|p| name.starts_with(p))
        {
            continue;
        }
        prepared.extend(line);
        prepared.extend_from_slice(b"\r\n");
    }
    prepared.extend_from_slice(b"\r\n");
    prepared.extend_from_slice(&raw[split + 4..]);
    if prepared.len() > MAX_MESSAGE - 128 {
        return Err(Error::Invalid);
    }
    Ok(PreparedMessage {
        from,
        to,
        hash: hex::encode(Sha256::digest(&prepared)),
        raw: prepared,
    })
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attachment {
    pub filename: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_id: Option<String>,
}
#[derive(Clone)]
pub struct SendRequest {
    pub from: Sender,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub text: Option<String>,
    pub html: Option<String>,
    pub attachments: Option<Vec<Attachment>>,
    pub scheduled_at: Option<String>,
    cc_present: bool,
    bcc_present: bool,
}
impl Serialize for SendRequest {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        // Retained admission hashes cover JSON.stringify of the parsed request. Its optional
        // recipient arrays were appended after parsing when absent; retain those exact bytes.
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("from", &self.from)?;
        map.serialize_entry("to", &self.to)?;
        if self.cc_present {
            map.serialize_entry("cc", &self.cc)?;
        }
        if self.bcc_present {
            map.serialize_entry("bcc", &self.bcc)?;
        }
        map.serialize_entry("subject", &self.subject)?;
        if let Some(value) = &self.text {
            map.serialize_entry("text", value)?;
        }
        if let Some(value) = &self.html {
            map.serialize_entry("html", value)?;
        }
        if let Some(value) = &self.attachments {
            map.serialize_entry("attachments", value)?;
        }
        if let Some(value) = &self.scheduled_at {
            map.serialize_entry("scheduled_at", value)?;
        }
        if !self.cc_present {
            map.serialize_entry("cc", &self.cc)?;
        }
        if !self.bcc_present {
            map.serialize_entry("bcc", &self.bcc)?;
        }
        map.end()
    }
}
#[derive(Deserialize)]
#[serde(untagged)]
enum Addresses {
    One(String),
    Many(Vec<String>),
}
impl Addresses {
    fn parse(self) -> Result<Vec<String>> {
        let values = match self {
            Self::One(v) => vec![v],
            Self::Many(v) => v,
        };
        if values.len() > 100 {
            return Err(Error::Invalid);
        }
        values
            .into_iter()
            .map(|v| parse_address(&v).map(|s| s.email))
            .collect()
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    from: String,
    to: Addresses,
    cc: Option<Addresses>,
    bcc: Option<Addresses>,
    subject: String,
    text: Option<String>,
    html: Option<String>,
    attachments: Option<Vec<Attachment>>,
    scheduled_at: Option<String>,
}
fn parse_address(value: &str) -> Result<Sender> {
    if js_len(value) > 400 {
        return Err(Error::Invalid);
    }
    let value = value.trim();
    if let Some((name, tail)) = value.split_once('<')
        && let Some(email) = tail.strip_suffix('>')
    {
        let name = name.trim_end();
        if name.is_empty()
            || js_len(name) > 80
            || name.contains(['<', '>', '\r', '\n'])
            || email.contains(['<', '>'])
        {
            return Err(Error::Invalid);
        }
        let name = name.trim();
        let name = name.strip_prefix('"').unwrap_or(name);
        let name = name.strip_suffix('"').unwrap_or(name);
        return Ok(Sender {
            email: envelope_address(email)?,
            name: Some(name.to_owned()),
        });
    }
    Ok(Sender {
        email: envelope_address(value)?,
        name: None,
    })
}
impl SendRequest {
    pub fn parse(value: Value, now: DateTime<Utc>) -> Result<Self> {
        // Optional means absent, not JSON null (the retained public schema is strict).
        let object = value.as_object().ok_or(Error::Invalid)?;
        let cc_present = object.contains_key("cc");
        let bcc_present = object.contains_key("bcc");
        if object.values().any(Value::is_null)
            || object
                .get("attachments")
                .and_then(Value::as_array)
                .is_some_and(|a| {
                    a.iter().any(|v| {
                        v.as_object()
                            .is_some_and(|o| o.values().any(Value::is_null))
                    })
                })
        {
            return Err(Error::Invalid);
        }
        let input: Input = serde_json::from_value(value).map_err(|_| Error::Invalid)?;
        let from = parse_address(&input.from)?;
        let mut to = input.to.parse()?;
        let mut cc = input
            .cc
            .map(Addresses::parse)
            .transpose()?
            .unwrap_or_default();
        let mut bcc = input
            .bcc
            .map(Addresses::parse)
            .transpose()?
            .unwrap_or_default();
        if to.is_empty()
            || to.len() + cc.len() + bcc.len() > 100
            || js_len(&input.subject) > 700
            || control(&input.subject)
            || (input.text.is_none() && input.html.is_none())
            || [&input.text, &input.html]
                .iter()
                .any(|s| s.as_ref().is_some_and(|s| js_len(s) > 2_000_000))
        {
            return Err(Error::Invalid);
        }
        let mut seen = HashSet::new();
        for list in [&mut to, &mut cc, &mut bcc] {
            list.retain(|s| seen.insert(s.clone()));
        }
        if let Some(items) = &input.attachments {
            if items.len() > 10 {
                return Err(Error::Invalid);
            }
            let mut total = 0;
            for item in items {
                if item.filename.is_empty()
                    || js_len(&item.filename) > 255
                    || control(&item.filename)
                    || item.filename.contains(['/', '\\'])
                    || item.content.len() > 14_000_000
                {
                    return Err(Error::Invalid);
                }
                let bytes = STANDARD.decode(&item.content).map_err(|_| Error::Invalid)?;
                if STANDARD.encode(&bytes) != item.content {
                    return Err(Error::Invalid);
                }
                total += bytes.len();
                if let Some(content_type) = &item.content_type {
                    let parts = content_type.split('/').collect::<Vec<_>>();
                    if content_type.len() > 100
                        || parts.len() != 2
                        || parts.iter().any(|p| {
                            p.is_empty()
                                || !p
                                    .bytes()
                                    .all(|b| b.is_ascii_alphanumeric() || b"_.+-".contains(&b))
                        })
                    {
                        return Err(Error::Invalid);
                    }
                }
                if let Some(cid) = &item.content_id
                    && (cid.is_empty()
                        || cid.len() > 200
                        || !cid
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"_.@-".contains(&b)))
                {
                    return Err(Error::Invalid);
                }
            }
            if total > 10 * 1024 * 1024 {
                return Err(Error::Invalid);
            }
        }
        if let Some(at) = &input.scheduled_at {
            let time = DateTime::parse_from_rfc3339(at).map_err(|_| Error::Invalid)?;
            if !at.ends_with('Z') || time > now + chrono::Duration::days(30) {
                return Err(Error::Invalid);
            }
        }
        Ok(Self {
            from,
            to,
            cc,
            bcc,
            subject: input.subject,
            text: input.text,
            html: input.html,
            attachments: input.attachments,
            scheduled_at: input.scheduled_at,
            cc_present,
            bcc_present,
        })
    }
    pub fn parse_stored(mut value: Value, now: DateTime<Utc>) -> Result<Self> {
        let sender: Sender =
            serde_json::from_value(value.get("from").ok_or(Error::Invalid)?.clone())
                .map_err(|_| Error::Invalid)?;
        value["from"] = Value::String(match sender.name {
            Some(name) if !name.is_empty() => format!("{name} <{}>", sender.email),
            _ => sender.email,
        });
        Self::parse(value, now)
    }
    /// Parse canonical action payloads without interpreting display names as public address syntax.
    pub fn parse_stored_action(mut value: Value) -> Result<Self> {
        if !value
            .get("to")
            .and_then(Value::as_array)
            .is_some_and(|to| (1..=10).contains(&to.len()))
        {
            return Err(Error::Invalid);
        }
        let sender: Sender =
            serde_json::from_value(value.get("from").ok_or(Error::Invalid)?.clone())
                .map_err(|_| Error::Invalid)?;
        value["from"] = Value::String(sender.email.clone());
        let mut request = Self::parse(value, Utc::now())?;
        request.from = sender;
        request.validate_action()?;
        Ok(request)
    }
    fn validate_action(&self) -> Result<()> {
        let local = self.from.email.split_once('@').ok_or(Error::Invalid)?.0;
        if local.is_empty()
            || local.len() > 64
            || local.starts_with('.')
            || local.ends_with('.')
            || local.contains("..")
            || !local
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
            || envelope_address(&self.from.email)? != self.from.email
            || self.from.name.as_ref().is_some_and(|name| {
                name.is_empty()
                    || name.trim() != name
                    || !ValidationProfile::Action.valid_name(name)
            })
            || self.to.is_empty()
            || self.to.len() > 10
            || !self.cc.is_empty()
            || !self.bcc.is_empty()
            || self.subject.trim().is_empty()
            || self.subject.trim() != self.subject
            || self.subject.chars().count() > 200
            || self.subject.chars().any(char::is_control)
            || self
                .text
                .as_ref()
                .is_none_or(|text| text.is_empty() || text.chars().count() > 30_000)
            || self.html.is_some()
            || self.attachments.is_some()
            || self.scheduled_at.is_some()
        {
            return Err(Error::Invalid);
        }
        for address in std::iter::once(&self.from.email).chain(&self.to) {
            address
                .parse::<lettre::Address>()
                .map_err(|_| Error::Invalid)?;
        }
        Ok(())
    }
    pub fn compile(&self) -> Result<DispatchMessage> {
        self.compile_with_profile(ValidationProfile::Native)
    }
    /// Compile only an authenticated action payload, preserving the plugin's character bounds.
    pub fn compile_action(&self) -> Result<DispatchMessage> {
        self.compile_with_profile(ValidationProfile::Action)
    }
    fn compile_with_profile(&self, profile: ValidationProfile) -> Result<DispatchMessage> {
        // Public fields are intentionally revalidated at the final compilation boundary.
        let value = serde_json::to_value(self).map_err(|_| Error::Invalid)?;
        let mut request = match profile {
            ValidationProfile::Native => Self::parse_stored(value, Utc::now())?,
            ValidationProfile::Action => Self::parse_stored_action(value)?,
        };
        // Parsing an address removes one enclosing quote pair. Revalidation must not
        // normalize an already parsed display name again; prepare_message validates it.
        request.from.name = self.from.name.clone();
        let mut message = MessageBuilder::new()
            .from(request.from.email.clone())
            .to(request.to.clone())
            .subject(request.subject.clone())
            .message_id(format!("{}@webdock.invalid", uuid::Uuid::new_v4()));
        if !request.cc.is_empty() {
            message = message.cc(request.cc.clone());
        }
        if let Some(text) = &request.text {
            message = message.text_body(text.clone());
        }
        if let Some(html) = &request.html {
            message = message.html_body(html.clone());
        }
        for attachment in request.attachments.as_deref().unwrap_or_default() {
            let bytes = STANDARD
                .decode(&attachment.content)
                .map_err(|_| Error::Invalid)?;
            let mut part = MimePart::new(
                attachment
                    .content_type
                    .clone()
                    .unwrap_or_else(|| "application/octet-stream".into()),
                bytes,
            )
            .attachment(attachment.filename.clone());
            if let Some(cid) = &attachment.content_id {
                part = part.cid(cid.clone());
            }
            message.attachments.get_or_insert_default().push(part);
        }
        // BCC is envelope-only and is never handed to the MIME builder.
        let raw = message.write_to_vec().map_err(|_| Error::Invalid)?;
        if raw.len() > 23_000_000 {
            return Err(Error::Invalid);
        }
        let recipients = request
            .to
            .iter()
            .chain(&request.cc)
            .chain(&request.bcc)
            .cloned()
            .collect::<Vec<_>>();
        let raw = prepare_with_profile(&raw, &request.from, &recipients, profile)?.raw;
        Ok(DispatchMessage {
            sender: request.from,
            to: request.to,
            cc: request.cc,
            bcc: request.bcc,
            subject: request.subject,
            raw,
        })
    }
}

/// Strict existing plugin input. The resource is resolved by the caller's authority layer.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActionInput {
    #[serde(rename = "resource_id")]
    _resource_id: String,
    from: String,
    #[serde(default)]
    from_name: String,
    to: String,
    subject: String,
    text: String,
}

pub struct CompiledAction {
    pub request: SendRequest,
    pub payload: Vec<u8>,
    pub envelope: Value,
}

/// Compile bounded mail.send input with an already authorized canonical owned domain.
/// No caller-supplied resource, customer, job or invocation identity enters the result.
pub fn compile_action(input: Value, owned_domain: &str) -> Result<CompiledAction> {
    let input: ActionInput = serde_json::from_value(input).map_err(|_| Error::Invalid)?;
    let local = input.from.trim();
    let name = input.from_name.trim();
    let subject = input.subject.trim();
    if local.is_empty()
        || local.len() > 64
        || local.starts_with('.')
        || local.ends_with('.')
        || local.contains("..")
        || !local
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
        || name.chars().count() > 80
        || name.chars().any(char::is_control)
        || subject.is_empty()
        || subject.chars().count() > 200
        || subject.chars().any(char::is_control)
        || input.text.is_empty()
        || input.text.chars().count() > 30_000
    {
        return Err(Error::Invalid);
    }
    let address = format!("{local}@{owned_domain}");
    address
        .parse::<lettre::Address>()
        .map_err(|_| Error::Invalid)?;
    let from = envelope_address(&address)?;
    if from.split_once('@').map(|(_, domain)| domain) != Some(owned_domain) {
        return Err(Error::Invalid);
    }
    let recipients = input
        .to
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.parse::<lettre::Address>().map_err(|_| Error::Invalid)?;
            envelope_address(s)
        })
        .collect::<Result<Vec<_>>>()?;
    if recipients.is_empty() || recipients.len() > 10 {
        return Err(Error::Invalid);
    }
    let mut request = SendRequest::parse(
        serde_json::json!({"from":from,"to":recipients,"subject":subject,"text":input.text}),
        Utc::now(),
    )?;
    request.from.name = (!name.is_empty()).then(|| name.to_owned());
    request.validate_action()?;
    let payload = serde_json::to_vec(&request).map_err(|_| Error::Invalid)?;
    let envelope = serde_json::json!({"from":request.from.email,"to":request.to});
    Ok(CompiledAction {
        request,
        payload,
        envelope,
    })
}
