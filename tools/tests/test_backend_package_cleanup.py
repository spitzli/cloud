"""Controlled Docker CLI state only; never contacts or changes a real daemon."""
import importlib.util
import json
import os
from pathlib import Path
import stat
import subprocess
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("backend_package", ROOT / "tools/tests/backend-package.py")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class Docker:
    def __init__(self, root, nonce):
        self.root, self.nonce = root, nonce
        self.builder = None
        self.container = None
        self.volume = None
        self.events = []
        self.fail = None
        self.down = False
        self.captured_environment = None

    def __call__(self, argv, **options):
        assert argv[:3] == ["docker", "--host", "unix:///var/run/docker.sock"]
        self.captured_environment = options["env"]
        args = argv[3:]
        if self.down:
            raise subprocess.CalledProcessError(1, argv)
        result = ""
        if args[0] == "info":
            result = json.dumps({"ID": "owned-daemon"})
        elif args[:2] == ["buildx", "ls"]:
            result = json.dumps(self.builder) if self.builder else ""
        elif args[:2] == ["container", "ls"]:
            selected = self.container
            condition = args[args.index("--filter") + 1]
            if selected and condition.startswith("name=") and selected["Name"] != "/" + condition.removeprefix("name=^/").removesuffix("$"):
                selected = None
            if selected and condition.startswith("id=") and selected["Id"] != condition[3:]:
                selected = None
            result = selected["Id"] + "\n" if selected else ""
        elif args[:2] == ["container", "inspect"]:
            result = json.dumps([self.container])
        elif args[:2] == ["volume", "ls"]:
            result = self.volume["Name"] + "\n" if self.volume else ""
        elif args[:2] == ["volume", "inspect"]:
            result = json.dumps([self.volume])
        else:
            journal = json.loads((self.root / "ownership.json").read_text())
            assert journal["creationIntent"] and journal["pendingMutation"]
            self.events.append(tuple(args[:2]))
            if args[:2] == ["volume", "create"]:
                self.volume = {"Name": args[-1], "Driver": "local", "Scope": "local", "CreatedAt": "2026-10-30T10:00:00Z", "Mountpoint": "/owned/docker/volume", "Labels": {"dev.webdock.backend-package": self.nonce}, "Options": None}
            elif args[:2] == ["buildx", "create"]:
                name = args[args.index("--name") + 1]
                node = args[args.index("--node") + 1]
                self.builder = {"Name": name, "Driver": "docker-container", "Nodes": [{"Name": node, "Endpoint": args[-1]}]}
            elif args[:2] == ["buildx", "inspect"]:
                if self.fail == "unobserved-bootstrap":
                    raise subprocess.CalledProcessError(1, argv)
                name = "buildx_buildkit_" + self.builder["Nodes"][0]["Name"]
                self.container = {"Id": "a" * 64, "Name": "/" + name, "Image": "sha256:" + "b" * 64, "Config": {"Image": MODULE.BUILDKIT, "Env": ["WEBDOCK_BACKEND_PACKAGE_OWNER=" + self.nonce]}, "HostConfig": {"Memory": 8 * 1024**3, "MemorySwap": 8 * 1024**3, "CpuPeriod": 100000, "CpuQuota": 400000, "Privileged": True, "PidsLimit": 0}, "Mounts": [{"Type": "volume", "Name": self.volume["Name"], "Destination": "/var/lib/buildkit", "RW": True}]}
                if self.fail == "budget":
                    self.container["HostConfig"]["Memory"] = 1
            elif args[0] == "update":
                self.container["HostConfig"]["PidsLimit"] = 1024
            elif args[:2] == ["buildx", "rm"]:
                if self.fail != "remove-noop":
                    self.builder = None
                    if self.container:
                        self.container = None
                        self.volume = None
            elif args[:2] == ["container", "rm"]:
                self.container = None
            elif args[:2] == ["volume", "rm"]:
                self.volume = None
            else:
                raise AssertionError(args)
            if self.fail == tuple(args[:2]):
                raise subprocess.CalledProcessError(1, argv)
        return subprocess.CompletedProcess(argv, 0, result, "")


