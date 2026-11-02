import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { parsePluginManifest } from "../../packages/plugin-sdk/src/contracts/manifest";
import { nativeSchema } from './native-schema';

export const MAX_MANIFEST_BYTES = 64 * 1024;

/** Local preflight only: this does not change registry admission. */
export function preflightV1(raw: string) {
  if (Buffer.byteLength(raw) > MAX_MANIFEST_BYTES) throw new Error("Registry manifest exceeds 64 KiB");
  const manifest = parsePluginManifest(JSON.parse(raw));
  if (manifest.image.endsWith("@sha256:" + "0".repeat(64))) throw new Error("Placeholder image digest");
  return manifest;
}

/** Refuse to emit an unsafe per-field patch for the reviewed API implementation. */
export function inspectContract(tabulariumRoot: string, schemaPath: string) {
  const paths = ["apps/api/src/lib/extensions-schema.ts", "packages/manifest/src/schema.ts"];
  const sources = paths.map(path => readFileSync(resolve(tabulariumRoot, path), "utf8"));
  const rootSupport = readFileSync(resolve(tabulariumRoot, 'packages/manifest/src/schema.ts'), 'utf8').includes('input.kindSchemas') &&
    readFileSync(resolve(tabulariumRoot, 'apps/api/src/lib/kinds.ts'), 'utf8').includes('validateKindManifestSchema(o.manifestSchema, key)');
  if (rootSupport) for (const path of ['packages/manifest/src/root-schema.ts', 'apps/api/src/lib/kinds.ts', 'apps/api/src/lib/manifest.ts']) {
    paths.push(path);
    sources.push(readFileSync(resolve(tabulariumRoot, path), 'utf8'));
  }
  if (!sources[0].includes("return input.properties as Record<string, unknown>") ||
      !sources[1].includes("properties: { ...coreProps, ...extProps }")) {
    throw new Error("Registry schema implementation changed; review root-constraint support before preparing any migration");
  }
  const legacy = readFileSync(schemaPath);
  const delta = JSON.parse(legacy.toString());
  for (const key of ["protocolVersion", "sdkVersion", "dataProtocolVersion"]) {
    if (delta[key]?.allOf?.[0]?.const !== 1 || delta[key]?.required !== true) {
      throw new Error("Unexpected legacy protocol schema; obtain and review the current kind snapshot");
    }
  }
  return {
    status: rootSupport ? "supported" : "blocked",
    kind: "extension",
    reason: rootSupport ? "Explicit kind.manifestSchema root admission is available in this source checkout; deployment and authenticated current-kind review remain separate." : "extensionsSchema only installs independent field schemas; root oneOf/allOf is discarded. Per-field [1,2] accepts mixed protocol tuples.",
    patch: null,
    legacySchemaSha256: createHash("sha256").update(legacy).digest("hex"),
    reviewedSources: paths.map((path, i) => ({ path, sha256: createHash("sha256").update(sources[i]).digest("hex") })),
    requiredUpstreamContract: "Validated, bounded kind-scoped root constraints merged into BOTH kind-specific and kindless schemas and used by authoring, source/release ingestion, refresh and schema discovery. Retain the v1 schema as one complete branch and a separately reviewed native-v2 schema as the other; preserve closed properties and all bounds.",
  };
}

export async function prepareKindSchema(tabulariumRoot: string, schemaPath: string) {
  const report = inspectContract(tabulariumRoot, schemaPath);
  if (report.status !== 'supported') return report;
  const { buildSchema } = await import(resolve(tabulariumRoot, 'packages/manifest/src/schema.ts'));
  const { ManifestSchema } = await import(resolve(tabulariumRoot, 'packages/manifest/src/core.ts'));
  const { validateKindManifestSchema } = await import(resolve(tabulariumRoot, 'packages/manifest/src/root-schema.ts'));
  const delta = JSON.parse(readFileSync(schemaPath, 'utf8'));
  const legacy = buildSchema({ coreSchema: ManifestSchema, kind: 'extension', kindOverrides: { extension: delta } });
  delete legacy.$schema; delete legacy.$id; delete legacy.title;
  legacy.properties.kind = { const: 'extension' };
  if (!legacy.required.includes('kind')) legacy.required.push('kind');
  const manifestSchema = validateKindManifestSchema({ oneOf: [legacy, nativeSchema(legacy)] }, 'extension');
  return { ...report, status: 'prepared', patch: { manifestSchema },
    nativeProtocolConstantsSha256: createHash('sha256').update(readFileSync('crates/plugin-protocol/src/lib.rs')).digest('hex'),
    reason: 'Local complete v1/v2 union prepared. Merge ONLY this manifestSchema field into the exact authenticated current kind; retain every existing kind field and extensionsSchema. No API write was performed.' };
}

if (import.meta.main) {
  const [source, schema = "tools/plugins/tabularium-kind.schema.json"] = process.argv.slice(2);
  if (!source) throw new Error("Usage: bun --no-env-file tools/plugin-registry/prepare.ts TABULARIUM_SOURCE_ROOT [REVIEWED_V1_DELTA]");
  const result = await prepareKindSchema(source, schema);
  console.log(JSON.stringify(result, null, 2));
  process.exitCode = result.status === 'prepared' ? 0 : 2;
}
