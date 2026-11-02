use super::*;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AgentInput {
    #[serde(rename = "platformID")]
    platform_id: String,
    #[serde(rename = "agentID")]
    agent_id: String,
    name: String,
    country: String,
    region: String,
    location_evidence: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_runtime_kind"
    )]
    runtime_kind: Option<RuntimeKind>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_runtime_profile"
    )]
    runtime_profile: Option<RuntimeProfile>,
    transport: Transport,
}
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum RuntimeKind {
    Kubernetes,
    Docker,
}
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
enum RuntimeProfile {
    #[serde(rename = "local-recovery-fixture-v1")]
    LocalRecoveryFixtureV1,
}
fn present_runtime_kind<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<RuntimeKind>, D::Error> {
    match String::deserialize(d)?.as_str() {
        "kubernetes" => Ok(Some(RuntimeKind::Kubernetes)),
        "docker" => Ok(Some(RuntimeKind::Docker)),
        _ => Err(serde::de::Error::custom("Invalid reviewed runtime kind")),
    }
}
fn present_runtime_profile<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<RuntimeProfile>, D::Error> {
    match String::deserialize(d)?.as_str() {
        "local-recovery-fixture-v1" => Ok(Some(RuntimeProfile::LocalRecoveryFixtureV1)),
        _ => Err(serde::de::Error::custom("Invalid reviewed runtime profile")),
    }
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum Transport {
    Unix { uid: u32 },
    Mtls { certificate: PathBuf },
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AgentIdentity {
    installation: String,
    #[serde(rename = "agentID")]
    agent_id: String,
    generation: u8,
}
/// Private file only; intentionally no Debug.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EnrollmentOutput {
    version: u8,
    #[serde(rename = "platformID")]
    platform_id: String,
    #[serde(rename = "clusterID")]
    cluster_id: String,
    identity: AgentIdentity,
    credential: String,
    checksum: String,
}
struct ReviewedTransport {
    kind: &'static str,
    uid: Option<i64>,
    certificate: Option<String>,
}
impl AgentInput {
    fn runtime_kind(&self) -> &'static str {
        if self.runtime_kind == Some(RuntimeKind::Docker) {
            "docker"
        } else {
            "kubernetes"
        }
    }
    fn review(&mut self) -> Result<ReviewedTransport> {
        self.country.make_ascii_lowercase();
        if !slug(&self.platform_id, 24)
            || !slug(&self.agent_id, 64)
            || self.region != "eu"
            || self.name.trim() != self.name
            || self.name.is_empty()
            || self.name.chars().count() > 160
            || self.name.chars().any(char::is_control)
            || self.location_evidence.trim().is_empty()
            || self.location_evidence.len() > 4000
            || self.location_evidence.chars().any(char::is_control)
            || self.country.len() != 2
            || !self.country.bytes().all(|b| b.is_ascii_lowercase())
        {
            return Err(SetupError::Input);
        }
        match (self.runtime_kind, self.runtime_profile) {
            (None | Some(RuntimeKind::Kubernetes), None) => {}
            (Some(RuntimeKind::Docker), Some(RuntimeProfile::LocalRecoveryFixtureV1))
                if matches!(self.transport, Transport::Unix { .. }) => {}
            _ => return Err(SetupError::Input),
        }
        match &self.transport {
            Transport::Unix { uid } => Ok(ReviewedTransport {
                kind: "unix",
                uid: Some(i64::from(*uid)),
                certificate: None,
            }),
            Transport::Mtls { certificate } => {
                let bytes = files::read(certificate, 65536)?;
                let (rest, pem) =
                    x509_parser::pem::parse_x509_pem(&bytes).map_err(|_| SetupError::Input)?;
                if !rest.iter().all(u8::is_ascii_whitespace) {
                    return Err(SetupError::Input);
                }
                let now = u64::try_from(x509_parser::time::ASN1Time::now().timestamp())
                    .map_err(|_| SetupError::Input)?;
                let identity = webdock_agent_protocol::certificate_identity(&pem.contents, now)
                    .map_err(|_| SetupError::Input)?;
                let (_, leaf) = x509_parser::parse_x509_certificate(&pem.contents)
                    .map_err(|_| SetupError::Input)?;
                if leaf.is_ca()
                    || !leaf
                        .extended_key_usage()
                        .map_err(|_| SetupError::Input)?
                        .is_some_and(|eku| eku.value.client_auth)
                {
                    return Err(SetupError::Input);
                }
                Ok(ReviewedTransport {
                    kind: "mtls",
                    uid: None,
                    certificate: Some(identity.sha256),
                })
            }
        }
    }
}
async fn existing(
    offline: &Offline,
    input: &AgentInput,
    transport: &ReviewedTransport,
    output: &EnrollmentOutput,
) -> Result<()> {
    let hash = sha(output.credential.as_bytes());
    let row=offline.tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
        "SELECT c.name,c.provider,c.country,c.region,c.location_evidence,c.generation,c.verified,c.ownership,c.runtime_kind,c.runtime_installation,c.dedicated_customer_id,c.verification_evidence,a.credential_hash,a.generation AS agent_generation,a.revoked,e.installation,e.agent_id,e.generation AS transport_generation,e.cluster_id,e.credential_hash AS transport_hash,e.transport,e.unix_uid,e.certificate_sha256,e.revoked AS transport_revoked FROM webdock_auth.hosting_cluster c JOIN webdock_auth.hosting_agent a ON a.cluster_id=c.id JOIN webdock_auth.agent_transport_enrollment e ON e.cluster_id=c.id WHERE c.id=$1",
        [output.cluster_id.clone().into()])).await.map_err(|_|SetupError::Database)?.ok_or(SetupError::Existing)?;
    for (field, expected) in [
        ("name", input.name.as_str()),
        ("provider", "k3s"),
        ("country", &input.country),
        ("region", "eu"),
        ("location_evidence", &input.location_evidence),
        ("ownership", "platform"),
        ("runtime_kind", input.runtime_kind()),
        ("runtime_installation", &input.platform_id),
        ("verification_evidence", &input.location_evidence),
        ("credential_hash", &hash),
        ("installation", &input.platform_id),
        ("agent_id", &input.agent_id),
        ("cluster_id", &output.cluster_id),
        ("transport_hash", &hash),
        ("transport", transport.kind),
    ] {
        if row
            .try_get::<String>("", field)
            .map_err(|_| SetupError::Existing)?
            != expected
        {
            return Err(SetupError::Existing);
        }
    }
    for field in ["generation", "agent_generation", "transport_generation"] {
        if row
            .try_get::<i32>("", field)
            .map_err(|_| SetupError::Existing)?
            != 1
        {
            return Err(SetupError::Existing);
        }
    }
    if !row
        .try_get::<bool>("", "verified")
        .map_err(|_| SetupError::Existing)?
        || row
            .try_get::<bool>("", "revoked")
            .map_err(|_| SetupError::Existing)?
        || row
            .try_get::<bool>("", "transport_revoked")
            .map_err(|_| SetupError::Existing)?
        || row
            .try_get::<Option<String>>("", "dedicated_customer_id")
            .map_err(|_| SetupError::Existing)?
            .is_some()
        || row
            .try_get::<Option<i64>>("", "unix_uid")
            .map_err(|_| SetupError::Existing)?
            != transport.uid
        || row
            .try_get::<Option<String>>("", "certificate_sha256")
            .map_err(|_| SetupError::Existing)?
            != transport.certificate
    {
        return Err(SetupError::Existing);
    }
    Ok(())
}
/// Offline reviewed cluster + agent transport enrollment. No replacement, rotation or dummy tenant.
pub async fn enroll_agent(
    environment: &HashMap<String, String>,
    target: &str,
    input_file: &Path,
    destination: &Path,
) -> Result<()> {
    let offline = Offline::open(environment, target).await?;
    let mut input: AgentInput = super::input(input_file)?;
    let transport = input.review()?;
    files::parent(destination)?;
    let checksum = sha(encode(
        &json!({"target":target,"input":input,"certificateSha256":transport.certificate}),
    )?);
    offline.tx.execute_unprepared("LOCK TABLE webdock_auth.hosting_cluster,webdock_auth.hosting_agent,webdock_auth.agent_transport_enrollment IN SHARE ROW EXCLUSIVE MODE").await.map_err(|_|SetupError::Database)?;
    if destination.try_exists().map_err(|_| SetupError::Output)? {
        let output: EnrollmentOutput = super::input(destination)?;
        if output.version != 1
            || output.platform_id != input.platform_id
            || output.checksum != checksum
            || output.identity.installation != input.platform_id
            || output.identity.agent_id != input.agent_id
            || output.identity.generation != 1
            || output.cluster_id.is_empty()
            || !output.cluster_id.bytes().all(|b| b.is_ascii_digit())
            || output.credential.len() != 43
            || !output
                .credential
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        {
            return Err(SetupError::Existing);
        }
        existing(&offline, &input, &transport, &output).await?;
        return offline.finish().await;
    }
    let conflict=offline.tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT 1 FROM webdock_auth.hosting_cluster WHERE name=$1 UNION ALL SELECT 1 FROM webdock_auth.agent_transport_enrollment WHERE (installation=$2 AND agent_id=$3) OR (certificate_sha256=$4 AND $4 IS NOT NULL) LIMIT 1",[input.name.clone().into(),input.platform_id.clone().into(),input.agent_id.clone().into(),transport.certificate.clone().into()])).await.map_err(|_|SetupError::Database)?.is_some();
    if conflict {
        return Err(SetupError::Existing);
    }
    let cluster_id = offline.allocate().await?;
    let mut credential = [0u8; 32];
    getrandom::getrandom(&mut credential).map_err(|_| SetupError::Database)?;
    let credential = URL_SAFE_NO_PAD.encode(credential);
    let hash = sha(credential.as_bytes());
    offline.tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.hosting_cluster(id,name,provider,country,region,location_evidence,generation,verified,verification_evidence,ownership,runtime_kind,runtime_installation) VALUES($1,$2,'k3s',$3,'eu',$4,1,true,$4,'platform',$6,$5)",[cluster_id.clone().into(),input.name.clone().into(),input.country.clone().into(),input.location_evidence.clone().into(),input.platform_id.clone().into(),input.runtime_kind().into()])).await.map_err(|_|SetupError::Database)?;
    offline.tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.hosting_agent(cluster_id,credential_hash,generation) VALUES($1,$2,1)",[cluster_id.clone().into(),hash.clone().into()])).await.map_err(|_|SetupError::Database)?;
    offline.tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.agent_transport_enrollment(installation,agent_id,generation,cluster_id,credential_hash,transport,unix_uid,certificate_sha256) VALUES($1,$2,1,$3,$4,$5,$6,$7)",[input.platform_id.clone().into(),input.agent_id.clone().into(),cluster_id.clone().into(),hash.into(),transport.kind.into(),transport.uid.into(),transport.certificate.into()])).await.map_err(|_|SetupError::Database)?;
    let output = EnrollmentOutput {
        version: 1,
        platform_id: input.platform_id.clone(),
        cluster_id,
        identity: AgentIdentity {
            installation: input.platform_id,
            agent_id: input.agent_id,
            generation: 1,
        },
        credential,
        checksum,
    };
    // Write durable private material before commit. A lost commit reply retains this exact token;
    // a replay succeeds only if all three retained enrollment records agree with it.
    files::write_exclusive(destination, &encode(&output)?)?;
    offline.finish().await
}

