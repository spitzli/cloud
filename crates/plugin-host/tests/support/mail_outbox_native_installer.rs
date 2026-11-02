//! PUBLIC signed install -> actual Worker::advance -> six encrypted files. No Kubernetes.
use super::*;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use ed25519_dalek::Signer;
use std::collections::BTreeMap;
use webdock_agent_protocol::plugin::{MATERIAL_FILES, PluginRuntimeMaterial};
use webdock_control_plane::{
    plugin_config::PluginConfig, plugins::installation::worker::InstallationWorker,
};
use webdock_plugin_host::Catalog;
impl Fixture {
    pub async fn run_installer() -> TestResult {
        let _case = owned::CASE_LOCK.lock().await;
        let _privacy = owned::PanicPrivacy::new();
        let mut fixture = Self::planned(Source::Action, Mode::Accept).await?;
        let result = std::panic::AssertUnwindSafe(fixture.initialize(true))
            .catch_unwind()
            .await;
        if !cleanup_after_case(&result) {
            if let Err(payload) = &result {
                self_private_panic(&fixture.path("panic-diagnostic.txt"), payload.as_ref())?;
            }
            let failure = match &result {
                Ok(Err(error)) => *error,
                _ => "generic material fixture panic; private payload withheld",
            };
            fixture.preserve_failure(failure).await?;
            return Err(failure);
        }
        if fixture.cleanup().await.is_err() {
            return Err("generic material cleanup uncertain; exact ownership journal retained");
        }
        match result {
            Ok(result) => result,
            Err(_) => {
                Err("generic material fixture panicked; private details withheld after cleanup")
            }
        }
    }
    pub(super) fn install_catalog(&self) -> TestResult<Value> {
        let raw = self.manifest_raw.as_str();
        let manifest: Value = checked(serde_json::from_str(raw), "actual raw manifest invalid")?;
        let version = manifest["version"]
            .as_str()
            .ok_or("actual Mail version absent")?;
        let hash = sha256(raw.as_bytes());
        let key = self.invocation_key.clone();
        let payload = json!({"v":1,"registry":"https://registry.example.test","plugin_slug":"mail","release_version":version,"kid":"task4-fixture","manifest_sha256":hash,"assets":[{"name":"mail-universal.json","sha256":hash,"size":raw.len()}]});
        let body = format!(
            "{}.{}",
            B64.encode(r#"{"alg":"EdDSA","kid":"task4-fixture"}"#),
            B64.encode(payload.to_string())
        );
        let policy = json!({"runtimeDomain":"plugins.example.test","runtimePort":8443,"entryPoint":"websecure","storageID":"task4-metadata","core":{"origin":self.callback_origin,"domain":"localhost","certificate":self.path("core.crt"),"privateKey":self.path("core.key"),"authority":self.path("ca.crt"),"serverCertificateSha256":self.core_identity.certificate_sha256},"runtimeIdentity":{"authority":self.path("ca.crt"),"privateKey":self.path("ca.key")},"resources":{"cpuMillicores":100,"memoryBytes":268435456},"egress":[{"cidr":"127.0.0.1/32","port":5432}]});
        let recipe = json!({"storage":true,"sealingKey":true,"privateConfiguration":{"platformTransport":{"kind":"smtp","host":"127.0.0.1","port":self.catcher.address().port()},"mailboxes":null,"trustUnverifiedDomains":true,"delivery":{"nativeLeaseIssuerPublicKeyHex":hex::encode(self.issuer.verifying_key().as_bytes()),"gatewayPeers":[{"kind":"mutualTls","certificateSha256":self.gateway_identity.certificate_sha256}],"nativeAccounts":[]}}});
        Ok(
            json!({"version":1,"platformID":fixture_platform_id(&self.run)?,"keys":[{"registryURL":"https://registry.example.test","kid":"task4-fixture","key":{"kty":"OKP","crv":"Ed25519","x":B64.encode(key.verifying_key().as_bytes())}}],"targets":[{"id":"mail-eu","label":"Task4 prepare-only target","clusterID":"42","policy":policy}],"releases":[{"id":"mail","version":version,"registryURL":"https://registry.example.test","kid":"task4-fixture","jws":format!("{body}.{}",B64.encode(key.sign(body.as_bytes()).to_bytes())),"manifestRaw":raw,"descriptor":raw,"recipe":recipe,"supportedFeatures":["add-domains","create-mailboxes","manage-incoming"],"targetIDs":["mail-eu"]}]}),
        )
    }
    pub(super) async fn prepare_installer_material(&mut self, catalog: Value) -> TestResult {
        self.pki.write_key(&self.path("ca.key"))?;
        let catalog = checked(
            Catalog::from_bytes(
                &serde_json::to_vec(&catalog).map_err(|_| "signed catalog encode failed")?,
            ),
            "actual signed catalog invalid",
        )?;
        let descriptors = catalog.descriptors();
        let descriptor = descriptors
            .first()
            .ok_or("signed catalog descriptor absent")?;
        let target = descriptor.targets.first().ok_or("signed target absent")?;
        let input = json!({"catalogID":"mail","version":descriptor.version,"reviewSha256":target.review_sha256,"targetID":"mail-eu","configuration":{},"approvedCapabilities":["plugin.workloads"],"idempotencyKey":"task4-generic-install"});
        let response = checked(
            self.http()?
                .post(format!("{}/api/console/plugins/install", self.core_origin))
                .header("cookie", &self.cookie)
                .header("origin", &self.core_origin)
                .header("content-type", "application/json")
                .body(serde_json::to_vec(&input).map_err(|_| "install HTTP encoding failed")?)
                .send()
                .await,
            "actual signed install HTTP admission failed",
        )?;
        if response.status().as_u16() != 200 {
            return Err("actual signed install operator admission denied");
        }
        if read_reply(response).await?["pluginID"] != "mail" {
            return Err("actual install receipt identity invalid");
        }
        let worker_path = self.path("installation-worker.json");
        private(&worker_path,serde_json::to_vec(&json!({"storageTargets":[{"id":"task4-metadata","adminUrl":self.metadata.url,"authority":self.metadata.ca_path,"runtimeHost":"localhost","runtimePort":self.metadata.port}]})).map_err(|_|"private installer config encode failed")?)?;
        let plugin_path = self.path("installation-plugin.json");
        private(&plugin_path,serde_json::to_vec(&json!({"hostBindings":self.path("bindings.json"),"invocationSigningKey":self.path("invocation.hex"),"connections":self.path("connections.json"),"control":{"kind":"mutualTls","address":"127.0.0.1:0","certificate":self.path("core.crt"),"privateKey":self.path("core.key"),"authority":self.path("ca.crt")}})).map_err(|_|"private control config encode failed")?)?;
        let plugin = checked(
            PluginConfig::load(&plugin_path),
            "actual installer config load failed",
        )?;
        let worker = checked(
            InstallationWorker::load(&worker_path, self.db()?, &plugin).await,
            "actual public Worker load failed",
        )?;
        let endpoints = PluginEndpoints::new(
            self.core.as_ref().ok_or("real Core absent")?.host.clone(),
            RuntimeConnections::new(self.path("connections.json")),
            None,
        );
        if !checked(
            worker.advance(&endpoints).await,
            "actual generic advance failed",
        )? {
            return Err("generic worker did not claim admitted work");
        }
        let value=json_row(self.db()?,"SELECT jsonb_build_object('parts',e.private_components,'spec',e.spec,'binding',e.binding,'phase',o.phase,'current',p.current_generation,'candidate',p.candidate_generation,'manifest',r.manifest_bytes) AS value FROM webdock_auth.plugin_install_execution e JOIN webdock_auth.plugin_operation o ON o.id=e.operation_id JOIN webdock_auth.plugin_installation p ON p.id=e.plugin_id JOIN webdock_auth.plugin_release r ON r.plugin_id=e.plugin_id AND r.version=p.version WHERE e.plugin_id='mail'",vec![]).await?;
        if value["phase"] != "deploying" || !value["current"].is_null() || value["candidate"] != 1 {
            return Err("generic preparation lost unpromoted candidate state");
        }
        let cipher = checked(
            webdock_secrets::SecretCipher::new(&self.install_secret),
            "installation cipher construction failed",
        )?;
        let mut files = BTreeMap::new();
        for file in &MATERIAL_FILES {
            let text = if *file == "manifest.json" {
                value["manifest"]
                    .as_str()
                    .ok_or("persisted raw manifest absent")?
                    .to_owned()
            } else {
                open(&cipher, &value["parts"], &format!("material.{file}"))?
            };
            files.insert((*file).to_owned(), text);
        }
        let mut material = PluginRuntimeMaterial { files };
        if material.files.len() != 6
            || checked(
                material.fingerprint(),
                "generic material fingerprint failed",
            )? != value["spec"]["materialSha256"]
            || material.files["manifest.json"].as_bytes() != self.manifest_raw.as_bytes()
        {
            return Err("persisted generic material/raw manifest fingerprint mismatch");
        }
        let mut config: Value = checked(
            serde_json::from_str(&material.files["runtime.json"]),
            "generic runtime config invalid",
        )?;
        if config["delivery"].get("providerBinding").is_some()
            || config["generation"] != 1
            || config["mailboxes"] != Value::Null
            || config["corePeers"][0]["certificateSha256"] != self.core_identity.certificate_sha256
            || material.files["core-ca.crt"] != self.pki.ca
            || material.files["storage-ca.crt"] != self.metadata.pki.ca
            || material.files["storage-ca.crt"] == material.files["core-ca.crt"]
        {
            return Err("generic identity/trust/omitted-binding invariant failed");
        }
        let (_, pem) = checked(
            x509_parser::pem::parse_x509_pem(material.files["runtime.crt"].as_bytes()),
            "generated runtime certificate invalid",
        )?;
        let pin = sha256(&pem.contents);
        if value["binding"]["credentialSha256"] != pin
            || value["binding"]["peers"][0]["certificateSha256"] != pin
            || value["spec"]["manifestSha256"] != sha256(material.files["manifest.json"].as_bytes())
            || value["binding"]["workloadCluster"] != "42"
        {
            return Err("generated leaf/raw manifest differs from actual prepared enrollment");
        }
        let url = config["storageUrl"]
            .as_str()
            .ok_or("generic metadata login absent")?
            .to_owned();
        let connection = checked(
            webdock_plugin_kit::postgres::connect(
                &url,
                Some(material.files["storage-ca.crt"].as_bytes()),
            )
            .await,
            "generic private TLS login failed",
        )?;
        let row=checked(connection.query_one("SELECT (SELECT ssl FROM pg_stat_ssl WHERE pid=pg_backend_pid()),rolsuper OR rolcreatedb OR rolcreaterole OR rolbypassrls OR rolinherit OR rolreplication,to_regclass('public.customer_engines') IS NULL FROM pg_roles WHERE rolname=current_user",&[]).await,"restricted generic login observation failed")?;
        if !row.get::<_, bool>(0) || row.get::<_, bool>(1) || !row.get::<_, bool>(2) {
            return Err("generic preparation created engines or privileged/plaintext login");
        }
        owned::assert_postgres_rejects_ca(self.metadata.port, &self.pki.ca).await?;
        private(&self.path("metadata.url"), &url)?;
        self.meta = Some(connection);
        owned::scrub(&mut self.retained_key);
        self.retained_key = config["secretKeyHex"]
            .as_str()
            .filter(|s| s.len() == 64)
            .ok_or("generic retained sealing key absent")?
            .into();
        self.runtime_identity = webdock_plugin_host::provisioning::tls::RuntimeIdentity {
            certificate_pem: material.files["runtime.crt"].clone(),
            private_key_pem: material.files["runtime.key"].clone(),
            certificate_sha256: pin.clone(),
        };
        for (name, text) in &material.files {
            replace_private(&self.path(name), text)?;
        }
        // Relocate ONLY mounted paths/listen address; all generated private semantics stay intact.
        config["manifest"] = json!(self.path("manifest.json"));
        config["storageCa"] = json!(self.path("storage-ca.crt"));
        for section in ["listenTls", "coreTls"] {
            for (field, file) in [
                ("certificate", "runtime.crt"),
                ("privateKey", "runtime.key"),
                ("authority", "core-ca.crt"),
            ] {
                config[section][field] = json!(self.path(file));
            }
        }
        let port = dynamic_port()?;
        config["listenTls"]["address"] = json!(format!("127.0.0.1:{port}"));
        replace_private(
            &self.path("runtime.json"),
            serde_json::to_vec(&config).map_err(|_| "relocated generic config encode failed")?,
        )?;
        self.config = config;
        self.mail = Some(Process::start(&self.binary, &self.path("runtime.json"))?);
        let host = value["spec"]["runtimeHost"]
            .as_str()
            .ok_or("generic runtime DNS identity absent")?
            .to_owned();
        let mut ready = false;
        for _ in 0..150 {
            let tls = PinnedMutualTls {
                certificate: self.core_identity.certificate_pem.as_bytes().to_vec(),
                private_key: self.core_identity.private_key_pem.as_bytes().to_vec(),
                authority: self.pki.ca.as_bytes().to_vec(),
                server_certificate_sha256: pin.clone(),
            };
            if let Ok(mut client) = webdock_plugin_runtime::connect_pinned_mtls(
                format!("https://localhost:{port}"),
                host.clone(),
                tls,
            )
            .await
                && let Ok(health) = client
                    .health(webdock_plugin_runtime::wire::HealthRequest {})
                    .await
                && health.into_inner().plugin_id == "mail"
            {
                ready = true;
                break;
            }
            if !self
                .mail
                .as_mut()
                .ok_or("generic Mail child absent")?
                .running()?
            {
                return Err(
                    "Mail rejected generic automatic-binding material; diagnostics withheld",
                );
            }
            sleep(Duration::from_millis(100)).await;
        }
        if !ready {
            return Err("actual generic Mail readiness deadline");
        }
        let row=checked(self.metadata_client()?.query_one("SELECT (SELECT count(*) FROM customer_engines)+(SELECT count(*) FROM engines)+(SELECT count(*) FROM mailboxes), (SELECT max(version) FROM schema_migrations)",&[]).await,"generic actual process storage observation failed")?;
        if row.get::<_, i64>(0) != 0 || row.get::<_, i32>(1) != 4 {
            return Err("generic actual Mail private migration/zero-engine invariant failed");
        }
        let tls = PinnedMutualTls {
            certificate: self.runtime_identity.certificate_pem.as_bytes().to_vec(),
            private_key: self.runtime_identity.private_key_pem.as_bytes().to_vec(),
            authority: self.pki.ca.as_bytes().to_vec(),
            server_certificate_sha256: self.core_identity.certificate_sha256.clone(),
        };
        let channel = checked(
            webdock_plugin_runtime::pinned_mtls_channel(
                self.callback_origin.clone(),
                "localhost".into(),
                tls,
            )
            .await,
            "generic candidate actual Core channel failed",
        )?;
        let client = checked(
            NativeControlClient::new(channel, 1),
            "generic candidate control client failed",
        )?;
        require_control_denial(client.call(json!({"action":"send.claim"})).await, 403)?;
        if json_row(
            self.db()?,
            "SELECT to_jsonb(count(*)) AS value FROM webdock_auth.plugin_workload",
            vec![],
        )
        .await?
            != 0
            || self.catcher.counts().stored != 0
        {
            return Err("prepare-only candidate caused workload/mail effects");
        }
        for text in material.files.values_mut() {
            owned::scrub(text);
        }
        Ok(())
    }
}
fn open(cipher: &webdock_secrets::SecretCipher, parts: &Value, name: &str) -> TestResult<String> {
    let parts = parts[name]
        .as_array()
        .ok_or("persisted generic encrypted component absent")?;
    if parts.len() > 1024 {
        return Err("generic encrypted chunk bound exceeded");
    }
    let mut result = String::new();
    for (index, part) in parts.iter().enumerate() {
        let mut text: String = checked(
            cipher.open(
                part.as_str().ok_or("generic encrypted chunk invalid")?,
                &format!("plugin-install:mail:1:{name}:{index}"),
            ),
            "actual generic decryption failed",
        )?;
        if result.len() + text.len() > 1024 * 1024 {
            return Err("generic private material bound exceeded");
        }
        result.push_str(&text);
        owned::scrub(&mut text);
    }
    Ok(result)
}
