import test from "node:test";
import assert from "node:assert/strict";
test(
  "real database binding mismatch keeps repeated startup and health checks closed",
  { skip: process.env.CMS_BINDING_REJECTION !== "true" },
  async () => {
    const { getCMS } = await import("../src/lib/cms.ts");
    const { GET } = await import("../src/app/api/health/route.ts");
    for (let i = 0; i < 3; i++) {
      assert.equal((await GET()).status, 503);
      await assert.rejects(getCMS());
    }
  },
);
