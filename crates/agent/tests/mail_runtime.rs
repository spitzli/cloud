#![cfg(feature = "docker")]
#[path = "support/configuration_fixture.rs"]
mod configuration_fixture;
use configuration_fixture::{ConfigurationAuthority, configuration_claim};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::Mutex;
use webdock_agent::mail::{MailAuthority, MailRuntime, docker::DockerMailRuntime};
use webdock_agent_protocol::mail::*;
use webdock_supervisor::{ExecutionError, Result};

const IMAGE: &str = "registry.example.test/webdock/mail@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
struct Authority {
    allowed: AtomicBool,
    accept: AtomicBool,
    checkpoint: Mutex<Option<MailCheckpoint>>,
}
#[async_trait::async_trait]
impl MailAuthority for Authority {
    async fn guard(&self, _: &MailClaim) -> Result<()> {
        if self.allowed.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(ExecutionError::Denied)
        }
    }
    async fn checkpoint(&self, claim: &MailClaim, checkpoint: MailCheckpoint) -> Result<()> {
        self.guard(claim).await?;
        let mut stored = self.checkpoint.lock().await;
        if stored
            .as_ref()
            .is_some_and(|old| old.runtime_id != checkpoint.runtime_id)
        {
            return Err(ExecutionError::Denied);
        }
        *stored = Some(checkpoint);
        if !self.accept.load(Ordering::SeqCst) {
            return Err(ExecutionError::Journal);
        }
        Ok(())
    }
}
fn claim(image: &str) -> MailClaim {
    let material = MailMaterial::Credentials {
        bootstrap_password: Some(uuid::Uuid::new_v4().to_string()),
        username: "webdock-admin@mail-123.example.test".into(),
        password: uuid::Uuid::new_v4().to_string(),
    };
    MailClaim {
        spec: MailSpec {
            version: 1,
            operation_id: "456".into(),
            customer_id: "123".into(),
            cluster_id: "789".into(),
            revision: 1,
            agent_generation: 1,
            instance_key: "mail-123".into(),
            hostname: "mail-123.example.test".into(),
            image: image.into(),
            resources: MailResources::FIXED,
            material_sha256: material.fingerprint().unwrap(),
            action: MailAction::Ensure,
        },
        material,
        lease: MailLease {
            lease_token: uuid::Uuid::new_v4().to_string(),
        },
        lease_until: chrono::Utc::now() + chrono::Duration::seconds(120),
        checkpoint: None,
    }
}
#[tokio::test]
#[ignore = "explicit local Docker CE fixture"]
async fn incoming_bootstrap_checkpoint_replay_retained_suspend_and_guard() {
    assert_eq!(
        std::env::var("WEBDOCK_MAIL_RUNTIME_DOCKER_TEST").as_deref(),
        Ok("1")
    );
    let image=std::env::var("WEBDOCK_MAIL_RUNTIME_IMAGE").expect("set the reviewed restricted Mail image repository@sha256 digest built from infra/mail/Dockerfile");
    let installation = format!("test{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let _cleanup = Cleanup(installation.clone());
    let runtime =
        DockerMailRuntime::connect("/var/run/docker.sock", image.clone(), installation.clone())
            .await
            .unwrap();
    let authority = Arc::new(Authority {
        allowed: AtomicBool::new(true),
        accept: AtomicBool::new(false),
        checkpoint: Mutex::new(None),
    });
    let mut claim = claim(&image);
    let mut unallocated = claim.clone();
    unallocated.spec.action = MailAction::Suspend;
    unallocated.material = MailMaterial::Unallocated {};
    unallocated.spec.material_sha256 = unallocated.material.fingerprint().unwrap();
    let empty = runtime
        .execute(&unallocated, authority.as_ref())
        .await
        .unwrap();
    assert!(!empty.running);
    assert_eq!(empty.runtime_id, "unallocated");
    let absent = std::process::Command::new("docker")
        .args([
            "--host",
            "unix:///var/run/docker.sock",
            "volume",
            "inspect",
            &format!("wd-mail-{installation}-123-data"),
        ])
        .output()
        .unwrap();
    assert!(
        !absent.status.success(),
        "unallocated suspend must not allocate storage"
    );
    let foreign_name = format!("wd-mail-{installation}-124-data");
    let created = std::process::Command::new("docker")
        .args([
            "--host",
            "unix:///var/run/docker.sock",
            "volume",
            "create",
            "--label",
            &format!("webdock.mail.installation={installation}"),
            "--label",
            "webdock.mail.customer=foreign",
            &foreign_name,
        ])
        .output()
        .unwrap();
    assert!(created.status.success());
    let mut foreign = claim.clone();
    foreign.spec.customer_id = "124".into();
    foreign.spec.instance_key = "mail-124".into();
    foreign.spec.hostname = "mail-124.example.test".into();
    assert!(matches!(
        runtime.execute(&foreign, authority.as_ref()).await,
        Err(ExecutionError::Ownership)
    ));
    let first = runtime.execute(&claim, authority.as_ref()).await.err();
    assert!(
        matches!(first, Some(ExecutionError::Journal)),
        "initial checkpoint must be reached: {first:?}"
    );
    claim.checkpoint = authority.checkpoint.lock().await.clone();
    assert!(
        claim.checkpoint.is_some(),
        "lost checkpoint acknowledgement retains exact accepted identity"
    );
    authority.accept.store(true, Ordering::SeqCst);
    let proof = runtime.execute(&claim, authority.as_ref()).await.unwrap();
    assert!(proof.running && proof.recovery_disabled);
    assert_eq!(
        proof.submission_certificate_sha256.as_ref().unwrap().len(),
        64
    );
    claim.checkpoint = authority.checkpoint.lock().await.clone();
    let original_hostname = claim.spec.hostname.clone();
    claim.spec.hostname = "mail-123.foreign.test".into();
    assert!(matches!(
        runtime.execute(&claim, authority.as_ref()).await,
        Err(ExecutionError::Ownership)
    ));
    claim.spec.hostname = original_hostname;
    let replay = runtime.execute(&claim, authority.as_ref()).await.unwrap();
    assert_eq!(proof.runtime_id, replay.runtime_id);
    assert_eq!(
        proof.submission_certificate_sha256,
        replay.submission_certificate_sha256
    );
    let retained = management(
        &claim,
        &proof,
        "x:Domain/set",
        serde_json::json!({"create":{"retained":{"name":"retained.example.test"}}}),
    )
    .await;
    let domain_id = retained["created"]["retained"]["id"].as_str().unwrap();
    let mailbox_password = uuid::Uuid::new_v4().to_string();
    let account=management(&claim,&proof,"x:Account/set",serde_json::json!({"create":{"mailbox":{"@type":"User","name":"recipient","domainId":domain_id,"credentials":{"0":{"@type":"Password","secret":mailbox_password}},"roles":{"@type":"User"},"permissions":{"@type":"Inherit"},"encryptionAtRest":{"@type":"Disabled"},"quotas":{"maxDiskQuota":10485760}}}})).await;
    let account_id = account["created"]["mailbox"]["id"].as_str().unwrap();
    let folders = user_call(
        &proof,
        &mailbox_password,
        "Mailbox/get",
        serde_json::json!({"accountId":account_id,"ids":null}),
    )
    .await;
    let drafts = folders["list"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["role"] == "drafts")
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let created=user_call(&proof,&mailbox_password,"Email/set",serde_json::json!({"accountId":account_id,"create":{"draft":{"mailboxIds":{drafts:true},"keywords":{"$draft":true},"from":[{"email":"recipient@retained.example.test"}],"subject":"Retained native Mail message","textBody":[{"partId":"body","type":"text/plain"}],"bodyValues":{"body":{"value":"Retained body survives suspension."}}}}})).await;
    let email_id = created["created"]["draft"]["id"].as_str().unwrap();
    authority.allowed.store(false, Ordering::SeqCst);
    assert!(runtime.execute(&claim, authority.as_ref()).await.is_err());
    authority.allowed.store(true, Ordering::SeqCst);
    claim.spec.action = MailAction::Suspend;
    claim.spec.operation_id = "457".into();
    claim.spec.revision = 2;
    claim.checkpoint = None;
    let stopped = runtime.execute(&claim, authority.as_ref()).await.unwrap();
    assert!(!stopped.running);
    claim.spec.action = MailAction::Ensure;
    claim.spec.operation_id = "458".into();
    claim.spec.revision = 3;
    if let MailMaterial::Credentials {
        bootstrap_password, ..
    } = &mut claim.material
    {
        *bootstrap_password = None;
    }
    claim.spec.material_sha256 = claim.material.fingerprint().unwrap();
    *authority.checkpoint.lock().await = None;
    let resumed = runtime.execute(&claim, authority.as_ref()).await.unwrap();
    assert_eq!(proof.runtime_id, resumed.runtime_id);
    let retained_email = user_call(
        &resumed,
        &mailbox_password,
        "Email/get",
        serde_json::json!({"accountId":account_id,"ids":[email_id],"fetchTextBodyValues":true}),
    )
    .await;
    assert_eq!(
        retained_email["list"][0]["subject"],
        "Retained native Mail message"
    );
    assert!(
        retained_email["list"][0]["bodyValues"]
            .as_object()
            .unwrap()
            .values()
            .any(|body| body["value"]
                .as_str()
                .is_some_and(|body| body.contains("Retained body survives suspension.")))
    );
    let domains = management(
        &claim,
        &resumed,
        "x:Domain/get",
        serde_json::json!({"ids":null}),
    )
    .await;
    assert!(
        domains["list"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["name"] == "retained.example.test")
    );
    assert_eq!(
        proof.submission_certificate_sha256,
        resumed.submission_certificate_sha256
    );
    claim.spec.agent_generation = 2;
    claim.spec.revision = 4;
    claim.spec.operation_id = "459".into();
    claim.lease.lease_token = uuid::Uuid::new_v4().to_string();
    *authority.checkpoint.lock().await = None;
    let reenrolled = runtime.execute(&claim, authority.as_ref()).await.unwrap();
    assert_eq!(reenrolled.runtime_id, resumed.runtime_id);
    assert_eq!(
        reenrolled.submission_certificate_sha256,
        resumed.submission_certificate_sha256
    );
}
fn cleanup(installation: &str) {
    for (kind, args) in [
        ("container", vec!["rm", "-fv"]),
        ("volume", vec!["volume", "rm"]),
        ("network", vec!["network", "rm"]),
    ] {
        let mut command = std::process::Command::new("docker");
        command.args([
            "--host",
            "unix:///var/run/docker.sock",
            kind,
            "ls",
            "-q",
            "--filter",
            &format!("label=webdock.mail.installation={installation}"),
        ]);
        if kind == "container" {
            command.arg("--all");
        }
        let output = command.output().unwrap();
        assert!(output.status.success());
        for id in String::from_utf8(output.stdout).unwrap().lines() {
            let status = std::process::Command::new("docker")
                .args(["--host", "unix:///var/run/docker.sock"])
                .args(&args)
                .arg(id)
                .status()
                .unwrap();
            assert!(status.success());
        }
    }
}

struct Cleanup(String);
impl Drop for Cleanup {
    fn drop(&mut self) {
        cleanup(&self.0);
    }
}

async fn management(
    claim: &MailClaim,
    proof: &MailProof,
    method: &str,
    arguments: serde_json::Value,
) -> serde_json::Value {
    let MailMaterial::Credentials {
        username, password, ..
    } = &claim.material
    else {
        panic!("fixture credentials missing")
    };
    let response:serde_json::Value=reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).timeout(std::time::Duration::from_secs(5)).build().unwrap().post(format!("http://{}:8080/jmap",proof.private_ipv4.unwrap())).basic_auth(username,Some(password)).json(&serde_json::json!({"using":["urn:ietf:params:jmap:core","urn:stalwart:jmap"],"methodCalls":[[method,arguments,"test"]]})).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
    assert_eq!(response["methodResponses"][0][0], method);
    response["methodResponses"][0][1].clone()
}

