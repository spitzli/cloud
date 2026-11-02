//! Existing pinned Core channel; no public transport can ask for private source material.
use crate::{MAX_BYTES, Result, now};
use std::path::Path;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use webdock_export_protocol::{
    SourceKind, SourceRef,
    wire::{self, export_sources_client::ExportSourcesClient},
};
#[derive(Clone)]
pub struct Sources {
    client: ExportSourcesClient<tonic::transport::Channel>,
}
impl Sources {
    pub fn new(channel: tonic::transport::Channel) -> Self {
        Self {
            client: ExportSourcesClient::new(channel)
                .max_decoding_message_size(128 * 1024)
                .max_encoding_message_size(32768),
        }
    }
    pub async fn choices(&self, token: &str) -> Result<serde_json::Value> {
        let reply = self
            .client
            .clone()
            .sources(wire::Invocation {
                invocation_token: token.into(),
            })
            .await
            .map_err(|_| "Export sources unavailable")?
            .into_inner();
        if reply.choices_json.len() > 32768 {
            return Err("Export source metadata limit");
        }
        serde_json::from_slice(&reply.choices_json).map_err(|_| "Invalid export sources")
    }
    pub async fn bind(&self, token: &str, kind: &str, id: &str) -> Result<SourceRef> {
        let reply = self
            .client
            .clone()
            .bind(wire::SourceRequest {
                invocation_token: token.into(),
                kind: kind.into(),
                source_id: id.into(),
                reference_json: vec![],
            })
            .await
            .map_err(|_| "Export source admission denied")?
            .into_inner();
        let r: SourceRef =
            serde_json::from_slice(&reply.reference_json).map_err(|_| "Invalid source binding")?;
        r.validate()?;
        Ok(r)
    }
    pub async fn validate(&self, token: &str, source: &SourceRef) -> Result<()> {
        self.client
            .clone()
            .validate(wire::Validation {
                invocation_token: token.into(),
                storage_authorization_json: vec![],
                reference_json: serde_json::to_vec(source).map_err(|_| "Invalid source binding")?,
                producer_job_id: String::new(),
            })
            .await
            .map_err(|_| "Export source authority changed")?;
        Ok(())
    }
    pub async fn validate_download(
        &self,
        a: &webdock_plugin_protocol::storage::StorageAuthorization,
        source: Option<&SourceRef>,
        job: &str,
    ) -> Result<()> {
        self.client
            .clone()
            .validate(wire::Validation {
                invocation_token: String::new(),
                storage_authorization_json: serde_json::to_vec(a)
                    .map_err(|_| "Invalid transfer authority")?,
                reference_json: match source {
                    Some(source) => {
                        serde_json::to_vec(source).map_err(|_| "Invalid source binding")?
                    }
                    None => vec![],
                },
                producer_job_id: job.into(),
            })
            .await
            .map_err(|_| "Export completion or source authority changed")?;
        Ok(())
    }
    pub async fn produce(
        &self,
        token: &str,
        source: &SourceRef,
        root: &Path,
    ) -> Result<(SourceRef, tokio::fs::File)> {
        source.validate()?;
        let request = wire::SourceRequest {
            invocation_token: token.into(),
            kind: String::new(),
            source_id: String::new(),
            reference_json: serde_json::to_vec(source).map_err(|_| "Invalid source binding")?,
        };
        if source.kind == SourceKind::Postgres {
            let material = self
                .client
                .clone()
                .database(request)
                .await
                .map_err(|_| "Readonly database source is not reconciled or authorized")?
                .into_inner();
            if material.binding != source.binding
                || material.expires_at <= now()
                || material.username != format!("wd_export_{}", source.id)
                || material.database != format!("db_{}", source.id)
            {
                return Err("Readonly source identity changed");
            }
            let file = crate::producer::dump_managed(material, root).await?;
            self.validate(token, source).await?;
            return Ok((source.clone(), file));
        }
        let mut stream = self
            .client
            .clone()
            .stream(request)
            .await
            .map_err(|_| "Export source stream unavailable")?
            .into_inner();
        let first = stream
            .message()
            .await
            .map_err(|_| "Export source interrupted")?
            .ok_or("Export source missing binding")?;
        if !first.data.is_empty() {
            return Err("Export source missing binding");
        }
        let resolved: SourceRef =
            serde_json::from_slice(&first.reference_json).map_err(|_| "Invalid source binding")?;
        resolved.finished()?;
        if resolved.kind != source.kind
            || resolved.id != source.id
            || resolved.binding != source.binding
            || source
                .commit
                .as_ref()
                .is_some_and(|s| Some(s) != resolved.commit.as_ref())
        {
            return Err("Export source binding changed");
        }
        let file = tempfile::tempfile_in(root).map_err(|_| "Private source staging unavailable")?;
        let mut file = tokio::fs::File::from_std(file);
        let source_limit = if source.kind == SourceKind::BuildLog {
            webdock_export_protocol::MAX_BUILD_LOG_BYTES as u64
        } else {
            MAX_BYTES
        };
        let archive_limit = if source.kind == SourceKind::BuildLog {
            webdock_export_protocol::MAX_BUILD_LOG_GZIP_BYTES
        } else {
            MAX_BYTES
        };
        let mut bytes = 0u64;
        while let Some(chunk) = stream
            .message()
            .await
            .map_err(|_| "Export source interrupted")?
        {
            if !chunk.reference_json.is_empty()
                || chunk.data.len() > webdock_export_protocol::CHUNK_BYTES
            {
                return Err("Invalid export source frame");
            }
            bytes = bytes
                .checked_add(chunk.data.len() as u64)
                .ok_or("Export source limit")?;
            if bytes > source_limit {
                return Err("Export source limit");
            }
            file.write_all(&chunk.data)
                .await
                .map_err(|_| "Private source staging failed")?;
        }
        self.validate(token, &resolved).await?;
        file.rewind()
            .await
            .map_err(|_| "Private source staging failed")?;
        // Compress only a completed, authorized source; never publish an in-progress archive.
        let input = file.into_std().await;
        let output =
            tempfile::tempfile_in(root).map_err(|_| "Private source staging unavailable")?;
        let compressed = tokio::task::spawn_blocking(move || {
            use std::io::{Seek, Write};
            let mut input = input;
            let mut gzip = flate2::write::GzEncoder::new(output, flate2::Compression::default());
            std::io::copy(&mut input, &mut gzip).map_err(|_| "Source compression failed")?;
            gzip.flush().map_err(|_| "Source compression failed")?;
            let mut output = gzip.finish().map_err(|_| "Source compression failed")?;
            if output
                .metadata()
                .map_err(|_| "Source staging failed")?
                .len()
                > archive_limit
            {
                return Err("Compressed source limit");
            }
            output.rewind().map_err(|_| "Source staging failed")?;
            Ok(output)
        })
        .await
        .map_err(|_| "Source compression unavailable")??;
        Ok((resolved, tokio::fs::File::from_std(compressed)))
    }
}
