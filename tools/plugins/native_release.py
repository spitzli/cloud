#!/usr/bin/env python3
"""Validate OCI-only native releases; never emits an extension or signed descriptor."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import sys
import tomllib


def require(condition, message):
    if not condition:
        raise ValueError(message)


def check(root: Path):
    metadata = json.loads((root / "standalone.json").read_text())
    manifest = tomllib.loads((root / "Cargo.toml").read_text())
    release = metadata["release"]
    require(release["mode"] == "oci-only", "OCI-only workflow cannot release an extension manifest")
    require(not (root / "manifest.json").exists(), "Root manifest requires the native extension validation/release lane")
    require(release["version"] == manifest["package"]["version"], "Cargo/release version mismatch")
    require(re.fullmatch(r"\d+\.\d+\.\d+(?:-[A-Za-z0-9.-]+)?(?:\+[A-Za-z0-9.-]+)?", release["version"]), "Invalid release version")
    require((root / "Cargo.lock").is_file(), "Reviewed Cargo.lock is required")
    require(re.fullmatch(r"[0-9a-f]{40}", metadata["sdkRevision"]), "Pinned SDK revision required")
    def dependencies(document):
        for kind in ("dependencies", "dev-dependencies", "build-dependencies"):
            yield from document.get(kind, {}).items()
        for platform in document.get("target", {}).values():
            yield from dependencies(platform)
    for name, options in dependencies(manifest):
        package = options.get("package", name) if isinstance(options, dict) else name
        if package.startswith(("spitzli-", "webdock-")):
            require(isinstance(options, dict) and options.get("git") == metadata["sdkGit"] and options.get("rev") == metadata["sdkRevision"] and "path" not in options and not options.get("workspace"), "Every SDK dependency must match the reviewed repository and revision")
    toolchain = tomllib.loads((root / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    require(re.fullmatch(r"\d+\.\d+\.\d+", toolchain), "Pinned Rust toolchain required")
    stages = set()
    for line in (root / "Dockerfile").read_text().splitlines():
        if not re.match(r"^\s*FROM\s", line, re.I):
            continue
        match = re.fullmatch(r"\s*FROM\s+(\S+)(?:\s+AS\s+(\S+))?\s*", line, re.I)
        require(match, "Unsupported Docker FROM")
        image, stage = match.groups()
        require(image in stages or re.fullmatch(r"[A-Za-z0-9._:/-]+@sha256:[0-9a-f]{64}", image), "Immutable Docker base required")
        require(not re.search(r"([0-9a-f])\1{63}$", image) and ":latest@" not in image, "Placeholder or latest Docker base forbidden")
        if stage:
            stages.add(stage)
    require(stages, "Docker build stage required")
    require(metadata["entrypoint"] in metadata["binaries"], "Declared native entrypoint required")
    return metadata


def finalize(root: Path, image: str, tag: str, source_sha: str, repository: str):
    metadata = check(root)
    release = metadata["release"]
    require(tag == "v" + release["version"], "Tag must match compiled release version")
    require(re.fullmatch(r"[0-9a-f]{40}", source_sha) and source_sha != "0" * 40, "Exact source commit required")
    require(re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository), "Exact GitHub repository required")
    require(re.fullmatch(r"ghcr\.io/" + re.escape(repository.lower()) + r"@sha256:[0-9a-f]{64}", image), "Pushed immutable image must belong to this repository")
    require(not re.search(r"([0-9a-f])\1{63}$", image), "Placeholder image digest forbidden")
    return {
        "schemaVersion": 1,
        "kind": "webdock-native-oci-release",
        "plugin": metadata["name"],
        "package": metadata["package"],
        "version": release["version"],
        "protocol": release["protocol"],
        "image": image,
        "sourceRepository": "https://github.com/" + repository,
        "sourceCommit": source_sha,
        "sdkGit": metadata["sdkGit"],
        "sdkRevision": metadata["sdkRevision"],
        "entrypoint": metadata["entrypoint"],
        "binaries": metadata["binaries"],
        "runtimeBinding": release["runtimeBinding"],
        "registryAdmission": release["registryAdmission"],
        "connectionsOwner": "Core",
        "sourceContractSha256": hashlib.sha256((root / "standalone.json").read_bytes()).hexdigest(),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("check", "finalize"))
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--image")
    parser.add_argument("--tag")
    parser.add_argument("--source-sha")
    parser.add_argument("--repository")
    parser.add_argument("--output", type=Path, default=Path("release/native-release.json"))
    args = parser.parse_args()
    if args.command == "check":
        check(args.root)
        print("Native OCI package and release contract valid; registry admission is separate")
    else:
        result = finalize(args.root, args.image or "", args.tag or "", args.source_sha or "", args.repository or "")
        args.output.parent.mkdir(parents=True, exist_ok=True)
        with args.output.open("x") as stream:
            stream.write(json.dumps(result, indent=2, sort_keys=True) + "\n")
        print("Native OCI release metadata written; no extension or signed descriptor was emitted")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"native release refused: {error}", file=sys.stderr)
        sys.exit(1)