#[test]
fn mail_configuration_requires_a_pinned_image_and_bounded_installation() {
    use webdock_agent::mail::docker::DockerMailConfig;
    for (image, installation) in [
        ("stalwartlabs/stalwart:latest", "fixture"),
        (IMAGE, "../other"),
        (IMAGE, ""),
    ] {
        assert!(
            DockerMailConfig {
                image: image.into(),
                installation: installation.into()
            }
            .validate()
            .is_err()
        );
    }
    assert!(
        DockerMailConfig {
            image: IMAGE.into(),
            installation: "local-fixture".into()
        }
        .validate()
        .is_ok()
    );
    assert!(
        serde_json::from_str::<DockerMailConfig>(
            r#"{"image":"ignored","installation":"fixture","remoteSocket":"tcp://host:2375"}"#
        )
        .is_err()
    );
}

async fn user_call(
    proof: &MailProof,
    password: &str,
    method: &str,
    arguments: serde_json::Value,
) -> serde_json::Value {
    let response:serde_json::Value=reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).timeout(std::time::Duration::from_secs(5)).build().unwrap().post(format!("http://{}:8080/jmap",proof.private_ipv4.unwrap())).basic_auth("recipient@retained.example.test",Some(password)).json(&serde_json::json!({"using":["urn:ietf:params:jmap:core","urn:ietf:params:jmap:mail"],"methodCalls":[[method,arguments,"test"]]})).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
    assert_eq!(response["methodResponses"][0][0], method);
    response["methodResponses"][0][1].clone()
}

