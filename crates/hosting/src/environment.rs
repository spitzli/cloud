use crate::{Result, error};
use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit, Payload},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
pub fn validate_patch(patch: &Value) -> Result<()> {
    let rows = patch
        .as_array()
        .filter(|v| v.len() <= 32)
        .ok_or_else(|| error(400, "Invalid application environment."))?;
    let mut names = HashSet::new();
    let mut bytes = 0;
    let name_pattern = regex::Regex::new("^[A-Za-z_][A-Za-z0-9_]*$").unwrap();
    for row in rows {
        let object = row
            .as_object()
            .filter(|o| o.len() == 2 && o.contains_key("name") && o.contains_key("value"))
            .ok_or_else(|| error(400, "Invalid environment variable."))?;
        let name = object["name"]
            .as_str()
            .ok_or_else(|| error(400, "Invalid environment name."))?;
        if name.len() > 128
            || !name_pattern.is_match(name)
            || ["__proto__", "constructor", "prototype"].contains(&name)
            || !names.insert(name)
        {
            return Err(error(400, "Invalid or duplicate environment name."));
        }
        let value = if object["value"].is_null() {
            ""
        } else {
            object["value"]
                .as_str()
                .ok_or_else(|| error(400, "Invalid environment value."))?
        };
        if value.len() > 4096 || value.contains('\0') {
            return Err(error(400, "Invalid environment value."));
        }
        bytes += name.len() + value.len();
    }
    if bytes > 12000 {
        return Err(error(400, "Application environment is too large."));
    }
    Ok(())
}
pub(crate) struct Environment {
    secret: String,
}
impl Environment {
    pub(crate) fn new(secret: String) -> Result<Self> {
        if secret.len() < 32 {
            return Err(error(
                503,
                "Application environment encryption is not configured.",
            ));
        }
        Ok(Self { secret })
    }
    pub(crate) fn hash_secret(&self) -> &str {
        &self.secret
    }
    fn cipher(&self) -> Aes256Gcm {
        let key = Sha256::digest(format!("webdock-mail-v1\0{}", self.secret).as_bytes());
        Aes256Gcm::new_from_slice(&key).unwrap()
    }
    pub(crate) fn open(&self, id: &str, sealed: Option<&str>) -> Result<BTreeMap<String, String>> {
        let Some(sealed) = sealed else {
            return Ok(BTreeMap::new());
        };
        let failed = || error(503, "Stored application environment could not be opened.");
        if sealed.len() > 32768 {
            return Err(failed());
        }
        let parts = sealed.split('.').collect::<Vec<_>>();
        if parts.len() != 4 || parts[0] != "v1" {
            return Err(failed());
        }
        let nonce = URL_SAFE_NO_PAD.decode(parts[1]).map_err(|_| failed())?;
        let tag = URL_SAFE_NO_PAD.decode(parts[2]).map_err(|_| failed())?;
        let mut ciphertext = URL_SAFE_NO_PAD.decode(parts[3]).map_err(|_| failed())?;
        if nonce.len() != 12 || tag.len() != 16 {
            return Err(failed());
        }
        ciphertext.extend(tag);
        let plain = self
            .cipher()
            .decrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &ciphertext,
                    aad: format!("hosting-env:{id}").as_bytes(),
                },
            )
            .map_err(|_| failed())?;
        let values: BTreeMap<String, String> =
            serde_json::from_slice(&plain).map_err(|_| failed())?;
        Self::patch(values.clone(), &serde_json::json!([])).map_err(|_| failed())?;
        Ok(values)
    }
    pub(crate) fn patch(
        mut values: BTreeMap<String, String>,
        patch: &Value,
    ) -> Result<BTreeMap<String, String>> {
        validate_patch(patch)?;
        for row in patch.as_array().unwrap() {
            let name = row["name"].as_str().unwrap();
            if let Some(value) = row["value"].as_str() {
                values.insert(name.into(), value.into());
            } else {
                values.remove(name);
            }
        }
        validate_patch(&Value::Array(
            values
                .iter()
                .map(|(name, value)| serde_json::json!({"name":name,"value":value}))
                .collect(),
        ))?;
        if serde_json::to_vec(&values).unwrap().len() > 16000 {
            return Err(error(400, "Environment variables are too large."));
        }
        Ok(values)
    }
    pub(crate) fn seal(
        &self,
        id: &str,
        values: &BTreeMap<String, String>,
    ) -> Result<Option<String>> {
        if values.is_empty() {
            return Ok(None);
        }
        self.seal_snapshot(id, values).map(Some)
    }
    pub(crate) fn seal_snapshot(
        &self,
        id: &str,
        values: &BTreeMap<String, String>,
    ) -> Result<String> {
        let mut nonce = [0; 12];
        getrandom::getrandom(&mut nonce).map_err(|_| error(503, "Encryption unavailable."))?;
        let mut sealed = self
            .cipher()
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &serde_json::to_vec(values).unwrap(),
                    aad: format!("hosting-env:{id}").as_bytes(),
                },
            )
            .map_err(|_| error(503, "Encryption unavailable."))?;
        let tag = sealed.split_off(sealed.len() - 16);
        Ok(format!(
            "v1.{}.{}.{}",
            URL_SAFE_NO_PAD.encode(nonce),
            URL_SAFE_NO_PAD.encode(tag),
            URL_SAFE_NO_PAD.encode(sealed)
        ))
    }
}
pub(crate) fn redact(mut logs: String, values: Vec<String>, truncated: bool) -> String {
    let mut values = values
        .into_iter()
        .flat_map(|v| {
            std::iter::once(v.clone()).chain(v.lines().map(str::to_owned).collect::<Vec<_>>())
        })
        .filter(|v| !v.is_empty())
        .collect::<Vec<_>>();
    values.sort_by_key(|v| std::cmp::Reverse(v.len()));
    values.dedup();
    for value in &values {
        if truncated {
            for (n, _) in value.char_indices().rev().filter(|(n, _)| *n > 0) {
                if logs.ends_with(&value[..n]) {
                    logs.truncate(logs.len() - n);
                    logs.push_str("[redacted]");
                    break;
                }
            }
            for (n, _) in value.char_indices().skip(1) {
                if logs.starts_with(&value[n..]) {
                    logs = format!("[redacted]{}", &logs[value.len() - n..]);
                    break;
                }
            }
        }
        logs = logs.replace(value, "[redacted]");
    }
    let mut units = 0;
    logs.chars()
        .take_while(|c| {
            units += c.len_utf16();
            units <= 16000
        })
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retained_node_cipher_and_scope_and_redaction() {
        let fixture: Value =
            serde_json::from_str(include_str!("../tests/node-environment.json")).unwrap();
        let env = Environment::new(fixture["secret"].as_str().unwrap().into()).unwrap();
        let values = env.open("123", fixture["sealed"].as_str()).unwrap();
        assert_eq!(serde_json::to_value(&values).unwrap(), fixture["values"]);
        assert!(env.open("124", fixture["sealed"].as_str()).is_err());
        assert_eq!(
            env.open("123", env.seal("123", &values).unwrap().as_deref())
                .unwrap(),
            values
        );
        assert_eq!(
            redact("secret-value".into(), vec!["secret-value".into()], false),
            "[redacted]"
        );
        assert_eq!(
            redact(
                "value middle secr".into(),
                vec!["secret-value".into()],
                true
            ),
            "[redacted] middle [redacted]"
        );
    }
}
