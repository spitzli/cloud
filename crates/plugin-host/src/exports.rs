//! Fixed private source lane for the signed archives product. Never general SQL/HTTP/file access.
use super::*;
use base64::engine::general_purpose::STANDARD;
use futures_util::Stream;
use std::pin::Pin;
use webdock_export_protocol::{CHUNK_BYTES, MAX_BYTES, MAX_METADATA, SourceKind, SourceRef, wire};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportJob {
    claims: InvocationClaims,
    peer: ExportPeer,
    subject: String,
    session: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum ExportPeer {
    Unix { uid: u32 },
    Tls { leaf: String },
}
impl ExportJob {
    pub fn claims(&self) -> &InvocationClaims {
        &self.claims
    }
    pub fn subject(&self) -> &str {
        &self.subject
    }
    pub fn session(&self) -> &str {
        &self.session
    }
    pub fn snapshot(&self) -> Result<Value> {
        serde_json::to_value(self).map_err(|_| unavailable())
    }
}
#[async_trait]
pub trait ExportSources: Send + Sync + 'static {
    async fn choices(&self, _: &InvocationScope, _: &DatabaseTransaction) -> Result<Value> {
        Err(PluginError::denied())
    }
    async fn bind(
        &self,
        _: &InvocationScope,
        _: &DatabaseTransaction,
        _: SourceKind,
        _: &str,
    ) -> Result<SourceRef> {
        Err(PluginError::denied())
    }
    async fn validate(
        &self,
        _: &InvocationScope,
        _: &DatabaseTransaction,
        _: &SourceRef,
    ) -> Result<()> {
        Err(PluginError::denied())
    }
    async fn database(
        &self,
        _: &InvocationScope,
        _: &DatabaseTransaction,
        _: &SourceRef,
        _: i64,
    ) -> Result<wire::DatabaseMaterial> {
        Err(PluginError::denied())
    }
    async fn report(
        &self,
        _: &InvocationScope,
        _: &DatabaseTransaction,
        _: &SourceRef,
    ) -> Result<Vec<u8>> {
        Err(PluginError::denied())
    }
    async fn build_log(
        &self,
        _: &InvocationScope,
        _: &DatabaseTransaction,
        _: &SourceRef,
    ) -> Result<Vec<u8>> {
        Err(PluginError::denied())
    }
    async fn archive(
        &self,
        _: ExportJob,
        _: &InvocationScope,
        _: &SourceRef,
    ) -> Result<(SourceRef, Vec<u8>)> {
        Err(PluginError::denied())
    }
}
pub struct UnconfiguredExportSources;
#[async_trait]
impl ExportSources for UnconfiguredExportSources {}
impl HostService {
    pub async fn validate_export_job(&self, snapshot: &Value) -> Result<ExportJob> {
        let proof: ExportJob =
            serde_json::from_value(snapshot.clone()).map_err(|_| PluginError::denied())?;
        let peer = match &proof.peer {
            ExportPeer::Unix { uid } => PeerIdentity::Unix { uid: *uid },
            ExportPeer::Tls { leaf } => PeerIdentity::MutualTls {
                certificates: vec![STANDARD.decode(leaf).map_err(|_| PluginError::denied())?],
            },
        };
        let handle = InvocationHandle {
            claims: proof.claims.clone(),
            peer,
        };
        if handle.claims.plugin_id != "archives"
            || handle.claims.action != "archives.produce"
            || handle.claims.job.is_none()
        {
            return denied();
        }
        let context = self.authorized(&handle, None).await?;
        if context.scope.subject != proof.subject || context.scope.session_id != proof.session {
            return denied();
        }
        self.finish(context, &handle, None).await?;
        Ok(proof)
    }
    fn export_job(handle: &InvocationHandle, scope: &InvocationScope) -> Result<ExportJob> {
        if handle.claims.job.is_none() {
            return denied();
        }
        let peer = match &handle.peer {
            PeerIdentity::Unix { uid } => ExportPeer::Unix { uid: *uid },
            PeerIdentity::MutualTls { certificates } => ExportPeer::Tls {
                leaf: STANDARD.encode(certificates.first().ok_or_else(PluginError::denied)?),
            },
        };
        Ok(ExportJob {
            claims: handle.claims.clone(),
            peer,
            subject: scope.subject.clone(),
            session: scope.session_id.clone(),
        })
    }
    async fn export_handle(
        &self,
        peer: PeerIdentity,
        token: &str,
        job: bool,
    ) -> Result<InvocationHandle> {
        let h = self.authenticate(peer, token).await?;
        if h.claims.plugin_id != "archives"
            || if job {
                h.claims.action != "archives.produce" || h.claims.job.is_none()
            } else {
                h.claims.job.is_some()
                    || !["archives.create", "archives.sources", "archives.download"]
                        .contains(&h.claims.action.as_str())
            }
        {
            return denied();
        }
        Ok(h)
    }
    pub async fn export_completed_job(
        &self,
        scope: &InvocationScope,
        resource: &str,
        job: &str,
    ) -> Result<()> {
        let tx = self.controller().await?;
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT 1 AS ready FROM webdock_auth.plugin_job_execution j WHERE j.id=$1 AND j.plugin_id='archives' AND j.customer_id=$2 AND j.project_id=$3 AND j.name='archives.produce' AND j.payload->>'resource_id'=$4 AND j.status='succeeded' AND j.result->>'state'='ready'",vec![job.into(),scope.customer_id.clone().into(),scope.project_id.clone().into(),resource.into()])).await.map_err(db_error)?;
        if row.is_none() {
            return denied();
        }
        tx.commit().await.map_err(db_error)
    }
    pub fn export_service(
        self: Arc<Self>,
        sources: Arc<dyn ExportSources>,
    ) -> wire::export_sources_server::ExportSourcesServer<ExportRpc> {
        wire::export_sources_server::ExportSourcesServer::new(ExportRpc {
            host: self,
            sources,
        })
        .max_decoding_message_size(32768)
        .max_encoding_message_size(CHUNK_BYTES + 32768)
    }
}
pub struct ExportRpc {
    host: Arc<HostService>,
    sources: Arc<dyn ExportSources>,
}
fn failure(_: PluginError) -> tonic::Status {
    tonic::Status::permission_denied("Export source unavailable")
}
fn reference(raw: &[u8]) -> Result<SourceRef> {
    if raw.len() > MAX_METADATA {
        return denied();
    }
    let r: SourceRef = serde_json::from_slice(raw).map_err(|_| PluginError::denied())?;
    r.validate().map_err(|_| PluginError::denied())?;
    Ok(r)
}
fn encoded(v: &impl Serialize) -> Result<Vec<u8>> {
    let b = serde_json::to_vec(v).map_err(|_| unavailable())?;
    if b.len() > MAX_METADATA {
        return Err(unavailable());
    }
    Ok(b)
}
fn source_frames<F, Fut>(
    bytes: Vec<u8>,
    metadata: Vec<u8>,
    guard: F,
) -> Pin<Box<dyn Stream<Item = std::result::Result<wire::Chunk, tonic::Status>> + Send>>
where
    F: FnMut() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = std::result::Result<(), tonic::Status>> + Send,
{
    Box::pin(futures_util::stream::try_unfold(
        (bytes, 0usize, Some(metadata), guard),
        |(bytes, offset, metadata, mut guard)| async move {
            guard().await?;
            if let Some(metadata) = metadata {
                return Ok(Some((
                    wire::Chunk {
                        data: vec![],
                        reference_json: metadata,
                    },
                    (bytes, offset, None, guard),
                )));
            }
            if offset >= bytes.len() {
                return Ok(None);
            }
            let end = (offset + CHUNK_BYTES).min(bytes.len());
            let data = bytes[offset..end].to_vec();
            Ok(Some((
                wire::Chunk {
                    data,
                    reference_json: vec![],
                },
                (bytes, end, None, guard),
            )))
        },
    ))
}

