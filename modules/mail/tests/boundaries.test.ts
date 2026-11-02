import { test } from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
const root = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");
function files(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) =>
    entry.isDirectory()
      ? files(resolve(directory, entry.name))
      : /\.tsx?$/.test(entry.name)
        ? [resolve(directory, entry.name)]
        : [],
  );
}
test("module and adapters never import application internals; module never imports adapters", () => {
  for (const folder of [
    "modules/mail/src",
    "packages/core/src",
    "packages/auth-better-auth/src",
    "adapters/mail/turbosmtp/src",
  ])
    for (const file of files(resolve(root, folder))) {
      const source = readFileSync(file, "utf8");
      assert.doesNotMatch(
        source,
        /(?:from\s+|import\s*\()["'][^"']*(?:apps\/|@webdock\/api|@\/)/,
        file,
      );
      if (folder.startsWith("modules"))
        assert.doesNotMatch(
          source,
          /(?:from\s+|import\s*\()["'][^"']*(?:adapters\/|@webdock\/adapter-)/,
          file,
        );
    }
});

test("Mail access policy does not read identity provider tables and API Mail composition uses the principal boundary", () => {
  for (const name of ["native-access", "product-access"]) {
    const source = readFileSync(
      resolve(root, "modules/mail/src/server/" + name + ".ts"),
      "utf8",
    );
    assert.doesNotMatch(
      source,
      /(?:FROM|JOIN)\s+webdock_auth\.(?:"user"|session|member|studio_tenant_preview)/,
    );
  }
  for (const file of files(resolve(root, "legacy/control-plane/src/modules/mail"))) {
    const source = readFileSync(file, "utf8");
    assert.doesNotMatch(
      source,
      /auth\.api\.getSession|(?:from\s+|import\s*\()["'][^"']*auth\/auth["']/,
      file,
    );
  }
});

test("product compatibility paths contain composition rather than database business rules", () => {
  for (const file of files(
    resolve(root, "legacy/control-plane/src/modules/mail/product"),
  )) {
    const source = readFileSync(file, "utf8");
    assert.doesNotMatch(
      source,
      /\b(?:SELECT|INSERT INTO|UPDATE|DELETE FROM|CREATE TABLE|ALTER TABLE)\b/,
      file,
    );
    assert.doesNotMatch(
      source,
      /new (?:Pool|TurboSMTPClient|TurboDomainsClient|TrackingClient)\([^)]*\)\.(?:query|create|send)/,
      file,
    );
  }
});
