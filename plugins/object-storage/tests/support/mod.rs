//! Test-only databases on the fresh container owned by fixtures/postgres.py.
use serde_json::Value;
use tokio_postgres::Client;
use webdock_object_storage_plugin::store::Store;

pub fn database_url(database: &str) -> String {
    assert!(
        database.starts_with("wd_object_test_")
            && database
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    );
    let path = std::path::PathBuf::from(
        std::env::var_os("WEBDOCK_OBJECT_STORAGE_POSTGRES_CONFIG")
            .expect("run fixtures/postgres.py"),
    );
    let config: Value = serde_json::from_slice(
        &webdock_plugin_kit::read_file(&path, true).expect("private fixture config"),
    )
    .expect("fixture JSON");
    let mut url =
        url::Url::parse(config["url"].as_str().expect("fixture URL")).expect("fixture URL syntax");
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    url.set_path(database);
    url.to_string()
}
pub struct Database {
    pub name: String,
    pub url: String,
}
impl Database {
    pub async fn new() -> Self {
        let name = format!("wd_object_test_{}", uuid::Uuid::new_v4().simple());
        let url = database_url(&name);
        let mut admin = url::Url::parse(&url).unwrap();
        admin.set_path("postgres");
        let client = webdock_plugin_kit::postgres::connect(admin.as_str(), None)
            .await
            .expect("owned fixture connection");
        client
            .batch_execute(&format!("CREATE DATABASE {name}"))
            .await
            .expect("owned fixture database");
        Self { name, url }
    }
    pub async fn store(&self) -> Store {
        Store::connect(&self.url, None)
            .await
            .expect("metadata store")
    }
    pub async fn client(&self) -> Client {
        webdock_plugin_kit::postgres::connect(&self.url, None)
            .await
            .expect("owned fixture connection")
    }
}
pub async fn released(store: &Store) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match store.lock().await {
                Ok(fence) => {
                    drop(fence);
                    break;
                }
                Err("another storage operation holds the fence") => {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await
                }
                Err(error) => panic!("{error}"),
            }
        }
    })
    .await
    .expect("dropped fence must close its dedicated session");
    // The successful probe itself closed a session; let the server observe that drop too.
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
}
