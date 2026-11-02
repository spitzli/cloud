import importlib.util
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("webdock_package_proxy", ROOT / "manage.py")
manage = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(manage)

class GuardTests(unittest.TestCase):
    def inventory(self):
        return {"hostname": "webdock-build-eu-01", "addresses": ["167.86.75.144", "10.83.0.1"], "os_id": "ubuntu", "os_version": "24.04", "nodes": ["webdock-build-eu-01"], "proxy_uid": 13}

    def test_installer_refuses_foreign_host_identity_before_mutation(self):
        manage.assert_inventory(self.inventory())
        for key, value in [("hostname", "runtime"), ("addresses", ["167.86.75.144"]), ("nodes", ["webdock-build-eu-01", "runtime"]), ("proxy_uid", 1000), ("os_version", "22.04")]:
            inventory = self.inventory()
            inventory[key] = value
            with self.assertRaisesRegex(RuntimeError, "wrong_build_host"):
                manage.assert_inventory(inventory)

    def test_active_and_unobserved_native_jobs_block_install_but_terminal_jobs_do_not(self):
        def job(status):
            return {"metadata": {"labels": {"webdock.dev/component": "build-job"}}, "status": status}
        for status in [{}, {"active": 1}, {"conditions": [{"type": "FailureTarget", "status": "True"}]}]:
            with self.assertRaisesRegex(RuntimeError, "active_native_build_job"):
                manage.assert_idle_jobs({"items": [job(status)]})
        for status in [{"conditions": [{"type": "Complete", "status": "True"}]}, {"conditions": [{"type": "Failed", "status": "True"}]}]:
            manage.assert_idle_jobs({"items": [job(status)]})

    def test_dpkg_success_for_unknown_package_is_not_an_installed_service(self):
        self.assertFalse(manage.package_present("unknown ok not-installed"))
        self.assertFalse(manage.package_present(""))
        for status in ["install ok installed", "deinstall ok config-files", "install ok unpacked"]:
            self.assertTrue(manage.package_present(status))

    def test_host_allowlist_rejects_broad_suffixes_ips_and_unapproved_domains(self):
        self.assertEqual(manage.validate_hosts("registry.npmjs.org\n# comment\nnodejs.org\n"), ["registry.npmjs.org", "nodejs.org"])
        for value in [".com", "*", "*.github.com", "127.0.0.1", "example.com", "https://github.com", "github.com evil.test", ".cloudflarestorage.com"]:
            with self.assertRaisesRegex(RuntimeError, "invalid_package_host"):
                manage.validate_hosts(value)

class EndpointUpdateTests(unittest.TestCase):
    def fixture(self, root, active=False, fail_reload=False):
        import hashlib, json
        from contextlib import ExitStack
        from types import SimpleNamespace
        from unittest.mock import patch
        root=Path(root);bundle=root/"bundle";bundle.mkdir();destination=root/"installed-hosts";destination.write_text("auth.docker.io\n");(bundle/"hosts.allow").write_text("auth.docker.io\nproduction.cloudfront.docker.com\n")
        state=root/"owner.json";record={"owner":"webdock-package-proxy-v1","phase":"ready","files":{str(destination):hashlib.sha256(destination.read_bytes()).hexdigest()},"untouched":"original authority"};state.write_text(json.dumps(record))
        calls=[]
        def run(args,**kwargs):
            calls.append(args)
            if fail_reload and "reconfigure" in args and kwargs.get("check",True):raise RuntimeError("reload_failed")
            return SimpleNamespace(returncode=0,stdout=b"")
        stack=ExitStack()
        for name,value in [("BUNDLE",bundle),("STATE",state),("FILES",{"hosts.allow":destination})]:stack.enter_context(patch.object(manage,name,value))
        stack.enter_context(patch.object(manage.os,"geteuid",return_value=0));stack.enter_context(patch.object(manage,"inventory",return_value=GuardTests().inventory()));stack.enter_context(patch.object(manage,"check_installed",return_value=record));stack.enter_context(patch.object(manage,"assert_default_deny"));stack.enter_context(patch.object(manage,"owned_bytes",side_effect=lambda path,*_:Path(path).read_bytes()));stack.enter_context(patch.object(manage,"run",side_effect=run))
        jobs={"items":[{"metadata":{"labels":{"webdock.dev/component":"build-job"}},"status":{"active":1}}] if active else []}
        stack.enter_context(patch.object(manage,"kubectl",return_value=SimpleNamespace(stdout=json.dumps(jobs).encode())))
        return stack,destination,state,calls

    def test_reviewed_endpoint_update_changes_only_hosts_and_its_recorded_hash(self):
        import tempfile,json,hashlib
        with tempfile.TemporaryDirectory() as root:
            stack,destination,state,calls=self.fixture(root)
            with stack:manage.update_hosts()
            record=json.loads(state.read_text());self.assertEqual(record["untouched"],"original authority");self.assertEqual(record["files"][str(destination)],hashlib.sha256(destination.read_bytes()).hexdigest());self.assertIn("production.cloudfront.docker.com",destination.read_text());self.assertTrue(all(args[0]=="/usr/sbin/squid" for args in calls))

    def test_active_execution_prevents_endpoint_mutation(self):
        import tempfile
        with tempfile.TemporaryDirectory() as root:
            stack,destination,state,calls=self.fixture(root,active=True);before=state.read_bytes()
            with stack,self.assertRaisesRegex(RuntimeError,"active_native_build_job"):manage.update_hosts()
            self.assertEqual(destination.read_text(),"auth.docker.io\n");self.assertEqual(state.read_bytes(),before);self.assertEqual(calls,[])

    def test_reload_failure_restores_original_hosts_and_authority(self):
        import tempfile
        with tempfile.TemporaryDirectory() as root:
            stack,destination,state,calls=self.fixture(root,fail_reload=True);before=state.read_bytes()
            with stack,self.assertRaisesRegex(RuntimeError,"reload_failed"):manage.update_hosts()
            self.assertEqual(destination.read_text(),"auth.docker.io\n");self.assertEqual(state.read_bytes(),before);self.assertEqual(sum("reconfigure" in args for args in calls),2)

if __name__ == "__main__":
    unittest.main()
