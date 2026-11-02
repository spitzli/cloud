#[path = "database_common.rs"]
mod common;
pub use common::*;
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};
pub fn write_private(path: &Path, value: &serde_json::Value) {
    std::fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}
pub async fn client() -> kube::Client {
    let context = std::env::var("WEBDOCK_KUBE_TEST_CONTEXT").unwrap();
    assert!(context.starts_with("kind-webdock-rust-native-"));
    kube::Client::try_from(
        kube::Config::from_custom_kubeconfig(
            kube::config::Kubeconfig::read_from(std::env::var("WEBDOCK_KUBE_TEST_CONFIG").unwrap())
                .unwrap(),
            &kube::config::KubeConfigOptions {
                context: Some(context),
                ..Default::default()
            },
        )
        .await
        .unwrap(),
    )
    .unwrap()
}
pub struct RuntimeFixture {
    pub base: PathBuf,
    pub node: String,
    pub pool: String,
    pub cluster: String,
    pub runtime: PathBuf,
    pub socket: PathBuf,
    stop: tokio::sync::oneshot::Sender<()>,
    server: tokio::task::JoinHandle<webdock_supervisor::Result<()>>,
}
impl RuntimeFixture {
    pub async fn new(cluster: String) -> Self {
        use webdock_agent::storage::{StorageKind, StoragePool};
        assert_eq!(rustix::process::geteuid().as_raw(), 0);
        let base = tempfile::Builder::new()
            .prefix("webdock-native-db-owned-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap()
            .keep();
        let volumes = base.join("volumes");
        std::fs::create_dir(&volumes).unwrap();
        std::fs::set_permissions(&volumes, std::fs::Permissions::from_mode(0o700)).unwrap();
        let context = std::env::var("WEBDOCK_KUBE_TEST_CONTEXT").unwrap();
        let node = format!("{}-control-plane", context.trim_start_matches("kind-"));
        let pool = uuid::Uuid::new_v4().to_string();
        let storage = base.join("storage.json");
        write_private(
            &storage,
            &serde_json::json!({"root":volumes,"node":node,"clusterID":cluster,"poolID":pool,"capacityBytes":2147483648_u64,"reserveBytes":67108864}),
        );
        StoragePool::initialize(&storage).unwrap();
        let socket_root = Path::new("/run/webdock-postgres-storage");
        if !socket_root.exists() {
            std::fs::create_dir(socket_root).unwrap();
            std::fs::set_permissions(socket_root, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let socket = socket_root.join(format!("domain-{cluster}.sock"));
        let server_socket = socket.clone();
        let (stop, shutdown) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            webdock_agent::storage::serve_kind(
                &storage,
                &server_socket,
                StorageKind::Database,
                async {
                    let _ = shutdown.await;
                },
            )
            .await
        });
        for _ in 0..100 {
            if socket.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let runtime = base.join("database.json");
        write_private(
            &runtime,
            &serde_json::json!({"image":std::env::var("WEBDOCK_DATABASE_TEST_IMAGE").unwrap(),"poolerImage":std::env::var("WEBDOCK_DATABASE_TEST_POOLER_IMAGE").unwrap(),"issuer":"webdock-runtime-database-ca","apiCIDRs":serde_json::from_str::<serde_json::Value>(&std::env::var("WEBDOCK_DATABASE_TEST_API_CIDRS").unwrap()).unwrap(),"podCIDRs":["192.168.0.0/16"],"storageSocket":socket,"caFile":std::env::var("WEBDOCK_DATABASE_TEST_CA").unwrap()}),
        );
        Self {
            base,
            node,
            pool,
            cluster,
            runtime,
            socket,
            stop,
            server,
        }
    }
    pub async fn cleanup(self, backend: Option<String>) {
        use k8s_openapi::api::core::v1::{Namespace, PersistentVolume};
        use kube::{
            Api,
            api::{DeleteParams, Preconditions},
        };
        if let Some(id) = backend {
            let client = client().await;
            let namespace = format!("wd-db-{id}");
            let namespaces: Api<Namespace> = Api::all(client.clone());
            if let Some(object) = namespaces.get_opt(&namespace).await.unwrap() {
                assert_eq!(
                    object.metadata.labels.as_ref().unwrap()["webdock.dev/database-installation"],
                    id
                );
                namespaces
                    .delete(
                        &namespace,
                        &DeleteParams {
                            preconditions: Some(Preconditions {
                                uid: object.metadata.uid,
                                resource_version: object.metadata.resource_version,
                            }),
                            ..Default::default()
                        },
                    )
                    .await
                    .unwrap();
            }
            let mut gone = false;
            for _ in 0..240 {
                if namespaces.get_opt(&namespace).await.unwrap().is_none() {
                    gone = true;
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
            assert!(
                gone,
                "preserving exact owned pool until namespace termination"
            );
            let volumes: Api<PersistentVolume> = Api::all(client);
            if let Some(object) = volumes.get_opt(&namespace).await.unwrap() {
                assert_eq!(
                    object.metadata.labels.as_ref().unwrap()["webdock.dev/database-installation"],
                    id
                );
                volumes
                    .delete(
                        &namespace,
                        &DeleteParams {
                            preconditions: Some(Preconditions {
                                uid: object.metadata.uid,
                                resource_version: object.metadata.resource_version,
                            }),
                            ..Default::default()
                        },
                    )
                    .await
                    .unwrap();
            }
            let packet: webdock_agent::storage::database::DatabaseStoragePacket=serde_json::from_value(serde_json::json!({"version":1,"installationID":id,"clusterID":self.cluster,"revision":1,"node":self.node,"poolID":self.pool,"volumeBytes":536870912,"leaseUntil":chrono::Utc::now()+chrono::Duration::seconds(120)})).unwrap();
            let state =
                webdock_agent::storage::database::DatabaseStorageClient::new(self.socket.clone());
            if self.base.join("volumes").join(&id).exists() {
                state.purge(&packet).await.unwrap();
            }
        }
        let _ = self.stop.send(());
        self.server.await.unwrap().unwrap();
        std::fs::remove_dir_all(self.base).unwrap();
    }
}