#[cfg(test)]
mod tests {
    use super::*;
    fn reviewed(transport: serde_json::Value) -> AgentInput {
        serde_json::from_value(json!({"platformID":"owned-platform","agentID":"owned-agent","name":"EU cluster","country":"de","region":"eu","locationEvidence":"Reviewed local fixture","transport":transport})).unwrap()
    }
    #[test]
    fn explicit_local_docker_input_is_supported_without_changing_omitted_legacy_bytes() {
        let legacy = reviewed(json!({"kind":"unix","uid":0}));
        let bytes = serde_json::to_vec(&legacy).unwrap();
        let expected=br#"{"platformID":"owned-platform","agentID":"owned-agent","name":"EU cluster","country":"de","region":"eu","locationEvidence":"Reviewed local fixture","transport":{"kind":"unix","uid":0}}"#;
        assert_eq!(bytes, expected);
        let old: serde_json::Value = serde_json::from_slice(expected).unwrap();
        assert_eq!(sha(encode(&json!({"target":"127.0.0.1:5432/owned_core","input":legacy,"certificateSha256":null})).unwrap()),sha(encode(&json!({"target":"127.0.0.1:5432/owned_core","input":old,"certificateSha256":null})).unwrap()));
        let mut docker = old.clone();
        docker["runtimeKind"] = json!("docker");
        docker["runtimeProfile"] = json!("local-recovery-fixture-v1");
        let mut parsed: AgentInput =
            serde_json::from_value(docker.clone()).expect("explicit reviewed local Docker input");
        parsed.review().unwrap();
        for changes in [
            json!({"runtimeKind":{"docker":null},"runtimeProfile":"local-recovery-fixture-v1"}),
            json!({"runtimeKind":"docker","runtimeProfile":{"local-recovery-fixture-v1":null}}),
            json!({"runtimeKind":null}),
            json!({"runtimeKind":"remote"}),
            json!({"runtimeKind":"docker"}),
            json!({"runtimeProfile":"local-recovery-fixture-v1"}),
            json!({"runtimeKind":"kubernetes","runtimeProfile":"local-recovery-fixture-v1"}),
            json!({"runtimeKind":"docker","runtimeProfile":null}),
            json!({"runtimeKind":"docker","runtimeProfile":"production"}),
        ] {
            let mut value = old.clone();
            value
                .as_object_mut()
                .unwrap()
                .extend(changes.as_object().unwrap().clone());
            assert!(
                serde_json::from_value::<AgentInput>(value)
                    .map(|mut input| input.review().is_err())
                    .unwrap_or(true)
            );
        }
        docker["transport"] =
            json!({"kind":"mtls","certificate":"/must-not-read-unrelated-certificate"});
        assert!(
            serde_json::from_value::<AgentInput>(docker)
                .map(|mut input| input.review().is_err())
                .unwrap_or(true)
        );
    }
    #[test]
    fn local_docker_rejects_mtls_before_opening_an_unrelated_certificate() {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let certificate = directory.path().join("must-not-open-certificate");
        assert!(
            std::process::Command::new("/usr/bin/mkfifo")
                .arg(&certificate)
                .status()
                .unwrap()
                .success()
        );
        let mut value =
            serde_json::to_value(reviewed(json!({"kind":"mtls","certificate":certificate})))
                .unwrap();
        value["runtimeKind"] = json!("docker");
        value["runtimeProfile"] = json!("local-recovery-fixture-v1");
        let mut input: AgentInput = serde_json::from_value(value).unwrap();
        let (send, receive) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || send.send(input.review().is_err()).unwrap());
        let outcome = receive.recv_timeout(std::time::Duration::from_secs(2));
        if outcome.is_err() {
            // Release a defective reader so even the failed regression remains bounded.
            let writer = std::fs::OpenOptions::new()
                .write(true)
                .custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32)
                .open(&certificate);
            drop(writer);
        }
        thread.join().unwrap();
        assert!(
            matches!(outcome, Ok(true)),
            "Docker must deny before opening the unrelated FIFO certificate"
        );
    }
    #[test]
    fn enrollment_checks_actual_current_client_leaf_instead_of_accepting_a_claimed_pin() {
        let directory = tempfile::tempdir().unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let ca_key = rcgen::KeyPair::generate().unwrap();
        let mut ca = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
        ca.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        ca.key_usages = vec![rcgen::KeyUsagePurpose::KeyCertSign];
        let ca_pem = ca.self_signed(&ca_key).unwrap().pem();
        let leaf = webdock_plugin_host::provisioning::tls::issue_runtime_identity(
            &ca_pem,
            &ca_key.serialize_pem(),
            "agent.example.test",
        )
        .unwrap();
        let cert = directory.path().join("agent.crt");
        files::write_exclusive(&cert, leaf.certificate_pem.as_bytes()).unwrap();
        let transport = reviewed(json!({"kind":"mtls","certificate":cert}))
            .review()
            .unwrap();
        assert_eq!(transport.kind, "mtls");
        assert_eq!(
            transport.certificate.as_deref(),
            Some(leaf.certificate_sha256.as_str())
        );
        assert!(serde_json::from_value::<AgentInput>(json!({"platformID":"owned-platform","agentID":"owned-agent","name":"EU cluster","country":"de","region":"eu","locationEvidence":"Reviewed","transport":{"kind":"mtls","certificate":cert,"certificateSha256":"00".repeat(32)}})).is_err());
        std::fs::write(&cert, ca_pem).unwrap();
        assert!(
            reviewed(json!({"kind":"mtls","certificate":cert}))
                .review()
                .is_err()
        );
        let key = rcgen::KeyPair::generate().unwrap();
        let mut expired = rcgen::CertificateParams::new(vec!["agent.example.test".into()]).unwrap();
        expired.not_before = rcgen::date_time_ymd(2000, 1, 1);
        expired.not_after = rcgen::date_time_ymd(2001, 1, 1);
        expired.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ClientAuth];
        std::fs::write(&cert, expired.self_signed(&key).unwrap().pem()).unwrap();
        assert!(
            reviewed(json!({"kind":"mtls","certificate":cert}))
                .review()
                .is_err()
        );
        assert!(reviewed(json!({"kind":"unix","uid":0})).review().is_ok());
    }
}
