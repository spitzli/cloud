//! Compile-time-only native fixture profile. This module is absent from the production build.
use serde::Deserialize;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeProfile {
    pub run_id: String,
    pub expires_at: i64,
    pub ca_sha256: String,
}
impl NativeProfile {
    pub fn validate(&self, c: &webdock_object_storage::S3Config, now: i64) -> crate::Result<()> {
        let endpoint =
            url::Url::parse(&c.endpoint).map_err(|_| "Invalid archive fixture target")?;
        if self.run_id.len() != 12
            || !self
                .run_id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || endpoint.scheme() != "https"
            || endpoint.host_str() != Some(format!("{}.native-archives.test", self.run_id).as_str())
            || endpoint.port() != Some(19443)
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.path() != "/"
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
            || c.bucket != format!("wd-archives-{}", self.run_id)
            || c.namespace != "archives"
            || self.expires_at <= now
            || self.expires_at - now > 4 * 3600
            || self.ca_sha256.len() != 64
            || !self
                .ca_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("Invalid bounded archive fixture profile");
        }
        Ok(())
    }
    pub fn verify_ca(&self) -> crate::Result<()> {
        use sha2::{Digest, Sha256};
        use std::os::unix::fs::MetadataExt;
        let path = "/usr/local/share/ca-certificates/native-archives.crt";
        let meta =
            std::fs::symlink_metadata(path).map_err(|_| "Pinned archive fixture CA required")?;
        if !meta.is_file() || meta.uid() != 0 || meta.mode() & 0o022 != 0 || meta.len() > 65536 {
            return Err("Pinned archive fixture CA required");
        }
        let ca = std::fs::read(path).map_err(|_| "Pinned archive fixture CA unavailable")?;
        if hex::encode(Sha256::digest(ca)) != self.ca_sha256 {
            return Err("Archive fixture CA mismatch");
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixture_profile_cannot_point_at_other_origins_buckets_or_unbounded_runs() {
        let p = NativeProfile {
            run_id: "123456abcdef".into(),
            expires_at: 2000,
            ca_sha256: "a".repeat(64),
        };
        let c = webdock_object_storage::S3Config {
            id: "fixture".into(),
            endpoint: "https://123456abcdef.native-archives.test:19443".into(),
            region: "us-east-1".into(),
            bucket: "wd-archives-123456abcdef".into(),
            namespace: "archives".into(),
        };
        assert!(p.validate(&c, 1000).is_ok());
        for endpoint in [
            "http://123456abcdef.native-archives.test:19443",
            "https://other.invalid",
            "https://127.0.0.1:19443",
        ] {
            let mut c = c.clone();
            c.endpoint = endpoint.into();
            assert!(p.validate(&c, 1000).is_err());
        }
        let mut other = c.clone();
        other.bucket = "existing-customer".into();
        assert!(p.validate(&other, 1000).is_err());
        assert!(p.validate(&c, 2000).is_err());
        assert!(p.validate(&c, -20000).is_err());
    }
}
