#!/usr/bin/env python3
"""Assert the trusted overlayfs scratch-probe OCI export; not a tenant OCI parser."""
import hashlib
import io
import json
from pathlib import Path
import sys
import tarfile

raw = Path(sys.argv[1]).read_bytes()
assert len(raw) <= 16 * 1024 * 1024
with tarfile.open(fileobj=io.BytesIO(raw)) as outer:
    assert json.load(outer.extractfile("oci-layout"))["imageLayoutVersion"] == "1.0.0"
    index = json.load(outer.extractfile("index.json"))

    def blob(descriptor):
        assert not descriptor.get("urls")
        expected = descriptor["digest"]
        assert expected.startswith("sha256:")
        data = outer.extractfile("blobs/sha256/" + expected.removeprefix("sha256:")).read()
        assert len(data) == descriptor["size"]
        assert "sha256:" + hashlib.sha256(data).hexdigest() == expected
        return data

    manifest = json.loads(blob(index["manifests"][0]))
    config = json.loads(blob(manifest["config"]))
    assert (config["os"], config["architecture"]) == ("linux", "amd64")
    proof = None
    removed = False
    whiteout = False
    for descriptor in manifest["layers"]:
        with tarfile.open(fileobj=io.BytesIO(blob(descriptor))) as layer:
            for member in layer:
                name = member.name.removeprefix("./")
                assert not name.startswith("/") and ".." not in name.split("/")
                if name == "proof":
                    assert member.isfile()
                    proof = layer.extractfile(member).read()
                if name == "remove":
                    assert member.isfile() and layer.extractfile(member).read() == b"seed"
                    removed = False
                if name == ".wh.remove":
                    assert member.size == 0
                    removed = True
                    whiteout = True
    assert proof == b"webdock-overlayfs-buildkit-proof"
    assert removed and whiteout
    print(json.dumps({"artifactDigest": "sha256:" + hashlib.sha256(raw).hexdigest(),
                      "artifactBytes": len(raw), "manifestDigest": index["manifests"][0]["digest"],
                      "proofVerified": True, "seedDeletedWithWhiteout": True,
                      "descriptorHashesAndSizesVerified": True}, indent=2))
