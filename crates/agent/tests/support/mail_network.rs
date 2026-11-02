use k8s_openapi::api::core::v1::{Namespace, Pod, Service};
use kube::{
    Api, Client,
    api::{DeleteParams, PostParams, Preconditions},
};
use serde_json::json;
use std::time::Duration;
pub struct MailNetwork {
    client: Client,
    pub gateway: String,
    pub peer: String,
    image: String,
    owner: String,
    server_ip: String,
}
impl MailNetwork {
    pub async fn create(
        client: Client,
        gateway: String,
        peer: String,
        image: String,
        owner: String,
    ) -> Self {
        let namespaces = Api::<Namespace>::all(client.clone());
        for name in [&gateway, &peer] {
            assert!(namespaces.get_opt(name).await.unwrap().is_none());
            let namespace:Namespace=serde_json::from_value(json!({"apiVersion":"v1","kind":"Namespace","metadata":{"name":name,"labels":{"webdock.dev/mail-test-fixture":owner,"pod-security.kubernetes.io/enforce":"restricted"}}})).unwrap();
            namespaces
                .create(&PostParams::default(), &namespace)
                .await
                .unwrap();
        }
        let mut fixture = Self {
            client,
            gateway,
            peer,
            image,
            owner,
            server_ip: String::new(),
        };
        let server = fixture.pod(
            "server",
            &fixture.peer,
            "probe-server",
            vec![
                "httpd".into(),
                "-f".into(),
                "-p".into(),
                "8080".into(),
                "-h".into(),
                "/tmp".into(),
            ],
        );
        let pods = Api::<Pod>::namespaced(fixture.client.clone(), &fixture.peer);
        pods.create(&PostParams::default(), &server).await.unwrap();
        for _ in 0..120 {
            let pod = pods.get("server").await.unwrap();
            if pod
                .status
                .as_ref()
                .and_then(|s| s.conditions.as_ref())
                .is_some_and(|c| c.iter().any(|c| c.type_ == "Ready" && c.status == "True"))
            {
                fixture.server_ip = pod.status.unwrap().pod_ip.unwrap();
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        assert!(
            fixture
                .server_ip
                .parse::<std::net::Ipv4Addr>()
                .unwrap()
                .is_private()
        );
        fixture
    }
    fn pod(&self, name: &str, namespace: &str, app: &str, command: Vec<String>) -> Pod {
        serde_json::from_value(json!({"apiVersion":"v1","kind":"Pod","metadata":{"name":name,"namespace":namespace,"labels":{"app":app,"webdock.dev/mail-test-fixture":self.owner}},"spec":{"restartPolicy":"Never","automountServiceAccountToken":false,"securityContext":{"runAsNonRoot":true,"runAsUser":65532,"runAsGroup":65532,"seccompProfile":{"type":"RuntimeDefault"}},"containers":[{"name":"probe","image":self.image,"imagePullPolicy":"IfNotPresent","command":command,"resources":{"requests":{"cpu":"10m","memory":"16Mi","ephemeral-storage":"1Mi"},"limits":{"cpu":"100m","memory":"32Mi","ephemeral-storage":"2Mi"}},"securityContext":{"allowPrivilegeEscalation":false,"readOnlyRootFilesystem":true,"capabilities":{"drop":["ALL"]}}}]}})).unwrap()
    }
    async fn probe(&self, namespace: &str, app: &str, command: Vec<String>, allowed: bool) {
        let name = format!("probe-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
        let pods = Api::<Pod>::namespaced(self.client.clone(), namespace);
        let created = pods
            .create(
                &PostParams::default(),
                &self.pod(&name, namespace, app, command),
            )
            .await
            .unwrap();
        let uid = created.metadata.uid.unwrap();
        let mut exit = None;
        for _ in 0..100 {
            let pod = pods.get(&name).await.unwrap();
            assert_eq!(pod.metadata.uid.as_ref(), Some(&uid));
            exit = pod
                .status
                .as_ref()
                .and_then(|s| s.container_statuses.as_ref())
                .and_then(|rows| rows.first())
                .and_then(|c| c.state.as_ref())
                .and_then(|s| s.terminated.as_ref())
                .map(|s| s.exit_code);
            if exit.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        let current = pods.get(&name).await.unwrap();
        assert_eq!(
            current.metadata.labels.as_ref().unwrap()["webdock.dev/mail-test-fixture"],
            self.owner
        );
        pods.delete(
            &name,
            &DeleteParams {
                preconditions: Some(Preconditions {
                    uid: Some(uid),
                    resource_version: current.metadata.resource_version,
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        for _ in 0..120 {
            if pods.get_opt(&name).await.unwrap().is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(pods.get_opt(&name).await.unwrap().is_none());
        assert!(exit.is_some(), "owned network probe must terminate");
        assert_eq!(
            exit == Some(0),
            allowed,
            "network path {namespace}/{app} returned {exit:?}"
        );
    }
    fn tcp(ip: &str, port: u16) -> Vec<String> {
        assert!(ip.parse::<std::net::Ipv4Addr>().unwrap().is_private());
        vec![
            "sh".into(),
            "-c".into(),
            format!("nc -w 3 {ip} {port} </dev/null"),
        ]
    }
    pub async fn ingress(&self, mail_namespace: &str, customer: &str) {
        let service = Api::<Service>::namespaced(self.client.clone(), mail_namespace)
            .get(&format!("app-{customer}"))
            .await
            .unwrap();
        let ip = service.spec.unwrap().cluster_ip.unwrap();
        self.probe(&self.gateway, "mail-gateway", Self::tcp(&ip, 8080), true)
            .await;
        self.probe(&self.gateway, "wrong-client", Self::tcp(&ip, 8080), false)
            .await;
        self.probe(&self.peer, "mail-gateway", Self::tcp(&ip, 8080), false)
            .await;
        self.probe(&self.gateway, "mail-gateway", Self::tcp(&ip, 2465), false)
            .await;
    }
    pub async fn suspended_egress(&self, mail_namespace: &str) {
        self.probe(
            &self.gateway,
            "mail-gateway",
            Self::tcp(&self.server_ip, 8080),
            true,
        )
        .await;
        self.probe(
            mail_namespace,
            "mail-probe",
            Self::tcp(&self.server_ip, 8080),
            false,
        )
        .await;
        self.probe(
            mail_namespace,
            "mail-probe",
            vec![
                "nslookup".into(),
                "kubernetes.default.svc.cluster.local".into(),
            ],
            true,
        )
        .await;
    }
}
pub async fn cleanup(client: Client, name: &str, owner: &str) {
    let api = Api::<Namespace>::all(client);
    if let Some(namespace) = api.get_opt(name).await.unwrap() {
        assert_eq!(
            namespace.metadata.labels.as_ref().unwrap()["webdock.dev/mail-test-fixture"],
            owner
        );
        api.delete(
            name,
            &DeleteParams {
                preconditions: Some(Preconditions {
                    uid: namespace.metadata.uid,
                    resource_version: namespace.metadata.resource_version,
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        for _ in 0..240 {
            if api.get_opt(name).await.unwrap().is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        assert!(api.get_opt(name).await.unwrap().is_none());
    }
}
