import test from "node:test";
import assert from "node:assert/strict";
import { loadCMSConfig } from "../src/lib/config.ts";
import {
  mediaBinding,
  checkMediaBinding,
  checkedMediaBytes,
  checkedMediaResponse,
} from "../src/lib/media-storage.ts";

const base = {
  CMS_INSTANCE_ID: "instance-1",
  CMS_PROJECT_ID: "project-1",
  CMS_ENVIRONMENT_ID: "production",
  CMS_ORIGIN: "https://cms.example.test",
  CMS_SITE_NAME: "Example",
  PAYLOAD_SECRET: "p".repeat(32),
  CMS_BRIDGE_SECRET: "b".repeat(32),
  DATABASE_URL: "postgresql://runtime:private@127.0.0.1/cms",
};
const remote = {
  ...base,
  CMS_MEDIA_STORAGE: "s3",
  CMS_MEDIA_S3_RESOURCE_ID: "bbba0aa0-2f38-4fb0-a870-81808ccbd1b0",
  CMS_MEDIA_S3_ENDPOINT: "https://s3.eu-central-2.wasabisys.com",
  CMS_MEDIA_S3_REGION: "eu-central-2",
  CMS_MEDIA_S3_BUCKET: "owned-private-cms",
  CMS_MEDIA_S3_ACCESS_KEY_ID: "restricted-app-key",
  CMS_MEDIA_S3_SECRET_ACCESS_KEY: "private-fixture-secret",
};
test("S3 configuration is explicit, privately scoped, HTTPS-only and has no silent local fallback", () => {
  assert.equal(loadCMSConfig(base).mediaStorage.kind, "local");
  const config = loadCMSConfig(remote);
  assert.equal(config.mediaStorage.kind, "s3");
  for (const patch of [
    { CMS_MEDIA_STORAGE: "local" },
    { CMS_ENVIRONMENT_ID: "" },
    { CMS_MEDIA_S3_SECRET_ACCESS_KEY: "" },
    { CMS_MEDIA_S3_RESOURCE_ID: "not-an-id" },
    { CMS_MEDIA_S3_ENDPOINT: "http://storage.example.test" },
    { CMS_MEDIA_S3_ENDPOINT: "https://user:password@example.test" },
    { CMS_MEDIA_S3_ENDPOINT: "https://example.test/path" },
    { CMS_MEDIA_S3_ENDPOINT: "https://example.test/?secret=hidden" },
  ])
    assert.throws(() => loadCMSConfig({ ...remote, ...patch }));
  assert.throws(() =>
    loadCMSConfig({
      ...remote,
      CMS_MEDIA_S3_ENDPOINT: "http://127.0.0.1:55900",
    }),
  );
  assert.equal(
    loadCMSConfig({
      ...remote,
      CMS_MEDIA_S3_ENDPOINT: "http://127.0.0.1:55900",
      CMS_MEDIA_S3_LOOPBACK_TEST: "true",
    }).mediaStorage.kind,
    "s3",
  );
  assert.throws(() =>
    loadCMSConfig({
      ...remote,
      CMS_MEDIA_S3_ENDPOINT: "http://10.0.0.1",
      CMS_MEDIA_S3_LOOPBACK_TEST: "true",
    }),
  );
});
test("binding excludes rotating keys but prevents destination, environment or instance adoption", () => {
  const binding = mediaBinding(loadCMSConfig(remote));
  assert.ok(!JSON.stringify(binding).includes("private-fixture-secret"));
  assert.deepEqual(
    mediaBinding(
      loadCMSConfig({
        ...remote,
        CMS_MEDIA_S3_SECRET_ACCESS_KEY: "rotated-secret",
      }),
    ),
    binding,
  );
  checkMediaBinding(null, 0, binding);
  assert.throws(() => checkMediaBinding(null, 1, binding), /migration/i);
  checkMediaBinding(binding, 12, binding);
  for (const patch of [
    { CMS_INSTANCE_ID: "other" },
    { CMS_ENVIRONMENT_ID: "preview" },
    { CMS_MEDIA_S3_BUCKET: "different-bucket" },
  ]) {
    assert.throws(() =>
      checkMediaBinding(
        binding,
        12,
        mediaBinding(loadCMSConfig({ ...remote, ...patch })),
      ),
    );
  }
  assert.throws(() =>
    checkMediaBinding(binding, 0, mediaBinding(loadCMSConfig(base))),
  );
});
test("media response bytes must match exact metadata and SHA-256 before being exposed", () => {
  const bytes = Buffer.from("private raster fixture");
  const hash =
    "ccf24f8ee0ba5650643fe927e2bb6771e8b54c1ac7051830de9b5aede2f79680";
  assert.deepEqual(checkedMediaBytes(bytes, bytes.length, hash), bytes);
  assert.throws(() => checkedMediaBytes(bytes, bytes.length + 1, hash));
  assert.throws(() => checkedMediaBytes(bytes, bytes.length, "00".repeat(32)));
});

test("provider responses reject redirects, truncation and oversized bodies before release", async () => {
  const expected = Buffer.from("abc"),
    hash = (await import("node:crypto"))
      .createHash("sha256")
      .update(expected)
      .digest("hex");
  for (const response of [
    new Response(null, { status: 302 }),
    new Response("ab", { headers: { "content-length": "3" } }),
    new Response("abcd", { headers: { "content-length": "3" } }),
  ])
    await assert.rejects(checkedMediaResponse(response, 3, hash));
  assert.deepEqual(
    await checkedMediaResponse(
      new Response("abc", { headers: { "content-length": "3" } }),
      3,
      hash,
    ),
    expected,
  );
});
