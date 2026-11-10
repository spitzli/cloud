#!/usr/bin/env python3
"""Export reviewed source trees; never invokes Git, publishes, or reads credentials."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import sys
import tempfile
import tomllib
from urllib.parse import urlsplit


DEPENDENCIES = {"dependencies", "build-dependencies", "dev-dependencies"}
CORE_CRATES = {"core", "control-plane", "provider-connections", "identity", "persistence", "browser-session", "oauth", "access", "registry", "plugin-host", "hosting", "databases", "build", "agent", "agent-control", "secrets", "ids", "supervisor"}
ARCHIVES_CORE_TESTS = {"core.rs", "owned.rs", "readonly.rs", "reports.rs", "repository.rs", "connected_export_compile.rs"}
SOURCE_DIRECTORIES = {"src", "tests", "proto", "migrations", "ui", "assets", "screenshots"}
EXCLUDED_DIRECTORIES = {"target", "node_modules", "__pycache__", ".git", ".runtime-test", ".cache", "private", "secrets"}
SECRET_SUFFIXES = {".pem", ".key", ".p12", ".pfx", ".crt", ".sqlite", ".db"}
SOURCE_SUFFIXES = {".rs", ".toml", ".json", ".sql", ".proto", ".md", ".txt", ".tsx", ".ts", ".css", ".html", ".sh", ".py", ".eml", ".csv", ".svg", ".png"}
SECRET = re.compile(rb"-----BEGIN (?:RSA |EC |OPENSSH |ENCRYPTED )?PRIVATE KEY-----|github_pat_[A-Za-z0-9_]{20,}|gh[pousr]_[A-Za-z0-9]{30,}|AKIA[A-Z0-9]{16}")


def read_manifest(directory: Path) -> dict:
    return tomllib.loads((directory / "Cargo.toml").read_text())


def dependency_tables(manifest: dict):
    for kind in DEPENDENCIES:
        if kind in manifest:
            yield kind, manifest[kind]
    for platform, target in manifest.get("target", {}).items():
        for kind in DEPENDENCIES:
            if kind in target:
                yield f"target.{platform}.{kind}", target[kind]


def path_dependencies(source: Path, directory: Path, manifest: dict, dev: bool = True):
    workspace = read_manifest(source).get("workspace", {}).get("dependencies", {})
    for scope, table in dependency_tables(manifest):
        if not dev and scope.endswith("dev-dependencies"):
            continue
        for name, options in table.items():
            if not isinstance(options, dict):
                continue
            spec = workspace.get(name, {}) if options.get("workspace") else options
            if isinstance(spec, dict) and "path" in spec:
                base = source if options.get("workspace") else directory
                target = (base / spec["path"]).resolve()
                if not target.is_relative_to(source):
                    raise ValueError(f"dependency escapes source: {directory.relative_to(source)}: {name}")
                if not (target / "Cargo.toml").is_file():
                    raise ValueError(f"missing path dependency: {target.relative_to(source)}")
                yield scope, name, target


def closure(source: Path, roots: list[Path], dev: bool = False) -> set[Path]:
    pending, found = list(roots), set()
    while pending:
        directory = pending.pop()
        if directory in found:
            continue
        found.add(directory)
        pending.extend(target for _, _, target in path_dependencies(source, directory, read_manifest(directory), dev))
    return found


def inventory(source: Path) -> dict:
    roots = sorted(p.parent for p in source.glob("plugins/*/Cargo.toml"))
    if not roots:
        raise ValueError("no plugins/*/Cargo.toml found")
    runtime = closure(source, roots) - set(roots)
    # All SDK dev-dependencies must also be independently resolvable.
    sdk = closure(source, sorted(runtime), dev=True)
    blocked = sorted(str(p.relative_to(source)) for p in sdk if p.parent != source / "crates" or p.name in CORE_CRATES)
    plugins = []
    for directory in roots:
        manifest = read_manifest(directory)
        withheld = []
        for scope, name, target in path_dependencies(source, directory, manifest):
            if target not in sdk:
                if directory.name != "archives" or not scope.endswith("dev-dependencies"):
                    raise ValueError(f"plugin dependency outside SDK closure: {directory.name}: {name}")
                withheld.append(name)
        plugins.append({"name": directory.name, "package": manifest["package"]["name"], "source": str(directory.relative_to(source)), "sdkDependencies": sorted({name for _, name, target in path_dependencies(source, directory, manifest) if target in sdk}), "coreIntegrationDependencies": sorted(withheld), "coreIntegrationTests": sorted(ARCHIVES_CORE_TESTS) if withheld else []})
    return {"formatVersion": 1, "plugins": plugins, "sdkCrates": sorted(str(p.relative_to(source)) for p in sdk), "forbiddenSdkCrates": blocked, "allDependencyClosure": sorted(str(p.relative_to(source)) for p in closure(source, roots, dev=True) - set(roots)), "lockfilesRequireRegeneration": True, "connectionsOwner": "Core"}


def inline(value):
    if isinstance(value, dict):
        return "{ " + ", ".join(f"{json.dumps(k)} = {inline(v)}" for k, v in value.items()) + " }"
    if isinstance(value, list):
        return "[" + ", ".join(inline(v) for v in value) + "]"
    return json.dumps(value, ensure_ascii=False)


def dump_toml(document: dict) -> str:
    lines = []

    def table(values, path=()):
        for key, value in values.items():
            if not isinstance(value, dict):
                lines.append(f"{json.dumps(key)} = {inline(value)}")
        for key, value in values.items():
            if isinstance(value, dict):
                nested = (*path, key)
                lines.extend(["", "[" + ".".join(json.dumps(p) for p in nested) + "]"])
                table(value, nested)

    table(document)
    result = "\n".join(lines).lstrip() + "\n"
    if tomllib.loads(result) != document:
        raise ValueError("TOML serialization changed manifest semantics")
    return result


def write_json(path: Path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def copy_source(origin: Path, destination: Path, excluded_tests=()):
    destination.mkdir(parents=True, exist_ok=True)
    candidates = []
    for base, directories, files in os.walk(origin, followlinks=False):
        relative_base = Path(base).relative_to(origin)
        directories[:] = sorted(d for d in directories if d not in EXCLUDED_DIRECTORIES and not d.startswith(".env") and (relative_base.parts or d in SOURCE_DIRECTORIES))
        for directory in directories:
            if (Path(base) / directory).is_symlink():
                raise ValueError(f"symlink is not exportable: {Path(base) / directory}")
        candidates.extend(Path(base) / file for file in sorted(files))
    for path in sorted(candidates):
        relative = path.relative_to(origin)
        if any(p in EXCLUDED_DIRECTORIES or p.startswith(".env") for p in relative.parts):
            continue
        if relative.parts[0] not in SOURCE_DIRECTORIES and not (len(relative.parts) == 1 and (path.name in {"Cargo.toml", "build.rs", "manifest.json", "presentation.json"} or path.name.startswith(("README", "LICENSE", "NOTICE")))):
            continue
        if path.suffix.lower() in SECRET_SUFFIXES or path.name in {"Cargo.lock", *excluded_tests}:
            continue
        if path.is_symlink():
            raise ValueError(f"symlink is not exportable: {path}")
        if not path.is_file():
            continue
        if path.suffix not in SOURCE_SUFFIXES and not path.name.startswith(("LICENSE", "NOTICE")):
            raise ValueError(f"unreviewed source file type: {path}")
        content = path.read_bytes()
        if SECRET.search(content):
            raise ValueError(f"credential-like content in export source: {path}")
        target = destination / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(content)
        target.chmod(0o755 if path.stat().st_mode & 0o111 else 0o644)


def legal_files(source: Path, destination: Path):
    for path in sorted(source.iterdir()):
        if path.is_file() and (path.name.startswith(("LICENSE", "NOTICE")) or path.name == "COMMERCIAL-LICENSING.md"):
            if path.is_symlink() or SECRET.search(path.read_bytes()):
                raise ValueError(f"unsafe legal file: {path.name}")
            shutil.copyfile(path, destination / path.name)
    if not (destination / "LICENSE").is_file():
        raise ValueError("source LICENSE is required")


def copy_toolchain(source: Path, destination: Path):
    origin = source / "rust-toolchain.toml"
    if not origin.is_file() or origin.is_symlink():
        raise ValueError("reviewed root rust-toolchain.toml is required")
    channel = tomllib.loads(origin.read_text()).get("toolchain", {}).get("channel", "")
    if not re.fullmatch(r"\d+\.\d+\.\d+", channel):
        raise ValueError("Rust toolchain must be pinned to an exact version")
    shutil.copyfile(origin, destination / "rust-toolchain.toml")


def export_sdk(source: Path, destination: Path, plan: dict):
    root = read_manifest(source)
    root["workspace"]["members"] = plan["sdkCrates"]
    root["workspace"].pop("exclude", None)
    for key in ("package", "dependencies", "dev-dependencies", "build-dependencies", "bin", "lib", "target"):
        root.pop(key, None)
    inherited = set()
    for relative in plan["sdkCrates"]:
        for _, table in dependency_tables(read_manifest(source / relative)):
            inherited.update(name for name, spec in table.items() if isinstance(spec, dict) and spec.get("workspace"))
    # Copy only settings inherited by an SDK member, including registry features.
    deps = root["workspace"].get("dependencies", {})
    for name in list(deps):
        if name not in inherited:
            del deps[name]
    (destination / "Cargo.toml").write_text(dump_toml(root))
    for relative in plan["sdkCrates"]:
        copy_source(source / relative, destination / relative)
    for kind in ("plugin", "export"):
        origin = source / "proto" / "webdock" / kind
        if origin.exists():
            # Keep the build.rs relative paths and exact wire definitions intact.
            for path in sorted(origin.rglob("*.proto")):
                if path.is_symlink() or SECRET.search(path.read_bytes()):
                    raise ValueError(f"unsafe protocol source: {path}")
                target = destination / path.relative_to(source)
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(path.read_bytes())
    legal_files(source, destination)
    copy_toolchain(source, destination)
    (destination / "README.md").write_text("# Webdock plugin SDK\n\nShared Rust protocol, native transport, mTLS, authorization, storage and mail contracts. Core owns the catalog, customer metadata, provider connections and BYOK credentials. No runtime credentials are included.\n\nPin consumers to an immutable Git revision. Install protobuf-compiler before building. Generate Cargo.lock, then run `cargo test --workspace --locked`.\n")


def validate_pin(value: str | None, label: str):
    if not value or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._:/-]*@sha256:[0-9a-f]{64}", value):
        raise ValueError(f"{label} must be an explicit image reference pinned by sha256 digest")


def plugin_manifest(source: Path, directory: Path, plan: dict, git: str, revision: str):
    manifest = read_manifest(directory)
    sdk = {source / p for p in plan["sdkCrates"]}
    paths = {(scope, name): target for scope, name, target in path_dependencies(source, directory, manifest)}
    for scope, table in dependency_tables(manifest):
        for name in list(table):
            if (scope, name) not in paths:
                continue
            target = paths[scope, name]
            if target not in sdk:
                del table[name]
                continue
            options = table[name]
            if options.get("workspace"):
                inherited = read_manifest(source)["workspace"]["dependencies"][name]
                features = sorted(set(inherited.get("features", []) + options.get("features", [])))
                options = {**inherited, **options}
                if features:
                    options["features"] = features
            options = {k: v for k, v in options.items() if k not in {"path", "workspace", "registry", "version"}}
            options.update({"git": git, "rev": revision, "package": read_manifest(target)["package"]["name"]})
            table[name] = options
    return manifest


def native_binaries(directory: Path, manifest: dict) -> tuple[list[str], str]:
    name = manifest["package"]["name"]
    binaries = {}
    for binary in manifest.get("bin", []):
        bin_name = binary["name"]
        path = binary.get("path", "src/main.rs" if bin_name == name else f"src/bin/{bin_name}.rs")
        binaries[bin_name] = directory / path
    if manifest["package"].get("autobins", True):
        if (directory / "src/main.rs").is_file():
            binaries.setdefault(name, directory / "src/main.rs")
        for path in sorted((directory / "src/bin").glob("*")):
            if path.suffix == ".rs":
                binaries.setdefault(path.stem, path)
            elif path.is_dir() and (path / "main.rs").is_file():
                binaries.setdefault(path.name, path / "main.rs")
    if not binaries:
        raise ValueError(f"no native executable in {directory.name}")
    for bin_name, path in binaries.items():
        if not re.fullmatch(r"[A-Za-z0-9_-]+", bin_name) or not path.resolve().is_relative_to(directory.resolve()) or not path.is_file() or path.is_symlink():
            raise ValueError(f"invalid or escaping native binary: {directory.name}: {bin_name}")
    entrypoint = manifest["package"].get("default-run", name if name in binaries else next(iter(binaries)) if len(binaries) == 1 else None)
    if entrypoint not in binaries:
        raise ValueError(f"ambiguous native entrypoint; set package.default-run: {directory.name}")
    return sorted(binaries), entrypoint


def release_contract(origin: Path, manifest: dict) -> dict:
    specialized = {"github-repository": ("webdock.repository.v1", "publisher-signed repository Descriptor with exact manifest SHA-256, image digest and release version"), "mail-relay": ("webdock.mail.v1", "Core-admitted descriptor SHA-256 and private configuration release version")}
    authoring = origin / "manifest.json"
    if origin.name in specialized:
        if authoring.exists():
            raise ValueError(f"specialized protocol cannot use an extension manifest: {origin.name}")
        protocol, binding = specialized[origin.name]
        return {"mode": "oci-only", "protocol": protocol, "version": manifest["package"]["version"], "registryAdmission": "not-performed-specialized-protocol", "runtimeBinding": binding}
    if authoring.exists():
        value = json.loads(authoring.read_text())
        if value.get("name") != origin.name or value.get("kind") != "extension" or any(value.get(key) != 2 for key in ("protocolVersion", "sdkVersion", "dataProtocolVersion")):
            raise ValueError(f"root authoring manifest is not the matching native v2 extension: {origin.name}")
        version = value.get("version", "")
        if not re.fullmatch(r"\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?", version):
            raise ValueError(f"invalid native authoring manifest version: {origin.name}")
        manifest["package"]["version"] = version
        return {"mode": "native-extension", "protocol": "webdock.plugin.v2", "version": version, "registryAdmission": "requires-native-validation-and-signed-registry-receipt", "runtimeBinding": "Core-admitted native manifest with exact image digest and release version"}
    return {"mode": "oci-only", "protocol": "webdock.plugin.v2" if origin.name == "lumaserv-compute" else "unspecified-native", "version": manifest["package"]["version"], "registryAdmission": "not-performed-no-reviewed-authoring-manifest", "runtimeBinding": "Core-admitted native manifest required before installation; tests/manifest.json remains a fixture"}


def bind_runtime_version(target: Path, name: str):
    guards = {
        "github-repository": ('    let descriptor = verify_descriptor(&config.descriptor, &publisher)?;', 'descriptor.version'),
        "mail-relay": ('        .map_err(|_| "Invalid provider configuration")?;', 'config.version'),
        "lumaserv-compute": ('    spitzli_lumaserv_compute::validate_manifest(&manifest)?;', 'manifest.version()'),
    }
    if name not in guards:
        return
    anchor, value = guards[name]
    path = target / "src/main.rs"
    raw = path.read_text()
    if raw.count(anchor) != 1:
        raise ValueError(f"review runtime-version guard insertion after source change: {name}")
    guard = f'\n    if {value} != env!("CARGO_PKG_VERSION") {{\n        return Err("Native release version mismatch".into());\n    }}'
    path.write_text(raw.replace(anchor, anchor + guard, 1))
    if name == "github-repository":
        fixture = target / "tests/provider.rs"
        raw = fixture.read_text()
        anchor = 'version: "1.0.0".into(),'
        if raw.count(anchor) != 1:
            raise ValueError("review GitHub subprocess release-version fixture after source change")
        fixture.write_text(raw.replace(anchor, 'version: env!("CARGO_PKG_VERSION").into(),', 1))
    elif name == "lumaserv-compute":
        fixture = target / "tests/manifest.json"
        if fixture.exists():
            value = json.loads(fixture.read_text())
            value["version"] = read_manifest(target)["package"]["version"]
            write_json(fixture, value)


def dockerfile(binaries: list[str], entrypoint: str, builder: str, runtime: str, revision: str, provider_tests: bool) -> str:
    cache = f"spitzli-native-sdk-{revision}"
    mounts = f"--mount=type=cache,id={cache}-registry,target=/usr/local/cargo/registry,sharing=locked --mount=type=cache,id={cache}-git,target=/usr/local/cargo/git,sharing=locked --mount=type=cache,id={cache}-target,target=/src/target,sharing=locked"
    fixture_feature = " --features local-provider-test" if provider_tests else ""
    install = " && ".join(f"install -m 0555 /src/target/release/{binary} /out/{binary}" for binary in sorted(binaries))
    copies = "\n".join(f"COPY --from=build --chown=65532:65532 --chmod=0555 /out/{binary} /usr/local/bin/{binary}" for binary in sorted(binaries))
    return f"""# Context is this plugin repository. Commit regenerated Cargo.lock before building.
