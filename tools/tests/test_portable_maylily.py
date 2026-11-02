"""Repository-contained Cargo inputs and the exact reviewed Maylily snapshot."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[2]
VENDOR = ROOT / "vendor/maylily"


def repository_files():
    return subprocess.check_output(["git", "ls-files", "-co", "--exclude-standard", "-z"], cwd=ROOT).decode().split("\0")[:-1]


class PortableMaylily(unittest.TestCase):
    def test_cargo_paths_are_relative_and_remain_inside_repository(self):
        def check(value, manifest):
            if isinstance(value, dict):
                for key, child in value.items():
                    if key == "path" and isinstance(child, str):
                        self.assertFalse(Path(child).is_absolute(), f"{manifest}: absolute checkout dependency {child}")
                        self.assertTrue((ROOT / manifest).parent.joinpath(child).resolve().is_relative_to(ROOT), f"{manifest}: escaping checkout path")
                    check(child, manifest)
            elif isinstance(value, list):
                for child in value:
                    check(child, manifest)
        for name in repository_files():
            if Path(name).name == "Cargo.toml":
                check(tomllib.loads((ROOT / name).read_text()), name)

    def test_vendor_bytes_match_explicit_uncommitted_snapshot_provenance(self):
        provenance = json.loads((VENDOR / "UPSTREAM.json").read_text())
        self.assertEqual(provenance["snapshotState"], "uncommitted-local-library")
        self.assertEqual(provenance["baseCommit"], "ad61b82beedda46cf7006c331a3dac84499b53ff")
        self.assertFalse(provenance["baseCommitContainsLibrary"])
        files = provenance["filesSHA256"]
        self.assertEqual(set(files), {str(path.relative_to(VENDOR)) for path in VENDOR.rglob("*") if path.is_file() and path.name != "UPSTREAM.json"})
        for name, expected in files.items():
            self.assertFalse((VENDOR / name).is_symlink())
            self.assertEqual(hashlib.sha256((VENDOR / name).read_bytes()).hexdigest(), expected, name)
        self.assertEqual(hashlib.sha256(json.dumps(files, sort_keys=True, separators=(",", ":")).encode()).hexdigest(), provenance["snapshotSHA256"])
        self.assertIn("Copyright (c) 2026 NewtTheWolf", (VENDOR / "LICENSE").read_text())
        dependency = tomllib.loads((ROOT / "crates/ids/Cargo.toml").read_text())["dependencies"]["maylily"]
        self.assertEqual(dependency, {"path": "../../vendor/maylily", "features": ["parse"]})

    def test_locked_ids_build_in_clean_repository_packaging_context(self):
        scratch = ROOT / ".superpowers"
        scratch.mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(prefix="portable-maylily-", dir=scratch) as temporary:
            context = Path(temporary)
            for name in repository_files():
                if name not in {"Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "clippy.toml"} and not name.startswith(("crates/", "plugins/", "vendor/", "proto/", "legacy/control-plane/drizzle/")):
                    continue
                source = ROOT / name
                target = context / name
                target.parent.mkdir(parents=True, exist_ok=True)
                if source.is_symlink():
                    target.symlink_to(os.readlink(source))
                else:
                    shutil.copyfile(source, target)
            environment = {**os.environ, "CARGO_TARGET_DIR": str(ROOT / "target")}
            metadata = subprocess.run(["cargo", "metadata", "--locked", "--offline", "--format-version=1"], cwd=context, env=environment, capture_output=True, text=True, timeout=90)
            self.assertEqual(metadata.returncode, 0, metadata.stderr[-4000:])
            for package in json.loads(metadata.stdout)["packages"]:
                if package["source"] is None:
                    self.assertTrue(Path(package["manifest_path"]).is_relative_to(context), package["name"])
            build = subprocess.run(["cargo", "build", "--locked", "--offline", "-p", "webdock-ids"], cwd=context, env=environment, capture_output=True, text=True, timeout=180)
            self.assertEqual(build.returncode, 0, build.stderr[-4000:])


if __name__ == "__main__":
    unittest.main()
