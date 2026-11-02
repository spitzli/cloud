import { readFile, writeFile } from "node:fs/promises";
import { installationJSONSchema } from "../../packages/module-sdk/src/installation-json-schema";
import { fullManifests as manifests } from "../../apps/control-plane/src/full-installation";
const path = new URL(
  "../../deploy/helm/webdock/values.schema.json",
  import.meta.url,
);
const previous = await readFile(path, "utf8");
const schema = JSON.parse(previous);
schema.properties.modules = installationJSONSchema(manifests);
const next = JSON.stringify(schema, null, 2) + "\n";
if (process.argv.includes("--check")) {
  if (previous !== next)
    throw Error(
      "Helm module schema is stale; run tools/release/generate-chart-schema.ts",
    );
} else await writeFile(path, next);
