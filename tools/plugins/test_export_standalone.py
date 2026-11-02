"""Run with python3 -m unittest discover -s tools/plugins -p 'test_export*.py'."""
import json
import fnmatch
from pathlib import Path
import subprocess
import tempfile
import tomllib
import unittest


SCRIPT = Path(__file__).with_name("export_standalone.py")


class ExportTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "source"
        self.root.mkdir()
        self.put("Cargo.toml", '[workspace]\nmembers=["crates/*"]\n[workspace.package]\nversion="1.0.0"\nedition="2024"\nlicense="AGPL-3.0-only"\n[workspace.lints.rust]\nunsafe_code="deny"\n')
        self.put("LICENSE", "license fixture\n")
        self.put("NOTICE", "notice fixture\n")
        self.put("COMMERCIAL-LICENSING.md", "commercial licensing fixture\n")
        self.put("rust-toolchain.toml", '[toolchain]\nchannel="1.98.1"\nprofile="minimal"\n')
        self.put("tools/plugin-release/build_oci.py", "# reviewed rootless builder fixture\n")
        self.put("crates/wire/Cargo.toml", '[package]\nname="wire"\nversion.workspace=true\nedition.workspace=true\n[lints]\nworkspace=true\n')
        self.put("crates/wire/src/lib.rs", "pub fn version() -> u32 { 1 }\n")
        self.put("crates/kit/Cargo.toml", '[package]\nname="kit"\nversion.workspace=true\nedition.workspace=true\n[dependencies]\nwire={path="../wire",optional=true}\n[features]\nwire=["dep:wire"]\n')
        self.put("crates/kit/src/lib.rs", "pub fn kit() {}\n")
        self.put("crates/kit/build.rs", 'fn main() { assert!(std::fs::read_to_string("../../proto/webdock/plugin/v1/plugin.proto").unwrap().contains("fixture wire")); }\n')
        self.put("proto/webdock/plugin/v1/plugin.proto", "// fixture wire\n")
        self.put("plugins/demo/Cargo.toml", '[package]\nname="demo"\nversion="1.0.0"\nedition="2024"\n[workspace]\n[dependencies]\nkit={path="../../crates/kit",features=["wire"]}\n[target.\'cfg(unix)\'.dev-dependencies]\nwire={path="../../crates/wire"}\n')
        self.put("plugins/demo/src/main.rs", "fn main() {}\n")

    def put(self, name, content):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)

    def run_export(self, mode, output=None, *extra, success=True):
        args = ["python3", str(SCRIPT), mode, "--source", str(self.root)]
        if output:
            args += ["--output", str(output)]
        result = subprocess.run(args + list(extra), capture_output=True, text=True)
        if success:
            self.assertEqual(result.returncode, 0, result.stderr)
        else:
            self.assertNotEqual(result.returncode, 0, result.stdout)
            self.assertIn("export refused:", result.stderr)
        return result

    def test_inventory_finds_optional_target_and_transitive_paths(self):
        result = self.run_export("inventory")
        inventory = json.loads(result.stdout)
        self.assertEqual(inventory["sdkCrates"], ["crates/kit", "crates/wire"])
        self.assertEqual(inventory["plugins"][0]["name"], "demo")

    def test_sdk_is_resolvable_without_source_and_preserves_notices(self):
        out = Path(self.temp.name) / "sdk"
        self.run_export("sdk", out)
        self.assertEqual((out / "NOTICE").read_text(), "notice fixture\n")
        self.assertEqual((out / "rust-toolchain.toml").read_text(), '[toolchain]\nchannel="1.98.1"\nprofile="minimal"\n')
        self.root.rename(self.root.with_name("unavailable-source"))
        metadata = subprocess.run(["cargo", "metadata", "--manifest-path", str(out / "Cargo.toml"), "--no-deps", "--offline", "--format-version", "1"], capture_output=True, text=True)
        self.assertEqual(metadata.returncode, 0, metadata.stderr)
        self.assertEqual({p["name"] for p in json.loads(metadata.stdout)["packages"]}, {"wire", "kit"})
        build = subprocess.run(["cargo", "check", "--manifest-path", str(out / "Cargo.toml"), "--workspace", "--offline"], capture_output=True, text=True)
        self.assertEqual(build.returncode, 0, build.stderr)

    def test_plugin_uses_pinned_sdk_and_keeps_dependency_options(self):
        out = Path(self.temp.name) / "plugins"
        digest = "a" * 64
        self.put("tools/plugin-registry/manifest-validator/Cargo.toml", '[package]\nname="manifest-validator"\nversion="1.0.0"\nedition="2024"\n[workspace]\n[dependencies]\nwire={path="../../../crates/wire"}\n')
        self.put("tools/plugin-registry/manifest-validator/src/main.rs", "fn main() {}")
        self.put("plugins/demo/tests/fixtures/contract.json", '{"fixture":true}\n')
        self.put("plugins/demo/assets/icon.svg", '<svg xmlns="http://www.w3.org/2000/svg"/>\n')
        self.put("plugins/demo/screenshots/example.png", "PNG fixture")
        self.put("plugins/demo/presentation.json", '{"icon":"assets/icon.svg","readme":"README.md"}\n')
        self.run_export("plugins", out, "--sdk-git", "https://github.com/Spitzli/plugin-sdk.git", "--sdk-rev", "b" * 40, "--builder-image", f"rust:1.98.1-slim-trixie@sha256:{digest}", "--runtime-image", f"debian:trixie-slim@sha256:{digest}")
        manifest = tomllib.loads((out / "demo/Cargo.toml").read_text())
        dependency = manifest["dependencies"]["kit"]
        self.assertEqual(dependency["rev"], "b" * 40)
        self.assertEqual(dependency["features"], ["wire"])
        self.assertNotIn("path", dependency)
        self.assertEqual(manifest["target"]["cfg(unix)"]["dev-dependencies"]["wire"]["git"], "https://github.com/Spitzli/plugin-sdk.git")
        dockerfile = (out / "demo/Dockerfile").read_text()
        self.assertIn("USER 65532:65532", dockerfile)
        self.assertNotIn("plugins/demo/", dockerfile)
        self.assertIn("--locked", dockerfile)
        self.assertIn("AS check", dockerfile)
        self.assertIn("sharing=locked", dockerfile)
        self.assertIn("id=spitzli-native-sdk-" + "b" * 40, dockerfile)
        self.assertIn("/out/demo /usr/local/bin/demo", dockerfile)
        self.assertIn("COPY LICENSE COMMERCIAL-LICENSING.md NOTICE /usr/share/licenses/spitzli-cloud/", dockerfile.rsplit("\nFROM ", 1)[1])
        self.assertIn("--jobs 3", dockerfile)
        self.assertFalse((out / "demo/Cargo.lock").exists())
        validator = tomllib.loads((out / "demo/tools/manifest-validator/Cargo.toml").read_text())
        self.assertEqual(validator["dependencies"]["wire"]["rev"], "b" * 40)
        self.assertNotIn("path", validator["dependencies"]["wire"])
        self.assertTrue((out / "demo/rust-toolchain.toml").is_file())
        self.assertTrue((out / "demo/.github/workflows/native-oci.yml").is_file())
        for path in ["assets/icon.svg", "screenshots/example.png", "presentation.json"]:
            self.assertEqual((out / "demo" / path).read_bytes(), (self.root / "plugins/demo" / path).read_bytes())
        fixture_path = "tests/fixtures/contract.json"
        self.assertEqual((out / "demo" / fixture_path).read_text(), '{"fixture":true}\n')
        patterns = (out / "demo/.dockerignore").read_text().splitlines()
        for pattern in patterns:
            for prefix in ("tests", "tests/fixtures", fixture_path, "fixtures"):
                self.assertFalse(fnmatch.fnmatchcase(prefix, pattern), f"reviewed test input excluded: {pattern}")
        policy = json.loads((out / "demo/standalone.json").read_text())
        self.assertEqual(policy["release"]["mode"], "oci-only")

    def test_library_only_plugin_cannot_claim_default_native_binary(self):
        (self.root / "plugins/demo/src/main.rs").unlink()
        self.put("plugins/demo/src/lib.rs", "pub fn library() {}")
        args = self.plugin_args()
        result = self.run_export("plugins", Path(self.temp.name) / "plugins", *args, success=False)
        self.assertIn("no native executable", result.stderr)

    def plugin_args(self):
        return ["--sdk-git", "https://github.com/Spitzli/plugin-sdk.git", "--sdk-rev", "b" * 40, "--builder-image", "rust:1.98.1-slim-trixie@sha256:" + "a" * 64, "--runtime-image", "debian:trixie-slim@sha256:" + "a" * 64]

    def test_binary_discovery_honors_autobins_and_explicit_default_run(self):
        self.put("plugins/demo/Cargo.toml", '[package]\nname="demo"\nversion="1.0.0"\nedition="2024"\nautobins=false\ndefault-run="server"\n[workspace]\n[[bin]]\nname="server"\npath="src/server.rs"\n[dependencies]\nkit={path="../../crates/kit"}\n')
        self.put("plugins/demo/src/server.rs", "fn main() {}")
        self.put("plugins/demo/src/bin/unused.rs", "fn main() {}")
        out = Path(self.temp.name) / "plugins"
        self.run_export("plugins", out, *self.plugin_args())
        dockerfile = (out / "demo/Dockerfile").read_text()
        self.assertIn('ENTRYPOINT ["/usr/local/bin/server"]', dockerfile)
        self.assertNotIn("/release/demo", dockerfile)
        self.assertNotIn("/release/unused", dockerfile)

    def test_specialized_release_preserves_protocol_and_binds_runtime_version(self):
        original = self.root / "plugins/demo"
        original.rename(self.root / "plugins/github-repository")
        self.put("plugins/github-repository/src/main.rs", 'fn main() {\n    let descriptor = verify_descriptor(&config.descriptor, &publisher)?;\n}\n')
        self.put("plugins/github-repository/tests/provider.rs", 'version: "1.0.0".into(),\n')
        cargo = self.root / "plugins/github-repository/Cargo.toml"
        cargo.write_text(cargo.read_text() + '\n[features]\nlocal-provider-test=[]\n')
        out = Path(self.temp.name) / "plugins"
        self.run_export("plugins", out, *self.plugin_args())
        result = json.loads((out / "github-repository/standalone.json").read_text())
        self.assertEqual(result["release"]["protocol"], "webdock.repository.v1")
        self.assertEqual(result["release"]["mode"], "oci-only")
        self.assertFalse((out / "github-repository/manifest.json").exists())
        self.assertIn('descriptor.version != env!("CARGO_PKG_VERSION")', (out / "github-repository/src/main.rs").read_text())
        self.assertIn('version: env!("CARGO_PKG_VERSION").into()', (out / "github-repository/tests/provider.rs").read_text())
        dockerfile = (out / "github-repository/Dockerfile").read_text()
        self.assertIn("--features local-provider-test", dockerfile.split("FROM source AS build")[0])
        self.assertNotIn("--features", dockerfile.split("FROM source AS build")[1])

    def test_native_authoring_manifest_synchronizes_cargo_version(self):
        self.put("plugins/demo/manifest.json", json.dumps({"kind": "extension", "name": "demo", "version": "2.0.0", "protocolVersion": 2, "sdkVersion": 2, "dataProtocolVersion": 2}))
        self.put("tools/plugin-release/plugin.yml", "name: Native plugin\n")
        self.put("tools/plugin-release/release.mjs", "// reviewed native release helper\n")
        out = Path(self.temp.name) / "plugins"
        self.run_export("plugins", out, *self.plugin_args())
        exported = out / "demo"
        self.assertEqual(tomllib.loads((exported / "Cargo.toml").read_text())["package"]["version"], "2.0.0")
        self.assertEqual(json.loads((exported / "standalone.json").read_text())["release"]["mode"], "native-extension")
        self.assertTrue((exported / ".github/workflows/plugin.yml").is_file())
        self.assertFalse((exported / ".github/workflows/native-oci.yml").exists())

    def test_skips_environment_keys_and_build_outputs(self):
        for name in [".env.local", "src/credential.pem", "target/leak.rs", ".git/config"]:
            self.put("crates/kit/" + name, "private fixture")
        out = Path(self.temp.name) / "sdk"
        self.run_export("sdk", out)
        for name in [".env.local", "src/credential.pem", "target/leak.rs", ".git/config"]:
            self.assertFalse((out / "crates/kit" / name).exists(), name)

    def test_rejects_private_key_embedded_in_source_without_partial_export(self):
        self.put("crates/kit/src/leak.rs", 'const KEY: &str = "-----BEGIN PRIVATE KEY-----";')
        out = Path(self.temp.name) / "sdk"
        self.run_export("sdk", out, success=False)
        self.assertFalse(out.exists())

    def test_rejects_dependency_escape(self):
        self.put("crates/kit/Cargo.toml", '[package]\nname="kit"\nversion="1.0.0"\n[dependencies]\nexternal={path="../../../outside"}\n')
        self.run_export("inventory", success=False)

    def test_rejects_symlink_and_existing_output(self):
        (self.root / "crates/kit/src/leak.rs").symlink_to("/etc/passwd")
        out = Path(self.temp.name) / "sdk"
        self.run_export("sdk", out, success=False)
        self.assertFalse(out.exists())
        out.mkdir()
        (out / "keep").write_text("user data")
        self.run_export("sdk", out, success=False)
        self.assertEqual((out / "keep").read_text(), "user data")

    def test_rejects_mutable_sdk_revision_and_image(self):
        out = Path(self.temp.name) / "plugins"
        valid = {"--sdk-git": "https://github.com/Spitzli/plugin-sdk.git", "--sdk-rev": "b" * 40, "--builder-image": "rust:1.98.1@sha256:" + "a" * 64, "--runtime-image": "debian:trixie-slim@sha256:" + "a" * 64}
        for flag, bad_value in [("--sdk-rev", "main"), ("--builder-image", "rust:latest"), ("--runtime-image", "debian:latest"), ("--sdk-git", "https://token@github.com/Spitzli/sdk.git"), ("--sdk-git", "https://github.com/Spitzli/sdk.git?token=fixture")]:
            with self.subTest(flag=flag, value=bad_value):
                options = {**valid, flag: bad_value}
                self.run_export("plugins", out, *[v for pair in options.items() for v in pair], success=False)
                self.assertFalse(out.exists())

    def test_export_bytes_are_reproducible_and_source_is_unchanged(self):
        before = {str(p.relative_to(self.root)): p.read_bytes() for p in self.root.rglob("*") if p.is_file()}
        first, second = Path(self.temp.name) / "first", Path(self.temp.name) / "second"
        self.run_export("sdk", first)
        self.run_export("sdk", second)
        def files(root):
            return {str(p.relative_to(root)): p.read_bytes() for p in root.rglob("*") if p.is_file()}
        self.assertEqual(files(first), files(second))
        self.assertEqual(before, files(self.root))

    def test_refuses_to_export_core_in_runtime_sdk(self):
        self.put("crates/core/Cargo.toml", '[package]\nname="core-domain"\nversion="1.0.0"\n')
        self.put("crates/core/src/lib.rs", "pub fn core() {}")
        self.put("crates/wire/Cargo.toml", '[package]\nname="wire"\nversion="1.0.0"\n[dependencies]\ncore-domain={path="../core"}\n')
        plan = json.loads(self.run_export("inventory").stdout)
        self.assertEqual(plan["forbiddenSdkCrates"], ["crates/core"])
        self.run_export("sdk", Path(self.temp.name) / "sdk", success=False)


if __name__ == "__main__":
    unittest.main()
