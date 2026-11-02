use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Domain {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub dkim_management: DkimManagement,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "@type", rename_all_fields = "camelCase")]
pub(super) enum DkimManagement {
    Manual {},
    Automatic {
        algorithms: BTreeMap<String, bool>,
        selector_template: String,
    },
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Account {
    pub id: String,
    #[serde(rename = "@type")]
    pub kind: AccountKind,
    pub name: String,
    pub domain_id: String,
    pub description: Option<String>,
    pub roles: Roles,
    pub permissions: Permissions,
    pub quotas: BTreeMap<String, u64>,
    #[serde(default)]
    pub member_group_ids: BTreeMap<String, bool>,
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum AccountKind {
    User,
    Group,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "@type", rename_all_fields = "camelCase")]
pub(super) enum Roles {
    User {},
    Admin {},
    Default {},
    Custom { role_ids: BTreeMap<String, bool> },
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "@type", rename_all_fields = "camelCase")]
pub(super) enum Permissions {
    Inherit {},
    Replace {
        enabled_permissions: BTreeMap<String, bool>,
        disabled_permissions: BTreeMap<String, bool>,
    },
    Merge {
        enabled_permissions: BTreeMap<String, bool>,
        disabled_permissions: BTreeMap<String, bool>,
    },
}
#[derive(Deserialize)]
pub(super) struct Query {
    pub ids: Vec<String>,
    pub total: usize,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Dkim {
    pub id: String,
    pub domain_id: String,
    pub selector: String,
    pub public_key: String,
    pub stage: String,
}
#[derive(Deserialize)]
pub(super) struct Created {
    pub created: BTreeMap<String, CreatedId>,
}
#[derive(Deserialize)]
pub(super) struct CreatedId {
    pub id: String,
}

pub(super) fn permissions(enabled: bool) -> Permissions {
    let enabled_permissions = if enabled {
        MAIL_PERMISSIONS
            .iter()
            .map(|name| ((*name).into(), true))
            .collect()
    } else {
        BTreeMap::new()
    };
    Permissions::Replace {
        enabled_permissions,
        disabled_permissions: BTreeMap::new(),
    }
}
pub(super) const MAIL_PERMISSIONS: &[&str] = &[
    "authenticate",
    "jmapCoreEcho",
    "jmapMailboxGet",
    "jmapMailboxChanges",
    "jmapMailboxQuery",
    "jmapMailboxQueryChanges",
    "jmapMailboxCreate",
    "jmapMailboxUpdate",
    "jmapMailboxDestroy",
    "jmapThreadGet",
    "jmapThreadChanges",
    "jmapEmailGet",
    "jmapEmailChanges",
    "jmapEmailQuery",
    "jmapEmailQueryChanges",
    "jmapEmailCreate",
    "jmapEmailUpdate",
    "jmapEmailDestroy",
    "jmapEmailImport",
    "jmapEmailParse",
    "jmapSearchSnippetGet",
    "jmapIdentityGet",
    "jmapIdentityChanges",
    "jmapEmailSubmissionGet",
    "jmapEmailSubmissionChanges",
    "jmapEmailSubmissionQuery",
    "jmapEmailSubmissionQueryChanges",
    "jmapQuotaGet",
    "jmapQuotaQuery",
    "jmapBlobGet",
    "jmapBlobUpload",
    "jmapBlobLookup",
];
pub(super) fn dkim_management() -> DkimManagement {
    DkimManagement::Automatic {
        algorithms: BTreeMap::from([("Dkim1RsaSha256".into(), true)]),
        selector_template: "webdock-{algorithm}-{date-%Y%m%d}".into(),
    }
}

pub(super) fn valid_dkim_public(
    key: &webdock_agent_protocol::mail_configuration::ConfigurationDkim,
) -> bool {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use x509_parser::prelude::FromDer;
    let Some(suffix) = key
        .selector
        .strip_prefix("webdock-rsa-")
        .or_else(|| key.selector.strip_prefix("v1-rsa-"))
    else {
        return false;
    };
    if suffix.is_empty()
        || suffix.len() > 40
        || !suffix
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        || key.id.is_empty()
        || key.id.len() > 64
        || !key
            .id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
    {
        return false;
    }
    let Ok(der) = STANDARD.decode(&key.public_key) else {
        return false;
    };
    if !(128..=1024).contains(&der.len()) || STANDARD.encode(&der) != key.public_key {
        return false;
    }
    let Ok((remaining, parsed)) = x509_parser::x509::SubjectPublicKeyInfo::from_der(&der) else {
        return false;
    };
    remaining.is_empty()
        && matches!(parsed.parsed(), Ok(x509_parser::public_key::PublicKey::RSA(key)) if key.key_size() >= 2048 && key.try_exponent().is_ok_and(|exponent| exponent >= 3 && exponent % 2 == 1))
}
#[cfg(test)]
mod tests {
    use super::*;
    use webdock_agent_protocol::mail_configuration::ConfigurationDkim;
    #[test]
    fn dkim_uses_a_real_bounded_rsa_public_key_not_base64_shape() {
        use base64::{Engine, engine::general_purpose::STANDARD};
        use rcgen::PublicKeyData;
        let mut key = ConfigurationDkim {
            id: "provider-key".into(),
            selector: "webdock-rsa-20261020".into(),
            public_key: include_str!("../../../../mail/tests/configuration_public_key.txt")
                .trim()
                .into(),
        };
        assert!(valid_dkim_public(&key));
        key.public_key = STANDARD.encode(vec![0; 294]);
        assert!(!valid_dkim_public(&key));
        key.public_key = STANDARD.encode(
            rcgen::KeyPair::generate()
                .unwrap()
                .subject_public_key_info(),
        );
        assert!(!valid_dkim_public(&key));
    }
}
