use sea_orm::{Database, DatabaseConnection};
use std::{collections::BTreeMap, process::Command};
#[path = "local_postgres.rs"]
mod local_postgres;
fn docker() -> Command {
    let mut command = Command::new("docker");
    command.args(["--host", "unix:///var/run/docker.sock"]);
    command
}
pub struct DatabaseFixture {
    name: Option<String>,
    pub url: url::Url,
    passwords: BTreeMap<String, String>,
    local: Option<local_postgres::LocalPostgres>,
}
impl Drop for DatabaseFixture {
    fn drop(&mut self) {
        if let Some(name) = &self.name {
            let _ = docker().args(["rm", "-f", name]).output();
        }
    }
}
impl DatabaseFixture {
    pub async fn start() -> Self {
        let local = std::env::var_os("WEBDOCK_PLUGIN_TEST_LOCAL_POSTGRES").map(|value| {
            assert_eq!(value, "1", "local PostgreSQL fixture switch accepts only 1");
            local_postgres::LocalPostgres::start()
        });
        let (name, url) = if let Some(local) = &local {
            (None, local.url.clone())
        } else {
            let name = format!("wd-plugin-jobs-{}", uuid::Uuid::new_v4().simple());
            assert!(
                docker()
                    .args([
                        "run",
                        "-d",
                        "--rm",
                        "--name",
                        &name,
                        "-e",
                        "POSTGRES_HOST_AUTH_METHOD=trust",
                        "-e",
                        "POSTGRES_DB=webdock_jobs_test",
                        "-p",
                        "127.0.0.1::5432",
                        "postgres:18-alpine"
                    ])
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
            let output = docker().args(["port", &name, "5432/tcp"]).output().unwrap();
            let endpoint = String::from_utf8(output.stdout).unwrap();
            let endpoint = endpoint.trim();
            let url = url::Url::parse(&format!(
                "postgres://postgres@{endpoint}/webdock_jobs_test?sslmode=disable"
            ))
            .unwrap();
            (Some(name), url)
        };
        let passwords = webdock_persistence::roles::LOGIN_ROLES
            .iter()
            .copied()
            .chain(["webdock_offline_migrate"])
            .map(|role| {
                (
                    role.to_owned(),
                    format!("private-{}", uuid::Uuid::new_v4().simple()),
                )
            })
            .collect();
        let fixture = Self {
            name,
            url,
            passwords,
            local,
        };
        let db = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                if let Ok(db) = Database::connect(fixture.url.as_str()).await {
                    break db;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("owned PostgreSQL connection readiness");
        if let Some(local) = &fixture.local {
            local.verify(&db).await;
        }
        let database = fixture.url.path().trim_start_matches('/');
        webdock_persistence::roles::provision_roles(&db, database, &fixture.passwords)
            .await
            .unwrap();
        webdock_persistence::migrations::bootstrap(
            &db,
            database,
            "owned-plugin-jobs-rls-secret-32bytes",
        )
        .await
        .unwrap();
        fixture
    }
    pub async fn owner(&self) -> DatabaseConnection {
        Database::connect(self.url.as_str()).await.unwrap()
    }
    pub async fn role(&self, role: &str) -> DatabaseConnection {
        self.role_with_connections(role, 1).await
    }
    pub async fn role_with_connections(&self, role: &str, connections: u32) -> DatabaseConnection {
        let mut url = self.url.clone();
        url.set_username(role).unwrap();
        url.set_password(Some(&self.passwords[role])).unwrap();
        let mut options = sea_orm::ConnectOptions::new(url.as_str());
        options
            .max_connections(connections)
            .min_connections(0)
            .sqlx_logging(false);
        Database::connect(options).await.unwrap()
    }
}
use super::*;
pub struct JobFixture {
    pub host: Arc<HostService>,
    pub owner: DatabaseConnection,
    pub auth: DatabaseConnection,
    pub identity: Arc<IdentityService>,
    pub actor: webdock_identity::Principal,
    pub manifest: Value,
    pub directory: tempfile::TempDir,
    pub database: DatabaseFixture,
}
impl JobFixture {
    pub async fn start() -> Self {
        Self::start_with_connections(1).await
    }
    pub async fn start_with_connections(connections: u32) -> Self {
        let fixture = fixture::DatabaseFixture::start().await;
        let owner = fixture.owner().await;
        sql(&owner,"INSERT INTO webdock_auth.\"user\"(id,name,email,\"emailVerified\",role,\"twoFactorEnabled\",\"mustChangePassword\") VALUES('operator','Operator','jobs@example.test',true,'operator',true,false)",vec![]).await;
        sql(&owner,"INSERT INTO webdock_auth.session(id,token,\"userId\",\"expiresAt\",\"updatedAt\") VALUES('session','owned-token','operator',now()+interval '1 hour',now())",vec![]).await;
        sql(&owner,"INSERT INTO webdock_admin.users(id,name,email,role,auth_subject) VALUES('100','Operator','jobs@example.test','operator','operator')",vec![]).await;
        let seed = owner.begin().await.unwrap();
        seed.execute_unprepared("SELECT set_config('webdock.organization_allocation','{\"customer_id\":\"123\",\"organization_id\":\"125\"}',true)").await.unwrap();
        seed.execute_unprepared(
            "INSERT INTO webdock_admin.customers(id,name) VALUES('123','Jobs')",
        )
        .await
        .unwrap();
        seed.commit().await.unwrap();
        sql(
            &owner,
            "INSERT INTO webdock_admin.projects(id,name,customer_id) VALUES('124','Jobs','123')",
            vec![],
        )
        .await;
        let mut manifest: Value = serde_json::from_str(include_str!(
            "../../../plugin-protocol/tests/fixtures/project-notes.json"
        ))
        .unwrap();
        manifest["name"] = json!("job-fixture");
        manifest["jobs"] = json!([{"name":"work","capabilities":["plugin.data.write"]}]);
        manifest["capabilities"] = json!([
            "plugin.data.read",
            "plugin.data.write",
            "plugin.jobs.enqueue",
            "plugin.jobs.read",
            "plugin.jobs.cancel"
        ]);
        manifest["services"]["required"] =
            json!([{"name":"data","version":1},{"name":"jobs","version":1}]);
        manifest["actions"] = json!([
            {"name":"start","intent":"write","audience":"operator","capabilities":["plugin.jobs.enqueue","plugin.jobs.read","plugin.jobs.cancel"]},
            {"name":"inspect","intent":"read","audience":"operator","capabilities":["plugin.jobs.read"]},
            {"name":"cancel","intent":"write","audience":"operator","capabilities":["plugin.jobs.cancel"]}
        ]);
        manifest["areas"]["selfService"] = json!({"pages":[]});
        manifest["selfService"]["features"] = json!([{"id":"run","label":"Run"}]);
        manifest["actions"].as_array_mut().unwrap().push(json!({"name":"customer-start","intent":"write","audience":"customer","feature":"run","resource":"use","capabilities":["plugin.jobs.enqueue"],"input":{"type":"object","properties":{"resource_id":{"type":"string","maxLength":128}},"required":["resource_id"],"additionalProperties":false}}));
        let raw = manifest.to_string();
        use sha2::{Digest, Sha256};
        let hash = hex::encode(Sha256::digest(raw.as_bytes()));
        let credential = "a".repeat(64);
        sql(&owner,"INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version,runtime,current_generation) VALUES('job-fixture','external','enabled','enabled','1.0.0','ready',1)",vec![]).await;
        sql(&owner,"INSERT INTO webdock_auth.plugin_release(plugin_id,version,manifest_bytes,manifest_sha256,image,registry_url,signer_key_id) VALUES('job-fixture','1.0.0',$1,$2,$3,'https://registry.invalid','fixture')",vec![raw.into(),hash.into(),manifest["image"].as_str().unwrap().into()]).await;
        sql(&owner,"INSERT INTO webdock_auth.plugin_configuration(plugin_id,approved_capabilities,configuration) VALUES('job-fixture',$1,'{}')",vec![manifest["capabilities"].clone().into()]).await;
        sql(&owner,"INSERT INTO webdock_auth.plugin_runtime(plugin_id,generation,version,image,status,configuration_revision,credential_hash,namespace,service_name) VALUES('job-fixture',1,'1.0.0',$1,'ready',0,$2,'fixtures','fixture-plugin')",vec![manifest["image"].as_str().unwrap().into(),credential.clone().into()]).await;
        use std::os::unix::fs::PermissionsExt;
        sql(&owner,"INSERT INTO webdock_auth.plugin_resource(plugin_id,resource_id,offering_id,customer_id,project_id,name,created_via,created_by) VALUES('job-fixture','resource-one','unit','123','124','Resource','operator','operator')",vec![]).await;
        sql(&owner,"INSERT INTO webdock_auth.plugin_feature_policy(plugin_id,feature_id,mode,updated_by) VALUES('job-fixture','run','everyone','operator')",vec![]).await;
        sql(&owner,"INSERT INTO webdock_auth.\"user\"(id,name,email,\"emailVerified\",role,\"twoFactorEnabled\",\"mustChangePassword\") VALUES('member','Member','member@example.test',true,'user',true,false)",vec![]).await;
        sql(&owner,"INSERT INTO webdock_auth.session(id,token,\"userId\",\"expiresAt\",\"updatedAt\") VALUES('member-session','owned-member','member',now()+interval '1 hour',now())",vec![]).await;
        sql(&owner,"INSERT INTO webdock_auth.member(id,\"organizationId\",\"userId\",role,\"createdAt\") VALUES('member-binding','125','member','owner',now())",vec![]).await;
        sql(&owner,"INSERT INTO webdock_auth.\"user\"(id,name,email,\"emailVerified\",role,\"twoFactorEnabled\",\"mustChangePassword\") VALUES('backup-owner','Backup','backup@example.test',true,'user',true,false)",vec![]).await;
        sql(&owner,"INSERT INTO webdock_auth.member(id,\"organizationId\",\"userId\",role,\"createdAt\") VALUES('backup-binding','125','backup-owner','owner',now())",vec![]).await;
        let directory = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let path = directory.path().join("bindings.json");
        std::fs::write(&path,json!({"runtimes":{"job-fixture":{"generation":1,"namespace":"fixtures","service":"fixture-plugin","credentialSha256":credential,"peers":[{"kind":"unix","uid":rustix::process::geteuid().as_raw()}]}}}).to_string()).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let auth = fixture
            .role_with_connections("webdock_auth_bootstrap", connections)
            .await;
        let identity = Arc::new(IdentityService::new(
            auth.clone(),
            CookiePolicy::new(false, "owned-plugin-jobs-cookie-key-32bytes").unwrap(),
        ));
        let tx = auth.begin().await.unwrap();
        let actor = identity
            .authenticate_session_in(&tx, "operator", "session")
            .await
            .unwrap()
            .unwrap();
        tx.commit().await.unwrap();
        let host = Arc::new(
            HostService::new(
                fixture
                    .role_with_connections("webdock_auth_runtime", connections)
                    .await,
                fixture
                    .role_with_connections("webdock_worker_runtime", connections)
                    .await,
                "owned-plugin-jobs-rls-secret-32bytes".into(),
                Arc::new(
                    webdock_persistence::IdAllocator::connect(
                        fixture.url.as_str(),
                        webdock_ids::Identity::platform(0, 2).unwrap(),
                    )
                    .await
                    .unwrap(),
                ),
                Arc::new(FileBindings::new(path)),
                ed25519_dalek::SigningKey::from_bytes(&[19; 32]),
            )
            .await
            .unwrap()
            .with_session_authority(auth.clone(), identity.clone(), None)
            .await
            .unwrap(),
        );
        Self {
            host,
            owner,
            auth,
            identity,
            actor,
            manifest,
            directory,
            database: fixture,
        }
    }
    pub async fn admit_subject(
        &self,
        subject: &str,
        session: &str,
        action: &str,
        input: Value,
        key: &str,
    ) -> webdock_plugin_host::AdmittedInvocation {
        let tx = self.auth.begin().await.unwrap();
        let actor = self
            .identity
            .authenticate_session_in(&tx, subject, session)
            .await
            .unwrap()
            .unwrap();
        tx.commit().await.unwrap();
        self.host.admit_with_authority(&actor,serde_json::from_value::<AdmissionInput>(json!({"pluginID":"job-fixture","projectID":"124","action":action,"input":input,"idempotencyKey":key})).unwrap(),true,&webdock_browser_session::StoredAuthority::local_session(&actor)).await.unwrap()
    }
    pub async fn admit(&self, action: &str, key: &str) -> webdock_plugin_host::AdmittedInvocation {
        let tx = self.auth.begin().await.unwrap();
        let actor = self
            .identity
            .authenticate_session_in(&tx, "operator", "session")
            .await
            .unwrap()
            .unwrap();
        tx.commit().await.unwrap();
        self.host.admit_with_authority(&actor,serde_json::from_value::<AdmissionInput>(json!({"pluginID":"job-fixture","projectID":"124","action":action,"input":{},"idempotencyKey":key})).unwrap(),true,&webdock_browser_session::StoredAuthority::local_session(&actor)).await.unwrap()
    }
    pub async fn handle(&self, action: &str, key: &str) -> webdock_plugin_host::InvocationHandle {
        let admitted = self.admit(action, key).await;
        self.host
            .authenticate(peer(), &admitted.token)
            .await
            .unwrap()
    }
    pub async fn enqueue(&self, key: &str) -> Value {
        self.host
            .jobs_enqueue(
                &self.handle("start", key).await,
                json!({"name":"work","payload":{},"idempotencyKey":key}),
            )
            .await
            .unwrap()
    }
}
