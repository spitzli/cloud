//! Request-local oxide-auth primitives. Persistent grants are locked and committed by the service.
//! These slots never replace the database as the source of authority.
use crate::{
    AuthorizationRequest, CodeExchange, Error, RefreshExchange, Result, legacy_token_hash,
    random_token, types::Client,
};
use chrono::{DateTime, Utc};
use oxide_auth::{
    code_grant::{accesstoken, authorization, extensions::Pkce, refresh},
    primitives::{
        authorizer::Authorizer,
        grant::{Extensions, Grant},
        issuer::{IssuedToken, Issuer, RefreshedToken, TokenType},
        registrar::{
            BoundClient, Client as ProtocolClient, ClientMap, ClientUrl, ExactUrl, PreGrant,
            Registrar, RegistrarError,
        },
        scope::Scope,
    },
};
use std::borrow::Cow;
use subtle::ConstantTimeEq;

pub(crate) struct Engine {
    registrar: StoredRegistrar,
    authorizer: CodeSlot,
    pub tokens: TokenSlot,
    extension: PkceHandler,
}
struct StoredRegistrar {
    map: ClientMap,
    id: String,
    secret: Option<String>,
    allowed: Scope,
}
struct CodeSlot {
    expected: Option<String>,
    grant: Option<Grant>,
    pub created: Option<(String, Grant)>,
}
pub(crate) struct TokenSlot {
    expected_refresh: Option<String>,
    refresh_grant: Option<Grant>,
    pub issued: Option<IssuedToken>,
    pub granted: Option<Grant>,
    until: DateTime<Utc>,
    allow_refresh: bool,
}
struct PkceHandler;
impl Engine {
    pub fn new(client: &Client, until: DateTime<Utc>, allow_refresh: bool) -> Result<Self> {
        let allowed: Scope = client
            .scopes
            .join(" ")
            .parse()
            .map_err(|_| Error::InvalidScope)?;
        let mut redirects = client
            .redirects
            .iter()
            .map(|v| {
                ExactUrl::new(v.clone())
                    .map(Into::into)
                    .map_err(|_| Error::InvalidClient)
            })
            .collect::<Result<Vec<_>>>()?;
        if redirects.is_empty() {
            return Err(Error::InvalidClient);
        }
        let first = redirects.remove(0);
        let mut map = ClientMap::new();
        map.register_client(
            ProtocolClient::public(&client.id, first, allowed.clone())
                .with_additional_redirect_uris(redirects),
        );
        Ok(Self {
            registrar: StoredRegistrar {
                map,
                id: client.id.clone(),
                secret: client.secret.clone(),
                allowed,
            },
            authorizer: CodeSlot {
                expected: None,
                grant: None,
                created: None,
            },
            tokens: TokenSlot {
                expected_refresh: None,
                refresh_grant: None,
                issued: None,
                granted: None,
                until,
                allow_refresh,
            },
            extension: PkceHandler,
        })
    }
    pub fn validate(&mut self, request: &AuthorizationRequest) -> Result<authorization::Pending> {
        authorization::authorization_code(self, request).map_err(|_| Error::InvalidRequest)
    }
    pub fn authorize(&mut self, pending: authorization::Pending, subject: &str) -> Result<String> {
        pending
            .authorize(self, Cow::Borrowed(subject))
            .map(|u| u.to_string())
            .map_err(|_| Error::InvalidRequest)
    }
    pub fn created_code(&self) -> Result<&str> {
        self.authorizer
            .created
            .as_ref()
            .map(|(c, _)| c.as_str())
            .ok_or(Error::InvalidGrant)
    }
    pub fn load_code(&mut self, code: &str, grant: Grant) {
        self.authorizer.expected = Some(code.to_owned());
        self.authorizer.grant = Some(grant);
    }
    pub fn exchange(&mut self, request: &CodeExchange) -> Result<()> {
        accesstoken::access_token(self, request)
            .map(|_| ())
            .map_err(|_| Error::InvalidGrant)
    }
    pub fn load_refresh(&mut self, token: &str, grant: Grant) {
        self.tokens.expected_refresh = Some(token.into());
        self.tokens.refresh_grant = Some(grant);
    }
    pub fn refresh(&mut self, request: &RefreshExchange) -> Result<()> {
        refresh::refresh(self, request)
            .map(|_| ())
            .map_err(|_| Error::InvalidGrant)
    }
}
impl Registrar for StoredRegistrar {
    fn bound_redirect<'a>(
        &self,
        bound: ClientUrl<'a>,
    ) -> std::result::Result<BoundClient<'a>, RegistrarError> {
        self.map.bound_redirect(bound)
    }
    fn negotiate(
        &self,
        bound: BoundClient<'_>,
        scope: Option<Scope>,
    ) -> std::result::Result<PreGrant, RegistrarError> {
        let scope = scope.unwrap_or_else(|| self.allowed.clone());
        if !self.allowed.priviledged_to(&scope) {
            return Err(RegistrarError::Unspecified);
        }
        Ok(PreGrant {
            client_id: bound.client_id.into_owned(),
            redirect_uri: bound.redirect_uri.into_owned(),
            scope,
        })
    }
    fn check(&self, id: &str, pass: Option<&[u8]>) -> std::result::Result<(), RegistrarError> {
        if id != self.id {
            return Err(RegistrarError::Unspecified);
        }
        let valid = match (&self.secret, pass) {
            (None, None) => true,
            (Some(stored), Some(value)) => std::str::from_utf8(value).ok().is_some_and(|v| {
                bool::from(stored.as_bytes().ct_eq(legacy_token_hash(v).as_bytes()))
            }),
            _ => false,
        };
        if valid {
            Ok(())
        } else {
            Err(RegistrarError::Unspecified)
        }
    }
}
impl Authorizer for CodeSlot {
    fn authorize(&mut self, grant: Grant) -> std::result::Result<String, ()> {
        let code = random_token().map_err(|_| ())?;
        self.created = Some((code.clone(), grant));
        Ok(code)
    }
    fn extract(&mut self, code: &str) -> std::result::Result<Option<Grant>, ()> {
        Ok(if self.expected.as_deref() == Some(code) {
            self.grant.take()
        } else {
            None
        })
    }
}
impl Issuer for TokenSlot {
    fn issue(&mut self, grant: Grant) -> std::result::Result<IssuedToken, ()> {
        let token = IssuedToken {
            token: random_token().map_err(|_| ())?,
            refresh: if self.allow_refresh {
                Some(random_token().map_err(|_| ())?)
            } else {
                None
            },
            until: self.until,
            token_type: TokenType::Bearer,
        };
        self.granted = Some(grant);
        self.issued = Some(token.clone());
        Ok(token)
    }
    fn refresh(&mut self, token: &str, grant: Grant) -> std::result::Result<RefreshedToken, ()> {
        if self.expected_refresh.as_deref() != Some(token) {
            return Err(());
        }
        let issued = self.issue(grant)?;
        Ok(RefreshedToken {
            token: issued.token,
            refresh: issued.refresh,
            until: issued.until,
            token_type: TokenType::Bearer,
        })
    }
    fn recover_token<'a>(&'a self, _: &'a str) -> std::result::Result<Option<Grant>, ()> {
        Ok(None)
    }
    fn recover_refresh<'a>(&'a self, token: &'a str) -> std::result::Result<Option<Grant>, ()> {
        Ok(if self.expected_refresh.as_deref() == Some(token) {
            self.refresh_grant.clone()
        } else {
            None
        })
    }
}
impl authorization::Extension for PkceHandler {
    fn extend(
        &mut self,
        request: &dyn authorization::Request,
    ) -> std::result::Result<Extensions, ()> {
        let pkce = Pkce::required();
        let mut extensions = Extensions::new();
        let value = pkce
            .challenge(
                request.extension("code_challenge_method"),
                request.extension("code_challenge"),
            )?
            .ok_or(())?;
        extensions.set(&pkce, value);
        Ok(extensions)
    }
}
impl accesstoken::Extension for PkceHandler {
    fn extend(
        &mut self,
        request: &dyn accesstoken::Request,
        mut data: Extensions,
    ) -> std::result::Result<Extensions, ()> {
        let pkce = Pkce::required();
        pkce.verify(data.remove(&pkce), request.extension("code_verifier"))?;
        Ok(data)
    }
}
impl authorization::Endpoint for Engine {
    fn registrar(&self) -> &dyn Registrar {
        &self.registrar
    }
    fn authorizer(&mut self) -> &mut dyn Authorizer {
        &mut self.authorizer
    }
    fn extension(&mut self) -> &mut dyn authorization::Extension {
        &mut self.extension
    }
}
impl accesstoken::Endpoint for Engine {
    fn registrar(&self) -> &dyn Registrar {
        &self.registrar
    }
    fn authorizer(&mut self) -> &mut dyn Authorizer {
        &mut self.authorizer
    }
    fn issuer(&mut self) -> &mut dyn Issuer {
        &mut self.tokens
    }
    fn extension(&mut self) -> &mut dyn accesstoken::Extension {
        &mut self.extension
    }
}
impl refresh::Endpoint for Engine {
    fn registrar(&self) -> &dyn Registrar {
        &self.registrar
    }
    fn issuer(&mut self) -> &mut dyn Issuer {
        &mut self.tokens
    }
}
impl authorization::Request for AuthorizationRequest {
    fn valid(&self) -> bool {
        self.client_id.len() <= 128
            && self.redirect_uri.len() <= 4096
            && self.scope.len() <= 2048
            && self.state.as_ref().is_none_or(|s| s.len() <= 2048)
            && self.nonce.as_ref().is_none_or(|s| s.len() <= 512)
            && self.code_challenge.len() == 43
            && self.code_challenge_method == "S256"
    }
    fn client_id(&self) -> Option<Cow<'_, str>> {
        Some(Cow::Borrowed(&self.client_id))
    }
    fn scope(&self) -> Option<Cow<'_, str>> {
        (!self.scope.is_empty()).then_some(Cow::Borrowed(&self.scope))
    }
    fn redirect_uri(&self) -> Option<Cow<'_, str>> {
        Some(Cow::Borrowed(&self.redirect_uri))
    }
    fn state(&self) -> Option<Cow<'_, str>> {
        self.state.as_deref().map(Cow::Borrowed)
    }
    fn response_type(&self) -> Option<Cow<'_, str>> {
        Some(Cow::Borrowed(&self.response_type))
    }
    fn extension(&self, key: &str) -> Option<Cow<'_, str>> {
        match key {
            "code_challenge" => Some(Cow::Borrowed(&self.code_challenge)),
            "code_challenge_method" => Some(Cow::Borrowed(&self.code_challenge_method)),
            _ => None,
        }
    }
}
impl accesstoken::Request for CodeExchange {
    fn valid(&self) -> bool {
        (43..=128).contains(&self.code_verifier.len())
            && self
                .code_verifier
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
            && self.code.len() <= 4096
    }
    fn code(&self) -> Option<Cow<'_, str>> {
        Some(Cow::Borrowed(&self.code))
    }
    fn authorization(&self) -> accesstoken::Authorization<'_> {
        match &self.client.client_secret {
            Some(s) => accesstoken::Authorization::UsernamePassword(
                Cow::Borrowed(&self.client.client_id),
                Cow::Borrowed(s.as_bytes()),
            ),
            None => accesstoken::Authorization::None,
        }
    }
    fn client_id(&self) -> Option<Cow<'_, str>> {
        self.client
            .client_secret
            .is_none()
            .then_some(Cow::Borrowed(&self.client.client_id))
    }
    fn redirect_uri(&self) -> Option<Cow<'_, str>> {
        Some(Cow::Borrowed(&self.redirect_uri))
    }
    fn grant_type(&self) -> Option<Cow<'_, str>> {
        Some(Cow::Borrowed("authorization_code"))
    }
    fn extension(&self, key: &str) -> Option<Cow<'_, str>> {
        (key == "code_verifier").then_some(Cow::Borrowed(&self.code_verifier))
    }
}
impl refresh::Request for RefreshExchange {
    fn valid(&self) -> bool {
        self.refresh_token.len() <= 4096
    }
    fn refresh_token(&self) -> Option<Cow<'_, str>> {
        Some(Cow::Borrowed(&self.refresh_token))
    }
    fn scope(&self) -> Option<Cow<'_, str>> {
        self.scope.as_deref().map(Cow::Borrowed)
    }
    fn grant_type(&self) -> Option<Cow<'_, str>> {
        Some(Cow::Borrowed("refresh_token"))
    }
    fn authorization(&self) -> Option<(Cow<'_, str>, Cow<'_, [u8]>)> {
        self.client.client_secret.as_ref().map(|s| {
            (
                Cow::Borrowed(self.client.client_id.as_str()),
                Cow::Borrowed(s.as_bytes()),
            )
        })
    }
    fn extension(&self, _: &str) -> Option<Cow<'_, str>> {
        None
    }
}
pub(crate) fn grant(context: &crate::types::GrantContext, until: DateTime<Utc>) -> Result<Grant> {
    let request = &context.request;
    let mut extension = PkceHandler;
    let extensions = authorization::Extension::extend(&mut extension, request)
        .map_err(|_| Error::InvalidRequest)?;
    Ok(Grant {
        owner_id: context.subject.clone(),
        client_id: request.client_id.clone(),
        scope: request.scope.parse().map_err(|_| Error::InvalidScope)?,
        redirect_uri: url::Url::parse(&request.redirect_uri).map_err(|_| Error::InvalidRequest)?,
        until,
        extensions,
    })
}
