#!/usr/bin/env bash
set -euo pipefail
version=${1:?Usage: build.sh VERSION OUTPUT_DIRECTORY linux/amd64|linux/arm64}
out=${2:?Output directory required}
platform=${3:?Explicit native target platform required}
case "$platform" in linux/amd64|linux/arm64) ;; *) echo 'Unsupported target platform' >&2; exit 1;; esac
native=$(uname -m)
case "$native" in x86_64) native=amd64 ;; aarch64|arm64) native=arm64 ;; esac
test "linux/$native" = "$platform" || { echo 'Use a native runner for the selected architecture' >&2; exit 1; }
bun -e 'import {validateVersion} from "./tools/release/manifest.mjs"; validateVersion(process.argv[1])' "$version"
test -z "$(git status --porcelain --untracked-files=normal)" || { echo "Release builds require a clean source checkout" >&2; exit 1; }
# Empty directory prevents accidentally mixing artifacts from separate revisions.
mkdir -p "$out"
test -z "$(ls -A "$out")" || { echo 'Output directory must be empty' >&2; exit 1; }
revision=$(git rev-parse HEAD)
case "${WEBDOCK_RELEASE_CMS:-false}" in true|false) ;; *) echo 'WEBDOCK_RELEASE_CMS must be true or false' >&2; exit 1;; esac
case "${WEBDOCK_RELEASE_DATABASES:-false}" in true|false) ;; *) echo 'WEBDOCK_RELEASE_DATABASES must be true or false' >&2; exit 1;; esac
if [ "${WEBDOCK_RELEASE_DATABASES:-false}" = true ]; then
  : "${TABULARIS_IMAGE:?Database artifacts require the verified immutable runtime image}"
  bun -e 'if(!/^[a-z0-9][a-z0-9./:_-]*@sha256:[a-f0-9]{64}$/.test(process.env.TABULARIS_IMAGE||""))throw Error("Immutable Tabularis image digest required")'
fi
for target in api-core console-core console auth api worker migrate; do
  docker buildx build --platform "$platform" --file deploy/docker/Dockerfile --target "$target" \
    --label "org.opencontainers.image.revision=$revision" --label "org.opencontainers.image.version=$version" \
    --label "org.opencontainers.image.source=https://github.com/spitzli/webdock" \
    --provenance=mode=max --sbom=true --output "type=oci,compression=gzip,force-compression=true,dest=$out/$target.oci.tar" .
done
docker buildx build --platform "$platform" --file deploy/docker/agent.Dockerfile --label "org.opencontainers.image.source=https://github.com/spitzli/webdock" --label "org.opencontainers.image.revision=$revision" --label "org.opencontainers.image.version=$version" --provenance=mode=max --sbom=true --output "type=oci,compression=gzip,force-compression=true,dest=$out/agent.oci.tar" .
docker buildx build --platform "$platform" --file infra/mail/Dockerfile --label "org.opencontainers.image.source=https://github.com/spitzli/webdock" --label "org.opencontainers.image.revision=$revision" --label "org.opencontainers.image.version=$version" --provenance=mode=max --sbom=true --output "type=oci,compression=gzip,force-compression=true,dest=$out/mail-runtime.oci.tar" infra/mail
if [ "${WEBDOCK_RELEASE_DATABASES:-false}" = true ]; then
  tabularis_commit=$(bun -e 'import {readFileSync} from "node:fs"; console.log(JSON.parse(readFileSync("vendor/tabularis-web-ui.json")).sourceCommit)')
  for target in database-gateway tabularis-runtime; do
    docker buildx build --platform "$platform" --file "deploy/docker/$target.Dockerfile"       --build-arg "TABULARIS_IMAGE=$TABULARIS_IMAGE" --build-arg "TABULARIS_SOURCE_COMMIT=$tabularis_commit"       --provenance=mode=max --sbom=true --output "type=oci,compression=gzip,force-compression=true,dest=$out/$target.oci.tar" .
  done
  helm package deploy/helm/webdock-databases --version "$version" --app-version "$version" --destination "$out"
fi
if [ "${WEBDOCK_RELEASE_CMS:-false}" = true ]; then
  for target in cms-instance managed-cms; do
    case "$target" in cms-instance) dockerfile=apps/cms-instance/Dockerfile ;; managed-cms) dockerfile=extensions/managed-cms/Dockerfile ;; esac
    docker buildx build --platform "$platform" --file "$dockerfile" \
      --label "org.opencontainers.image.revision=$revision" --label "org.opencontainers.image.version=$version" \
      --label "org.opencontainers.image.source=https://github.com/spitzli/webdock" \
      --provenance=mode=max --sbom=true --output "type=oci,compression=gzip,force-compression=true,dest=$out/$target.oci.tar" .
  done
fi
helm package deploy/helm/webdock --version "$version" --app-version "$version" --destination "$out"
helm package deploy/helm/webdock-agent --version "$version" --app-version "$version" --destination "$out"
bun tools/release/verify-architecture.mjs "$out" "$platform" "$revision"
if [ "${WEBDOCK_RELEASE_CMS:-false}" = true ]; then
  bun run - "$out" "${WEBDOCK_RELEASE_REGISTRY:-ghcr.io/spitzli/webdock}" <<'JS'
import { readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { createManifest } from "./extensions/managed-cms/src/manifest.ts";
import { validateCMSPlugin } from "./tools/release/manifest.mjs";
const [out, registry] = process.argv.slice(2);
const evidence = JSON.parse(await readFile(join(out, "architecture-evidence.json"), "utf8"));
const plugin = createManifest(`${registry}/managed-cms@${evidence.imageRegistryDigests["managed-cms"]}`);
validateCMSPlugin(plugin, evidence, registry);
await writeFile(join(out, "managed-cms.plugin.json"), JSON.stringify(plugin, null, 2) + "\n");
JS
fi
bun tools/release/manifest.mjs create "$out" "$version" "$revision"
bun tools/release/manifest.mjs verify "$out"
