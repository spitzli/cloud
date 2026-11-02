use super::*;

impl DockerAdapter {
    pub(super) async fn inspect_stopped(&self, command: &Command) -> Result<ObservedState> {
        command.validate()?;
        self.validate(&command.spec)?;
        self.check_expected(command).await?;
        let container = self
            .owned(&command.spec, &command.spec.name())
            .await?
            .ok_or(ExecutionError::Ownership)?;
        fence(&container, command.spec.revision, command.generation)?;
        let runtime_id = container.id.ok_or(ExecutionError::Ownership)?;
        let mut observed = self.inspect(&command.spec).await?;
        if observed.runtime_id.as_deref() != Some(&runtime_id) {
            return Err(ExecutionError::Ownership);
        }
        if observed.phase != Phase::Stopped {
            return Err(ExecutionError::Runtime);
        }
        // As in Stop execution, this fresh proof acknowledges intent, not immutable image labels.
        observed.revision = command.spec.revision;
        Ok(observed)
    }
    pub(super) async fn inspect_retirement(&self, command: &Command) -> Result<ObservedState> {
        command.validate()?;
        let spec = &command.spec;
        self.validate(spec)?;
        for name in [spec.name(), format!("{}-init", spec.name())] {
            if self.owned(spec, &name).await?.is_some() {
                return Err(ExecutionError::Ownership);
            }
        }
        match self
            .docker
            .inspect_network(&network_name(spec), None::<InspectNetworkOptions>)
            .await
        {
            Err(DockerError::DockerResponseServerError {
                status_code: 404, ..
            }) => {}
            Ok(_) => return Err(ExecutionError::Ownership),
            Err(_) => return Err(ExecutionError::Runtime),
        }
        let consumers = self
            .docker
            .list_containers(Some(
                ListContainersOptionsBuilder::default()
                    .all(true)
                    .filters(&HashMap::from([(
                        "volume".to_owned(),
                        vec![volume_name(spec)],
                    )]))
                    .build(),
            ))
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        if !consumers.is_empty() {
            return Err(ExecutionError::Ownership);
        }
        let volume = self.lifecycle_volume(spec).await?;
        let storage = match &spec.volume {
            None => None,
            Some(desired) => {
                let purged = command.action == Action::PurgeStorage;
                if volume.is_some() == purged {
                    return Err(ExecutionError::Ownership);
                }
                if let Some(bytes) = desired.size_bytes {
                    let (helper, config) =
                        self.storage.as_ref().ok_or(ExecutionError::Ownership)?;
                    if helper.information().await? != *config {
                        return Err(ExecutionError::Ownership);
                    }
                    let packet = crate::storage::StoragePacket::new(
                        spec.resource_id.as_str(),
                        spec.project_id.as_str(),
                        config.cluster_id.as_str(),
                        spec.revision,
                        bytes,
                        command.lease_until,
                    )?;
                    let expected = if purged {
                        crate::storage::VolumeState::Purged
                    } else {
                        crate::storage::VolumeState::Ready
                    };
                    if helper.volume_state(&packet).await? != expected {
                        return Err(ExecutionError::Ownership);
                    }
                }
                Some(StorageObservation {
                    volume_bytes: if purged { Some(0) } else { desired.size_bytes },
                    retained: !purged,
                })
            }
        };
        Ok(ObservedState {
            resource_id: spec.resource_id.to_string(),
            revision: spec.revision,
            phase: Phase::Absent,
            ready: true,
            observed_at: timestamp()?,
            runtime_id: None,
            conditions: vec![],
            storage,
        })
    }
}
