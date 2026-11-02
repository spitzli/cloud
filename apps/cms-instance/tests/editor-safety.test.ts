import test from "node:test";
import assert from "node:assert/strict";
import { randomUUID, createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import sharp from "sharp";
test(
  "isolated editor file safety, unpublishing and per-locale privacy",
  { skip: process.env.CMS_ACCEPTANCE_RUN !== "true" },
  async (t) => {
    const { getCMS, handleNativeCMS } = await import("../src/lib/cms.ts");
    const { publishedPage } = await import("../src/lib/public.ts");
    const { projectBridgeUser } = await import("@webdock/cms-bridge/auth");
    const config = (await import("../src/lib/config.ts")).loadCMSConfig();
    const payload = await getCMS();
    const user = await projectBridgeUser(payload, {
      instance: config.instance,
      project: config.project,
      subject: `safety-${randomUUID()}`,
      role: "editor",
      grantEpoch: "safety",
    });
    const actor = user as never;
    const call = (data: unknown, acting = actor) =>
      handleNativeCMS(
        new Request(`${config.origin}/api/cms`, {
          method: "POST",
          headers: {
            origin: config.origin,
            "content-type": "application/json",
          },
          body: JSON.stringify(data),
        }),
        acting,
      );
    const read = async (id: string, locale = "en") =>
      (
        await (
          await handleNativeCMS(
            new Request(
              `${config.origin}/api/cms?module=pages&id=${id}&locale=${locale}`,
            ),
            actor,
          )
        ).json()
      ).doc;
    const save = async (data: unknown) => {
      const response = await call(data);
      const result = await response.json();
      assert.ok(response.ok, JSON.stringify(result));
      return result.doc;
    };
    const upload = async (
      bytes: Buffer,
      metadata: unknown,
      mimetype = "image/png",
    ) => {
      const form = new FormData();
      form.set("data", JSON.stringify(metadata));
      form.set(
        "file",
        new File(
          [new Uint8Array(bytes)],
          mimetype === "image/avif" ? "test.avif" : "test.png",
          { type: mimetype },
        ),
      );
      return handleNativeCMS(
        new Request(`${config.origin}/api/cms`, {
          method: "POST",
          headers: { origin: config.origin },
          body: form,
        }),
        actor,
      );
    };
    try {
      await t.test(
        "an invalid replacement never removes original media bytes",
        async () => {
          const original = await sharp({
            create: { width: 3, height: 3, channels: 3, background: "#abc" },
          })
            .png()
            .toBuffer();
          const replacement = await sharp({
            create: { width: 4, height: 4, channels: 3, background: "#def" },
          })
            .png()
            .toBuffer();
          const created = await upload(original, {
            module: "media",
            id: "new",
            locale: "en",
            data: { alt: "Original image" },
          });
          const media = await created.json();
          assert.equal(created.status, 201, JSON.stringify(media));
          const originalBytes = async () => {
            if (config.mediaStorage.kind === "local")
              return readFile(`/data/media/${media.doc.filename}`);
            const { GET } =
              await import("../src/app/api/media/file/[filename]/route.ts");
            const response = await GET(
              new Request(
                `${config.origin}/api/media/file/${media.doc.filename}`,
              ),
              { params: Promise.resolve({ filename: media.doc.filename }) },
            );
            assert.equal(response.status, 200);
            return Buffer.from(await response.arrayBuffer());
          };
          const failed = await upload(replacement, {
            module: "media",
            id: String(media.doc.id),
            locale: "en",
            updatedAt: media.doc.updatedAt,
            data: { alt: "x".repeat(501) },
          });
          assert.ok(failed.status >= 400);
          assert.equal(
            createHash("sha256")
              .update(await originalBytes())
              .digest("hex"),
            createHash("sha256").update(original).digest("hex"),
          );
          const manifest = await (
            await handleNativeCMS(
              new Request(`${config.origin}/api/cms`),
              actor,
            )
          ).json();
          assert.equal(manifest.siteURL, config.origin);
          const descriptor = await (
            await handleNativeCMS(
              new Request(
                `${config.origin}/api/cms?module=media&id=${media.doc.id}&locale=en`,
              ),
              actor,
            )
          ).json();
          assert.equal(descriptor.canReplaceFile, false);
          assert.equal(
            descriptor.fields.some((field: { name: string }) =>
              ["contentSha256", "prefix", "_objectKey"].includes(field.name),
            ),
            false,
          );
          const current = await payload.findByID({
            collection: "media",
            id: media.doc.id,
            overrideAccess: true,
            locale: "en",
          });
          assert.equal(current.filename, media.doc.filename);
          const valid = await upload(replacement, {
            module: "media",
            id: String(media.doc.id),
            locale: "en",
            updatedAt: media.doc.updatedAt,
            data: { alt: "Valid new image" },
          });
          assert.equal(valid.status, 409);
          assert.equal((await originalBytes()).length, original.length);
          const metadata = await call({
            module: "media",
            id: String(media.doc.id),
            locale: "en",
            updatedAt: media.doc.updatedAt,
            data: { alt: "Updated image description" },
          });
          assert.equal(metadata.status, 200);
          assert.deepEqual(await originalBytes(), original);
        },
      );
      await t.test(
        "a real AVIF upload reaches public media with the original bytes",
        async () => {
          const bytes = await sharp({
            create: { width: 4, height: 4, channels: 3, background: "#bcd" },
          })
            .avif()
            .toBuffer();
          const response = await upload(
            bytes,
            {
              module: "media",
              id: "new",
              locale: "en",
              data: { alt: "AVIF acceptance image" },
            },
            "image/avif",
          );
          const result = await response.json();
          assert.equal(response.status, 201, JSON.stringify(result));
          assert.match(result.doc.filename, /\.avif$/);
          const { GET } =
            await import("../src/app/api/media/file/[filename]/route.ts");
          const served = await GET(
            new Request(`${config.origin}${result.doc.url}`),
            { params: Promise.resolve({ filename: result.doc.filename }) },
          );
          assert.equal(served.status, 200);
          assert.equal(served.headers.get("content-type"), "image/avif");
          assert.deepEqual(Buffer.from(await served.arrayBuffer()), bytes);
        },
      );
      await t.test(
        "an editor can unpublish with CAS while preserving pending draft and history",
        async () => {
          const slug = `unpublish-${randomUUID()}`;
          const first = await save({
            module: "pages",
            id: "new",
            locale: "en",
            mode: "publish",
            data: {
              title: "Public title",
              slug,
              content: [{ blockType: "text", text: "Public block" }],
            },
          });
          const id = String(first.id);
          const draft = await save({
            module: "pages",
            id,
            locale: "en",
            mode: "draft",
            updatedAt: first.updatedAt,
            data: {
              title: "Pending private title",
              slug,
              content: [{ blockType: "text", text: "Pending private block" }],
            },
          });
          assert.equal(
            (await publishedPage(slug, "en"))?.title,
            "Public title",
          );
          const descriptor = await (
            await handleNativeCMS(
              new Request(
                `${config.origin}/api/cms?module=pages&id=${id}&locale=en`,
              ),
              actor,
            )
          ).json();
          assert.equal(descriptor.published, true);
          assert.equal(descriptor.localizedStatus, true);
          assert.equal(
            (
              await call({
                module: "pages",
                id,
                locale: "en",
                mode: "unpublish",
                updatedAt: "stale",
              })
            ).status,
            409,
          );
          assert.equal(
            (
              await call(
                {
                  module: "pages",
                  id,
                  locale: "en",
                  mode: "unpublish",
                  updatedAt: draft.updatedAt,
                },
                { ...user, role: "reader" } as never,
              )
            ).status,
            403,
          );
          await save({
            module: "pages",
            id,
            locale: "en",
            mode: "unpublish",
            updatedAt: draft.updatedAt,
            data: { title: "Do not overwrite the pending draft" },
          });
          assert.equal(await publishedPage(slug, "en"), null);
          assert.equal((await read(id)).title, "Pending private title");
          assert.equal(
            (await read(id)).content[0].text,
            "Pending private block",
          );
          const history = await (
            await handleNativeCMS(
              new Request(
                `${config.origin}/api/cms?module=pages&id=${id}&locale=en&operation=versions`,
              ),
              actor,
            )
          ).json();
          assert.ok(history.docs.length >= 3);
        },
      );
      await t.test(
        "publishing or unpublishing one locale never exposes or removes another locale draft",
        async () => {
          const slug = `locale-${randomUUID()}`;
          const en = await save({
            module: "pages",
            id: "new",
            locale: "en",
            mode: "draft",
            data: { title: "English private", slug },
          });
          const id = String(en.id);
          await save({
            module: "pages",
            id,
            locale: "de",
            mode: "draft",
            updatedAt: en.updatedAt,
            data: { title: "Deutsch geheim", slug },
          });
          let current = await read(id, "en");
          await save({
            module: "pages",
            id,
            locale: "en",
            mode: "publish",
            updatedAt: current.updatedAt,
            data: { title: "English public", slug },
          });
          assert.equal(
            (await publishedPage(slug, "en"))?.title,
            "English public",
          );
          assert.equal(await publishedPage(slug, "de"), null);
          current = await read(id, "en");
          await save({
            module: "pages",
            id,
            locale: "en",
            mode: "draft",
            updatedAt: current.updatedAt,
            data: { title: "English pending second draft", slug },
          });
          current = await read(id, "de");
          await save({
            module: "pages",
            id,
            locale: "de",
            mode: "publish",
            updatedAt: current.updatedAt,
            data: { title: "Deutsch öffentlich", slug },
          });
          assert.equal(
            (await publishedPage(slug, "de"))?.title,
            "Deutsch öffentlich",
          );
          assert.equal(
            (await publishedPage(slug, "en"))?.title,
            "English public",
          );
          current = await read(id, "de");
          await save({
            module: "pages",
            id,
            locale: "de",
            mode: "draft",
            updatedAt: current.updatedAt,
            data: { title: "Deutsch noch geheim", slug },
          });
          current = await read(id, "en");
          await save({
            module: "pages",
            id,
            locale: "en",
            mode: "unpublish",
            updatedAt: current.updatedAt,
          });
          assert.equal(await publishedPage(slug, "en"), null);
          assert.equal(
            (await publishedPage(slug, "de"))?.title,
            "Deutsch öffentlich",
          );
          assert.equal(
            (await read(id, "en")).title,
            "English pending second draft",
          );
          assert.equal((await read(id, "de")).title, "Deutsch noch geheim");
        },
      );
      await t.test(
        "localized global unpublishing retains the other published language and both pending drafts",
        async () => {
          const readGlobal = async (locale: string) =>
            (
              await (
                await handleNativeCMS(
                  new Request(
                    `${config.origin}/api/cms?module=settings&locale=${locale}`,
                  ),
                  actor,
                )
              ).json()
            ).doc;
          const liveGlobal = (locale: "en" | "de") =>
            payload.findGlobal({
              slug: "settings",
              draft: false,
              locale,
              fallbackLocale: false,
              user: actor,
              overrideAccess: false,
            });
          let current = await readGlobal("en");
          await save({
            module: "settings",
            locale: "en",
            mode: "publish",
            updatedAt: current.updatedAt ?? null,
            data: { title: "English public settings" },
          });
          current = await readGlobal("de");
          await save({
            module: "settings",
            locale: "de",
            mode: "publish",
            updatedAt: current.updatedAt ?? null,
            data: { title: "Deutsche öffentliche Einstellungen" },
          });
          current = await readGlobal("en");
          await save({
            module: "settings",
            locale: "en",
            mode: "draft",
            updatedAt: current.updatedAt,
            data: { title: "English pending settings" },
          });
          current = await readGlobal("de");
          await save({
            module: "settings",
            locale: "de",
            mode: "draft",
            updatedAt: current.updatedAt,
            data: { title: "Deutsche geheime Einstellungen" },
          });
          assert.equal(
            (await liveGlobal("en")).title,
            "English public settings",
          );
          assert.equal(
            (await liveGlobal("de")).title,
            "Deutsche öffentliche Einstellungen",
          );
          current = await readGlobal("en");
          await save({
            module: "settings",
            locale: "en",
            mode: "unpublish",
            updatedAt: current.updatedAt,
          });
          assert.equal((await liveGlobal("en"))._status, "draft");
          assert.equal((await liveGlobal("de"))._status, "published");
          assert.equal(
            (await liveGlobal("de")).title,
            "Deutsche öffentliche Einstellungen",
          );
          assert.equal(
            (await readGlobal("en")).title,
            "English pending settings",
          );
          assert.equal(
            (await readGlobal("de")).title,
            "Deutsche geheime Einstellungen",
          );
        },
      );
    } finally {
      await payload.destroy();
    }
  },
);
