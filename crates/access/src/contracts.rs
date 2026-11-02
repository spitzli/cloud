use crate::{Error, Result};
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CustomerType {
    Person,
    #[default]
    Company,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MemberRole {
    Owner,
    Admin,
    Member,
}
impl MemberRole {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
            Self::Member => "member",
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SiteRole {
    Admin,
    Editor,
    Reader,
}
impl SiteRole {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Editor => "editor",
            Self::Reader => "reader",
        }
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantProfileInput {
    pub customer_type: CustomerType,
    pub name: String,
    #[serde(default)]
    pub first_name: Option<String>,
    #[serde(default)]
    pub last_name: Option<String>,
    #[serde(default)]
    pub company_name: Option<String>,
    #[serde(default)]
    pub contact_name: Option<String>,
    #[serde(default)]
    pub contact_email: Option<String>,
    #[serde(default)]
    pub phone: Option<String>,
    #[serde(default)]
    pub address_line1: Option<String>,
    #[serde(default)]
    pub address_line2: Option<String>,
    #[serde(default)]
    pub postal_code: Option<String>,
    #[serde(default)]
    pub city: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub country: Option<String>,
}
impl TenantProfileInput {
    pub fn validate(mut self) -> Result<Self> {
        self.name = text(&self.name, 160)?;
        for field in [
            &mut self.first_name,
            &mut self.last_name,
            &mut self.company_name,
            &mut self.contact_name,
            &mut self.address_line1,
            &mut self.address_line2,
            &mut self.city,
            &mut self.region,
        ] {
            *field = optional_text(field.take(), 160)?;
        }
        self.contact_email = optional_text(self.contact_email, 254)?;
        if let Some(email) = &self.contact_email {
            valid_email(email)?;
        }
        self.phone = optional_text(self.phone, 50)?;
        self.postal_code = optional_text(self.postal_code, 32)?;
        self.country = optional_text(self.country, 2)?;
        if let Some(country) = &mut self.country {
            if country.len() != 2 || !country.bytes().all(|b| b.is_ascii_alphabetic()) {
                return Err(Error::InvalidInput);
            }
            *country = country.to_ascii_uppercase();
        }
        Ok(self)
    }
}
fn optional_text(value: Option<String>, max: usize) -> Result<Option<String>> {
    value
        .map(|v| {
            let v = v.trim();
            if v.encode_utf16().count() > max || v.chars().any(|c| c < ' ') {
                return Err(Error::InvalidInput);
            }
            Ok((!v.is_empty()).then(|| v.to_owned()))
        })
        .transpose()
        .map(Option::flatten)
}
pub(crate) fn text(value: &str, max: usize) -> Result<String> {
    let value = value.trim();
    if value.is_empty() || value.encode_utf16().count() > max || value.chars().any(|c| c < ' ') {
        return Err(Error::InvalidInput);
    }
    Ok(value.into())
}
pub(crate) fn valid_email(raw: &str) -> Result<String> {
    let email = raw.trim().to_lowercase();
    let Some((local, domain)) = email.split_once('@') else {
        return Err(Error::InvalidInput);
    };
    if email.len() > 254
        || local.is_empty()
        || domain.is_empty()
        || domain.contains('@')
        || !domain.contains('.')
        || domain.starts_with('.')
        || domain.ends_with('.')
        || email.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(Error::InvalidInput);
    }
    Ok(email)
}
pub(crate) fn id(raw: &str) -> Result<&str> {
    webdock_core::id::RecordId::parse(raw).map_err(|_| Error::InvalidInput)?;
    Ok(raw)
}
pub fn website_url(
    redirects: &[String],
    console_origins: &[url::Url],
    allow_local_http: bool,
) -> Option<String> {
    let reserved = |url: &url::Url| {
        [Some("studio.webdock.dev"), Some("admin.webdock.dev")].contains(&url.host_str())
            || console_origins
                .iter()
                .any(|origin| origin.origin() == url.origin())
    };
    if redirects
        .iter()
        .filter_map(|uri| url::Url::parse(uri).ok())
        .any(|url| reserved(&url))
    {
        return None;
    }
    redirects.iter().find_map(|uri| {
        let url = url::Url::parse(uri).ok()?;
        let local = allow_local_http
            && url.scheme() == "http"
            && [Some("localhost"), Some("127.0.0.1")].contains(&url.host_str());
        if !url.username().is_empty()
            || url.password().is_some()
            || (url.scheme() != "https" && !local)
            || url.path() != "/api/sso/callback"
        {
            return None;
        }
        url.join("/cms").ok().map(Into::into)
    })
}
pub struct AccessConfig {
    pub auth_origin: url::Url,
    pub console_origins: Vec<url::Url>,
    pub allow_local_http: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case", deny_unknown_fields)]
pub enum TenantCommand {
    Profile {
        profile: Box<TenantProfileInput>,
    },
    Invite {
        email: String,
        name: String,
        role: MemberRole,
    },
    Resend {
        invitation: String,
    },
    Cancel {
        invitation: String,
    },
    Role {
        member: String,
        role: MemberRole,
    },
    Remove {
        member: String,
    },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case", deny_unknown_fields)]
pub enum AccessCommand {
    CreateOrganization {
        name: String,
    },
    LinkWebsite {
        binding: String,
        organization: String,
    },
    Invite {
        binding: String,
        email: String,
        name: String,
        role: SiteRole,
    },
    ChangeAccess {
        grant: String,
        role: SiteRole,
    },
    RevokeAccess {
        grant: String,
    },
    Suspend {
        user: String,
        blocked: bool,
    },
    CancelInvite {
        id: String,
    },
}
#[derive(Debug, Serialize)]
pub struct ChangeResult {
    pub id: Option<String>,
    /// One-use setup or invitation link, returned to operators only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    pub message: String,
    pub delivery: Option<&'static str>,
}
#[derive(Debug, Default, Serialize)]
pub struct DeliveryBatch {
    pub delivered: u32,
    pub failed: u32,
    pub canceled: u32,
}

/// Explicit owner acting is opt-in; existing callers keep read-only previews.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewMode {
    #[default]
    ReadOnly,
    Owner,
}
impl PreviewMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::Owner => "owner",
        }
    }
}
