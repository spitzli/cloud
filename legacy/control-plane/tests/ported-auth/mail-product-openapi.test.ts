import assert from "node:assert/strict";
import test from "node:test";
import { existsSync } from "node:fs";
import { mailOpenAPI } from "../../src/modules/mail/product/openapi";
import { mailOperations } from "../../src/modules/mail/product/api-contract";
test("every published operation has an actual route and schemas describe batch, keys and uncertain outcomes", () => {
  const spec = mailOpenAPI();
  assert.equal(spec.openapi, "3.1.0");
  let total = 0;
  for (const operation of mailOperations) {
    const route = new URL(
      "../../src/protocols/legacy-auth/api/mail/v1" +
        operation.path.replaceAll("{", "[").replaceAll("}", "]") +
        "/route.ts",
      import.meta.url,
    );
    assert.ok(existsSync(route));
    assert.ok(spec.paths[operation.path][operation.method.toLowerCase()]);
    total++;
  }
  assert.equal(
    Object.values(spec.paths).reduce(
      (n, path) => n + Object.keys(path).length,
      0,
    ),
    total,
  );
  assert.ok(JSON.stringify(spec).includes("202"));
  assert.ok(JSON.stringify(spec).includes("If-Match"));
  assert.ok(!JSON.stringify(spec).includes("consumerSecret"));
});
