import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { createHash } from "node:crypto";

test(
  "forward migration preserves a preexisting local row, version, URL and file with a null hash",
  { skip: process.env.CMS_LEGACY_ACCEPTANCE !== "true" },
  async () => {
    const expected = JSON.parse(
      await readFile("/data/transfers/legacy-media.json", "utf8"),
    );
    const { getCMS } = await import("../src/lib/cms.ts");
    const payload = await getCMS();
    try {
      const doc = await payload.findByID({
        collection: "media",
        id: expected.id,
        locale: "en",
        overrideAccess: true,
        showHiddenFields: true,
      });
      assert.equal(doc.contentSha256, null);
      assert.equal(doc.filename, expected.filename);
      assert.equal(doc.createdAt, expected.created_at);
      assert.equal(doc.updatedAt, expected.updated_at);
      assert.equal(doc.url, expected.url);
      const versions = await payload.findVersions({
        collection: "media",
        where: { parent: { equals: doc.id } },
        overrideAccess: true,
        showHiddenFields: true,
      });
      assert.equal(versions.totalDocs, 1);
      assert.equal(versions.docs[0].id, expected.version);
      assert.equal(versions.docs[0].version.contentSha256, null);
      const { GET } =
        await import("../src/app/api/media/file/[filename]/route.ts");
      const response = await GET(new Request(expected.url), {
        params: Promise.resolve({ filename: expected.filename }),
      });
      assert.equal(response.status, 200);
      const bytes = Buffer.from(await response.arrayBuffer());
      assert.equal(bytes.length, expected.size);
      assert.equal(
        createHash("sha256").update(bytes).digest("hex"),
        expected.hash,
      );
      assert.deepEqual(
        bytes,
        await readFile(`/data/media/${expected.filename}`),
      );
    } finally {
      await payload.destroy();
    }
  },
);
