"""Fail closed before modifying an arbitrary local Docker node or private input."""
import importlib.util
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("build_runtime", Path(__file__).with_name("kubernetes-build-runtime.py"))
HARNESS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(HARNESS)


class RuntimeFixtureGuards(unittest.TestCase):
    def test_only_owned_private_regular_input_is_accepted(self):
        with tempfile.TemporaryDirectory() as root:
            source = Path(root) / "image.tar"
            source.write_bytes(b"owned")
            source.chmod(0o600)
            self.assertEqual(HARNESS.private_file(str(source)), source)
            source.chmod(0o644)
            with self.assertRaises(RuntimeError):
                HARNESS.private_file(str(source))
            source.chmod(0o600)
            link = Path(root) / "link.tar"
            link.symlink_to(source)
            with self.assertRaises(RuntimeError):
                HARNESS.private_file(str(link))
            hard = Path(root) / "hard.tar"
            hard.hardlink_to(source)
            with self.assertRaises(RuntimeError):
                HARNESS.private_file(str(source))

    def test_foreign_container_identity_never_authorizes_local_changes(self):
        record = {"container": "owned-id", "name": "webdock-build-run-test-0123456789abcdef", "nonce": "0123456789abcdef"}
        node = {"Id": record["container"], "Name": "/" + record["name"], "Config": {"Labels": {"webdock.build-runtime": record["nonce"]}}}
        with patch.object(HARNESS, "inspect_container", return_value=node):
            self.assertEqual(HARNESS.owned_node(record), node)
            for key, value in [("container", "foreign-id"), ("name", "production"), ("nonce", "foreign")]:
                wrong = {**record, key: value}
                with self.assertRaises(RuntimeError):
                    HARNESS.owned_node(wrong)

    def test_cleanup_never_adopts_a_resource_not_created_by_this_invocation(self):
        with patch.object(HARNESS, "run") as process:
            HARNESS.cleanup_created_run({"volumeCreated": False, "containerCreationStarted": False})
            process.assert_not_called()

    def test_cleanup_refuses_foreign_volume_without_any_removal(self):
        record = {"volumeCreated": True, "containerCreationStarted": False, "volume": "owned-volume", "nonce": "owned"}
        result = SimpleNamespace(returncode=0, stdout=b'[{"Name":"owned-volume","Labels":{"webdock.build-runtime":"foreign"}}]')
        with patch.object(HARNESS, "run", return_value=result) as process:
            with self.assertRaisesRegex(RuntimeError, "volume cleanup ownership mismatch"):
                HARNESS.cleanup_created_run(record)
            self.assertEqual(process.call_count, 1)
            self.assertEqual(process.call_args.args[0][:3], ["docker", "volume", "inspect"])

    def test_unavailable_docker_is_not_proof_of_cleanup(self):
        record = {"volumeCreated": True, "containerCreationStarted": False, "volume": "owned-volume", "nonce": "owned"}
        result = SimpleNamespace(returncode=1, stdout=b"", stderr=b"Cannot connect to the Docker daemon")
        with patch.object(HARNESS, "run", return_value=result):
            with self.assertRaisesRegex(RuntimeError, "absence is uncertain"):
                HARNESS.cleanup_created_run(record)


if __name__ == "__main__":
    unittest.main()
