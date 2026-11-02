use super::{
    client::{Client, Domain, Rows},
    *,
};
use serde_json::{Value, json};
impl DockerMailRuntime {
    async fn ready(
        &self,
        claim: &MailClaim,
        authority: &dyn MailAuthority,
        client: &Client,
        method: &str,
    ) -> Result<()> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            authority.guard(claim).await?;
            if client
                .call::<Value>(method, json!({"ids":null}))
                .await
                .is_ok()
            {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(ExecutionError::Runtime);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
    pub(super) async fn ensure_ready(
        &self,
        claim: &MailClaim,
        authority: &dyn MailAuthority,
    ) -> Result<MailProof> {
        let MailMaterial::Credentials {
            bootstrap_password,
            username,
            password,
        } = &claim.material
        else {
            return Err(ExecutionError::Denied);
        };
        let c = self
            .container(claim)
            .await?
            .ok_or(ExecutionError::Runtime)?;
        let bootstrapping = self.recovery(&c);
        if bootstrapping {
            let bootstrap = bootstrap_password
                .as_deref()
                .ok_or(ExecutionError::Ownership)?;
            // Restart before probing: a lost bootstrap response may already have written the durable store.
            self.stop(claim, authority).await?;
            self.start(claim, authority).await?;
            let (_, ip) = self.address(claim).await?;
            let admin = Client::new(ip, "admin", bootstrap)?;
            self.ready(claim, authority, &admin, "x:Bootstrap/get")
                .await?;
            if admin
                .call::<Rows<Domain>>("x:Domain/get", json!({"ids":null}))
                .await
                .is_err()
            {
                authority.guard(claim).await?;
                let _:Value=admin.call("x:Bootstrap/set",json!({"update":{"singleton":{"serverHostname":claim.spec.hostname,"defaultDomain":claim.spec.hostname,"requestTlsCertificate":false,"generateDkimKeys":false,"dataStore":{"@type":"RocksDb","path":"/data/store"},"tracer":{"@type":"Stdout","level":"warn"}}}})).await?;
                self.stop(claim, authority).await?;
                self.start(claim, authority).await?;
            }
            let (_, ip) = self.address(claim).await?;
            let admin = Client::new(ip, "admin", bootstrap)?;
            self.ready(claim, authority, &admin, "x:Domain/get").await?;
            super::super::engine::configure_account(claim, authority, &admin, username, password)
                .await?;
            super::super::engine::configure_listeners(claim, authority, &admin).await?;
            super::super::engine::configure_certificate(claim, authority, &admin).await?;
            // The checkpoint confirms credential custody; only the final proof establishes readiness.
            if claim.checkpoint.is_none() {
                authority
                    .checkpoint(
                        claim,
                        MailCheckpoint {
                            lease: claim.lease.clone(),
                            runtime_id: c.id.ok_or(ExecutionError::Runtime)?,
                            material_sha256: claim.spec.material_sha256.clone(),
                        },
                    )
                    .await?;
            }
            self.stop(claim, authority).await?;
            authority.guard(claim).await?;
            self.docker
                .remove_container(&self.name(claim), None::<RemoveContainerOptions>)
                .await
                .map_err(|_| ExecutionError::Runtime)?;
            self.create(claim, authority, false).await?;
            self.start(claim, authority).await?;
        }
        let (id, ip) = self.address(claim).await?;
        let permanent = Client::new(ip, username, password)?;
        self.ready(claim, authority, &permanent, "x:Domain/get")
            .await?;
        permanent.verify().await?;
        if let Some(bootstrap) = bootstrap_password
            && Client::new(ip, "admin", bootstrap)?.verify().await.is_ok()
        {
            return Err(ExecutionError::Ownership);
        }
        authority.guard(claim).await?;
        let pin = tls::certificate(ip).await?;
        if permanent.certificate_pin(&claim.spec.hostname).await? != pin {
            return Err(ExecutionError::Ownership);
        }
        let inspected = self
            .container(claim)
            .await?
            .ok_or(ExecutionError::Runtime)?;
        if self.recovery(&inspected) || inspected.id.as_ref() != Some(&id) {
            return Err(ExecutionError::Ownership);
        }
        if !bootstrapping && claim.checkpoint.is_none() {
            authority
                .checkpoint(
                    claim,
                    MailCheckpoint {
                        lease: claim.lease.clone(),
                        runtime_id: id.clone(),
                        material_sha256: claim.spec.material_sha256.clone(),
                    },
                )
                .await?;
        }
        authority.guard(claim).await?;
        Ok(self.proof(claim, id, true, Some(pin), Some(ip)))
    }
}
