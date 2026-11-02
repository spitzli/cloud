use crate::{
    Result,
    backend::Backend,
    diagnostics::Diagnostics,
    model::{Archive, Failure, Kind, State},
    now,
    producer::DatabaseSource,
    store::Store,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{os::unix::fs::MetadataExt, path::PathBuf, sync::Arc};
use tokio::sync::Semaphore;
use uuid::Uuid;
use webdock_plugin_protocol::storage::{
    Attachment, StorageAuthorization, StorageDirection, StoragePlan,
};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Create {
    request_id: Uuid,
    kind: Kind,
    retention_seconds: i64,
    diagnostics: Option<Value>,
    source_id: Option<String>,
}
pub struct Engine {
    store: Store,
    backend: Arc<Backend>,
    staging: PathBuf,
    permits: Semaphore,
}
impl Engine {
    pub async fn connect(
        database: &str,
        ca: Option<&[u8]>,
        backend: Arc<Backend>,
        staging: PathBuf,
        sources: Vec<DatabaseSource>,
    ) -> Result<Self> {
        let m =
            std::fs::symlink_metadata(&staging).map_err(|_| "Private archive staging required")?;
        if !staging.is_absolute()
            || !m.is_dir()
            || m.mode() & 0o077 != 0
            || m.uid() != rustix::process::geteuid().as_raw()
        {
            return Err("Private archive staging required");
        }
        if !sources.is_empty() {
            return Err("Legacy databaseSources are disabled; managed Core source required");
        }
        Ok(Self {
            store: Store::connect(database, ca).await?,
            backend,
            staging,
            permits: Semaphore::new(2),
        })
    }
    async fn owned(&self, customer: &str, project: &str, id: Uuid) -> Result<Archive> {
        let a = self.store.read(id).await?.ok_or("Archive unavailable")?;
        a.owned(customer, project)?;
        Ok(a)
    }
    pub async fn create(&self, customer: &str, project: &str, input: Value) -> Result<Value> {
        self.create_bound(customer, project, input, None).await
    }
    pub async fn create_bound(
        &self,
        customer: &str,
        project: &str,
        input: Value,
        core_source: Option<webdock_export_protocol::SourceRef>,
    ) -> Result<Value> {
        let c: Create =
            serde_json::from_value(input.clone()).map_err(|_| "Invalid archive request")?;
        if let Some(source) = &core_source {
            source.validate()?;
            let expected = match c.kind {
                Kind::Postgres => webdock_export_protocol::SourceKind::Postgres,
                Kind::Project => webdock_export_protocol::SourceKind::Project,
                Kind::Report => webdock_export_protocol::SourceKind::Report,
                Kind::BuildLog => webdock_export_protocol::SourceKind::BuildLog,
                Kind::Diagnostics => return Err("Invalid source kind"),
            };
            if source.kind != expected || c.source_id.as_deref() != Some(source.id.as_str()) {
                return Err("Source binding mismatch");
            }
        } else if c.source_id.is_some() {
            return Err("Source admission required");
        }
        if !(1..=86400).contains(&c.retention_seconds) {
            return Err("Archive expiry exceeds limit");
        }
        let diagnostics = match (c.kind, c.diagnostics) {
            (Kind::Diagnostics, Some(d)) => Some(Diagnostics::parse(d)?),
            (Kind::Postgres | Kind::Project | Kind::Report | Kind::BuildLog, None)
                if core_source.is_some() =>
            {
                None
            }
            _ => return Err("Archive producer unavailable"),
        };
        let hash = hex::encode(Sha256::digest(
            serde_json::to_vec(&json!([customer, project, input]))
                .map_err(|_| "Invalid archive request")?,
        ));
        let source_binding = core_source.as_ref().map(|source| source.binding.clone());
        let a = Archive {
            id: c.request_id,
            customer: customer.into(),
            project: project.into(),
            kind: c.kind,
            state: State::Pending,
            epoch: Uuid::new_v4().to_string(),
            expires_at: now() + c.retention_seconds,
            source_binding,
            core_source,
            job: None,
            failure: None,
            receipt: None,
            diagnostics,
        };
        let lock = self.store.lock(a.id).await?;
        lock.create(&a, &hash).await?;
        let a = lock.read(a.id).await?.ok_or("Archive unavailable")?;
        a.owned(customer, project)?;
        if a.state == State::Deleted {
            return Err("Archive deleted");
        }
        Ok(
            json!({"resource":{"id":a.id,"name":match a.kind {Kind::Diagnostics=>"Structured diagnostics",Kind::Postgres=>"PostgreSQL export",Kind::Project=>"Project source archive",Kind::Report=>"Project inventory report",Kind::BuildLog=>"Stored completed build log"}}}),
        )
    }
    pub async fn status(&self, customer: &str, project: &str, id: Uuid) -> Result<Value> {
        let a = self.owned(customer, project, id).await?;
        Ok(
            json!({"state":if a.expires_at<=now() && a.state!=State::Deleted {json!("expired")} else {json!(a.state)},"expires_at":a.expires_at,"kind":a.kind,"source":a.core_source,"failure":a.failure,"jobID":a.job,"bytes":a.receipt.as_ref().map(|r|r.size_bytes),"sha256":a.receipt.as_ref().map(|r|&r.sha256)}),
        )
    }
    pub async fn attach_job(
        &self,
        customer: &str,
        project: &str,
        id: Uuid,
        job: &str,
    ) -> Result<()> {
        let lock = self.store.lock(id).await?;
        let mut a = lock.read(id).await?.ok_or("Archive unavailable")?;
        a.owned(customer, project)?;
        if a.job.as_deref().is_some_and(|old| old != job) {
            return Err("Archive job identity changed");
        }
        a.job = Some(job.into());
        lock.save(&a).await
    }
    pub async fn core_source(
        &self,
        customer: &str,
        project: &str,
        id: Uuid,
    ) -> Result<Option<webdock_export_protocol::SourceRef>> {
        let archive = self.owned(customer, project, id).await?;
        Ok(archive.source()?.cloned())
    }
    pub async fn available(&self, customer: &str, project: &str, id: Uuid) -> Result<()> {
        let a = self.owned(customer, project, id).await?;
        a.source()?;
        if a.expires_at <= now() || matches!(a.state, State::Deleted | State::Deleting) {
            return Err("Archive unavailable");
        }
        Ok(())
    }
    pub async fn produce(
        &self,
        customer: &str,
        project: &str,
        id: Uuid,
        job: &str,
    ) -> Result<Value> {
        self.produce_bound(customer, project, id, job, None).await
    }
    pub async fn produce_bound(
        &self,
        customer: &str,
        project: &str,
        id: Uuid,
        job: &str,
        core: Option<(&crate::sources::Sources, &str)>,
    ) -> Result<Value> {
        let _permit = self
            .permits
            .try_acquire()
            .map_err(|_| "Archive producer busy")?;
        let lock = self.store.lock(id).await?;
        let mut a = lock.read(id).await?.ok_or("Archive unavailable")?;
        a.owned(customer, project)?;
        a.source()?;
        if a.expires_at <= now() || matches!(a.state, State::Deleting | State::Deleted) {
            return Err("Archive unavailable");
        }
        if a.state == State::Ready {
            if let Some(source) = &a.core_source {
                let (client, token) = core.ok_or("Core source required")?;
                client.validate(token, source).await?;
            }
            return Ok(json!({"state":"ready"}));
        }
        if a.job.as_deref().is_some_and(|j| j != job) && a.state == State::Producing {
            return Err("Another archive job owns this output");
        }
        a.state = State::Producing;
        a.failure = None;
        a.job = Some(job.into());
        lock.save(&a).await?;
        let result = async {
            if let Some(source) = a.core_source.clone() {
                let (client, token) = core.ok_or("Core export source unavailable")?;
                let work=client.produce(token,&source,&self.staging);tokio::pin!(work);
                let mut check=tokio::time::interval(std::time::Duration::from_millis(250));
                let (resolved,file)=loop {tokio::select! {value=&mut work=>break value?,_=check.tick()=>client.validate(token,&source).await?,}};
                a.core_source = Some(resolved.clone());
                lock.save(&a).await?;
                client.validate(token, &resolved).await?;
                let receipt = self.backend.upload(&a.scope(), file).await?;
                client.validate(token, &resolved).await?;
                return Ok(receipt);
            }
            match a.kind {
                Kind::Diagnostics => {
                    let bytes = a
                        .diagnostics
                        .as_ref()
                        .ok_or("Archive input unavailable")?
                        .compress()?;
                    self.backend.upload(&a.scope(), bytes.as_slice()).await
                }
                Kind::Postgres | Kind::Project | Kind::Report | Kind::BuildLog => Err("Core export source required"),
            }
        }
        .await;
        match result {
            Ok(receipt) => {
                a.receipt = Some(receipt);
                a.diagnostics = None;
                a.state = State::Ready;
                a.epoch = Uuid::new_v4().to_string();
                lock.save(&a).await?;
                Ok(json!({"state":"ready"}))
            }
            Err(e) => {
                a.state = State::Failed;
                a.failure = Some(Failure::from_error(e));
                lock.save(&a).await?;
                Err(e)
            }
        }
    }
    pub async fn plan(&self, customer: &str, project: &str, id: Uuid) -> Result<StoragePlan> {
        let a = self.owned(customer, project, id).await?;
        let r = a.ready(now())?;
        let expires_at = a.expires_at.min(now() + 240);
        let (url, headers) = self
            .backend
            .sign(&a.scope(), r, (expires_at - now()) as u64)
            .await?;
        let p = StoragePlan {
            version: 1,
            binding: "archives".into(),
            direction: StorageDirection::Download,
            resource_id: id.to_string(),
            resource_epoch: a.epoch.clone(),
            url,
            headers,
            expires_at,
            exact_bytes: r.size_bytes,
            expected_sha256: Some(r.sha256.clone()),
            attachment: Attachment {
                filename: match a.kind {
                    Kind::Postgres => "database.dump",
                    Kind::Diagnostics => "diagnostics.json.gz",
                    Kind::Project => "project.tar.gz",
                    Kind::Report => "project-report.csv.gz",
                    Kind::BuildLog => "build.log.gz",
                }
                .into(),
                content_type: match a.kind {
                    Kind::Postgres => "application/octet-stream",
                    Kind::Diagnostics | Kind::Project | Kind::Report | Kind::BuildLog => {
                        "application/gzip"
                    }
                }
                .into(),
            },
        };
        p.validate("download", &id.to_string(), now())
            .map_err(|_| "Archive plan unavailable")?;
        // Check the current epoch again after the provider call, without waiting on a mutation fence.
        let live = self.owned(customer, project, id).await?;
        if live.epoch != a.epoch {
            return Err("Archive authority changed");
        }
        live.ready(now())?;
        Ok(p)
    }
    pub async fn authorize(&self, a: &StorageAuthorization) -> Result<()> {
        self.backend.active()?;
        a.validate("archives", a.generation, now())
            .map_err(|_| "Archive unavailable")?;
        let id = a.resource_id.parse().map_err(|_| "Archive unavailable")?;
        self.owned(&a.customer_id, &a.project_id, id)
            .await?
            .authorize(a, now())
    }
    pub async fn delete(&self, customer: &str, project: &str, id: Uuid) -> Result<()> {
        let lock = self.store.lock(id).await?;
        let mut a = lock.read(id).await?.ok_or("Archive unavailable")?;
        a.owned(customer, project)?;
        if a.state == State::Deleted {
            return Ok(());
        }
        a.state = State::Deleting;
        a.epoch = Uuid::new_v4().to_string();
        lock.save(&a).await?;
        if let Some(r) = &a.receipt {
            self.backend.delete(&a.scope(), r).await?;
        }
        a.state = State::Deleted;
        a.diagnostics = None;
        lock.save(&a).await
    }
}
