//! The plugin's own database: sending domains, connected relay accounts and a short send log.
use std::sync::Arc;
use tokio_postgres::Client;
use uuid::Uuid;

const MIGRATIONS: &[&str] = &[
    "CREATE TABLE domains (
        id uuid PRIMARY KEY,
        project_id text NOT NULL,
        domain text NOT NULL UNIQUE,
        token text NOT NULL,
        verified_at timestamptz,
        created_at timestamptz NOT NULL DEFAULT now()
    );
    CREATE TABLE relays (
        id uuid PRIMARY KEY,
        project_id text NOT NULL UNIQUE,
        key_sealed bytea NOT NULL,
        secret_sealed bytea NOT NULL,
        key_hint text NOT NULL,
        created_at timestamptz NOT NULL DEFAULT now()
    );
    CREATE TABLE sends (
        id uuid PRIMARY KEY,
        domain_id uuid NOT NULL REFERENCES domains(id) ON DELETE CASCADE,
        sender text NOT NULL,
        recipients text NOT NULL,
        subject text NOT NULL,
        transport text NOT NULL,
        status text NOT NULL CHECK (status IN ('accepted','failed')),
        provider_id text,
        created_at timestamptz NOT NULL DEFAULT now()
    );
    CREATE INDEX sends_domain ON sends(domain_id, created_at DESC);",
    "ALTER TABLE domains DROP CONSTRAINT domains_domain_key;
    CREATE UNIQUE INDEX domains_project_domain ON domains(project_id, domain);
    CREATE UNIQUE INDEX domains_verified ON domains(domain) WHERE verified_at IS NOT NULL;
    CREATE TABLE engines (
        project_id text PRIMARY KEY,
        id uuid NOT NULL,
        admin_sealed bytea NOT NULL,
        state text NOT NULL CHECK (state IN ('provisioning','ready','failed'))
    );
    CREATE TABLE mailboxes (
        id uuid PRIMARY KEY,
        project_id text NOT NULL,
        domain_id uuid NOT NULL REFERENCES domains(id),
        local_part text NOT NULL,
        display_name text NOT NULL,
        password_sealed bytea NOT NULL,
        account_id text,
        quota_mb integer NOT NULL,
        state text NOT NULL CHECK (state IN ('creating','ready','failed')),
        error text,
        created_at timestamptz NOT NULL DEFAULT now(),
        UNIQUE (domain_id, local_part)
    );",
    "CREATE TABLE project_customers (
        project_id text PRIMARY KEY,
        customer_id text NOT NULL
    );
    CREATE TABLE customer_engines (
        customer_id text PRIMARY KEY,
        id uuid NOT NULL UNIQUE,
        admin_sealed bytea NOT NULL,
        desired_enabled boolean NOT NULL DEFAULT true,
        state text NOT NULL CHECK (state IN ('provisioning','ready','failed','suspending','suspended'))
    );
    ALTER TABLE mailboxes ADD COLUMN customer_id text REFERENCES customer_engines(customer_id);
    CREATE INDEX mailboxes_customer ON mailboxes(customer_id);",
    "CREATE TABLE private_payload_key (
        singleton boolean PRIMARY KEY CHECK (singleton),
        sealed text NOT NULL CHECK (octet_length(sealed) <= 4096)
    );
    CREATE TABLE private_payloads (
        reference uuid PRIMARY KEY,
        customer_id text NOT NULL CHECK (customer_id ~ '^[1-9][0-9]{0,18}$'),
        sealed text NOT NULL CHECK (octet_length(sealed) <= 50331648),
        payload_bytes integer NOT NULL CHECK (payload_bytes BETWEEN 1 AND 26214400),
        sha256 text NOT NULL CHECK (sha256 ~ '^[0-9a-f]{64}$')
    );",

    include_str!("recovery.sql"),
];

pub type Error = tokio_postgres::Error;
#[derive(Clone, Debug)]
pub struct Domain {
    pub id: Uuid,
    pub domain: String,
    pub token: String,
    pub verified: bool,
}
pub struct Relay {
    pub id: Uuid,
    pub key_sealed: Vec<u8>,
    pub secret_sealed: Vec<u8>,
    pub key_hint: String,
    pub created_at: String,
}
pub struct Send {
    pub sender: String,
    pub recipients: String,
    pub subject: String,
    pub transport: String,
    pub status: String,
    pub created_at: String,
}

