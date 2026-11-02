import test from "node:test";
import assert from "node:assert/strict";
import { mock } from "bun:test";
import { buildConfig, getPayload } from "payload";
import { loadCMSConfig } from "../../src/lib/config.ts";
import * as storage from "../../src/lib/media-storage.ts";

// Only the database boundary is offline; Payload's failed-init cache is real.
test("cached Payload never bypasses a rejected binding and recovers after transient verification failure", async () => {
  Object.assign(process.env, {
    CMS_INSTANCE_ID: "startup-fixture",
    CMS_PROJECT_ID: "startup-project",
    CMS_ORIGIN: "https://cms.invalid",
    CMS_SITE_NAME: "Startup fixture",
    PAYLOAD_SECRET: "p".repeat(32),
    CMS_BRIDGE_SECRET: "b".repeat(32),
    DATABASE_URL: "postgresql://offline:offline@127.0.0.1/offline",
    NODE_ENV: "test",
    PAYLOAD_DISABLE_DEPENDENCY_CHECKER: "true",
  });
  const wanted = storage.mediaBinding(loadCMSConfig());
  let stored = { ...wanted, sha256: "0".repeat(64) };
  let unavailable = false;
  const verify = async () => {
    if (unavailable) throw Error("Offline transient database failure");
    storage.checkMediaBinding(stored, 1, wanted);
  };
  const config = buildConfig({
    secret: "p".repeat(32),
    admin: { disable: true, importMap: { autoGenerate: false } },
    typescript: { autoGenerate: false },
    telemetry: false,
    db: { init: () => ({ connect: async () => {}, destroy: async () => {} }) },
    onInit: verify,
    logger: { options: { level: "silent" } },
    email: () => ({
      name: "offline",
      defaultFromName: "offline",
      defaultFromAddress: "offline@invalid.test",
      sendEmail: async () => {},
    }),
  });
  mock.module("../../src/payload.config.ts", () => ({ default: config }));
  mock.module("../../src/lib/media-storage.ts", () => ({
    ...storage,
    verifyMediaBinding: verify,
  }));
  const { getCMS } = await import("../../src/lib/cms.ts");
  const { GET: health } = await import("../../src/app/api/health/route.ts");
  // Seed the installed library's failed init entry exactly as the original bug did.
  await assert.rejects(getPayload({ config }));
  try {
    for (let i = 0; i < 3; i++) {
      assert.equal((await health()).status, 503);
      await assert.rejects(getCMS());
    }
    stored = wanted;
    const payload = await getCMS();
    assert.ok(payload.collections.users);
    unavailable = true;
    await assert.rejects(getCMS());
    assert.equal((await health()).status, 503);
    unavailable = false;
    assert.equal(await getCMS(), payload);
    stored = { ...wanted, sha256: "f".repeat(64) };
    await assert.rejects(getCMS());
    await payload.destroy();
  } finally {
    mock.restore();
  }
});
