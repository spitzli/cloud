//! Opt-in owned PostgreSQL18 process for the daemonless EU qualification stage; no supplied DSN.
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use std::{
    fs::{OpenOptions, Permissions},
    io::Write,
    net::TcpListener,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Output},
};

const PG: &str = "/usr/lib/postgresql/18/bin";
// The retained baseline assigns its migration journals to this bootstrap role.
const BOOTSTRAP_USER: &str = "postgres";
pub(super) struct LocalPostgres {
    root: Option<tempfile::TempDir>,
    marker: String,
    pub url: url::Url,
}
fn command(program: &str, args: &[&str]) -> std::io::Result<Output> {
    Command::new(Path::new(PG).join(program))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LANG", "C")
        .args(args)
        .output()
}
fn run(program: &str, args: &[&str]) {
    assert!(
        command(program, args).is_ok_and(|output| output.status.success()),
        "owned PostgreSQL fixture {program} failed"
    );
}
fn private_file(path: &Path, text: &str) {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
}
impl LocalPostgres {
    pub fn start() -> Self {
        assert_eq!(
            rustix::process::geteuid().as_raw(),
            1001,
            "qualified nonroot PostgreSQL fixture UID required"
        );
        let root = tempfile::Builder::new()
            .prefix("webdock-owned-plugin-jobs-")
            .permissions(Permissions::from_mode(0o700))
            .tempdir_in("/tmp")
            .unwrap();
        let marker = uuid::Uuid::new_v4().simple().to_string();
        private_file(&root.path().join("owner"), &marker);
        let port = TcpListener::bind(("127.0.0.1", 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let password = uuid::Uuid::new_v4().simple().to_string();
        let database = format!("owned_plugin_jobs_{}", uuid::Uuid::new_v4().simple());
        let mut url = url::Url::parse(&format!(
            "postgresql://{BOOTSTRAP_USER}@127.0.0.1:{port}/{database}?sslmode=disable"
        ))
        .unwrap();
        url.set_password(Some(&password)).unwrap();
        // Own cleanup before the first command so partial initialization also retires its process.
        let fixture = Self {
            root: Some(root),
            marker,
            url,
        };
        assert!(
            fixture.owned(),
            "local PostgreSQL fixture ownership required"
        );
        let root = fixture.path();
        let data = root.join("data");
        let socket = root.join("socket");
        std::fs::create_dir(&socket).unwrap();
        std::fs::set_permissions(&socket, Permissions::from_mode(0o700)).unwrap();
        let password_file = root.join("password");
        private_file(&password_file, &format!("{password}\n"));
        run(
            "initdb",
            &[
                "-D",
                data.to_str().unwrap(),
                "--username",
                BOOTSTRAP_USER,
                "--auth-local=trust",
                "--auth-host=scram-sha-256",
                "--pwfile",
                password_file.to_str().unwrap(),
                "--encoding=UTF8",
                "--no-locale",
            ],
        );
        assert_eq!(
            std::fs::read_to_string(data.join("PG_VERSION"))
                .unwrap()
                .trim(),
            "18"
        );
        run(
            "pg_ctl",
            &[
                "-D",
                data.to_str().unwrap(),
                "-l",
                root.join("postgres.log").to_str().unwrap(),
                "-o",
                &format!(
                    "-h 127.0.0.1 -p {port} -k {} -c max_connections=40 -c shared_buffers=32MB",
                    socket.display()
                ),
                "-t",
                "60",
                "-w",
                "start",
            ],
        );
        run(
            "createdb",
            &[
                "--host",
                socket.to_str().unwrap(),
                "--port",
                &port.to_string(),
                "--username",
                BOOTSTRAP_USER,
                "--no-password",
                &database,
            ],
        );
        fixture
    }
    fn path(&self) -> &Path {
        self.root.as_ref().unwrap().path()
    }
    fn owned(&self) -> bool {
        let root = self.path();
        let marker = root.join("owner");
        root.parent() == Some(Path::new("/tmp"))
            && root.file_name().is_some_and(|name| {
                name.to_string_lossy()
                    .starts_with("webdock-owned-plugin-jobs-")
            })
            && root.canonicalize().is_ok_and(|path| path == root)
            && root.symlink_metadata().is_ok_and(|meta| {
                !meta.file_type().is_symlink()
                    && meta.uid() == 1001
                    && meta.mode() & 0o7777 == 0o700
            })
            && marker.symlink_metadata().is_ok_and(|meta| {
                meta.file_type().is_file() && meta.uid() == 1001 && meta.mode() & 0o7777 == 0o600
            })
            && std::fs::read_to_string(marker).is_ok_and(|value| value == self.marker)
    }
    pub async fn verify(&self, db: &DatabaseConnection) {
        assert!(self.owned());
        assert_eq!(self.url.scheme(), "postgresql");
        assert_eq!(self.url.host_str(), Some("127.0.0.1"));
        assert_eq!(self.url.username(), BOOTSTRAP_USER);
        assert!(self.url.port().is_some());
        let suffix = self.url.path().strip_prefix("/owned_plugin_jobs_").unwrap();
        assert!(suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(self.url.query(), Some("sslmode=disable"));
        assert_eq!(
            self.path().join("data").canonicalize().unwrap(),
            self.path().join("data")
        );
        let row = db
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT current_setting('data_directory') AS data, current_database() AS database, session_user AS bootstrap_user",
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            row.try_get::<String>("", "bootstrap_user").unwrap(),
            BOOTSTRAP_USER
        );
        assert_eq!(
            PathBuf::from(row.try_get::<String>("", "data").unwrap()),
            self.path().join("data")
        );
        assert_eq!(
            row.try_get::<String>("", "database").unwrap(),
            self.url.path().trim_start_matches('/')
        );
    }
}
impl Drop for LocalPostgres {
    fn drop(&mut self) {
        let data = self.path().join("data");
        let owned =
            self.owned() && (!data.exists() || data.canonicalize().is_ok_and(|path| path == data));
        let stopped = owned
            && (!data.join("postmaster.pid").exists()
                || command(
                    "pg_ctl",
                    &[
                        "-D",
                        data.to_str().unwrap(),
                        "-m",
                        "fast",
                        "-t",
                        "60",
                        "-w",
                        "stop",
                    ],
                )
                .is_ok_and(|output| output.status.success()));
        let root = self.root.take().unwrap();
        if !stopped || data.join("postmaster.pid").exists() {
            let path = root.keep();
            eprintln!(
                "Preserved unretired local PostgreSQL fixture at {}",
                path.display()
            );
            assert!(
                std::thread::panicking(),
                "owned PostgreSQL fixture stop/ownership verification failed"
            );
            return;
        }
        let path = root.path().to_owned();
        root.close()
            .expect("owned PostgreSQL fixture cleanup failed");
        assert!(!path.exists(), "owned PostgreSQL fixture root remains");
    }
}
