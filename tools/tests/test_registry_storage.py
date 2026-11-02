"""Offline render/policy checks; this suite never starts containers or touches a cluster."""
import copy
from pathlib import Path
import subprocess
import tempfile
import unittest
import yaml

ROOT = Path(__file__).resolve().parents[2]
CHART = ROOT / "deploy/helm/webdock-registry"
VALUES = {
    "image": "registry@sha256:ddf754342cfc8acc51a56d5d0ab6af06826461864460636d8bd5c546dab2a7b8",
    "existingSecret": "operator-private-registry",
    "locationEvidence": "operator reviewed EU storage assignment",
    "s3": {"endpoint": "https://s3.eu-central-2.wasabisys.com", "region": "eu-central-2", "bucket": "private-registry-fixture", "port": 443},
    "objectStoreCIDRs": ["192.0.2.1/32"],
    "clientNamespaceSelector": {"webdock.dev/role": "trusted-build-workers"},
    "clientPodSelector": {"webdock.dev/role": "publisher"},
}


def render(values):
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory) / "values.yaml"
        path.write_text(yaml.safe_dump(values))
        return subprocess.run(["helm", "template", "owned-registry", str(CHART), "--values", str(path)], capture_output=True, text=True, timeout=20)


class RegistryPolicy(unittest.TestCase):
    def test_private_s3_profile_keeps_auth_tls_and_release_data(self):
        result = render(VALUES)
        self.assertEqual(result.returncode, 0, result.stderr)
        documents = {item["kind"]: item for item in yaml.safe_load_all(result.stdout) if item}
        self.assertTrue(documents["ConfigMap"]["immutable"])
        config = yaml.safe_load(documents["ConfigMap"]["data"]["config.yml"])
        self.assertEqual(config["storage"]["s3"]["regionendpoint"], VALUES["s3"]["endpoint"])
        self.assertTrue(config["storage"]["s3"]["secure"])
        self.assertFalse(config["storage"]["s3"]["skipverify"])
        self.assertTrue(config["storage"]["s3"]["forcepathstyle"])
        self.assertTrue(config["storage"]["redirect"]["disable"])
        self.assertFalse(config["storage"]["delete"]["enabled"])
        self.assertFalse(config["storage"]["maintenance"]["uploadpurging"]["enabled"])
        self.assertEqual(config["auth"]["htpasswd"]["path"], "/run/registry-private/htpasswd")
        self.assertEqual(config["http"]["tls"]["key"], "/run/registry-private/tls.key")
        self.assertNotIn("accesskey", config["storage"]["s3"])
        self.assertNotIn("secretkey", config["storage"]["s3"])
        pod = documents["Deployment"]["spec"]["template"]["spec"]
        self.assertFalse(pod["automountServiceAccountToken"])
        self.assertTrue(pod["securityContext"]["runAsNonRoot"])
        container = pod["containers"][0]
        self.assertTrue(container["securityContext"]["readOnlyRootFilesystem"])
        self.assertFalse(container["securityContext"]["allowPrivilegeEscalation"])
        self.assertEqual(container["securityContext"]["capabilities"]["drop"], ["ALL"])
        self.assertIn("limits", container["resources"])
        self.assertEqual(container["readinessProbe"]["httpGet"]["path"], "/debug/health")
        refs = {entry["name"]: entry["valueFrom"]["secretKeyRef"] for entry in container["env"]}
        self.assertEqual(refs["REGISTRY_STORAGE_S3_ACCESSKEY"]["key"], "s3AccessKeyId")
        self.assertEqual(refs["REGISTRY_STORAGE_S3_SECRETKEY"]["key"], "s3SecretAccessKey")
        self.assertEqual(documents["Service"]["spec"]["type"], "ClusterIP")
        self.assertEqual([p["port"] for p in documents["Service"]["spec"]["ports"]], [5000])
        self.assertNotIn("PersistentVolumeClaim", documents)
        policy = documents["NetworkPolicy"]["spec"]
        self.assertEqual(set(policy["policyTypes"]), {"Ingress", "Egress"})
        self.assertEqual(policy["ingress"][0]["from"][0]["namespaceSelector"]["matchLabels"], VALUES["clientNamespaceSelector"])
        self.assertEqual(policy["egress"][1]["to"], [{"ipBlock": {"cidr": "192.0.2.1/32"}}])

    def test_unsafe_or_incomplete_operator_configuration_fails_closed(self):
        for field in ["image", "existingSecret", "locationEvidence", "objectStoreCIDRs", "clientNamespaceSelector", "clientPodSelector"]:
            values = copy.deepcopy(VALUES)
            values.pop(field)
            self.assertNotEqual(render(values).returncode, 0, field)
        for endpoint in ["http://127.0.0.1:55900", "https://user:secret@example.test", "https://example.test/path", "https://example.test/?key=hidden"]:
            values = copy.deepcopy(VALUES)
            values["s3"]["endpoint"] = endpoint
            self.assertNotEqual(render(values).returncode, 0, endpoint)
        values = copy.deepcopy(VALUES)
        values["image"] = "registry:latest"
        self.assertNotEqual(render(values).returncode, 0)
        values = copy.deepcopy(VALUES)
        values["objectStoreCIDRs"] = ["0.0.0.0/0"]
        self.assertNotEqual(render(values).returncode, 0)


if __name__ == "__main__":
    unittest.main()
