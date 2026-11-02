#!/usr/bin/env bash
# Explicit operator action. Never called by the candidate workflow.
set -euo pipefail
out=${1:?Usage: publish.sh DIRECTORY OCI_REGISTRY GITHUB_REPOSITORY}
registry=${2:?OCI registry/repository required}
repository=${3:?GitHub repository required}
: "${RELEASE_CERTIFICATE_IDENTITY:?Expected signing workflow identity required}"
: "${RELEASE_CERTIFICATE_OIDC_ISSUER:?Expected OIDC issuer required}"
cosign verify-blob --bundle "$out/SHA256SUMS.sigstore.json" \
  --certificate-identity "$RELEASE_CERTIFICATE_IDENTITY" \
  --certificate-oidc-issuer "$RELEASE_CERTIFICATE_OIDC_ISSUER" "$out/SHA256SUMS"
(cd "$out" && sha256sum --check SHA256SUMS)
bun tools/release/manifest.mjs verify "$out"
# Validate the signed companion before any registry writes.
if [ -f "$out/managed-cms.plugin.json" ]; then
  bun run - "$out" "$registry" <<'JS'
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { validateCMSPlugin } from "./tools/release/manifest.mjs";
const [out, registry] = process.argv.slice(2);
const manifest = JSON.parse(await readFile(join(out, "manifest.json"), "utf8"));
validateCMSPlugin(JSON.parse(await readFile(join(out, "managed-cms.plugin.json"), "utf8")), manifest.architectureEvidence, registry);
JS
fi
version=$(bun -e 'import {readFileSync} from "node:fs"; console.log(JSON.parse(readFileSync(process.argv[1])).version)' "$out/manifest.json")
revision=$(bun -e 'import {readFileSync} from "node:fs"; console.log(JSON.parse(readFileSync(process.argv[1])).revision)' "$out/manifest.json")
test "$(gh api "repos/$repository/commits/v$version" --jq .sha)" = "$revision" || { echo 'Release tag differs from signed source revision' >&2; exit 1; }
for image in api-core console-core console auth api worker migrate agent mail-runtime; do
  skopeo copy --all --preserve-digests "oci-archive:$out/$image.oci.tar" "docker://$registry/$image:$version"
done
if [ -f "$out/database-gateway.oci.tar" ]; then
  for image in database-gateway tabularis-runtime; do
    skopeo copy --all --preserve-digests "oci-archive:$out/$image.oci.tar" "docker://$registry/$image:$version"
  done
  helm push "$out/webdock-databases-$version.tgz" "oci://$registry/charts"
fi
companions=()
if [ -f "$out/cms-instance.oci.tar" ]; then
  for image in cms-instance managed-cms; do
    skopeo copy --all --preserve-digests "oci-archive:$out/$image.oci.tar" "docker://$registry/$image:$version"
  done
  companions+=("$out/managed-cms.plugin.json")
fi
helm push "$out/webdock-$version.tgz" "oci://$registry/charts"
helm push "$out/webdock-agent-$version.tgz" "oci://$registry/charts"
gh release create "v$version" --repo "$repository" --draft --verify-tag --title "Webdock $version" --notes 'Candidate artifacts; review installation and migration gates before rollout.' "$out/manifest.json" "$out/architecture-evidence.json" "$out/SHA256SUMS" "$out/SHA256SUMS.sigstore.json" "$out/"*.tgz "$out/"*.oci.tar "${companions[@]}"
