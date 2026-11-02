//! Mailbox engines on the cluster through `plugin.workloads`: one Stalwart Community Edition
//! server per explicitly incoming-enabled customer with its own volume. A NetworkPolicy denies all egress, so nothing a mailbox
//! sends leaves the cluster. Customers reach IMAP and submission through Traefik by TLS server
//! name; the plugin manages the engine over Stalwart's HTTP API on its own route.
// ponytail: the management route shares Traefik's HTTP entry point, guarded only by the engine's
// admin password; give production an internal entry point (or run the plugin in the cluster).
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{collections::HashMap, time::Duration};
pub use webdock_plugin_kit::workloads::{HostWorkloads, Workloads};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EngineConfig {
    /// Stalwart CE, pinned by digest.
    pub image: String,
    /// Customers connect to `c<customer>.<domain>`.
    pub domain: String,
    /// The public ports of Traefik's `imaps` and `submissions` entry points.
    pub imap_port: u16,
    pub submission_port: u16,
    pub admin: AdminRoute,
    /// cert-manager ClusterIssuer for the engines' certificates, so mail clients can verify them.
    /// Without one, each engine gets a self-signed certificate.
    #[serde(default)]
    pub issuer: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminRoute {
    /// Where the plugin reaches Traefik, e.g. `http://127.0.0.1:55480`.
    pub origin: String,
    /// Engines' management hosts are `c<customer>.<domain>`.
    pub domain: String,
    pub entry_points: Vec<String>,
}
pub type Error = Box<dyn std::error::Error + Send + Sync>;
const IMAP_PORT: u16 = 1993;
const SUBMISSION_PORT: u16 = 2465;
const DATA: &str = "/var/lib/stalwart";
const TLS: &str = "/etc/stalwart-tls";

pub struct Engines {
    workloads: Box<dyn Workloads>,
    config: EngineConfig,
}
pub fn unit(customer: &str) -> String {
    format!("engine-c{customer}")
}
fn hostname(customer: &str) -> String {
    format!("mail.c{customer}.webdock.internal")
}
fn admin_user(customer: &str) -> String {
    format!("webdock-admin@{}", hostname(customer))
}

/// Management calls over Stalwart's JMAP endpoint.
pub struct Client {
    http: reqwest::Client,
    origin: String,
    /// Traefik routes management calls by this host.
    host: String,
    user: String,
    password: String,
}
impl Client {
    fn new(origin: &str, host: String, user: &str, password: &str) -> Self {
        Self {
            http: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(10))
                .build()
                .expect("static HTTP client configuration"),
            origin: origin.trim_end_matches('/').into(),
            host,
            user: user.into(),
            password: password.into(),
        }
    }
    pub async fn call<T: DeserializeOwned>(&self, method: &str, args: Value) -> Result<T, Error> {
        let response: Value = self
            .http
            .post(format!("{}/jmap", self.origin))
            .header(reqwest::header::HOST, &self.host)
            .basic_auth(&self.user, Some(&self.password))
            .json(&json!({"using":["urn:ietf:params:jmap:core","urn:stalwart:jmap"],"methodCalls":[[method,args,"w"]]}))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let result = &response["methodResponses"][0];
        if result[0] != method {
            return Err(format!("{method} failed").into());
        }
        let value = &result[1];
        for key in ["notCreated", "notUpdated", "notDestroyed"] {
            if let Some(rejected) = value[key].as_object().filter(|v| !v.is_empty()) {
                let reasons: String = Value::Object(rejected.clone())
                    .to_string()
                    .chars()
                    .take(300)
                    .collect();
                return Err(format!("{method} rejected: {reasons}").into());
            }
        }
        Ok(serde_json::from_value(value.clone())?)
    }
}
#[derive(Deserialize)]
pub struct Rows<T> {
    pub list: Vec<T>,
}
#[derive(Deserialize)]
pub struct Named {
    pub id: String,
    pub name: String,
}
#[derive(Deserialize)]
pub struct Created {
    pub created: HashMap<String, Named0>,
}
#[derive(Deserialize)]
pub struct Named0 {
    pub id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Account {
    id: String,
    name: String,
    domain_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dkim {
    pub domain_id: String,
    pub selector: String,
    pub public_key: String,
    pub stage: String,
}

impl Engines {
    pub fn new(workloads: Box<dyn Workloads>, config: EngineConfig) -> Result<Self, Error> {
        if !config.image.contains("@sha256:") {
            return Err("the mail engine image must be pinned by digest".into());
        }
        Ok(Self { workloads, config })
    }
    /// Where customers' mail clients connect.
    pub fn host(&self, customer: &str) -> String {
        format!("c{customer}.{}", self.config.domain)
    }
    pub fn ports(&self) -> (u16, u16) {
        (self.config.imap_port, self.config.submission_port)
    }
    pub fn tls_note(&self) -> &'static str {
        if self.config.issuer.is_some() {
            "Implicit TLS"
        } else {
            "Implicit TLS, self-signed certificate"
        }
    }
    fn admin_host(&self, customer: &str) -> String {
        format!("c{customer}.{}", self.config.admin.domain)
    }
    fn connect(&self, customer: &str, user: &str, password: &str) -> Client {
        Client::new(
            &self.config.admin.origin,
            self.admin_host(customer),
            user,
            password,
        )
    }
    pub fn client(&self, customer: &str, admin_password: &str) -> Client {
        self.connect(customer, &admin_user(customer), admin_password)
    }
    /// The engine's objects. With `recovery` it starts with a temporary administrator for setup;
    /// a new `restart` value rolls the pod.
    pub fn objects(&self, customer: &str, recovery: Option<&str>, restart: &str) -> Value {
        let app = json!({"app": "engine"});
        let mut objects = vec![
            json!({"apiVersion": "v1", "kind": "PersistentVolumeClaim", "metadata": {"name": "engine-data"},
                   "spec": {"accessModes": ["ReadWriteOnce"], "resources": {"requests": {"storage": "2Gi"}}}}),
            json!({"apiVersion": "apps/v1", "kind": "Deployment", "metadata": {"name": "engine", "labels": app},
            "spec": {"replicas": 1, "strategy": {"type": "Recreate"}, "selector": {"matchLabels": app},
             "template": {"metadata": {"labels": app, "annotations": {"restarted": restart}},
              "spec": {"containers": [{"name": "stalwart", "image": self.config.image,
                "args": ["--config", format!("{DATA}/config.json")],
                "envFrom": if recovery.is_some() { json!([{"secretRef": {"name": "engine-recovery"}}]) } else { json!([]) },
                "ports": [{"containerPort": 8080}, {"containerPort": IMAP_PORT}, {"containerPort": SUBMISSION_PORT}],
                "volumeMounts": if self.config.issuer.is_some() {
                    json!([{"name": "data", "mountPath": DATA}, {"name": "tls", "mountPath": TLS, "readOnly": true}])
                } else {
                    json!([{"name": "data", "mountPath": DATA}])
                },
                "resources": {"limits": {"memory": "512Mi"}, "requests": {"memory": "512Mi", "cpu": "50m"}},
                // The binary carries this file capability and refuses to exec without it.
                "securityContext": {"allowPrivilegeEscalation": false, "capabilities": {"drop": ["ALL"], "add": ["NET_BIND_SERVICE"]}}}],
               "volumes": if self.config.issuer.is_some() {
                   json!([{"name": "data", "persistentVolumeClaim": {"claimName": "engine-data"}}, {"name": "tls", "secret": {"secretName": "engine-tls"}}])
               } else {
                   json!([{"name": "data", "persistentVolumeClaim": {"claimName": "engine-data"}}])
               }}}}}),
            json!({"apiVersion": "networking.k8s.io/v1", "kind": "NetworkPolicy", "metadata": {"name": "engine-no-egress"},
                   "spec": {"podSelector": {"matchLabels": app}, "policyTypes": ["Egress"], "egress": []}}),
            json!({"apiVersion": "v1", "kind": "Service", "metadata": {"name": "engine"},
                   "spec": {"selector": app, "ports": [{"name": "http", "port": 8080}, {"name": "imaps", "port": IMAP_PORT}, {"name": "submissions", "port": SUBMISSION_PORT}]}}),
            json!({"apiVersion": "traefik.io/v1alpha1", "kind": "IngressRoute", "metadata": {"name": "engine-admin"},
                   "spec": {"entryPoints": self.config.admin.entry_points, "routes": [{"kind": "Rule",
                    "match": format!("Host(`{}`)", self.admin_host(customer)), "services": [{"name": "engine", "port": 8080}]}]}}),
        ];
        for (name, port) in [("imaps", IMAP_PORT), ("submissions", SUBMISSION_PORT)] {
            objects.push(json!({"apiVersion": "traefik.io/v1alpha1", "kind": "IngressRouteTCP", "metadata": {"name": format!("engine-{name}")},
                "spec": {"entryPoints": [name], "routes": [{"match": format!("HostSNI(`{}`)", self.host(customer)),
                 "services": [{"name": "engine", "port": port}]}], "tls": {"passthrough": true}}}));
        }
        if let Some(issuer) = &self.config.issuer {
            objects.push(json!({"apiVersion": "cert-manager.io/v1", "kind": "Certificate", "metadata": {"name": "engine-tls"},
                "spec": {"secretName": "engine-tls", "dnsNames": [self.host(customer)], "issuerRef": {"name": issuer, "kind": "ClusterIssuer"}}}));
        }
        if let Some(password) = recovery {
            objects.push(json!({"apiVersion": "v1", "kind": "Secret", "metadata": {"name": "engine-recovery"}, "type": "Opaque",
                "stringData": {"STALWART_RECOVERY_MODE": "1", "STALWART_RECOVERY_ADMIN": format!("admin:{password}")}}));
        }
        Value::Array(objects)
    }
    async fn apply(
        &self,
        customer: &str,
        recovery: Option<&str>,
        restart: &str,
    ) -> Result<(), Error> {
        let objects = self.objects(customer, recovery, restart);
        self.workloads
            .call(
                "apply",
                json!({"customer": customer, "unit": unit(customer), "objects": objects}),
            )
            .await?;
        Ok(())
    }
    /// Polls until `method` succeeds (or, with `works: false`, fails) for this user.
    async fn wait(
        &self,
        customer: &str,
        (user, password): (&str, &str),
        method: &str,
        works: bool,
    ) -> Result<Client, Error> {
        let client = self.connect(customer, user, password);
        for _ in 0..360 {
            let result = client.call::<Value>(method, json!({"ids":null})).await;
            let observed = if works {
                result.is_ok()
            } else {
                result
                    .err()
                    .and_then(|e| {
                        e.downcast_ref::<reqwest::Error>()
                            .and_then(reqwest::Error::status)
                    })
                    .is_some_and(|status| status == reqwest::StatusCode::UNAUTHORIZED)
            };
            if observed {
                return Ok(client);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        Err("mail engine did not become ready".into())
    }
    /// Reconciles the retained customer engine. Management failure never removes its data.
    pub async fn provision(&self, customer: &str, admin_password: &str) -> Result<(), Error> {
        let permanent = self.client(customer, admin_password);
        let bootstrap = admin_password.to_owned();
        let recovery = ("admin", bootstrap.as_str());
        let admin = if permanent
            .call::<Value>("x:Domain/get", json!({"ids":null}))
            .await
            .is_ok()
        {
            permanent
        } else {
            self.apply(customer, Some(&bootstrap), "setup").await?;
            let admin = self.connect(customer, recovery.0, recovery.1);
            let mut available = false;
            for _ in 0..360 {
                if admin
                    .call::<Value>("x:Domain/get", json!({"ids":null}))
                    .await
                    .is_ok()
                {
                    available = true;
                    break;
                }
                if admin
                    .call::<Value>("x:Bootstrap/get", json!({"ids":null}))
                    .await
                    .is_ok()
                {
                    let host = hostname(customer);
                    let _: Value = admin.call("x:Bootstrap/set", json!({"update":{"singleton":{"serverHostname":host,"defaultDomain":host,"requestTlsCertificate":false,"generateDkimKeys":false,"dataStore":{"@type":"RocksDb","path":format!("{DATA}/store")},"tracer":{"@type":"Stdout","level":"warn"}}}})).await?;
                    self.apply(customer, Some(&bootstrap), "bootstrapped")
                        .await?;
                    self.wait(customer, recovery, "x:Domain/get", true).await?;
                    available = true;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            if !available {
                return Err("mail engine did not become ready".into());
            }
            admin
        };
        let host = hostname(customer);
        let accounts: Rows<Named> = admin
            .call(
                "x:Account/get",
                json!({"ids":null,"properties":["id","name"]}),
            )
            .await?;
        let account = accounts
            .list
            .iter()
            .find(|a| a.name == "webdock-admin")
            .or_else(|| {
                if accounts.list.len() == 1 {
                    accounts.list.first()
                } else {
                    None
                }
            })
            .ok_or("mail engine administrator requires operator recovery")?;
        let _: Value = admin.call("x:Account/set", json!({"update":{(account.id.as_str()):{"name":"webdock-admin","credentials":{"0":{"@type":"Password","secret":admin_password}},"roles":{"@type":"Admin"}}}})).await?;
        let listeners = json!({
            "http": {"name":"http","bind":{"[::]:8080":true},"protocol":"http","useTls":false,"tlsImplicit":false},
            "submissions": {"name":"submissions","bind":{format!("[::]:{SUBMISSION_PORT}"):true},"protocol":"smtp","useTls":true,"tlsImplicit":true},
            "imaps": {"name":"imaps","bind":{format!("[::]:{IMAP_PORT}"):true},"protocol":"imap","useTls":true,"tlsImplicit":true},
        });
        let existing: Rows<Named> = admin
            .call(
                "x:NetworkListener/get",
                json!({"ids":null,"properties":["id","name"]}),
            )
            .await?;
        let missing: serde_json::Map<String, Value> = listeners
            .as_object()
            .expect("listeners")
            .iter()
            .filter(|(name, _)| !existing.list.iter().any(|v| v.name == **name))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        if !missing.is_empty() {
            let _: Value = admin
                .call("x:NetworkListener/set", json!({"create":missing}))
                .await?;
        }
        // An issued certificate is read from its mounted Secret; otherwise a self-signed one is stored.
        // ponytail: the engine reads the files when it loads them; restart it after a renewal.
        let certificate = if self.config.issuer.is_some() {
            json!({"certificate":{"@type":"File","filePath":format!("{TLS}/tls.crt")},"privateKey":{"@type":"File","filePath":format!("{TLS}/tls.key")}})
        } else {
            let rcgen::CertifiedKey { cert, signing_key } =
                rcgen::generate_simple_self_signed(vec![self.host(customer), host])?;
            json!({"certificate":{"@type":"Text","value":cert.pem()},"privateKey":{"@type":"Text","secret":signing_key.serialize_pem()}})
        };
        let certificates: Rows<Named0> = admin
            .call("x:Certificate/get", json!({"ids":null,"properties":["id"]}))
            .await?;
        let certificate = if let Some(existing) = certificates.list.first() {
            existing.id.clone()
        } else {
            let created: Created = admin
                .call("x:Certificate/set", json!({"create":{"c":certificate}}))
                .await?;
            created
                .created
                .get("c")
                .ok_or("certificate not created")?
                .id
                .clone()
        };
        let _: Value = admin
            .call(
                "x:SystemSettings/set",
                json!({"update":{"singleton":{"defaultCertificateId":certificate}}}),
            )
            .await?;
        // Without the recovery variables the temporary administrator no longer exists.
        self.apply(customer, None, "ready").await?;
        self.wait(customer, recovery, "x:Domain/get", false).await?;
        self.wait(
            customer,
            (&admin_user(customer), admin_password),
            "x:Domain/get",
            true,
        )
        .await?;
        Ok(())
    }
    /// Desired suspension keeps the same objects, routes, credentials and persistent volume.
    pub async fn suspend(&self, customer: &str) -> Result<(), Error> {
        let mut objects = self.objects(customer, None, "ready");
        for object in objects.as_array_mut().expect("objects") {
            if object["kind"] == "Deployment" {
                object["spec"]["replicas"] = json!(0);
            }
        }
        let applied = self
            .workloads
            .call(
                "apply",
                json!({"customer":customer,"unit":unit(customer),"objects":objects}),
            )
            .await?;
        let revision = applied["revision"]
            .as_i64()
            .filter(|v| *v > 0)
            .ok_or("workload revision missing")?;
        for _ in 0..240 {
            let observed = self
                .workloads
                .call("observe", json!({"unit":unit(customer)}))
                .await?;
            if observed["appliedRevision"]
                .as_i64()
                .is_some_and(|v| v >= revision)
                && observed["observed"]["ready"] == true
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        Err("mail engine suspension has not been observed".into())
    }
    /// Removes the engine with its volume and waits until the cluster has let go of it.
    pub async fn destroy(&self, customer: &str) -> Result<(), Error> {
        let unit = unit(customer);
        self.workloads.call("remove", json!({"unit": unit})).await?;
        for _ in 0..240 {
            if self
                .workloads
                .call("observe", json!({"unit": unit}))
                .await?["removed"]
                == true
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        Err("mail engine was not removed".into())
    }
}

/// Mailbox administration inside one engine.
impl Client {
    pub async fn domain(&self, domain: &str) -> Result<String, Error> {
        let rows: Rows<Named> = self
            .call(
                "x:Domain/get",
                json!({"ids":null,"properties":["id","name"]}),
            )
            .await?;
        if let Some(found) = rows.list.iter().find(|d| d.name == domain) {
            return Ok(found.id.clone());
        }
        let created: Created = self.call("x:Domain/set", json!({"create":{"d":{"name":domain,"certificateManagement":{"@type":"Manual"},"dnsManagement":{"@type":"Manual"},"dkimManagement":{"@type":"Automatic","algorithms":{"Dkim1RsaSha256":true},"selectorTemplate":"webdock-{algorithm}-{date-%Y%m%d}"}}}})).await?;
        Ok(created
            .created
            .get("d")
            .ok_or("domain not created")?
            .id
            .clone())
    }
    pub async fn create_mailbox(
        &self,
        domain_id: &str,
        local: &str,
        password: &str,
        quota: u64,
    ) -> Result<String, Error> {
        let accounts: Rows<Account> = self
            .call(
                "x:Account/get",
                json!({"ids":null,"properties":["id","name","domainId"]}),
            )
            .await?;
        if let Some(account) = accounts
            .list
            .into_iter()
            .find(|a| a.name == local && a.domain_id.as_deref() == Some(domain_id))
        {
            // Creation may have succeeded before the plugin could record its account ID.
            self.password(&account.id, password).await?;
            return Ok(account.id);
        }
        let created: Created = self.call("x:Account/set", json!({"create":{"u":{"@type":"User","name":local,"domainId":domain_id,"roles":{"@type":"User"},"permissions":{"@type":"Inherit"},"quotas":{"maxDiskQuota":quota},"encryptionAtRest":{"@type":"Disabled"},"credentials":{"0":{"@type":"Password","secret":password}}}}})).await?;
        Ok(created
            .created
            .get("u")
            .ok_or("mailbox not created")?
            .id
            .clone())
    }
    pub async fn password(&self, account: &str, password: &str) -> Result<(), Error> {
        let _: Value = self.call("x:Account/set", json!({"update":{account:{"credentials":{"0":{"@type":"Password","secret":password}}}}})).await?;
        Ok(())
    }
    pub async fn delete_mailbox(&self, account: &str) -> Result<(), Error> {
        let _: Value = self
            .call("x:Account/set", json!({"destroy":[account]}))
            .await?;
        Ok(())
    }
    pub async fn dkim(&self, domain_id: &str) -> Result<Option<(String, String)>, Error> {
        let rows: Rows<Dkim> = self
            .call(
                "x:DkimSignature/get",
                json!({"ids":null,"properties":["domainId","selector","publicKey","stage"]}),
            )
            .await?;
        Ok(rows
            .list
            .into_iter()
            .find(|k| k.domain_id == domain_id && k.stage == "active")
            .map(|k| (k.selector, k.public_key.split_whitespace().collect())))
    }
}
