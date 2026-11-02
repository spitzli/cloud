//! Existing SMTP admission digest and HMAC wire contract.
use crate::{
    Error, Result,
    message::{MAX_MESSAGE, header_name},
    secrets::key_bytes,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubmissionProof {
    #[serde(rename = "customerID")]
    pub customer_id: String,
    pub id: String,
    pub digest: String,
    pub from: String,
    pub to: Vec<String>,
    pub transport_revision: String,
    pub service_revision: String,
}
pub fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 19
        && value.as_bytes()[0] != b'0'
        && value.bytes().all(|b| b.is_ascii_digit())
}
pub fn message_digest(raw: &[u8]) -> Result<String> {
    if raw.len() > MAX_MESSAGE || raw.contains(&0) {
        return Err(Error::Invalid);
    }
    let text = std::str::from_utf8(raw).map_err(|_| Error::Invalid)?;
    let (header, body) = text.split_once("\r\n\r\n").ok_or(Error::Invalid)?;
    let mut headers: Vec<(String, String)> = Vec::new();
    for line in header.split("\r\n") {
        if line.starts_with([' ', '\t']) {
            let previous = headers.last_mut().ok_or(Error::Invalid)?;
            previous.1.push_str("\r\n");
            previous.1.push_str(line);
        } else {
            let (name, value) = line.split_once(':').ok_or(Error::Invalid)?;
            headers.push((name.into(), value.into()));
        }
    }
    hook_digest(&headers, body, raw.len())
}
pub fn hook_digest(headers: &[(String, String)], body: &str, size: usize) -> Result<String> {
    if size == 0 || size > MAX_MESSAGE || body.len() > MAX_MESSAGE || headers.len() > 1000 {
        return Err(Error::Invalid);
    }
    let mut total = 0;
    let mut normalized = Vec::new();
    for (name, value) in headers {
        total += name.len() + value.len();
        if total > 131072 || !header_name(name.as_bytes()) || value.contains('\0') {
            return Err(Error::Invalid);
        }
        let name = name.to_ascii_lowercase();
        if name == "x-webdock-submission" {
            continue;
        }
        // Unfold CRLF/LF followed by WSP, then collapse only SP/HTAB, matching the retained hook.
        let mut folded = String::new();
        let mut chars = value.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\r' && chars.peek() == Some(&'\n') {
                let mut look = chars.clone();
                look.next();
                if matches!(look.peek(), Some(' ' | '\t')) {
                    chars.next();
                    while matches!(chars.peek(), Some(' ' | '\t')) {
                        chars.next();
                    }
                    folded.push(' ');
                    continue;
                }
            }
            if c == '\n' && matches!(chars.peek(), Some(' ' | '\t')) {
                while matches!(chars.peek(), Some(' ' | '\t')) {
                    chars.next();
                }
                folded.push(' ');
                continue;
            }
            folded.push(c);
        }
        let mut value = String::new();
        let mut space = false;
        for c in folded.chars() {
            if matches!(c, ' ' | '\t') {
                if !space {
                    value.push(' ');
                }
                space = true;
            } else {
                value.push(c);
                space = false;
            }
        }
        normalized.push((
            name,
            value
                .trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
                .to_owned(),
        ));
    }
    let body = body.replace("\r\n", "\n");
    let canonical = serde_json::to_vec(&(normalized, body.trim_end_matches('\n')))
        .map_err(|_| Error::Invalid)?;
    Ok(hex::encode(Sha256::digest(canonical)))
}
fn signature(context: &SubmissionProof, key: &str) -> Result<Hmac<Sha256>> {
    if ![
        &context.customer_id,
        &context.id,
        &context.transport_revision,
        &context.service_revision,
    ]
    .iter()
    .all(|s| valid_id(s))
        || context.digest.len() != 64
        || !context
            .digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || context.to.is_empty()
        || context.to.len() > 100
    {
        return Err(Error::Invalid);
    }
    let mut derive = Hmac::<Sha256>::new_from_slice(&key_bytes(key)?).map_err(|_| Error::Secret)?;
    derive.update(b"webdock:submission-proof-key:v1");
    let mut mac = Hmac::<Sha256>::new_from_slice(&derive.finalize().into_bytes())
        .map_err(|_| Error::Secret)?;
    let mut to = context.to.clone();
    to.sort_by_cached_key(|s| s.encode_utf16().collect::<Vec<_>>());
    to.dedup();
    let data = serde_json::to_vec(&(
        &context.customer_id,
        &context.id,
        &context.digest,
        &context.from,
        to,
        &context.transport_revision,
        &context.service_revision,
    ))
    .map_err(|_| Error::Invalid)?;
    mac.update(&data);
    Ok(mac)
}
pub fn sign_submission(context: &SubmissionProof, key: &str) -> Result<String> {
    Ok(URL_SAFE_NO_PAD.encode(signature(context, key)?.finalize().into_bytes()))
}
pub fn verify_submission(context: &SubmissionProof, proof: &str, key: &str) -> Result<bool> {
    if proof.len() != 43 {
        return Ok(false);
    }
    let Ok(bytes) = URL_SAFE_NO_PAD.decode(proof) else {
        return Ok(false);
    };
    Ok(signature(context, key)?.verify_slice(&bytes).is_ok())
}
