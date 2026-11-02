import { test } from "node:test";
import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
test("Mail UI depends on explicit actions/data/shell ports, never application imports or runtime secrets", async () => {
  const directory = new URL("../src/ui/", import.meta.url);
  for (const file of await readdir(directory)) {
    const source = await readFile(new URL(file, directory), "utf8");
    assert.doesNotMatch(
      source,
      /(?:from\s*|import\s*\()["'](?:@\/|[^"']*apps\/)/,
      file,
    );
    assert.doesNotMatch(
      source,
      /process\.env|next\/headers|["']use server["']/,
      file,
    );
  }
  const actions = await readFile(
    new URL(
      "../../../legacy/console/src/components/mail/product-actions.ts",
      import.meta.url,
    ),
    "utf8",
  );
  assert.match(actions, /^['"]use server['"]/);
  const form = await readFile(
    new URL("../src/ui/product-form.tsx", import.meta.url),
    "utf8",
  );
  assert.match(form, /action:\s*MailAction/);
  assert.doesNotMatch(form, /from ["'].*product-actions/);
});