#[tonic::async_trait]
impl wire::export_sources_server::ExportSources for ExportRpc {
    async fn sources(
        &self,
        request: tonic::Request<wire::Invocation>,
    ) -> std::result::Result<tonic::Response<wire::Choices>, tonic::Status> {
        let peer = webdock_plugin_runtime::authenticated_peer(&request)?;
        let r = request.into_inner();
        let h = self
            .host
            .export_handle(peer, &r.invocation_token, false)
            .await
            .map_err(failure)?;
        if h.claims.action != "archives.sources" {
            return Err(failure(PluginError::denied()));
        }
        let a = self.host.authorized(&h, None).await.map_err(failure)?;
        let v = self
            .sources
            .choices(&a.scope, &a.tx)
            .await
            .map_err(failure)?;
        self.host.finish(a, &h, None).await.map_err(failure)?;
        Ok(tonic::Response::new(wire::Choices {
            choices_json: encoded(&v).map_err(failure)?,
        }))
    }
    async fn bind(
        &self,
        request: tonic::Request<wire::SourceRequest>,
    ) -> std::result::Result<tonic::Response<wire::Binding>, tonic::Status> {
        let peer = webdock_plugin_runtime::authenticated_peer(&request)?;
        let r = request.into_inner();
        let h = self
            .host
            .export_handle(peer, &r.invocation_token, false)
            .await
            .map_err(failure)?;
        if h.claims.action != "archives.create" {
            return Err(failure(PluginError::denied()));
        }
        let kind = match r.kind.as_str() {
            "postgres" => SourceKind::Postgres,
            "project" => SourceKind::Project,
            "report" => SourceKind::Report,
            "buildlog" => SourceKind::BuildLog,
            _ => return Err(failure(PluginError::denied())),
        };
        let a = self.host.authorized(&h, None).await.map_err(failure)?;
        let v = self
            .sources
            .bind(&a.scope, &a.tx, kind, &r.source_id)
            .await
            .map_err(failure)?;
        self.host.finish(a, &h, None).await.map_err(failure)?;
        Ok(tonic::Response::new(wire::Binding {
            reference_json: encoded(&v).map_err(failure)?,
        }))
    }
    async fn database(
        &self,
        request: tonic::Request<wire::SourceRequest>,
    ) -> std::result::Result<tonic::Response<wire::DatabaseMaterial>, tonic::Status> {
        let peer = webdock_plugin_runtime::authenticated_peer(&request)?;
        let r = request.into_inner();
        let h = self
            .host
            .export_handle(peer, &r.invocation_token, true)
            .await
            .map_err(failure)?;
        let source = reference(&r.reference_json).map_err(failure)?;
        if source.kind != SourceKind::Postgres {
            return Err(failure(PluginError::denied()));
        }
        let a = self.host.authorized(&h, None).await.map_err(failure)?;
        self.sources
            .validate(&a.scope, &a.tx, &source)
            .await
            .map_err(failure)?;
        let v = self
            .sources
            .database(
                &a.scope,
                &a.tx,
                &source,
                h.claims.exp.min(chrono::Utc::now().timestamp() + 35),
            )
            .await
            .map_err(failure)?;
        self.host.finish(a, &h, None).await.map_err(failure)?;
        Ok(tonic::Response::new(v))
    }
    type StreamStream =
        Pin<Box<dyn Stream<Item = std::result::Result<wire::Chunk, tonic::Status>> + Send>>;
    async fn stream(
        &self,
        request: tonic::Request<wire::SourceRequest>,
    ) -> std::result::Result<tonic::Response<Self::StreamStream>, tonic::Status> {
        let peer = webdock_plugin_runtime::authenticated_peer(&request)?;
        let r = request.into_inner();
        let h = self
            .host
            .export_handle(peer, &r.invocation_token, true)
            .await
            .map_err(failure)?;
        let source = reference(&r.reference_json).map_err(failure)?;
        let a = self.host.authorized(&h, None).await.map_err(failure)?;
        let scope = a.scope.clone();
        self.sources
            .validate(&scope, &a.tx, &source)
            .await
            .map_err(failure)?;
        let report = if source.kind == SourceKind::Report {
            Some(
                self.sources
                    .report(&scope, &a.tx, &source)
                    .await
                    .map_err(failure)?,
            )
        } else if source.kind == SourceKind::BuildLog {
            let bytes = self
                .sources
                .build_log(&scope, &a.tx, &source)
                .await
                .map_err(failure)?;
            if bytes.len() > webdock_export_protocol::MAX_BUILD_LOG_BYTES {
                return Err(tonic::Status::resource_exhausted("Build log limit"));
            }
            Some(bytes)
        } else {
            None
        };
        self.host.finish(a, &h, None).await.map_err(failure)?;
        let (resolved, bytes) = match report {
            Some(bytes) => (source, bytes),
            None => {
                if source.kind != SourceKind::Project {
                    return Err(failure(PluginError::denied()));
                }
                let proof = HostService::export_job(&h, &scope).map_err(failure)?;
                let work = self.sources.archive(proof, &scope, &source);
                tokio::pin!(work);
                let mut poll = tokio::time::interval(std::time::Duration::from_millis(200));
                loop {
                    tokio::select! {v=&mut work=>break v.map_err(failure)?,_ = poll.tick()=>self.host.revalidate(&h).await.map_err(failure)?,}
                }
            }
        };
        if bytes.len() as u64 > MAX_BYTES {
            return Err(tonic::Status::resource_exhausted("Export source limit"));
        }
        self.host.revalidate(&h).await.map_err(failure)?;
        let host = self.host.clone();
        let sources = self.sources.clone();
        let meta = encoded(&resolved).map_err(failure)?;
        let handle = Arc::new(h);
        let stream = source_frames(bytes, meta, move || {
            let host = host.clone();
            let sources = sources.clone();
            let source = resolved.clone();
            let handle = handle.clone();
            async move {
                // Never retain a transaction/scope across frames: both source and original
                // invocation/job/browser/API authority must still hold when yielding bytes.
                let current = host.authorized(&handle, None).await.map_err(failure)?;
                sources
                    .validate(&current.scope, &current.tx, &source)
                    .await
                    .map_err(failure)?;
                host.finish(current, &handle, None).await.map_err(failure)
            }
        });
        Ok(tonic::Response::new(stream))
    }

