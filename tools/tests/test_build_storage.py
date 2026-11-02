import importlib.util
import contextlib
import io
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("build_storage", Path(__file__).with_name("build-storage.py"))
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


class FreshCheckout(unittest.TestCase):
    def test_schema_only_has_no_vm_acceptance_target(self):
        self.assertEqual(fixture.acceptance_plan(fixture.parse_args(["--schema-only"])), (None, 0))

    def test_legacy_only_preserves_plain_dockerfile_target(self):
        self.assertEqual(fixture.acceptance_plan(fixture.parse_args(["--legacy-only"])),
                         ("git_build_publication_preserves_durable_authority_and_atomic_hosting", 360))
    def test_frontend_selector_runs_only_frontend_native_acceptance_with_finite_deadline(self):
        args = fixture.parse_args(["--frontend-only"])
        target, timeout = fixture.acceptance_plan(args)
        self.assertEqual(target, "git_frontend_remote_outputs_cross_worker_publication_and_rollback")
        self.assertGreater(timeout, 360)
        self.assertLessEqual(timeout, 900)

    def test_old_selectors_keep_existing_targets_and_conflicting_selectors_refuse(self):
        self.assertEqual(fixture.acceptance_plan(fixture.parse_args([])), ("git_", 360))
        self.assertEqual(fixture.acceptance_plan(fixture.parse_args(["--remote-only"])),
                         ("git_remote_artifact_cross_worker_publication_and_rollback", 360))
        with self.assertRaises(SystemExit), contextlib.redirect_stderr(io.StringIO()):
            fixture.parse_args(["--remote-only", "--frontend-only"])

    def test_diagnostics_parent_is_created_privately_in_a_fresh_checkout(self):
        with tempfile.TemporaryDirectory() as checkout:
            directory = fixture.diagnostics_directory(Path(checkout))
            self.assertTrue(directory.is_dir())
            self.assertEqual(directory.stat().st_mode & 0o777, 0o700)
            self.assertEqual(directory.parent.stat().st_mode & 0o777, 0o700)
            second = fixture.diagnostics_directory(Path(checkout))
            self.assertNotEqual(directory, second)


if __name__ == "__main__":
    unittest.main()