class Cleanup(unittest.TestCase):
    def fixture(self):
        self.assertTrue(hasattr(MODULE, "OwnedBuilder"), "partial builder creation needs durable ownership-aware cleanup")
        temporary = tempfile.TemporaryDirectory(prefix="backend-cleanup-test-", dir=ROOT / ".superpowers")
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        nonce = "1234567890abcdef"
        docker = Docker(root, nonce)
        owner = MODULE.OwnedBuilder(root, nonce, "c" * 40, runner=docker)
        return root, docker, owner

    def test_uncertain_create_and_bootstrap_remove_only_observed_owned_resources(self):
        for fault in [("volume", "create"), ("buildx", "create"), ("buildx", "inspect")]:
            with self.subTest(fault=fault):
                root, docker, owner = self.fixture()
                docker.fail = fault
                with self.assertRaises(subprocess.CalledProcessError):
                    owner.run(lambda: None)
                self.assertTrue(json.loads((root / "ownership.json").read_text())["cleanup"])
                self.assertIsNone(docker.builder)
                self.assertIsNone(docker.container)
                self.assertIsNone(docker.volume)

    def test_budget_assertion_captures_id_before_cleanup_and_preserves_failure(self):
        root, docker, owner = self.fixture()
        docker.fail = "budget"
        with self.assertRaises(MODULE.CleanupBlocked):
            owner.run(lambda: None)
        record = json.loads((root / "ownership.json").read_text())
        self.assertEqual(record["container"]["ID"], "a" * 64)
        self.assertTrue(record["cleanup"])

    def test_foreign_id_name_image_or_volume_replacement_is_never_removed(self):
        for field in ["id", "name", "image", "mount", "volume"]:
            with self.subTest(field=field):
                root, docker, owner = self.fixture()
                def changed():
                    if field == "id": docker.container["Id"] = "d" * 64
                    if field == "name": docker.container["Name"] = "/foreign"
                    if field == "image": docker.container["Image"] = "sha256:" + "d" * 64
                    if field == "mount": docker.container["Mounts"][0]["Name"] = "foreign"
                    if field == "volume": docker.volume["CreatedAt"] = "2026-10-30T11:00:00Z"
                    raise ValueError("original build failure")
                with self.assertRaisesRegex(ValueError, "original build failure"):
                    owner.run(changed)
                self.assertFalse(json.loads((root / "ownership.json").read_text())["cleanup"])
                self.assertFalse(any(action[1] == "rm" for action in docker.events))

    def test_daemon_unavailable_blocks_cleanup_instead_of_claiming_absence(self):
        root, docker, owner = self.fixture()
        def failed():
            docker.down = True
            raise ValueError("original build failure")
        with self.assertRaisesRegex(ValueError, "original build failure"):
            owner.run(failed)
        record = json.loads((root / "ownership.json").read_text())
        self.assertFalse(record["cleanup"])
        self.assertEqual(record["cleanupFailure"], "CalledProcessError")

    def test_unobserved_bootstrap_outcome_remains_blocked(self):
        root, docker, owner = self.fixture()
        docker.fail = "unobserved-bootstrap"
        with self.assertRaises(subprocess.CalledProcessError):
            owner.run(lambda: None)
        record = json.loads((root / "ownership.json").read_text())
        self.assertFalse(record["cleanup"])
        self.assertEqual(record["pendingMutation"], "bootstrap")
        self.assertFalse(any(action[1] == "rm" for action in docker.events))

    def test_successful_remove_response_without_actual_absence_is_not_cleanup(self):
        root, docker, owner = self.fixture()
        docker.fail = "remove-noop"
        with self.assertRaises(MODULE.CleanupBlocked):
            owner.run(lambda: None)
        self.assertFalse(json.loads((root / "ownership.json").read_text())["cleanup"])

    def test_foreign_builder_or_preexisting_volume_is_never_adopted(self):
        for field in ["builder", "volume"]:
            with self.subTest(field=field):
                root, docker, owner = self.fixture()
                if field == "builder":
                    docker.builder = {"Name": owner.builder, "Driver": "docker-container", "Nodes": [{"Name": owner.node, "Endpoint": "tcp://foreign:2375"}]}
                else:
                    docker.volume = {"Name": owner.volume, "Labels": {"foreign": "true"}}
                with self.assertRaises(MODULE.CleanupBlocked):
                    owner.run(lambda: None)
                self.assertFalse(docker.events)
                self.assertFalse(json.loads((root / "ownership.json").read_text())["cleanup"])

    def test_cleanup_failure_keeps_original_error_and_failed_evidence(self):
        root, docker, owner = self.fixture()
        def failed():
            docker.fail = ("buildx", "rm")
            raise ValueError("original build failure")
        with self.assertRaisesRegex(ValueError, "original build failure"):
            owner.run(failed)
        record = json.loads((root / "ownership.json").read_text())
        self.assertEqual(record["failure"], "ValueError")
        self.assertEqual(record["cleanupFailure"], "CalledProcessError")
        self.assertFalse(record["cleanup"])

    def test_normal_flow_confirms_all_absence_and_pins_environment_and_private_journal(self):
        with patch.dict(os.environ, {"DOCKER_HOST":"tcp://foreign:2375","DOCKER_CONTEXT":"foreign","DOCKER_TLS_VERIFY":"1","DOCKER_CONFIG":"/foreign","BUILDX_BUILDER":"foreign","BUILDX_CONFIG":"/foreign","BUILDKIT_HOST":"tcp://foreign:2375"}):
            root, docker, owner = self.fixture()
        owner.run(lambda: None)
        self.assertTrue(json.loads((root / "ownership.json").read_text())["cleanup"])
        self.assertEqual(stat.S_IMODE(root.stat().st_mode), 0o700)
        self.assertEqual(stat.S_IMODE((root / "ownership.json").stat().st_mode), 0o600)
        self.assertTrue(all(not key.startswith(("DOCKER_", "BUILDX_", "BUILDKIT_")) or key == "BUILDX_CONFIG" for key in docker.captured_environment))
        self.assertEqual(docker.captured_environment["BUILDX_CONFIG"], str(root / "buildx"))
        self.assertIsNone(docker.builder)
        self.assertIsNone(docker.container)
        self.assertIsNone(docker.volume)


if __name__ == "__main__":
    unittest.main()