    async fn validate(
        &self,
        request: tonic::Request<wire::Validation>,
    ) -> std::result::Result<tonic::Response<wire::Active>, tonic::Status> {
        let peer = webdock_plugin_runtime::authenticated_peer(&request)?;
        let r = request.into_inner();
        let source = if r.reference_json.is_empty() {
            None
        } else {
            Some(reference(&r.reference_json).map_err(failure)?)
        };
        if !r.storage_authorization_json.is_empty() {
            let a: webdock_plugin_protocol::storage::StorageAuthorization =
                serde_json::from_slice(&r.storage_authorization_json)
                    .map_err(|_| failure(PluginError::denied()))?;
            let (scope, tx) = self
                .host
                .export_storage_scope(peer, &a)
                .await
                .map_err(failure)?;
            self.host
                .export_completed_job(&scope, &a.resource_id, &r.producer_job_id)
                .await
                .map_err(failure)?;
            if let Some(source) = &source {
                source
                    .finished()
                    .map_err(|_| failure(PluginError::denied()))?;
                self.sources
                    .validate(&scope, &tx, source)
                    .await
                    .map_err(failure)?;
            }
            tx.commit().await.map_err(|_| failure(unavailable()))?;
        } else {
            let h = self
                .host
                .authenticate(peer, &r.invocation_token)
                .await
                .map_err(failure)?;
            if h.claims.plugin_id != "archives"
                || !["archives.produce", "archives.download", "archives.generate"]
                    .contains(&h.claims.action.as_str())
            {
                return Err(failure(PluginError::denied()));
            }
            let a = self.host.authorized(&h, None).await.map_err(failure)?;
            self.sources
                .validate(
                    &a.scope,
                    &a.tx,
                    source
                        .as_ref()
                        .ok_or_else(|| failure(PluginError::denied()))?,
                )
                .await
                .map_err(failure)?;
            self.host.finish(a, &h, None).await.map_err(failure)?;
        }
        Ok(tonic::Response::new(wire::Active { active: true }))
    }
}

