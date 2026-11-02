//! Managed first-factor transport. Provider credentials never become a browser/Core operator session.
use crate::{IdentityError, Result};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;

#[derive(Clone)]
pub struct NeonAuthClient {
    base: String,
    origin: String,
    http: reqwest::Client,
}
/// A bearer credential returned by the pinned provider. Deliberately no Debug/Serialize.
pub(crate) struct NeonLogin {
    pub(crate) token: String,
    pub(crate) subject: String,
    pub(crate) cookie: String,
}
#[derive(Deserialize)]
struct ProviderUser {
    id: String,
}
#[derive(Deserialize)]
struct ProviderLogin {
    token: String,
    user: ProviderUser,
}
impl NeonAuthClient {
    pub fn new(base: &str, origin: &str) -> Result<Self> {
        let url = url::Url::parse(base).map_err(|_| IdentityError::Configuration)?;
        if url.scheme() != "https"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.port().is_some()
            || !url
                .host_str()
                .is_some_and(|host| host.contains(".neonauth.") && host.ends_with(".neon.tech"))
            || url
                .path()
                .split('/')
                .filter(|part| !part.is_empty())
                .count()
                != 2
            || !url.path().ends_with("/auth")
        {
            return Err(IdentityError::Configuration);
        }
        let app = url::Url::parse(origin).map_err(|_| IdentityError::Configuration)?;
        crate::validate_account_origin(&app)?;
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| IdentityError::Configuration)?;
        Ok(Self {
            base: base.trim_end_matches('/').into(),
            origin: app.origin().ascii_serialization(),
            http,
        })
    }
    pub(crate) fn issuer(&self) -> &str {
        &self.base
    }
    async fn request(
        &self,
        path: &str,
        body: Value,
        cookie: Option<&str>,
    ) -> Result<(Value, String)> {
        let mut request = self
            .http
            .post(format!("{}{}", self.base, path))
            .header("origin", &self.origin)
            .header("accept", "application/json")
            .json(&body);
        if let Some(cookie) = cookie {
            request = request.header("cookie", cookie)
        }
        let mut response = request.send().await.map_err(|_| IdentityError::Worker)?;
        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(IdentityError::RateLimited);
        }
        if status.is_server_error() || status.is_redirection() {
            return Err(IdentityError::Worker);
        }
        if !status.is_success() {
            return Err(IdentityError::Unauthorized);
        }
        let cookies = response
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .filter_map(|value| value.split(';').next())
            .filter(|value| {
                value.split_once('=').is_some_and(|(name, _)| {
                    name.contains("neon-auth") || name.contains("better-auth")
                })
            })
            .collect::<Vec<_>>()
            .join("; ");
        if cookies.len() > 16384 {
            return Err(IdentityError::InvalidEncoding);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| IdentityError::Worker)? {
            if bytes.len() + chunk.len() > 65536 {
                return Err(IdentityError::InvalidEncoding);
            }
            bytes.extend_from_slice(&chunk)
        }
        let value = serde_json::from_slice(&bytes).map_err(|_| IdentityError::InvalidEncoding)?;
        Ok((value, cookies))
    }
    fn login(value: Value, cookie: String) -> Result<NeonLogin> {
        let value: ProviderLogin =
            serde_json::from_value(value).map_err(|_| IdentityError::InvalidEncoding)?;
        if !(16..=4096).contains(&value.token.len())
            || value.token.chars().any(char::is_control)
            || cookie.is_empty()
            || uuid::Uuid::parse_str(&value.user.id).is_err()
        {
            return Err(IdentityError::InvalidEncoding);
        }
        Ok(NeonLogin {
            token: value.token,
            subject: value.user.id,
            cookie,
        })
    }
    pub(crate) async fn sign_in(&self, email: &str, password: &str) -> Result<NeonLogin> {
        let (value, cookie) = self
            .request(
                "/sign-in/email",
                json!({"email":email,"password":password}),
                None,
            )
            .await?;
        Self::login(value, cookie)
    }
    pub(crate) async fn sign_up(
        &self,
        email: &str,
        password: &str,
        name: &str,
    ) -> Result<NeonLogin> {
        let (value, cookie) = self
            .request(
                "/sign-up/email",
                json!({"email":email,"password":password,"name":name}),
                None,
            )
            .await?;
        Self::login(value, cookie)
    }
    pub(crate) async fn sign_out(&self, cookie: &str) -> Result<()> {
        self.request("/sign-out", json!({}), Some(cookie)).await?;
        Ok(())
    }
    pub(crate) async fn change_password(
        &self,
        cookie: &str,
        current: &str,
        new: &str,
    ) -> Result<()> {
        self.request(
            "/change-password",
            json!({"currentPassword":current,"newPassword":new,"revokeOtherSessions":true}),
            Some(cookie),
        )
        .await?;
        Ok(())
    }
    pub(crate) async fn request_reset(&self, email: &str, redirect: &str) -> Result<()> {
        self.request(
            "/request-password-reset",
            json!({"email":email,"redirectTo":redirect}),
            None,
        )
        .await?;
        Ok(())
    }
    pub(crate) async fn reset_password(&self, token: &str, new: &str) -> Result<()> {
        self.request(
            "/reset-password",
            json!({"token":token,"newPassword":new}),
            None,
        )
        .await?;
        Ok(())
    }
}
