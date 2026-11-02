"""Offline digest-chain checks; fixture bytes come from official GHCR, not image pulls."""
import hashlib
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = Path(__file__).parent / "fixtures" / "railpack-base-pins"


def digest(data):
    return "sha256:" + hashlib.sha256(data).hexdigest()


class BuilderBasePins(unittest.TestCase):
    def test_operator_bases_match_amd64_registry_manifest_and_config(self):
        tools = json.loads((ROOT / "tools.json").read_bytes())
        for image in ("builder", "runtime"):
            with self.subTest(image=image):
                reference = tools["railpack"].get(image + "Image", "")
                prefix = f"ghcr.io/railwayapp/railpack-{image}:mise-2026.9.15@"
                self.assertTrue(reference.startswith(prefix), reference)
                manifest_bytes = (FIXTURES / f"{image}-manifest.json").read_bytes()
                self.assertEqual(reference.removeprefix(prefix), digest(manifest_bytes))
                index = json.loads((FIXTURES / f"{image}-index.json").read_bytes())
                matches = [m for m in index["manifests"]
                           if m.get("platform") == {"os": "linux", "architecture": "amd64"}]
                self.assertEqual(len(matches), 1)
                self.assertEqual(matches[0]["digest"], digest(manifest_bytes))
                self.assertEqual(matches[0]["size"], len(manifest_bytes))
                manifest = json.loads(manifest_bytes)
                config_bytes = (FIXTURES / f"{image}-config.json").read_bytes()
                self.assertEqual(manifest["config"]["digest"], digest(config_bytes))
                self.assertEqual(manifest["config"]["size"], len(config_bytes))
                config = json.loads(config_bytes)
                self.assertEqual((config["os"], config["architecture"]), ("linux", "amd64"))

    def test_evidence_hashes_all_original_registry_responses(self):
        evidence = json.loads((ROOT.parents[1] / "docs/verification/2026-10-30-build-builder-profile.json").read_bytes())
        for proof in evidence["registryProofs"]:
            for response in proof["responses"]:
                raw = (FIXTURES / response["file"]).read_bytes()
                self.assertEqual(response["digest"], digest(raw))
                self.assertEqual(response["bytes"], len(raw))
                if response["kind"] != "config":
                    self.assertEqual(response["dockerContentDigest"], digest(raw))


if __name__ == "__main__":
    unittest.main()
