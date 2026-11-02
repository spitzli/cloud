import importlib.util
import json
from pathlib import Path
import tempfile
import unittest


SPEC = importlib.util.spec_from_file_location("native_release", Path(__file__).with_name("native_release.py"))
RELEASE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RELEASE)
DIGEST = "1234567890abcdef" * 4


class NativeReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        metadata = {"name": "github-repository", "package": "webdock-github-repository", "sdkGit": "https://github.com/spitzli/cloud-plugin-sdk", "sdkRevision": "b" * 40, "entrypoint": "webdock-github-repository", "binaries": ["webdock-github-repository"], "release": {"mode": "oci-only", "version": "0.1.0", "protocol": "webdock.repository.v1", "runtimeBinding": "publisher-signed repository descriptor", "registryAdmission": "not-performed-specialized-protocol"}}
        (self.root / "standalone.json").write_text(json.dumps(metadata))
        (self.root / "Cargo.toml").write_text('[package]\nname="webdock-github-repository"\nversion="0.1.0"\n[dependencies]\nwebdock-plugin-runtime={git="https://github.com/spitzli/cloud-plugin-sdk",rev="' + "b" * 40 + '"}\n')
        (self.root / "Cargo.lock").write_text("# reviewed test lock\n")
        (self.root / "rust-toolchain.toml").write_text('[toolchain]\nchannel="1.98.1"\n')
        (self.root / "Dockerfile").write_text(f"FROM rust:1.98.1@sha256:{DIGEST} AS build\nFROM debian:trixie-slim@sha256:{DIGEST}\n")

    def finalize(self, **changes):
        values = {"image": "ghcr.io/spitzli/github-repository@sha256:" + DIGEST, "tag": "v0.1.0", "source_sha": "c" * 40, "repository": "Spitzli/github-repository", **changes}
        return RELEASE.finalize(self.root, **values)

    def test_final_metadata_binds_actual_image_source_version_without_forging_descriptor(self):
        metadata = self.finalize()
        self.assertEqual(metadata["kind"], "webdock-native-oci-release")
        self.assertEqual(metadata["protocol"], "webdock.repository.v1")
        self.assertEqual(metadata["registryAdmission"], "not-performed-specialized-protocol")
        self.assertEqual(metadata["version"], "0.1.0")
        self.assertEqual(metadata["sourceCommit"], "c" * 40)
        self.assertNotIn("signature", metadata)
        self.assertNotIn("manifestSha256", metadata)

    def test_rejects_version_image_tag_and_repository_substitution(self):
        for change in [{"tag": "v1.0.0"}, {"image": "ghcr.io/other/plugin@sha256:" + DIGEST}, {"image": "ghcr.io/spitzli/github-repository:latest"}, {"image": "ghcr.io/spitzli/github-repository@sha256:" + "0" * 64}, {"source_sha": "main"}, {"repository": "owner/repo/extra"}]:
            with self.subTest(change=change), self.assertRaises(ValueError):
                self.finalize(**change)

    def test_refuses_extension_lane_and_missing_lock(self):
        (self.root / "manifest.json").write_text('{}')
        with self.assertRaises(ValueError):
            self.finalize()
        (self.root / "manifest.json").unlink()
        (self.root / "Cargo.lock").unlink()
        with self.assertRaises(ValueError):
            self.finalize()

    def test_refuses_mutated_sdk_reference_and_toolchain(self):
        manifest = self.root / "Cargo.toml"
        manifest.write_text(manifest.read_text().replace("b" * 40, "d" * 40))
        with self.assertRaises(ValueError):
            self.finalize()
        manifest.write_text(manifest.read_text().replace("d" * 40, "b" * 40))
        (self.root / "rust-toolchain.toml").write_text('[toolchain]\nchannel="stable"\n')
        with self.assertRaises(ValueError):
            self.finalize()


if __name__ == "__main__":
    unittest.main()
