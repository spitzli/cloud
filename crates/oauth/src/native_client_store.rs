//! Explicit OAuth client/public-key reads. Grants and introspection keep their own boundaries.
use super::{OAuthService, authenticate_client, client_record};
use crate::{
    ClientAuthentication, Error, Result,
    types::{Client, ClientRecord},
};
use spitzli_persistence::postgres::Transaction;
use std::time::Duration;
use tokio_postgres::types::Type;
const BUDGET: Duration = Duration::from_secs(30);
pub(super) async fn load_client(tx: &Transaction<'_>, id: &str) -> Result<Client> {
    if tx
        .query_typed_opt(
            r#"SELECT "clientId" FROM webdock_auth."oauthClient" WHERE "clientId"=$1 FOR SHARE"#,
            &[(&id, Type::TEXT)],
        )
        .await
        .map_err(|_| Error::Configuration)?
        .is_none()
    {
        return Err(Error::InvalidClient);
    }
    // Read eligibility/credential/redirect/scope facts after acquiring the canonical lock.
    let row=tx.query_typed_opt(r#"SELECT "clientSecret",name,"tokenEndpointAuthMethod",metadata,"redirectUris","postLogoutRedirectUris",scopes,"grantTypes","skipConsent","enableEndSession" FROM webdock_auth."oauthClient" WHERE "clientId"=$1 AND NOT coalesce(disabled,false) AND NOT coalesce("dpopBoundAccessTokens",false) AND ("subjectType" IS NULL OR "subjectType"='public')"#,&[(&id,Type::TEXT)]).await.map_err(|_|Error::Configuration)?.ok_or(Error::InvalidClient)?;
    let record = (|| -> std::result::Result<ClientRecord, tokio_postgres::Error> {
        Ok(ClientRecord {
            secret: row.try_get("clientSecret")?,
            name: row.try_get("name")?,
            method: row.try_get("tokenEndpointAuthMethod")?,
            metadata: row.try_get("metadata")?,
            redirects: row.try_get("redirectUris")?,
            logout_redirects: row.try_get("postLogoutRedirectUris")?,
            scopes: row.try_get("scopes")?,
            grants: row.try_get("grantTypes")?,
            skip_consent: row.try_get("skipConsent")?,
            end_session: row.try_get("enableEndSession")?,
        })
    })()
    .map_err(|_| Error::Configuration)?;
    client_record(id, record)
}
impl OAuthService {
    pub(crate) async fn native_validate_portal(
        &self,
        credentials: &ClientAuthentication,
        redirect: &str,
    ) -> Result<()> {
        self.portal_credentials(credentials)?;
        tokio::time::timeout(BUDGET, async {
            let pool = self.native_client_pool().ok_or(Error::Configuration)?;
            let mut connection = pool.acquire().await.map_err(|_| Error::Configuration)?;
            let tx = connection
                .transaction()
                .await
                .map_err(|_| Error::Configuration)?;
            let tx = self
                .native_validate_portal_in(tx, credentials, redirect)
                .await?;
            tx.commit().await.map_err(|_| Error::Configuration)
        })
        .await
        .map_err(|_| Error::Configuration)?
    }
    pub(super) async fn native_validate_portal_in<'a>(
        &self,
        tx: Transaction<'a>,
        credentials: &ClientAuthentication,
        redirect: &str,
    ) -> Result<Transaction<'a>> {
        self.portal_credentials(credentials)?;
        tokio::time::timeout(BUDGET,async move{
   let client=load_client(&tx,&credentials.client_id).await?;authenticate_client(&client,credentials)?;
   if !client.redirects.iter().any(|uri|uri==redirect)||client.metadata.get("webdock_mcp").is_some()||client.metadata.get("webdock_hosting").is_some()||client.metadata.get("webdock_operator_only").is_some(){return Err(Error::InvalidClient);}
   let binding=client.metadata["webdock_binding"].as_str().ok_or(Error::InvalidClient)?;
   if tx.query_typed_opt("SELECT id FROM webdock_auth.app_binding WHERE id=$1 AND client_id=$2 AND enabled AND organization_id IS NULL",&[(&binding,Type::TEXT),(&client.id,Type::TEXT)]).await.map_err(|_|Error::Configuration)?.is_none(){return Err(Error::InvalidClient);}
   for scope in ["openid","profile","email"]{if !client.scopes.iter().any(|s|s==scope){return Err(Error::InvalidScope);}}
   Ok(tx)
  }).await.map_err(|_|Error::Configuration)?
    }
    pub(crate) async fn native_jwks(&self) -> Result<serde_json::Value> {
        tokio::time::timeout(BUDGET,async{
   let pool=self.native_client_pool().ok_or(Error::Configuration)?;let mut connection=pool.acquire().await.map_err(|_|Error::Configuration)?;let tx=connection.transaction().await.map_err(|_|Error::Configuration)?;
   let rows=tx.query_typed(r#"SELECT id,"publicKey" FROM webdock_auth.jwks WHERE alg='RS256' OR alg IS NULL ORDER BY "createdAt",id"#,&[]).await.map_err(|_|Error::Configuration)?;
   let records=rows.iter().map(|row|->std::result::Result<_,tokio_postgres::Error>{Ok((row.try_get::<_,String>("id")?,row.try_get::<_,String>("publicKey")?))}).collect::<std::result::Result<Vec<_>,_>>().map_err(|_|Error::Configuration)?;
   let public=crate::signing::public_jwks(records)?;tx.commit().await.map_err(|_|Error::Configuration)?;Ok(public)
  }).await.map_err(|_|Error::Configuration)?
    }
}
