//! Recover only the fixed, non-recursive volume-root ownership initializer.
use super::*;
impl DockerMailRuntime {
    fn initializer_config(&self, claim: &MailClaim) -> Result<ContainerCreateBody> {
        Ok(ContainerCreateBody {
            image: Some(self.image.clone()),
            entrypoint: Some(vec!["/bin/chown".into()]),
            cmd: Some(vec!["65532:65532".into(), "/data".into()]),
            user: Some("0:0".into()),
            env: Some(vec![
                "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".into(),
                "STALWART_HEALTHCHECK_URL=https://127.0.0.1:443/healthz/live".into(),
            ]),
            labels: Some(self.labels(claim)?),
            host_config: Some(HostConfig {
                network_mode: Some("none".into()),
                readonly_rootfs: Some(true),
                cap_drop: Some(vec!["ALL".into()]),
                cap_add: Some(vec!["CHOWN".into()]),
                security_opt: Some(vec!["no-new-privileges:true".into()]),
                mounts: Some(vec![self.mount(claim)]),
                tmpfs: Some(HashMap::from([
                    (
                        "/etc/stalwart".into(),
                        "rw,nosuid,nodev,noexec,size=1048576".into(),
                    ),
                    (
                        "/var/lib/stalwart".into(),
                        "rw,nosuid,nodev,noexec,size=1048576".into(),
                    ),
                ])),
                memory: Some(16_777_216),
                memory_swap: Some(33_554_432),
                restart_policy: Some(RestartPolicy {
                    name: Some(RestartPolicyNameEnum::NO),
                    ..Default::default()
                }),
                log_config: Some(HostConfigLogConfig {
                    typ: Some("json-file".into()),
                    config: Some(HashMap::new()),
                }),
                nano_cpus: Some(100_000_000),
                pids_limit: Some(16),
                ..Default::default()
            }),
            ..Default::default()
        })
    }
    fn validate_initializer(
        &self,
        resource: &ContainerInspectResponse,
        claim: &MailClaim,
    ) -> Result<()> {
        let config = resource.config.as_ref().ok_or(ExecutionError::Ownership)?;
        self.owner(
            config.labels.as_ref().ok_or(ExecutionError::Ownership)?,
            claim,
        )?;
        let expected = self.initializer_config(claim)?;
        if config.image != expected.image
            || config.entrypoint != expected.entrypoint
            || config.cmd != expected.cmd
            || config.user != expected.user
            || config.env != expected.env
        {
            return Err(ExecutionError::Ownership);
        }
        self.host_matches(
            resource
                .host_config
                .as_ref()
                .ok_or(ExecutionError::Ownership)?,
            expected
                .host_config
                .as_ref()
                .ok_or(ExecutionError::Ownership)?,
        )?;
        let mounts = resource.mounts.as_ref().ok_or(ExecutionError::Ownership)?;
        if mounts.len() != 1
            || mounts[0].name.as_deref() != Some(&format!("{}-data", self.name(claim)))
            || mounts[0].destination.as_deref() != Some("/data")
            || mounts[0].typ.as_deref() != Some("volume")
            || mounts[0].rw != Some(true)
        {
            return Err(ExecutionError::Ownership);
        }
        Ok(())
    }
    async fn inspect_initializer(
        &self,
        claim: &MailClaim,
    ) -> Result<Option<ContainerInspectResponse>> {
        let name = format!("{}-init", self.name(claim));
        let resource = match self
            .docker
            .inspect_container(&name, None::<InspectContainerOptions>)
            .await
        {
            Ok(resource) => resource,
            Err(error) if missing(&error) => return Ok(None),
            Err(_) => return Err(ExecutionError::Runtime),
        };
        self.validate_initializer(&resource, claim)?;
        Ok(Some(resource))
    }
    pub(super) async fn initialize(
        &self,
        claim: &MailClaim,
        authority: &dyn MailAuthority,
    ) -> Result<()> {
        let name = format!("{}-init", self.name(claim));
        if self.inspect_initializer(claim).await?.is_none() {
            authority.guard(claim).await?;
            self.docker
                .create_container(
                    Some(CreateContainerOptionsBuilder::default().name(&name).build()),
                    self.initializer_config(claim)?,
                )
                .await
                .map_err(|_| ExecutionError::Runtime)?;
        }
        let resource = self
            .inspect_initializer(claim)
            .await?
            .ok_or(ExecutionError::Ownership)?;
        let id = resource.id.ok_or(ExecutionError::Ownership)?;
        let state = resource.state.ok_or(ExecutionError::Ownership)?;
        match state.status {
            Some(ContainerStateStatusEnum::CREATED) => {
                authority.guard(claim).await?;
                self.docker
                    .start_container(&id, None::<StartContainerOptions>)
                    .await
                    .map_err(|_| ExecutionError::Runtime)?;
                self.wait_initializer(&id, claim, authority).await?;
            }
            Some(ContainerStateStatusEnum::RUNNING) => {
                self.wait_initializer(&id, claim, authority).await?
            }
            Some(ContainerStateStatusEnum::EXITED) if state.exit_code == Some(0) => {}
            _ => return Err(ExecutionError::Runtime),
        }
        let finished = self
            .inspect_initializer(claim)
            .await?
            .ok_or(ExecutionError::Ownership)?;
        if finished.id.as_ref() != Some(&id)
            || finished.state.as_ref().and_then(|s| s.exit_code) != Some(0)
            || finished.state.as_ref().and_then(|s| s.status)
                != Some(ContainerStateStatusEnum::EXITED)
        {
            return Err(ExecutionError::Ownership);
        }
        authority.guard(claim).await?;
        self.docker
            .remove_container(&id, None::<RemoveContainerOptions>)
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        Ok(())
    }
    async fn wait_initializer(
        &self,
        id: &str,
        claim: &MailClaim,
        authority: &dyn MailAuthority,
    ) -> Result<()> {
        authority.guard(claim).await?;
        let exited = tokio::time::timeout(
            Duration::from_secs(10),
            self.docker
                .wait_container(id, None::<WaitContainerOptions>)
                .try_next(),
        )
        .await
        .map_err(|_| ExecutionError::Runtime)?
        .map_err(|_| ExecutionError::Runtime)?
        .ok_or(ExecutionError::Runtime)?;
        if exited.status_code != 0 {
            return Err(ExecutionError::Runtime);
        }
        Ok(())
    }
}
