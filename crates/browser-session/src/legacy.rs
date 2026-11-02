use crate::{Error, Result};
use serde_json::Value;
use std::collections::HashMap;
/// Explicit bounded transition; retained cookie passwords are never used to issue new seals.
pub struct LegacyCookies {
    passwords: HashMap<String, String>,
    pub(crate) accept_until: i64,
}
impl LegacyCookies {
    pub fn new(passwords: HashMap<String, String>, accept_until: i64) -> Result<Self> {
        let now = chrono::Utc::now().timestamp();
        if passwords.is_empty()
            || passwords.values().any(|v| v.len() < 32)
            || accept_until <= 0
            || accept_until > now + 28800
        {
            return Err(Error::Configuration);
        }
        Ok(Self {
            passwords,
            accept_until,
        })
    }
    pub fn read(&self, header: &str, name: &str, max_age: i64) -> Result<Option<Value>> {
        if chrono::Utc::now().timestamp() >= self.accept_until {
            return Ok(None);
        }
        let Some(raw) = crate::service::unique_cookie(header, name)? else {
            return Ok(None);
        };
        self.decode(raw, max_age).map(Some)
    }
    pub fn decode(&self, raw: &str, max_age: i64) -> Result<Value> {
        self.decode_at(raw, max_age, chrono::Utc::now().timestamp())
    }
    fn decode_at(&self, raw: &str, max_age: i64, now: i64) -> Result<Value> {
        if now >= self.accept_until || raw.len() > 4096 || !(1..=28800).contains(&max_age) {
            return Err(Error::Unauthorized);
        }
        let seal = raw.strip_suffix("~2").ok_or(Error::Unauthorized)?;
        if seal.contains('~') {
            return Err(Error::Unauthorized);
        }
        // Require an authenticated finite envelope expiry as well as the original payload expiry.
        let expiration = seal
            .split('*')
            .nth(5)
            .and_then(|v| v.parse::<i64>().ok())
            .ok_or(Error::Unauthorized)?;
        if expiration <= now * 1000 || expiration > (now + max_age + 60) * 1000 {
            return Err(Error::Unauthorized);
        }
        let passwords = self
            .passwords
            .iter()
            .map(|(k, v)| (k.as_str(), iron_webcrypto_rs::Password::from(v.as_str())))
            .collect();
        let options = iron_webcrypto_rs::SealOptions {
            ttl: (max_age * 1000) as u64,
            timestamp_skew_sec: 0,
            localtime_offset_msec: now * 1000 - chrono::Utc::now().timestamp_millis(),
            ..Default::default()
        };
        let value: Value = iron_webcrypto_rs::unseal_with_map(seal, &passwords, &options)
            .map_err(|_| Error::Unauthorized)?;
        let exp = value["exp"].as_i64().ok_or(Error::Unauthorized)?;
        if exp <= now || exp > now + max_age || !value.is_object() {
            return Err(Error::Unauthorized);
        }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retained_javascript_seal_is_authenticated_expiring_and_version_bound() {
        let vector: Value =
            serde_json::from_str(include_str!("../tests/legacy-vector.json")).unwrap();
        let now = vector["now"].as_i64().unwrap();
        let keys = LegacyCookies {
            passwords: HashMap::from([("1".into(), vector["password"].as_str().unwrap().into())]),
            accept_until: now + 28800,
        };
        let token = vector["token"].as_str().unwrap();
        assert_eq!(keys.decode_at(token, 28800, now).unwrap(), vector["value"]);
        assert!(keys.decode_at(token, 28800, now + 3601).is_err());
        assert!(
            keys.decode_at(&token.replace("~2", "~1"), 28800, now)
                .is_err()
        );
        assert!(
            keys.decode_at(&token.replace("Fe26.2*1*", "Fe26.2*9*"), 28800, now)
                .is_err()
        );
        let mut forged = token.to_owned();
        forged.replace_range(100..101, "z");
        assert!(keys.decode_at(&forged, 28800, now).is_err());
        assert!(keys.decode_at(token, 300, now).is_err());
    }
}