#[cfg(test)]
mod stream_tests {
    use super::*;
    use futures_util::StreamExt;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    fn guarded(
        bytes: Vec<u8>,
        live: Arc<AtomicBool>,
        calls: Arc<AtomicUsize>,
    ) -> Pin<Box<dyn Stream<Item = std::result::Result<wire::Chunk, tonic::Status>> + Send>> {
        source_frames(bytes, b"bound-source".to_vec(), move || {
            let live = live.clone();
            let calls = calls.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                if live.load(Ordering::SeqCst) {
                    Ok(())
                } else {
                    Err(tonic::Status::permission_denied(
                        "source revoked while original job remains active",
                    ))
                }
            }
        })
    }
    #[tokio::test]
    async fn held_buildlog_stream_rechecks_source_after_creation_before_first_data() {
        let live = Arc::new(AtomicBool::new(true));
        let calls = Arc::new(AtomicUsize::new(0));
        let mut stream = guarded(
            b"actual stored build log".to_vec(),
            live.clone(),
            calls.clone(),
        );
        assert!(stream.next().await.unwrap().unwrap().data.is_empty());
        live.store(false, Ordering::SeqCst);
        assert!(
            stream.next().await.unwrap().is_err(),
            "a valid invocation/job alone must not release cached source bytes"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
    #[tokio::test]
    async fn held_metadata_and_multiframe_project_report_streams_recheck_each_frame() {
        let live = Arc::new(AtomicBool::new(true));
        let calls = Arc::new(AtomicUsize::new(0));
        let mut stream = guarded(vec![42; CHUNK_BYTES + 1], live.clone(), calls.clone());
        stream.next().await.unwrap().unwrap();
        assert_eq!(
            stream.next().await.unwrap().unwrap().data.len(),
            CHUNK_BYTES
        );
        live.store(false, Ordering::SeqCst);
        assert!(stream.next().await.unwrap().is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        let mut stream = guarded(vec![1], live, calls);
        assert!(
            stream.next().await.unwrap().is_err(),
            "metadata also rechecks a source revoked after stream construction"
        );
    }
}
