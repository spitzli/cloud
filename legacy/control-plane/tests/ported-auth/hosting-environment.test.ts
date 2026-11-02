import test from "node:test";
import assert from "node:assert/strict";
import { requireHostingEnvironment } from "../../src/modules/hosting/http";
test("production Vercel hosting requires explicit activation AND the real Frankfurt runtime", () => {
  const keys = [
    "NODE_ENV",
    "VERCEL",
    "VERCEL_REGION",
    "WEBDOCK_HOSTING_CONTROL_PLANE",
    "WEBDOCK_HOSTING_EU_VERIFIED",
  ];
  const saved = Object.fromEntries(keys.map((k) => [k, process.env[k]]));
  try {
    Object.assign(process.env, { NODE_ENV: "production" });
    process.env.VERCEL = "1";
    delete process.env.WEBDOCK_HOSTING_EU_VERIFIED;
    process.env.WEBDOCK_HOSTING_CONTROL_PLANE = "vercel-fra1";
    process.env.VERCEL_REGION = "fra1";
    assert.doesNotThrow(requireHostingEnvironment);
    for (const region of ["iad1", "ams1", ""]) {
      process.env.VERCEL_REGION = region;
      assert.throws(requireHostingEnvironment);
    }
    process.env.VERCEL_REGION = "fra1";
    delete process.env.WEBDOCK_HOSTING_CONTROL_PLANE;
    assert.throws(requireHostingEnvironment);
    process.env.WEBDOCK_HOSTING_EU_VERIFIED = "true";
    process.env.VERCEL_REGION = "iad1";
    assert.throws(requireHostingEnvironment);
    delete process.env.VERCEL;
    assert.doesNotThrow(requireHostingEnvironment);
    delete process.env.WEBDOCK_HOSTING_EU_VERIFIED;
    assert.throws(requireHostingEnvironment);
    Object.assign(process.env, { NODE_ENV: "development" });
    assert.doesNotThrow(requireHostingEnvironment);
  } finally {
    for (const k of keys) {
      if (saved[k] === undefined) delete process.env[k];
      else process.env[k] = saved[k];
    }
  }
});