pub struct Store(pub(crate) Client, Arc<Connection>);
struct Connection {
    url: String,
    ca: Option<Vec<u8>>,
    slots: Arc<tokio::sync::Semaphore>,
    recovery_slots: Arc<tokio::sync::Semaphore>,
}
/// A dedicated transaction/connection: cancellation closes it and releases the cross-process fence.
pub struct CustomerFence {
    _client: Client,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

impl Store {
    pub async fn connect(url: &str) -> Result<Self, webdock_plugin_kit::Error> {
        webdock_plugin_kit::loopback(url)?;
        Ok(Self(
            webdock_plugin_kit::storage(url, "mail", MIGRATIONS).await?,
            Arc::new(Connection {
                url: url.into(),
                ca: None,
                slots: Arc::new(tokio::sync::Semaphore::new(4)),
                recovery_slots: Arc::new(tokio::sync::Semaphore::new(64)),
            }),
        ))
    }
    /// Remote metadata storage verifies the actual PostgreSQL hostname with the private CA.
    pub async fn connect_tls(url: &str, ca: &[u8]) -> Result<Self, webdock_plugin_kit::Error> {
        Ok(Self(
            webdock_plugin_kit::storage_tls(url, ca, "mail", MIGRATIONS).await?,
            Arc::new(Connection {
                url: url.into(),
                ca: Some(ca.into()),
                slots: Arc::new(tokio::sync::Semaphore::new(4)),
                recovery_slots: Arc::new(tokio::sync::Semaphore::new(64)),
            }),
        ))
    }
    /// False when the domain is already claimed, by this or any other project.
    pub async fn add_domain(&self, project: &str, domain: &Domain) -> Result<bool, Error> {
        Ok(self.0.execute("INSERT INTO domains(id,project_id,domain,token) VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING", &[&domain.id, &project, &domain.domain, &domain.token]).await? == 1)
    }
    pub async fn domain(&self, project: &str, id: &Uuid) -> Result<Option<Domain>, Error> {
        Ok(self
            .0
            .query_opt(
                "SELECT id,domain,token,verified_at IS NOT NULL FROM domains WHERE id=$1 AND project_id=$2",
                &[id, &project],
            )
            .await?
            .map(|r| Domain {
                id: r.get(0),
                domain: r.get(1),
                token: r.get(2),
                verified: r.get(3),
            }))
    }
    /// False when another project already verified the same domain.
    pub async fn verified(&self, id: &Uuid) -> Result<bool, Error> {
        match self
            .0
            .execute(
                "UPDATE domains SET verified_at=COALESCE(verified_at,now()) WHERE id=$1",
                &[id],
            )
            .await
        {
            Ok(_) => Ok(true),
            Err(error)
                if error.code() == Some(&tokio_postgres::error::SqlState::UNIQUE_VIOLATION) =>
            {
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }
    /// False while mailboxes still use the domain.
    pub async fn delete_domain(&self, project: &str, id: &Uuid) -> Result<bool, Error> {
        match self
            .0
            .execute(
                "DELETE FROM domains WHERE id=$1 AND project_id=$2",
                &[id, &project],
            )
            .await
        {
            Ok(_) => Ok(true),
            Err(error)
                if error.code()
                    == Some(&tokio_postgres::error::SqlState::FOREIGN_KEY_VIOLATION) =>
            {
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }
    /// False when the project already has a connected account.
    pub async fn add_relay(
        &self,
        project: &str,
        id: &Uuid,
        key: &[u8],
        secret: &[u8],
        hint: &str,
    ) -> Result<bool, Error> {
        Ok(self.0.execute("INSERT INTO relays(id,project_id,key_sealed,secret_sealed,key_hint) VALUES($1,$2,$3,$4,$5) ON CONFLICT (project_id) DO NOTHING", &[id, &project, &key, &secret, &hint]).await? == 1)
    }
    pub async fn relay(&self, project: &str) -> Result<Option<Relay>, Error> {
        Ok(self.0.query_opt("SELECT id,key_sealed,secret_sealed,key_hint,created_at::text FROM relays WHERE project_id=$1", &[&project]).await?.map(|r| Relay {
            id: r.get(0),
            key_sealed: r.get(1),
            secret_sealed: r.get(2),
            key_hint: r.get(3),
            created_at: r.get(4),
        }))
    }
    pub async fn delete_relay(&self, project: &str, id: &Uuid) -> Result<(), Error> {
        self.0
            .execute(
                "DELETE FROM relays WHERE id=$1 AND project_id=$2",
                &[id, &project],
            )
            .await?;
        Ok(())
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "One row of the send log, column by column."
    )]
    pub async fn log(
        &self,
        domain: &Uuid,
        sender: &str,
        recipients: &str,
        subject: &str,
        transport: &str,
        status: &str,
        provider_id: Option<&str>,
    ) -> Result<(), Error> {
        self.0.execute("INSERT INTO sends(id,domain_id,sender,recipients,subject,transport,status,provider_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8)", &[&Uuid::new_v4(), domain, &sender, &recipients, &subject, &transport, &status, &provider_id]).await?;
        Ok(())
    }
    pub async fn recent(&self, domain: &Uuid) -> Result<Vec<Send>, Error> {
        Ok(self.0.query("SELECT sender,recipients,subject,transport,status,created_at::text FROM sends WHERE domain_id=$1 ORDER BY created_at DESC LIMIT 20", &[domain]).await?.iter().map(|r| Send {
            sender: r.get(0),
            recipients: r.get(1),
            subject: r.get(2),
            transport: r.get(3),
            status: r.get(4),
            created_at: r.get(5),
        }).collect())
    }
}

#[derive(Clone, Debug)]
pub struct Engine {
    pub customer_id: String,
    pub desired_enabled: bool,
    pub id: Uuid,
    pub admin_sealed: Vec<u8>,
    pub state: String,
}
#[derive(Clone, Debug)]
pub struct Mailbox {
    pub customer_id: String,
    pub id: Uuid,
    pub project_id: String,
    pub domain_id: Uuid,
    pub domain: String,
    pub local_part: String,
    pub display_name: String,
    pub password_sealed: Vec<u8>,
    pub account_id: Option<String>,
    pub quota_mb: i32,
    pub state: String,
    pub error: Option<String>,
    pub created_at: String,
}
const MAILBOX: &str = "SELECT m.id,m.project_id,m.domain_id,d.domain,m.local_part,m.display_name,m.password_sealed,m.account_id,m.quota_mb,m.state,m.error,m.created_at::text,COALESCE(m.customer_id,'') FROM mailboxes m JOIN domains d ON d.id=m.domain_id";
fn mailbox(r: &tokio_postgres::Row) -> Mailbox {
    Mailbox {
        id: r.get(0),
        customer_id: r.get(12),
        project_id: r.get(1),
        domain_id: r.get(2),
        domain: r.get(3),
        local_part: r.get(4),
        display_name: r.get(5),
        password_sealed: r.get(6),
        account_id: r.get(7),
        quota_mb: r.get(8),
        state: r.get(9),
        error: r.get(10),
        created_at: r.get(11),
    }
}
impl Store {
    /// Persist signed ownership; a transferred project needs explicit data migration.
    pub async fn bind_project(&self, project: &str, customer: &str) -> Result<bool, Error> {
        Ok(self.0.query_opt("INSERT INTO project_customers(project_id,customer_id) VALUES($1,$2) ON CONFLICT(project_id) DO UPDATE SET customer_id=EXCLUDED.customer_id WHERE project_customers.customer_id=EXCLUDED.customer_id RETURNING project_id", &[&project,&customer]).await?.is_some())
    }
    /// Legacy project records have no trustworthy customer mapping. Retain them until migrated.
    pub async fn legacy_engines(&self) -> Result<bool, Error> {
        Ok(self.0.query_one("SELECT EXISTS(SELECT 1 FROM engines) OR EXISTS(SELECT 1 FROM mailboxes WHERE customer_id IS NULL)", &[]).await?.get(0))
    }
    /// Customer reservation is stable across projects, retries and suspension.
    pub async fn engine(
        &self,
        customer: &str,
        id: &Uuid,
        admin_sealed: &[u8],
    ) -> Result<Engine, Error> {
        self.0.execute("INSERT INTO customer_engines(customer_id,id,admin_sealed,state) VALUES($1,$2,$3,'provisioning') ON CONFLICT(customer_id) DO NOTHING", &[&customer,id,&admin_sealed]).await?;
        Ok(self.find_engine(customer).await?.expect("reserved engine"))
    }
    pub async fn find_engine(&self, customer: &str) -> Result<Option<Engine>, Error> {
        Ok(self.0.query_opt("SELECT id,admin_sealed,state,customer_id,desired_enabled FROM customer_engines WHERE customer_id=$1", &[&customer]).await?.as_ref().map(engine))
    }
    pub async fn engines(&self) -> Result<Vec<Engine>, Error> {
        Ok(self.0.query("SELECT id,admin_sealed,state,customer_id,desired_enabled FROM customer_engines ORDER BY customer_id", &[]).await?.iter().map(engine).collect())
    }
    pub async fn engine_enabled(&self, customer: &str, enabled: bool) -> Result<(), Error> {
        self.0.execute("UPDATE customer_engines SET desired_enabled=$2,state=CASE WHEN $2 THEN 'provisioning' ELSE 'suspending' END WHERE customer_id=$1", &[&customer,&enabled]).await?;
        Ok(())
    }
    pub async fn engine_state(&self, customer: &str, state: &str) -> Result<(), Error> {
        self.0
            .query_one(
                "UPDATE customer_engines SET state=$2 WHERE customer_id=$1 AND desired_enabled=($2<>'suspended') RETURNING customer_id",
                &[&customer, &state],
            )
            .await?;
        Ok(())
    }
    /// False when the address already exists.
    pub async fn add_mailbox(&self, m: &Mailbox) -> Result<bool, Error> {
        Ok(self.0.execute("INSERT INTO mailboxes(id,project_id,domain_id,local_part,display_name,password_sealed,quota_mb,state,customer_id) VALUES($1,$2,$3,$4,$5,$6,$7,'creating',$8) ON CONFLICT (domain_id,local_part) DO NOTHING",
            &[&m.id, &m.project_id, &m.domain_id, &m.local_part, &m.display_name, &m.password_sealed, &m.quota_mb, &m.customer_id]).await? == 1)
    }
    pub async fn mailbox(&self, project: &str, id: &Uuid) -> Result<Option<Mailbox>, Error> {
        Ok(self
            .0
            .query_opt(
                &format!("{MAILBOX} WHERE m.id=$1 AND m.project_id=$2"),
                &[id, &project],
            )
            .await?
            .as_ref()
            .map(mailbox))
    }
    pub async fn pending_mailboxes(&self) -> Result<Vec<Mailbox>, Error> {
        Ok(self
            .0
            .query(
                &format!(
                    "{MAILBOX} WHERE m.state IN ('creating','failed') AND m.customer_id IS NOT NULL"
                ),
                &[],
            )
            .await?
            .iter()
            .map(mailbox)
            .collect())
    }
    pub async fn domain_mailboxes(&self, domain: &Uuid) -> Result<Vec<String>, Error> {
        Ok(self
            .0
            .query(
                "SELECT local_part FROM mailboxes WHERE domain_id=$1 ORDER BY local_part",
                &[domain],
            )
            .await?
            .iter()
            .map(|r| r.get(0))
            .collect())
    }
    pub async fn mailbox_ready(&self, id: &Uuid, account: &str) -> Result<(), Error> {
        self.0
            .query_one(
                "UPDATE mailboxes SET state='ready',account_id=$2,error=NULL WHERE id=$1 RETURNING id",
                &[id, &account],
            )
            .await?;
        Ok(())
    }
    pub async fn mailbox_failed(&self, id: &Uuid, error: &str) -> Result<(), Error> {
        self.0
            .execute(
                "UPDATE mailboxes SET state='failed',error=$2 WHERE id=$1",
                &[id, &error],
            )
            .await?;
        Ok(())
    }
    pub async fn mailbox_password(&self, id: &Uuid, sealed: &[u8]) -> Result<(), Error> {
        self.0
            .execute(
                "UPDATE mailboxes SET password_sealed=$2 WHERE id=$1",
                &[id, &sealed],
            )
            .await?;
        Ok(())
    }
    pub async fn delete_mailbox(&self, id: &Uuid) -> Result<(), Error> {
        self.0
            .execute("DELETE FROM mailboxes WHERE id=$1", &[id])
            .await?;
        Ok(())
    }
}

fn engine(r: &tokio_postgres::Row) -> Engine {
    Engine {
        id: r.get(0),
        admin_sealed: r.get(1),
        state: r.get(2),
        customer_id: r.get(3),
        desired_enabled: r.get(4),
    }
}

impl Store {
    pub(crate) fn recovery_slots(&self) -> Arc<tokio::sync::Semaphore> {
        self.1.recovery_slots.clone()
    }
    pub(crate) async fn private_connection(&self) -> Result<Client, webdock_plugin_kit::Error> {
        webdock_plugin_kit::postgres::connect(&self.1.url, self.1.ca.as_deref()).await
    }
    pub async fn customer_fence(
        &self,
        customer: &str,
    ) -> Result<CustomerFence, webdock_plugin_kit::Error> {
        if !webdock_mail_runtime::proof::valid_id(customer) {
            return Err("invalid customer fence".into());
        }
        let permit = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.1.slots.clone().acquire_owned(),
        )
        .await
        .map_err(|_| "customer fence deadline")?
        .map_err(|_| "customer fence unavailable")?;
        let client =
            webdock_plugin_kit::postgres::connect(&self.1.url, self.1.ca.as_deref()).await?;
        client.batch_execute("BEGIN; SET LOCAL lock_timeout='5s'; SET LOCAL statement_timeout='10s'; SET LOCAL idle_in_transaction_session_timeout='60s'").await.map_err(|_| "customer fence unavailable")?;
        client
            .query_one(
                "SELECT pg_advisory_xact_lock(hashtextextended($1, 704611))",
                &[&customer],
            )
            .await
            .map_err(|_| "customer fence unavailable")?;
        Ok(CustomerFence {
            _client: client,
            _permit: permit,
        })
    }
    pub async fn direct_allowed(&self, customer: &str) -> Result<bool, Error> {
        Ok(self.find_engine(customer).await?.is_none() && !self.legacy_engines().await?)
    }
}
