//! Offline private-RPC byte-contract tests; this mock does not qualify Core authority.
use std::pin::Pin;
use webdock_export_protocol::{SourceKind, SourceRef, wire};
#[derive(Clone)]
struct Source {
    bytes: Vec<u8>,
    active: bool,
}
fn reference() -> SourceRef {
    SourceRef {
        version: 1,
        kind: SourceKind::BuildLog,
        id: "125".into(),
        binding: "a".repeat(64),
        commit: None,
    }
}
#[tonic::async_trait]
impl wire::export_sources_server::ExportSources for Source {
    async fn sources(
        &self,
        _: tonic::Request<wire::Invocation>,
    ) -> Result<tonic::Response<wire::Choices>, tonic::Status> {
        Err(tonic::Status::unimplemented("unused mock method"))
    }
    async fn bind(
        &self,
        _: tonic::Request<wire::SourceRequest>,
    ) -> Result<tonic::Response<wire::Binding>, tonic::Status> {
        Err(tonic::Status::unimplemented("unused mock method"))
    }
    async fn database(
        &self,
        _: tonic::Request<wire::SourceRequest>,
    ) -> Result<tonic::Response<wire::DatabaseMaterial>, tonic::Status> {
        Err(tonic::Status::unimplemented("unused mock method"))
    }
    type StreamStream =
        Pin<Box<dyn futures_util::Stream<Item = Result<wire::Chunk, tonic::Status>> + Send>>;
    async fn stream(
        &self,
        _: tonic::Request<wire::SourceRequest>,
    ) -> Result<tonic::Response<Self::StreamStream>, tonic::Status> {
        Ok(tonic::Response::new(Box::pin(futures_util::stream::iter(
            [
                Ok(wire::Chunk {
                    reference_json: serde_json::to_vec(&reference()).unwrap(),
                    data: vec![],
                }),
                Ok(wire::Chunk {
                    reference_json: vec![],
                    data: self.bytes.clone(),
                }),
            ],
        ))))
    }
    async fn validate(
        &self,
        _: tonic::Request<wire::Validation>,
    ) -> Result<tonic::Response<wire::Active>, tonic::Status> {
        if self.active {
            Ok(tonic::Response::new(wire::Active { active: true }))
        } else {
            Err(tonic::Status::permission_denied("fixture source revoked"))
        }
    }
}
struct Server(tokio::task::JoinHandle<()>);
impl Drop for Server {
    fn drop(&mut self) {
        self.0.abort();
    }
}
async fn produce(
    bytes: Vec<u8>,
    active: bool,
) -> (
    tempfile::TempDir,
    webdock_archives::Result<(SourceRef, tokio::fs::File)>,
) {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = directory.path().join("core.sock");
    let mut socket = webdock_plugin_runtime::UnixSocket::bind(&path).unwrap();
    let incoming = socket.incoming().unwrap();
    let service = wire::export_sources_server::ExportSourcesServer::new(Source { bytes, active });
    let _server = Server(tokio::spawn(async move {
        let _socket = socket;
        tonic::transport::Server::builder()
            .add_service(service)
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    }));
    let channel = webdock_plugin_runtime::unix_channel(&path).await.unwrap();
    let result = webdock_archives::sources::Sources::new(channel)
        .produce("offline-typed-byte-fixture", &reference(), directory.path())
        .await;
    (directory, result)
}
#[tokio::test]
async fn exact_stored_utf8_bytes_are_gzipped_without_claiming_further_redaction() {
    use tokio::io::AsyncReadExt;
    let bytes = "é".repeat(12_000).into_bytes();
    let (_directory, result) = produce(bytes.clone(), true).await;
    let (source, mut file) = result.unwrap();
    assert_eq!(source, reference());
    let mut gzip = Vec::new();
    file.read_to_end(&mut gzip).await.unwrap();
    assert!(gzip.len() <= 65536);
    let mut decoded = Vec::new();
    std::io::Read::read_to_end(
        &mut flate2::read::GzDecoder::new(gzip.as_slice()),
        &mut decoded,
    )
    .unwrap();
    assert_eq!(decoded, bytes);
}
#[tokio::test]
async fn oversized_or_revoked_core_source_never_yields_a_finished_archive() {
    let (_directory, result) = produce(vec![b'x'; 24_001], true).await;
    assert!(result.is_err());
    let (_directory, result) = produce(b"stored output".to_vec(), false).await;
    assert!(result.is_err());
}
