"""The live fixture must retain its DB whenever remote cleanup is unproven."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from types import SimpleNamespace

SPEC=importlib.util.spec_from_file_location("kubernetes_builds",Path(__file__).with_name("kubernetes-builds.py"))
HARNESS=importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(HARNESS)

class LiveCleanupGuards(unittest.TestCase):
    def test_failed_policy_preflight_never_creates_postgres(self):
        with tempfile.TemporaryDirectory() as root:
            config=Path(root)/"live.json";config.write_text(json.dumps({"timeoutSeconds":30}));config.chmod(0o600)
            with patch.object(HARNESS,"ROOT",Path(root)),patch.object(HARNESS.subprocess,"run",return_value=SimpleNamespace(returncode=1)) as process:
                with self.assertRaisesRegex(AssertionError,"no PostgreSQL container created"):
                    HARNESS.main(["--live-config",str(config),"--live-only"])
                self.assertEqual(process.call_count,1)
                self.assertTrue(all(call.args[0][0]=="cargo" for call in process.call_args_list))

    def test_missing_false_foreign_and_malformed_proofs_keep_the_owned_ledger(self):
        with tempfile.TemporaryDirectory() as root:
            proof=Path(root)/"proof.json"
            self.assertFalse(HARNESS.live_cleanup_proven(proof,"owned-container"))
            for body in [{"containerID":"owned-container","allJobsAbsent":False},{"containerID":"other-container","allJobsAbsent":True},{"containerID":"owned-container","allJobsAbsent":"true"}]:
                proof.write_text(json.dumps(body));proof.chmod(0o600)
                self.assertFalse(HARNESS.live_cleanup_proven(proof,"owned-container"))
            proof.write_text("not JSON")
            self.assertFalse(HARNESS.live_cleanup_proven(proof,"owned-container"))

    def test_only_exact_private_cleanup_proof_allows_postgres_teardown(self):
        with tempfile.TemporaryDirectory() as root:
            proof=Path(root)/"proof.json";proof.write_text(json.dumps({"containerID":"owned-container","allJobsAbsent":True}));proof.chmod(0o600)
            self.assertTrue(HARNESS.live_cleanup_proven(proof,"owned-container"))
            proof.chmod(0o644)
            self.assertFalse(HARNESS.live_cleanup_proven(proof,"owned-container"))
            proof.chmod(0o600);link=Path(root)/"symlink.json";link.symlink_to(proof)
            self.assertFalse(HARNESS.live_cleanup_proven(link,"owned-container"))

if __name__=="__main__":unittest.main()
