import test from "node:test";
import assert from "node:assert/strict";
import { createHash, randomUUID } from "node:crypto";
import { readFile, writeFile, readdir } from "node:fs/promises";
import sharp from "sharp";

test(
  "private S3 media retains unchanged URLs across process recreation and fails closed",
  {
    skip: process.env.CMS_S3_ACCEPTANCE !== "true",
  },
  async () => {
    const { getCMS, handleNativeCMS } = await import("../src/lib/cms.ts");
    const { projectBridgeUser } = await import("@webdock/cms-bridge/auth");
    const config = (await import("../src/lib/config.ts")).loadCMSConfig();
    const payload = await getCMS();
    const route = (
      await import("../src/app/api/media/file/[filename]/route.ts")
    ).GET;
    const get = (filename: string) =>
      route(new Request(`${config.origin}/api/media/file/${filename}`), {
        params: Promise.resolve({ filename }),
      });
    try {
      const phase = process.env.CMS_S3_PHASE ?? "write";
      if (phase !== "write") {
        const expected = JSON.parse(
          await readFile("/data/transfers/s3-media.json", "utf8"),
        );
        const response = await get(expected.filename);
        if (phase === "denied") {
          assert.equal(response.status, 404);
          return;
        }
        assert.equal(
          response.status,
          ["denied-purge", "purge"].includes(phase) ? 404 : 200,
        );
        if (!["denied-purge", "purge"].includes(phase))
          assert.equal(
            createHash("sha256")
              .update(Buffer.from(await response.arrayBuffer()))
              .digest("hex"),
            expected.hash,
          );
        assert.deepEqual(await readdir("/data/media"), []);
        if (["prepare-purge", "denied-purge", "purge"].includes(phase)) {
          const doc = await payload.findByID({
            collection: "media",
            id: expected.id,
            overrideAccess: true,
            showHiddenFields: true,
            trash: true,
          });
          const identity = (value: typeof doc) => ({
            id: value.id,
            filename: value.filename,
            prefix: value.prefix,
            _objectKey: value._objectKey,
            contentSha256: value.contentSha256,
            deletedAt: value.deletedAt,
          });
          const versions = () =>
            payload.findVersions({
              collection: "media",
              where: { parent: { equals: doc.id } },
              overrideAccess: true,
              showHiddenFields: true,
              sort: "id",
              limit: 100,
            });
          if (phase === "prepare-purge") {
            const admin = await projectBridgeUser(payload, {
              instance: config.instance,
              project: config.project,
              subject: "denied-purge-fixture",
              role: "admin",
              grantEpoch: "purge",
            });
            const trashed = await payload.update({
              collection: "media",
              id: doc.id,
              data: { deletedAt: new Date().toISOString() },
              user: admin as never,
              overrideAccess: false,
              showHiddenFields: true,
            });
            assert.equal((await get(expected.filename)).status, 404);
            await writeFile(
              "/data/transfers/denied-purge.json",
              JSON.stringify({
                doc: identity(trashed),
                versions: await versions(),
              }),
            );
            return;
          }
          if (phase === "denied-purge") {
            const snapshot = JSON.parse(
              await readFile("/data/transfers/denied-purge.json", "utf8"),
            );
            await assert.rejects(
              payload.delete({
                collection: "media",
                id: doc.id,
                trash: true,
                overrideAccess: true,
                showHiddenFields: true,
              }),
            );
            const retained = await payload.findByID({
              collection: "media",
              id: doc.id,
              trash: true,
              overrideAccess: true,
              showHiddenFields: true,
            });
            assert.deepEqual(identity(retained), snapshot.doc);
            assert.deepEqual(await versions(), snapshot.versions);
            const req = await (
              await import("payload")
            ).createLocalReq(
              {
                req: {
                  headers: new Headers(),
                  signal: AbortSignal.timeout(5000),
                },
              },
              payload,
            );
            const handler =
              payload.collections.media.config.upload.handlers?.[0];
            assert.ok(handler);
            const bytes = await handler(req, {
              doc: retained,
              params: { collection: "media", filename: expected.filename },
            });
            assert.equal(bytes?.status, 200);
            assert.equal(
              createHash("sha256")
                .update(Buffer.from(await bytes!.arrayBuffer()))
                .digest("hex"),
              expected.hash,
            );
            return;
          }
          await payload.delete({
            collection: "media",
            id: expected.id,
            trash: true,
            overrideAccess: true,
          });
          assert.equal((await get(expected.filename)).status, 404);
          const req = await (
            await import("payload")
          ).createLocalReq({ req: { headers: new Headers() } }, payload);
          const handler = payload.collections.media.config.upload.handlers?.[0];
          assert.ok(handler);
          const removed = await handler(req, {
            doc,
            params: { collection: "media", filename: expected.filename },
          });
          assert.equal(removed?.status, 404);
        }
        return;
      }
      const actor = await projectBridgeUser(payload, {
        instance: config.instance,
        project: config.project,
        subject: `s3-${randomUUID()}`,
        role: "admin",
        grantEpoch: "s3",
      });
      const png = await sharp({
        create: { width: 8, height: 8, channels: 3, background: "#abc" },
      })
        .png()
        .toBuffer();
      const bytes = Buffer.concat([
        png,
        Buffer.alloc(10 * 1024 * 1024 - png.length),
      ]);
      const form = new FormData();
      form.set(
        "data",
        JSON.stringify({
          module: "media",
          id: "new",
          locale: "en",
          data: {
            alt: "Durable media",
            prefix: "foreign/path",
            _objectKey: "foreign",
            contentSha256: "0".repeat(64),
          },
        }),
      );
      form.set(
        "file",
        new File([new Uint8Array(bytes)], "upload.png", { type: "image/png" }),
      );
      const rejected = await handleNativeCMS(
        new Request(`${config.origin}/api/cms`, {
          method: "POST",
          headers: { origin: config.origin },
          body: form,
        }),
        actor as never,
      );
      assert.equal(rejected.status, 400);
      form.set(
        "data",
        JSON.stringify({
          module: "media",
          id: "new",
          locale: "en",
          data: { alt: "Durable media" },
        }),
      );
      const result = await handleNativeCMS(
        new Request(`${config.origin}/api/cms`, {
          method: "POST",
          headers: { origin: config.origin },
          body: form,
        }),
        actor as never,
      );
      const body = await result.json();
      assert.equal(result.status, 201, JSON.stringify(body));
      const doc = await payload.findByID({
        collection: "media",
        id: body.doc.id,
        locale: "en",
        overrideAccess: true,
        showHiddenFields: true,
      });
      const hash = createHash("sha256").update(bytes).digest("hex");
      assert.equal(doc.contentSha256, hash);
      assert.equal(doc.prefix, "");
      assert.equal(doc._objectKey ?? null, null);
      assert.equal(doc.url, `${config.origin}/api/media/file/${doc.filename}`);
      assert.equal(config.mediaStorage.kind, "s3");
      if (config.mediaStorage.kind !== "s3")
        throw Error("Wrong fixture backend");
      const anonymous = await fetch(
        `${config.mediaStorage.endpoint}/${config.mediaStorage.bucket}/${config.mediaStorage.prefix}/${doc.filename}`,
        { signal: AbortSignal.timeout(5000) },
      );
      assert.ok([403, 404].includes(anonymous.status));
      await anonymous.body?.cancel();
      const response = await get(doc.filename!);
      assert.equal(response.status, 200);
      assert.deepEqual(Buffer.from(await response.arrayBuffer()), bytes);
      assert.deepEqual(await readdir("/data/media"), []);
      // Trashing retains bytes; restoring metadata makes the same verified URL readable again.
      await payload.update({
        collection: "media",
        id: doc.id,
        data: { deletedAt: new Date().toISOString() },
        user: actor as never,
        overrideAccess: false,
      });
      assert.equal((await get(doc.filename!)).status, 404);
      await payload.update({
        collection: "media",
        id: doc.id,
        data: { deletedAt: null },
        trash: true,
        user: actor as never,
        overrideAccess: false,
      });
      assert.equal((await get(doc.filename!)).status, 200);
      await payload.db.drizzle.execute(
        (await import("@payloadcms/db-postgres"))
          .sql`UPDATE app.media SET content_sha256=${"0".repeat(64)} WHERE id=${doc.id}`,
      );
      assert.equal((await get(doc.filename!)).status, 404);
      await payload.db.drizzle.execute(
        (await import("@payloadcms/db-postgres"))
          .sql`UPDATE app.media SET content_sha256=${hash} WHERE id=${doc.id}`,
      );
      await writeFile(
        "/data/transfers/s3-media.json",
        JSON.stringify({ id: doc.id, filename: doc.filename, hash }),
      );
      const { verifyMediaBinding } =
        await import("../src/lib/media-storage.ts");
      if (config.mediaStorage.kind !== "s3")
        throw Error("Wrong fixture backend");
      await assert.rejects(
        verifyMediaBinding({
          ...config,
          mediaStorage: {
            ...config.mediaStorage,
            bucket: `${config.mediaStorage.bucket}-other`,
          },
        }),
      );
    } finally {
      await payload.destroy();
    }
  },
);