#[derive(Clone, Copy)]
enum Interruption {
    VolumeCreated,
    InitializerCreated,
}
struct InterruptAuthority {
    inner: Authority,
    name: String,
    phase: Interruption,
    armed: AtomicBool,
}
#[async_trait::async_trait]
impl MailAuthority for InterruptAuthority {
    async fn guard(&self, claim: &MailClaim) -> Result<()> {
        self.inner.guard(claim).await?;
        if !self.armed.load(Ordering::SeqCst) {
            return Ok(());
        }
        let interrupted = match self.phase {
            Interruption::VolumeCreated => {
                docker_status("volume", &format!("{}-data", self.name)).is_some()
            }
            Interruption::InitializerCreated => {
                docker_status("container", &format!("{}-init", self.name)).as_deref()
                    == Some("created")
            }
        };
        if interrupted {
            self.armed.store(false, Ordering::SeqCst);
            return Err(ExecutionError::Denied);
        }
        Ok(())
    }
    async fn checkpoint(&self, claim: &MailClaim, checkpoint: MailCheckpoint) -> Result<()> {
        self.inner.checkpoint(claim, checkpoint).await
    }
}
fn docker_status(kind: &str, name: &str) -> Option<String> {
    let field = if kind == "volume" {
        "{{.Name}}"
    } else {
        "{{.State.Status}}"
    };
    let output = std::process::Command::new("docker")
        .args([
            "--host",
            "unix:///var/run/docker.sock",
            kind,
            "inspect",
            "--format",
            field,
            name,
        ])
        .env_remove("DOCKER_CONTEXT")
        .output()
        .unwrap();
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).unwrap().trim().to_owned())
}
async fn interrupted_allocation(phase: Interruption) {
    assert_eq!(
        std::env::var("WEBDOCK_MAIL_RUNTIME_DOCKER_TEST").as_deref(),
        Ok("1")
    );
    let image = std::env::var("WEBDOCK_MAIL_RUNTIME_IMAGE").unwrap();
    let installation = format!("test{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let _cleanup = Cleanup(installation.clone());
    let runtime =
        DockerMailRuntime::connect("/var/run/docker.sock", image.clone(), installation.clone())
            .await
            .unwrap();
    let claim = claim(&image);
    let authority = InterruptAuthority {
        inner: Authority {
            allowed: AtomicBool::new(true),
            accept: AtomicBool::new(true),
            checkpoint: Mutex::new(None),
        },
        name: format!("wd-mail-{installation}-123"),
        phase,
        armed: AtomicBool::new(true),
    };
    assert!(matches!(
        runtime.execute(&claim, &authority).await,
        Err(ExecutionError::Denied)
    ));
    assert!(docker_status("volume", &format!("{}-data", authority.name)).is_some());
    assert!(docker_status("container", &authority.name).is_none());
    authority.inner.allowed.store(false, Ordering::SeqCst);
    assert!(matches!(
        runtime.execute(&claim, &authority).await,
        Err(ExecutionError::Denied)
    ));
    assert!(docker_status("container", &authority.name).is_none());
    authority.inner.allowed.store(true, Ordering::SeqCst);
    drop(runtime);
    let runtime = DockerMailRuntime::connect("/var/run/docker.sock", image, installation)
        .await
        .unwrap();
    if matches!(phase, Interruption::InitializerCreated) {
        assert_eq!(
            docker_status("container", &format!("{}-init", authority.name)).as_deref(),
            Some("created")
        );
        reject_altered_initializer(&runtime, &claim, &authority).await;
    }
    let proof = runtime.execute(&claim, &authority).await.unwrap();
    assert!(proof.running && proof.recovery_disabled);
    assert!(docker_status("container", &format!("{}-init", authority.name)).is_none());
}
#[tokio::test]
#[ignore = "explicit local Docker CE allocation interruption"]
async fn mail_recovers_after_owned_volume_creation() {
    interrupted_allocation(Interruption::VolumeCreated).await;
}
#[tokio::test]
#[ignore = "explicit local Docker CE initializer interruption"]
async fn mail_recovers_after_owned_initializer_creation() {
    interrupted_allocation(Interruption::InitializerCreated).await;
}

async fn reject_altered_initializer(
    runtime: &DockerMailRuntime,
    claim: &MailClaim,
    authority: &InterruptAuthority,
) {
    use bollard::{Docker, models::ContainerCreateBody, query_parameters::*};
    let docker =
        Docker::connect_with_unix("/var/run/docker.sock", 10, bollard::API_DEFAULT_VERSION)
            .unwrap()
            .negotiate_version()
            .await
            .unwrap();
    let name = format!("{}-init", authority.name);
    let saved = format!("{name}-saved");
    let original = docker
        .inspect_container(&name, None::<InspectContainerOptions>)
        .await
        .unwrap();
    let original_id = original.id.as_ref().unwrap();
    docker
        .rename_container(
            original_id,
            RenameContainerOptionsBuilder::default()
                .name(&saved)
                .build(),
        )
        .await
        .unwrap();
    for foreign_owner in [true, false] {
        let mut value = serde_json::to_value(original.config.as_ref().unwrap()).unwrap();
        value["HostConfig"] = serde_json::to_value(original.host_config.as_ref().unwrap()).unwrap();
        let mut config: ContainerCreateBody = serde_json::from_value(value).unwrap();
        if foreign_owner {
            config
                .labels
                .as_mut()
                .unwrap()
                .insert("webdock.mail.customer".into(), "foreign".into());
        } else {
            config.cmd = Some(vec!["0:0".into(), "/data".into()]);
        }
        let forged = docker
            .create_container(
                Some(CreateContainerOptionsBuilder::default().name(&name).build()),
                config,
            )
            .await
            .unwrap();
        assert!(matches!(
            runtime.execute(claim, authority).await,
            Err(ExecutionError::Ownership)
        ));
        let after = docker
            .inspect_container(&forged.id, None::<InspectContainerOptions>)
            .await
            .unwrap();
        assert_eq!(after.id.as_ref(), Some(&forged.id));
        assert_eq!(
            after.state.unwrap().status,
            Some(bollard::models::ContainerStateStatusEnum::CREATED)
        );
        docker
            .remove_container(&forged.id, None::<RemoveContainerOptions>)
            .await
            .unwrap();
    }
    docker
        .rename_container(
            original_id,
            RenameContainerOptionsBuilder::default().name(&name).build(),
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "explicit local CE configuration contract fixture"]
async fn ce_configuration_group_user_dkim_contract() {
    use serde_json::{Value, json};
    assert_eq!(
        std::env::var("WEBDOCK_MAIL_RUNTIME_DOCKER_TEST").as_deref(),
        Ok("1")
    );
    let image = std::env::var("WEBDOCK_MAIL_RUNTIME_IMAGE").unwrap();
    let installation = format!("cfg{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let _cleanup = Cleanup(installation.clone());
    let runtime = DockerMailRuntime::connect("/var/run/docker.sock", image.clone(), installation)
        .await
        .unwrap();
    let authority = Authority {
        allowed: AtomicBool::new(true),
        accept: AtomicBool::new(true),
        checkpoint: Mutex::new(None),
    };
    let claim = claim(&image);
    let proof = runtime.execute(&claim, &authority).await.unwrap();
    assert!(proof.running && proof.recovery_disabled);
    let domain_marker = "webdock:domain:123:7001";
    let created = management(&claim, &proof, "x:Domain/set", json!({"create":{"resource":{"name":"configuration.example.test","description":domain_marker,"certificateManagement":{"@type":"Manual"},"dnsManagement":{"@type":"Manual"},"dkimManagement":{"@type":"Automatic","algorithms":{"Dkim1RsaSha256":true},"selectorTemplate":"webdock-{algorithm}-{date-%Y%m%d}"}}}})).await;
    let domain = created["created"]["resource"]["id"]
        .as_str()
        .expect("CE must support an owned Domain marker")
        .to_owned();
    let readback = management(&claim, &proof, "x:Domain/get", json!({"ids":[domain]})).await;
    assert_eq!(readback["list"][0]["description"], domain_marker);
    let internal = management(&claim, &proof, "x:Domain/get", json!({"ids":null})).await;
    let internal = internal["list"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == claim.spec.hostname)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let group = management(&claim, &proof, "x:Account/set", json!({"create":{"resource":{"@type":"Group","name":"shared","domainId":domain,"description":"webdock:mailbox:123:7002","roles":{"@type":"Default"},"permissions":{"@type":"Inherit"},"quotas":{"maxDiskQuota":4194304}}}})).await;
    let group = group["created"]["resource"]["id"]
        .as_str()
        .expect("CE must create the Group mailbox")
        .to_owned();
    let permissions = [
        "authenticate",
        "jmapCoreEcho",
        "jmapMailboxGet",
        "jmapMailboxChanges",
        "jmapMailboxQuery",
        "jmapMailboxQueryChanges",
        "jmapMailboxCreate",
        "jmapMailboxUpdate",
        "jmapMailboxDestroy",
        "jmapThreadGet",
        "jmapThreadChanges",
        "jmapEmailGet",
        "jmapEmailChanges",
        "jmapEmailQuery",
        "jmapEmailQueryChanges",
        "jmapEmailCreate",
        "jmapEmailUpdate",
        "jmapEmailDestroy",
        "jmapEmailImport",
        "jmapEmailParse",
        "jmapSearchSnippetGet",
        "jmapIdentityGet",
        "jmapIdentityChanges",
        "jmapEmailSubmissionGet",
        "jmapEmailSubmissionChanges",
        "jmapEmailSubmissionQuery",
        "jmapEmailSubmissionQueryChanges",
        "jmapQuotaGet",
        "jmapQuotaQuery",
        "jmapBlobGet",
        "jmapBlobUpload",
        "jmapBlobLookup",
    ]
    .into_iter()
    .map(|name| (name.to_owned(), true))
    .collect::<std::collections::BTreeMap<_, _>>();
    let password = uuid::Uuid::new_v4().to_string();
    let username = format!("wd-7003@{}", claim.spec.hostname);
    let user = management(&claim, &proof, "x:Account/set", json!({"create":{"resource":{"@type":"User","name":"wd-7003","domainId":internal,"description":"webdock:identity:123:7003","roles":{"@type":"User"},"permissions":{"@type":"Replace","enabledPermissions":permissions,"disabledPermissions":{}},"quotas":{"maxDiskQuota":1048576},"encryptionAtRest":{"@type":"Disabled"},"credentials":{"0":{"@type":"Password","secret":password}},"memberGroupIds":{(group.as_str()):true}}}})).await;
    let user = user["created"]["resource"]["id"]
        .as_str()
        .expect("CE must create the restricted User identity")
        .to_owned();
    let observed = management(&claim, &proof, "x:Account/get", json!({"ids":[user]})).await;
    assert_eq!(
        observed["list"][0]["permissions"]["enabledPermissions"],
        json!(permissions)
    );
    assert_eq!(
        observed["list"][0]["memberGroupIds"],
        json!({(group.as_str()):true})
    );
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap();
    let origin = format!("http://{}:8080", proof.private_ipv4.unwrap());
    let session: Value = client
        .get(format!("{origin}/jmap/session"))
        .basic_auth(&username, Some(&password))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        session["accounts"].get(&group).is_some(),
        "enabled identity must see its Group mailbox"
    );
    let mailboxes: Value = client.post(format!("{origin}/jmap")).basic_auth(&username, Some(&password)).json(&json!({"using":["urn:ietf:params:jmap:core","urn:ietf:params:jmap:mail"],"methodCalls":[["Mailbox/get",{"accountId":group,"ids":null},"fixture"]]})).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
    assert_eq!(mailboxes["methodResponses"][0][0], "Mailbox/get");
    let forbidden: Value = client.post(format!("{origin}/jmap")).basic_auth(&username, Some(&password)).json(&json!({"using":["urn:ietf:params:jmap:core","urn:ietf:params:jmap:mail","urn:ietf:params:jmap:submission"],"methodCalls":[["EmailSubmission/set",{"accountId":user,"create":{"never-send":{"emailId":"invalid-fixture-id","identityId":"invalid-fixture-id"}}},"fixture"]]})).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
    assert_eq!(forbidden["methodResponses"][0][0], "error");
    assert_eq!(forbidden["methodResponses"][0][1]["type"], "forbidden");
    let mut keys = Vec::new();
    for _ in 0..100 {
        let read = management(
            &claim,
            &proof,
            "x:DkimSignature/get",
            json!({"ids":null,"properties":["id","domainId","selector","publicKey","stage"]}),
        )
        .await;
        keys = read["list"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|key| key["domainId"] == domain && key["stage"] == "active")
            .cloned()
            .collect();
        if !keys.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    assert!(
        !keys.is_empty() && keys.len() <= 2,
        "automatic RSA DKIM must provide bounded active public keys"
    );
    management(&claim, &proof, "x:Account/set", json!({"update":{(user.as_str()):{"memberGroupIds":{},"permissions":{"@type":"Replace","enabledPermissions":{},"disabledPermissions":{}}}}})).await;
    let revoked = client
        .get(format!("{origin}/jmap/session"))
        .basic_auth(&username, Some(&password))
        .send()
        .await
        .unwrap();
    assert!(
        !revoked.status().is_success(),
        "contraction must revoke interactive authentication"
    );
    management(&claim, &proof, "x:Account/set", json!({"update":{(user.as_str()):{"memberGroupIds":{(group.as_str()):true},"permissions":{"@type":"Replace","enabledPermissions":permissions,"disabledPermissions":{}}}}})).await;
    assert!(
        client
            .get(format!("{origin}/jmap/session"))
            .basic_auth(&username, Some(&password))
            .send()
            .await
            .unwrap()
            .status()
            .is_success(),
        "regrant must preserve the original credential"
    );
    let after = management(&claim, &proof, "x:DkimSignature/get", json!({"ids":keys.iter().map(|key| key["id"].clone()).collect::<Vec<_>>(),"properties":["id","domainId","selector","publicKey","stage"]})).await;
    assert_eq!(after["list"], json!(keys));
}

#[tokio::test]
#[ignore = "explicit owned Docker CE native configuration executor"]
async fn native_configuration_replays_checkpoint_and_contracts_membership_without_rotation() {
    use serde_json::json;
    use webdock_agent::mail::configuration::ConfigurationRuntime;
    use webdock_agent_protocol::mail_configuration::*;
    assert_eq!(
        std::env::var("WEBDOCK_MAIL_RUNTIME_DOCKER_TEST").as_deref(),
        Ok("1")
    );
    let image = std::env::var("WEBDOCK_MAIL_RUNTIME_IMAGE").unwrap();
    let installation = format!("cfg{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let _cleanup = Cleanup(installation.clone());
    let runtime =
        DockerMailRuntime::connect("/var/run/docker.sock", image.clone(), installation.clone())
            .await
            .unwrap();
    let lifecycle_authority = Authority {
        allowed: AtomicBool::new(true),
        accept: AtomicBool::new(true),
        checkpoint: Mutex::new(None),
    };
    let lifecycle = claim(&image);
    let instance = runtime
        .execute(&lifecycle, &lifecycle_authority)
        .await
        .unwrap();
    let mut claim = configuration_claim(&lifecycle, &instance);
    let authority = ConfigurationAuthority {
        allowed: AtomicBool::new(true),
        drop_applied: AtomicBool::new(true),
        checkpoints: Mutex::new(Vec::new()),
    };
    assert!(matches!(
        runtime.configure(&claim, &authority).await,
        Err(ExecutionError::Journal)
    ));
    claim.checkpoints = authority.checkpoints.lock().await.clone();
    assert!(claim.checkpoints.iter().any(|checkpoint| matches!(
        checkpoint.phase,
        ConfigurationCheckpointPhase::Applied { .. }
    )));
    let runtime = DockerMailRuntime::connect("/var/run/docker.sock", image, installation)
        .await
        .unwrap();
    let proof = runtime.configure(&claim, &authority).await.unwrap();
    assert_eq!(proof.runtime_id, instance.runtime_id);
    assert_eq!(proof.mailboxes.len(), 2);
    assert_eq!(proof.identities.len(), 1);
    let identity = proof.identities["7004"].clone().unwrap();
    let before = management(
        &lifecycle,
        &instance,
        "x:Account/get",
        json!({"ids":[identity]}),
    )
    .await;
    let credentials = claim.material.identities["7004"].clone();
    let original_dkim = serde_json::to_value(&proof.dkim).unwrap();
    authority.allowed.store(false, Ordering::SeqCst);
    assert!(matches!(
        runtime.configure(&claim, &authority).await,
        Err(ExecutionError::Denied)
    ));
    authority.allowed.store(true, Ordering::SeqCst);
    claim.spec.mode = ConfigurationMode::Restrict;
    claim.spec.operation_id = "7006".into();
    claim.spec.revision = "2".into();
    claim.spec.snapshot.domains.clear();
    claim
        .spec
        .snapshot
        .mailboxes
        .retain(|mailbox| mailbox.id == "7002");
    claim.spec.snapshot.mailboxes[0].provider_id = Some(proof.mailboxes["7002"].clone());
    claim.spec.snapshot.identities[0].provider_id = Some(identity.clone());
    claim.spec.snapshot.identities[0].mailbox_ids = vec!["7002".into()];
    claim.spec.snapshot.grants = vec![ConfigurationGrant {
        mailbox_id: "7003".into(),
        subject: "42".into(),
        revision: "2".into(),
        enabled: false,
    }];
    claim.material.identities.clear();
    claim.spec.material_sha256 = claim.material.fingerprint().unwrap();
    claim.lease.lease_token = uuid::Uuid::new_v4().to_string();
    claim.lease_until = chrono::Utc::now() + chrono::Duration::seconds(120);
    claim.checkpoints.clear();
    authority.checkpoints.lock().await.clear();
    let restricted = runtime.configure(&claim, &authority).await.unwrap();
    assert!(restricted.domains.is_empty() && restricted.dkim.is_empty());
    let after = management(
        &lifecycle,
        &instance,
        "x:Account/get",
        json!({"ids":[identity]}),
    )
    .await;
    assert_eq!(
        before["list"][0]["permissions"],
        after["list"][0]["permissions"]
    );
    assert_eq!(
        after["list"][0]["memberGroupIds"],
        json!({(proof.mailboxes["7002"].as_str()):true})
    );
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let origin = format!(
        "http://{}:8080/jmap/session",
        instance.private_ipv4.unwrap()
    );
    let session: serde_json::Value = client
        .get(&origin)
        .basic_auth(&credentials.username, Some(&credentials.password))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(session["accounts"].get(&proof.mailboxes["7002"]).is_some());
    assert!(session["accounts"].get(&proof.mailboxes["7003"]).is_none());
    claim.spec.operation_id = "7007".into();
    claim.spec.revision = "3".into();
    claim.spec.snapshot.mailboxes.clear();
    claim.spec.snapshot.identities[0].enabled = false;
    claim.spec.snapshot.identities[0].mailbox_ids.clear();
    claim.spec.snapshot.grants[0].mailbox_id = "7002".into();
    claim.spec.snapshot.grants[0].revision = "3".into();
    claim.lease.lease_token = uuid::Uuid::new_v4().to_string();
    claim.lease_until = chrono::Utc::now() + chrono::Duration::seconds(120);
    claim.checkpoints.clear();
    authority.checkpoints.lock().await.clear();
    runtime.configure(&claim, &authority).await.unwrap();
    assert!(matches!(
        client
            .get(&origin)
            .basic_auth(&credentials.username, Some(&credentials.password))
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
    ));
    let keys = management(&lifecycle, &instance, "x:DkimSignature/get", json!({"ids":proof.dkim["7001"].iter().map(|key| key.id.clone()).collect::<Vec<_>>(),"properties":["id","domainId","selector","publicKey","stage"]})).await;
    assert_eq!(
        keys["list"][0]["publicKey"],
        original_dkim["7001"][0]["publicKey"]
    );
}

#[tokio::test]
#[ignore = "explicit owned Docker Engine storage-custody transition probe"]
async fn stopped_replacement_pins_the_same_volume_across_remove_rename_start() {
    use bollard::{
        Docker, errors::Error as DockerError, models::ContainerCreateBody, query_parameters::*,
    };
    use serde_json::json;
    assert_eq!(
        std::env::var("WEBDOCK_MAIL_RUNTIME_DOCKER_TEST").as_deref(),
        Ok("1")
    );
    let image = std::env::var("WEBDOCK_MAIL_RUNTIME_IMAGE").unwrap();
    let installation = format!("rcv{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let _cleanup = Cleanup(installation.clone());
    let runtime = DockerMailRuntime::connect("/var/run/docker.sock", image, installation.clone())
        .await
        .unwrap();
    let authority = Authority {
        allowed: AtomicBool::new(true),
        accept: AtomicBool::new(true),
        checkpoint: Mutex::new(None),
    };
    let mut operation = claim(&std::env::var("WEBDOCK_MAIL_RUNTIME_IMAGE").unwrap());
    let original = runtime.execute(&operation, &authority).await.unwrap();
    management(&operation, &original, "x:Domain/set", json!({"create":{"retained":{"name":"custody-retained.example.test","description":"owned fixture retained data"}}})).await;
    let docker =
        Docker::connect_with_unix("/var/run/docker.sock", 10, bollard::API_DEFAULT_VERSION)
            .unwrap()
            .negotiate_version()
            .await
            .unwrap();
    let canonical = format!("wd-mail-{installation}-123");
    let volume_name = format!("{canonical}-data");
    let original_volume = docker.inspect_volume(&volume_name).await.unwrap();
    assert_eq!(
        original_volume.labels["webdock.mail.installation"],
        installation
    );
    assert!(
        original_volume.created_at.is_some(),
        "local Engine reports a creation timestamp, not a volume UUID"
    );
    assert_eq!(original_volume.driver, "local");
    assert_eq!(
        original_volume.scope,
        Some(bollard::models::VolumeScopeEnum::LOCAL)
    );
    let old = docker
        .inspect_container(&original.runtime_id, None::<InspectContainerOptions>)
        .await
        .unwrap();
    let config = old.config.unwrap();
    assert_eq!(
        config.labels.as_ref().unwrap()["webdock.mail.installation"],
        installation
    );
    assert_eq!(old.id.as_deref(), Some(original.runtime_id.as_str()));
    assert_eq!(
        old.mounts.as_ref().unwrap()[0].source.as_deref(),
        Some(original_volume.mountpoint.as_str())
    );
    docker
        .stop_container(
            &original.runtime_id,
            Some(StopContainerOptionsBuilder::default().t(3).build()),
        )
        .await
        .unwrap();
    let replacement_name = format!("{canonical}-recovery-457");
    let replacement = docker
        .create_container(
            Some(
                CreateContainerOptionsBuilder::default()
                    .name(&replacement_name)
                    .build(),
            ),
            ContainerCreateBody {
                image: config.image,
                user: config.user,
                entrypoint: config.entrypoint,
                cmd: config.cmd,
                env: config.env,
                labels: config.labels,
                host_config: old.host_config,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let prepared = docker
        .inspect_container(&replacement.id, None::<InspectContainerOptions>)
        .await
        .unwrap();
    assert_eq!(
        prepared.config.as_ref().unwrap().labels.as_ref().unwrap()["webdock.mail.installation"],
        installation
    );
    assert_eq!(prepared.state.as_ref().unwrap().running, Some(false));
    assert_eq!(
        prepared.mounts.as_ref().unwrap()[0].name.as_deref(),
        Some(volume_name.as_str())
    );
    assert!(
        matches!(
            docker
                .remove_volume(&volume_name, None::<RemoveVolumeOptions>)
                .await,
            Err(DockerError::DockerResponseServerError {
                status_code: 409,
                ..
            })
        ),
        "both exact containers retain the volume"
    );
    let old = docker
        .inspect_container(&original.runtime_id, None::<InspectContainerOptions>)
        .await
        .unwrap();
    assert_eq!(
        old.config.unwrap().labels.unwrap()["webdock.mail.installation"],
        installation
    );
    docker
        .remove_container(&original.runtime_id, None::<RemoveContainerOptions>)
        .await
        .unwrap();
    assert!(
        matches!(
            docker
                .remove_volume(&volume_name, None::<RemoveVolumeOptions>)
                .await,
            Err(DockerError::DockerResponseServerError {
                status_code: 409,
                ..
            })
        ),
        "the stopped prepared replacement still pins storage after old removal"
    );
    let prepared = docker
        .inspect_container(&replacement.id, None::<InspectContainerOptions>)
        .await
        .unwrap();
    assert_eq!(
        prepared.config.unwrap().labels.unwrap()["webdock.mail.installation"],
        installation
    );
    docker
        .rename_container(
            &replacement.id,
            RenameContainerOptionsBuilder::default()
                .name(&canonical)
                .build(),
        )
        .await
        .unwrap();
    let renamed = docker
        .inspect_container(&canonical, None::<InspectContainerOptions>)
        .await
        .unwrap();
    assert_eq!(renamed.id.as_deref(), Some(replacement.id.as_str()));
    assert_eq!(renamed.state.as_ref().unwrap().running, Some(false));
    docker
        .start_container(&replacement.id, None::<StartContainerOptions>)
        .await
        .unwrap();
    let after_volume = docker.inspect_volume(&volume_name).await.unwrap();
    assert_eq!(after_volume.name, original_volume.name);
    assert_eq!(after_volume.created_at, original_volume.created_at);
    assert_eq!(after_volume.mountpoint, original_volume.mountpoint);
    assert_eq!(after_volume.labels, original_volume.labels);
    assert_eq!(after_volume.options, original_volume.options);
    operation.spec.operation_id = "457".into();
    operation.spec.revision = 2;
    operation.lease.lease_token = uuid::Uuid::new_v4().to_string();
    operation.lease_until = chrono::Utc::now() + chrono::Duration::seconds(120);
    if let MailMaterial::Credentials {
        bootstrap_password, ..
    } = &mut operation.material
    {
        *bootstrap_password = None;
    }
    operation.spec.material_sha256 = operation.material.fingerprint().unwrap();
    operation.checkpoint = None;
    let authority = Authority {
        allowed: AtomicBool::new(true),
        accept: AtomicBool::new(true),
        checkpoint: Mutex::new(None),
    };
    let proof = runtime.execute(&operation, &authority).await.unwrap();
    assert_eq!(proof.runtime_id, replacement.id);
    assert_eq!(
        proof.submission_certificate_sha256,
        original.submission_certificate_sha256
    );
    let retained = management(&operation, &proof, "x:Domain/get", json!({"ids":null})).await;
    assert!(
        retained["list"]
            .as_array()
            .unwrap()
            .iter()
            .any(|domain| domain["name"] == "custody-retained.example.test")
    );
}