# Runtime configuration, issuer keys, mTLS identity and BYOK credentials are mounted by Core.
FROM {builder} AS source
RUN apt-get update && apt-get install -y --no-install-recommends protobuf-compiler ca-certificates pkg-config && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
# Shared only among reviewed Spitzli platform sources; never customer build code.
FROM source AS check
RUN {mounts} cargo check --locked --all-targets --jobs 3 && cargo test --locked --all-targets --jobs 3{fixture_feature}

FROM source AS build
RUN {mounts} cargo build --locked --release --bins --jobs 3 && mkdir /out && {install}

FROM {runtime}
COPY --from=build /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY LICENSE COMMERCIAL-LICENSING.md NOTICE /usr/share/licenses/spitzli-cloud/
{copies}
ENV HOME=/tmp
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/{entrypoint}"]
"""


def export_plugins(source: Path, destination: Path, plan: dict, args):
    if not args.sdk_rev or not re.fullmatch(r"[0-9a-f]{40}", args.sdk_rev):
        raise ValueError("--sdk-rev must be a full immutable 40-character Git commit")
    url = urlsplit(args.sdk_git or "")
    if url.scheme != "https" or not url.netloc or url.username or url.password or url.query or url.fragment:
        raise ValueError("--sdk-git must be a credential-free HTTPS Git repository URL")
    validate_pin(args.builder_image, "--builder-image")
    validate_pin(args.runtime_image, "--runtime-image")
    if any(p["name"] == "archives" for p in plan["plugins"]):
        validate_pin(args.archives_runtime_image, "--archives-runtime-image (PostgreSQL client runtime)")
    for package in plan["plugins"]:
        origin = source / package["source"]
        target = destination / package["name"]
        copy_source(origin, target, package["coreIntegrationTests"])
        manifest = plugin_manifest(source, origin, plan, args.sdk_git, args.sdk_rev)
        release = release_contract(origin, manifest)
        binaries, entrypoint = native_binaries(origin, manifest)
        (target / "Cargo.toml").write_text(dump_toml(manifest))
        bind_runtime_version(target, package["name"])
        validator = source / "tools/plugin-registry/manifest-validator"
        if (validator / "Cargo.toml").exists():
            sdk = {source / p for p in plan["sdkCrates"]}
            if any(path not in sdk for _, _, path in path_dependencies(source, validator, read_manifest(validator))):
                raise ValueError("manifest validator depends on a crate outside the SDK")
            validator_target = target / "tools/manifest-validator"
            copy_source(validator, validator_target)
            (validator_target / "Cargo.toml").write_text(dump_toml(plugin_manifest(source, validator, plan, args.sdk_git, args.sdk_rev)))
        legal_files(source, target)
        copy_toolchain(source, target)
        runtime = args.archives_runtime_image if package["name"] == "archives" else args.runtime_image
        (target / "Dockerfile").write_text(dockerfile(binaries, entrypoint, args.builder_image, runtime, args.sdk_rev, "local-provider-test" in manifest.get("features", {})))
        (target / ".dockerignore").write_text(".git\n.env*\n*.pem\n*.key\n*.p12\n*.pfx\n**/target\nui\nrelease\nEXPORT-PROVENANCE.json\n")
        (target / ".gitignore").write_text("/target/\n**/target/\n/release/\n.env*\n*.pem\n*.key\n")
        write_json(target / "standalone.json", {**package, "sdkGit": args.sdk_git, "sdkRevision": args.sdk_rev, "connectionsOwner": "Core", "lockfileRequiresRegeneration": True, "binaries": binaries, "entrypoint": entrypoint, "release": release})
        workflow = target / ".github/workflows"
        workflow.mkdir(parents=True)
        if release["mode"] == "native-extension":
            template = source / "tools/plugin-release"
            scripts = target / "scripts/plugin-release"
            scripts.mkdir(parents=True)
            shutil.copyfile(template / "plugin.yml", workflow / "plugin.yml")
            shutil.copyfile(template / "release.mjs", scripts / "release.mjs")
        else:
            shutil.copyfile(Path(__file__).with_name("native-oci.yml"), workflow / "native-oci.yml")
            scripts = target / "scripts"
            scripts.mkdir(parents=True)
            shutil.copyfile(Path(__file__).with_name("native_release.py"), scripts / "native-release.py")
        oci_builder = source / "tools/plugin-release/build_oci.py"
        if not oci_builder.is_file() or oci_builder.is_symlink():
            raise ValueError("reviewed rootless build helper tools/plugin-release/build_oci.py is required")
        builder_target = target / "scripts/plugin-release"
        builder_target.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(oci_builder, builder_target / "build_oci.py")
        (target / "STANDALONE.md").write_text(
            "# Standalone build\n\n"
            "Core owns installation/catalog metadata, customer connections and BYOK. "
            "The SDK revision and release contract are recorded in standalone.json.\n\n"
            "Generate/review/commit Cargo.lock and the validator helper's separate lock when present. "
            "Run the qualified rootless build helper with `--check` for locked Cargo checks/tests, then "
            "build the default production target. The check stage alone enables declared local provider "
            "fixture features; production binaries use default features. Both stages use three Cargo jobs "
            "and SDK-revision-scoped caches restricted to trusted Spitzli platform sources.\n\n"
            "Core mounts private configuration, issuer keys, mTLS identity, provider credentials and "
            "permitted writable storage. Deployment must enforce dropped capabilities, no host network "
            "or host socket mounts, no-new-privileges and a read-only root filesystem.\n\n"
            "Root native authoring manifests select the validated extension release workflow. "
            "Specialized providers and runtimes without a reviewed root manifest select OCI metadata "
            "publication only; installation/admission and protocol descriptors remain separate. Test "
            "manifests are never promoted. Archives' Core-connected acceptance tests remain in the "
            "source monorepo and are listed in standalone.json. UI source is retained separately.\n"
        )
        provenance(target, plan, args)


def provenance(destination: Path, plan: dict, args):
    files = {str(p.relative_to(destination)): hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(destination.rglob("*")) if p.is_file()}
    write_json(destination / "EXPORT-PROVENANCE.json", {"formatVersion": 1, "sourceLabel": args.source_label, "inventory": plan, "files": files})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("inventory", "sdk", "plugins"))
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--source-label", default="reviewed-source-snapshot")
    parser.add_argument("--sdk-git")
    parser.add_argument("--sdk-rev")
    parser.add_argument("--builder-image")
    parser.add_argument("--runtime-image")
    parser.add_argument("--archives-runtime-image")
    args = parser.parse_args()
    source = args.source.resolve()
    plan = inventory(source)
    if args.mode == "inventory":
        print(json.dumps(plan, indent=2, sort_keys=True))
        return
    if plan["forbiddenSdkCrates"]:
        raise ValueError(f"SDK includes Core/domain crates: {plan['forbiddenSdkCrates']}")
    if not args.output:
        raise ValueError("--output is required for export")
    output = args.output.absolute()
    if output.exists() or output.is_symlink():
        raise ValueError("output already exists; refusing to overwrite")
    if output.resolve().is_relative_to(source):
        raise ValueError("output must be outside the source snapshot")
    output.parent.mkdir(parents=True, exist_ok=True)
    staging = Path(tempfile.mkdtemp(prefix=".webdock-export-", dir=output.parent))
    try:
        if args.mode == "sdk":
            export_sdk(source, staging, plan)
        else:
            export_plugins(source, staging, plan, args)
        provenance(staging, plan, args)
        # rename never merges with an existing export or rewrites source data.
        os.rename(staging, output)
    finally:
        if staging.exists():
            shutil.rmtree(staging)
    print(json.dumps({"output": str(output), "mode": args.mode, "plugins": len(plan["plugins"]), "sdkCrates": len(plan["sdkCrates"])}))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError) as error:
        print(f"export refused: {error}", file=sys.stderr)
        sys.exit(1)
