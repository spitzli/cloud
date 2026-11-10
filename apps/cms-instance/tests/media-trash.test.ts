import test from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { readFile } from "node:fs/promises";
import sharp from "sharp";
test(
  "managed media removal retains bytes through rollback and concurrent deletes",
  { skip: process.env.CMS_ACCEPTANCE_RUN !== "true" },
  async (t) => {
    const { getCMS, handleNativeCMS } = await import("../src/lib/cms.ts");
    const { projectBridgeUser } = await import("@spitzli/cms-bridge/auth");
    const environment = (await import("../src/lib/config.ts")).loadCMSConfig();
    const payload = await getCMS();
    const admin = await projectBridgeUser(payload, {
      instance: environment.instance,
      project: environment.project,
      subject: `trash-${randomUUID()}`,
      role: "admin",
      grantEpoch: "trash",
    });
    const actor = admin as never,
      editor = { ...admin, role: "editor" } as never;
    const bytes = await sharp({
      create: { width: 4, height: 4, channels: 3, background: "#abc" },
    })
      .png()
      .toBuffer();
    const call = (method: string, data: unknown, user = actor) =>
      handleNativeCMS(
        new Request(`${environment.origin}/api/cms`, {
          method,
          headers: {
            origin: environment.origin,
            "content-type": "application/json",
          },
          body: JSON.stringify(data),
        }),
        user,
      );
    const upload = async () => {
      const form = new FormData();
      form.set(
        "data",
        JSON.stringify({
          module: "media",
          id: "new",
          locale: "en",
          data: { alt: "Retained media" },
        }),
      );
      form.set(
        "file",
        new File([new Uint8Array(bytes)], "test.png", { type: "image/png" }),
      );
      const response = await handleNativeCMS(
        new Request(`${environment.origin}/api/cms`, {
          method: "POST",
          headers: { origin: environment.origin },
          body: form,
        }),
        actor,
      );
      const result = await response.json();
      assert.equal(response.status, 201, JSON.stringify(result));
      return result.doc;
    };
    const remove = (doc: { id: number; updatedAt: string }, user = actor) =>
      call(
        "DELETE",
        {
          module: "media",
          id: String(doc.id),
          locale: "en",
          updatedAt: doc.updatedAt,
        },
        user,
      );
    const publicMedia = async (filename: string) =>
      (await import("../src/app/api/media/file/[filename]/route.ts")).GET(
        new Request(`${environment.origin}/api/media/file/${filename}`),
        { params: Promise.resolve({ filename }) },
      );
    try {
      await t.test(
        "an actual database rollback after a failed delete commit preserves original bytes and record",
        async () => {
          const media = await upload();
          const commit = payload.db.commitTransaction;
          try {
            payload.db.commitTransaction = async () => {
              throw Object.assign(
                Error("Injected serialization abort before commit"),
                { code: "40001" },
              );
            };
            const response = await remove(media);
            assert.equal(response.status, 409);
          } finally {
            payload.db.commitTransaction = commit;
          }
          const record = await payload.findByID({
            collection: "media",
            id: media.id,
            overrideAccess: true,
            locale: "en",
          });
          assert.equal(record.filename, media.filename);
          assert.deepEqual(
            await readFile(`/data/media/${media.filename}`),
            bytes,
          );
          assert.equal((await publicMedia(media.filename)).status, 200);
        },
      );
      await t.test(
        "concurrent admin removals trash metadata once and keep the underlying raster",
        async () => {
          const media = await upload();
          assert.equal((await remove(media, editor)).status, 403);
          const results = await Promise.all([remove(media), remove(media)]);
          assert.equal(
            results.filter((response) => response.status === 200).length,
            1,
          );
          assert.ok(
            results.some((response) =>
              [403, 404, 409].includes(response.status),
            ),
          );
          const record = await payload.findByID({
            collection: "media",
            id: media.id,
            overrideAccess: true,
            trash: true,
            locale: "en",
          });
          assert.ok(record.deletedAt);
          assert.deepEqual(
            await readFile(`/data/media/${media.filename}`),
            bytes,
          );
          assert.equal((await publicMedia(media.filename)).status, 404);
          const listing = await (
            await handleNativeCMS(
              new Request(
                `${environment.origin}/api/cms?module=media&locale=en`,
              ),
              actor,
            )
          ).json();
          assert.equal(
            listing.docs.some((doc: { id: number }) => doc.id === media.id),
            false,
          );
        },
      );
      await t.test(
        "native local metadata updates cannot let editors trash or restore media",
        async () => {
          const media = await upload();
          await assert.rejects(
            payload.update({
              collection: "media",
              id: media.id,
              overrideAccess: false,
              user: editor,
              data: { deletedAt: new Date().toISOString() },
            }),
          );
          const response = await call(
            "POST",
            {
              module: "media",
              id: String(media.id),
              locale: "en",
              updatedAt: media.updatedAt,
              data: { alt: "Updated metadata" },
            },
            editor,
          );
          const edited = await response.json();
          assert.equal(response.status, 200, JSON.stringify(edited));
          assert.deepEqual(
            await readFile(`/data/media/${media.filename}`),
            bytes,
          );
          assert.equal((await remove(edited.doc)).status, 200);
          await assert.rejects(
            payload.update({
              collection: "media",
              id: media.id,
              overrideAccess: false,
              user: editor,
              trash: true,
              data: { deletedAt: null },
            }),
          );
        },
      );
      await t.test(
        "normal page deletion keeps the existing administrator hard-delete behavior",
        async () => {
          const created = await call("POST", {
            module: "pages",
            id: "new",
            locale: "en",
            mode: "publish",
            data: {
              title: "Delete page fixture",
              slug: `page-trash-${randomUUID()}`,
            },
          });
          const result = await created.json();
          assert.equal(created.status, 201);
          const response = await call("DELETE", {
            module: "pages",
            id: String(result.doc.id),
            locale: "en",
            updatedAt: result.doc.updatedAt,
          });
          assert.equal(response.status, 200);
          await assert.rejects(
            payload.findByID({
              collection: "pages",
              id: result.doc.id,
              overrideAccess: true,
              trash: true,
            }),
          );
        },
      );
    } finally {
      await payload.destroy();
    }
  },
);
