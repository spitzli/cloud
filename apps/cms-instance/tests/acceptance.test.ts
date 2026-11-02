import test from "node:test";
import assert from "node:assert/strict";
import sharp from "sharp";
import { randomUUID } from "node:crypto";
/** Run only against a fresh isolated DB migrated by the explicit migration job. */
test(
  "isolated CMS DML role edits, CAS, drafts, role ceilings and raster media",
  { skip: process.env.CMS_ACCEPTANCE_RUN !== "true" },
  async () => {
    const { getCMS, handleNativeCMS } = await import("../src/lib/cms.ts");
    const { publishedPage } = await import("../src/lib/public.ts");
    const { projectBridgeUser } = await import("@webdock/cms-bridge/auth");
    const { loadCMSConfig } = await import("../src/lib/config.ts");
    const config = loadCMSConfig();
    const payload = await getCMS();
    const subject = `acceptance-${randomUUID()}`;
    const scope = {
      instance: config.instance,
      project: config.project,
      subject,
      role: "admin" as const,
      grantEpoch: "acceptance",
    };
    const user = await projectBridgeUser(payload, scope);
    const actor = user as never;
    const call = (method: string, body?: unknown, override = actor) =>
      handleNativeCMS(
        new Request(`${config.origin}/api/cms`, {
          method,
          headers: {
            origin: config.origin,
            "content-type": "application/json",
          },
          ...(body ? { body: JSON.stringify(body) } : {}),
        }),
        override,
      );
    try {
      assert.equal(
        (await handleNativeCMS(new Request(`${config.origin}/api/cms`))).status,
        401,
      );
      const created = await call("POST", {
        module: "pages",
        id: "new",
        locale: "en",
        mode: "draft",
        data: {
          title: "Draft page",
          slug: subject,
          content: [
            {
              blockType: "text",
              heading: "Heading",
              text: "Editable structured content",
            },
          ],
        },
      });
      const initial = await created.json();
      assert.equal(created.status, 201, JSON.stringify(initial));
      assert.equal(await publishedPage(subject, "en"), null);
      const id = String(initial.doc.id);
      const denied = await call(
        "POST",
        {
          module: "pages",
          id,
          locale: "en",
          mode: "publish",
          updatedAt: initial.doc.updatedAt,
          data: { title: "Reader cannot publish" },
        },
        { ...(actor as object), role: "reader" } as never,
      );
      assert.equal(denied.status, 403);
      const published = await call("POST", {
        module: "pages",
        id,
        locale: "en",
        mode: "publish",
        updatedAt: initial.doc.updatedAt,
        data: {
          title: "Published page",
          slug: subject,
          content: [
            {
              blockType: "text",
              heading: "Heading",
              text: "Editable structured content",
            },
          ],
        },
      });
      const live = await published.json();
      assert.equal(published.status, 200, JSON.stringify(live));
      assert.equal(
        (await publishedPage(subject, "en"))?.title,
        "Published page",
      );
      const stale = await call("POST", {
        module: "pages",
        id,
        locale: "en",
        mode: "draft",
        updatedAt: initial.doc.updatedAt,
        data: { title: "Stale overwrite" },
      });
      assert.equal(stale.status, 409);
      const draft = await call("POST", {
        module: "pages",
        id,
        locale: "en",
        mode: "draft",
        updatedAt: live.doc.updatedAt,
        data: { title: "Unpublished edit", slug: subject },
      });
      const saved = await draft.json();
      assert.equal(draft.status, 200, JSON.stringify(saved));
      assert.equal(
        (await publishedPage(subject, "en"))?.title,
        "Published page",
      );
      assert.equal(
        (
          await call("GET", undefined, {
            ...(actor as object),
            role: "reader",
          } as never)
        ).status,
        200,
      );
      const forbidden = await call("POST", {
        module: "users",
        id: "new",
        data: { role: "operator" },
      });
      assert.equal(forbidden.status, 404);
      await payload.update({
        collection: "users",
        id: user.id,
        overrideAccess: true,
        context: { bootstrap: true },
        data: { role: "reader" } as never,
      });
      assert.equal((await projectBridgeUser(payload, scope)).role, "reader");
      await payload.update({
        collection: "users",
        id: user.id,
        overrideAccess: true,
        context: { bootstrap: true },
        data: { disabled: true } as never,
      });
      await assert.rejects(projectBridgeUser(payload, scope));
      const bytes = await sharp({
        create: { width: 8, height: 8, channels: 3, background: "#eed" },
      })
        .png()
        .toBuffer();
      const form = new FormData();
      form.set(
        "data",
        JSON.stringify({
          module: "media",
          id: "new",
          locale: "en",
          data: { alt: "Acceptance raster" },
        }),
      );
      form.set(
        "file",
        new File([bytes], "../../unsafe.png", { type: "image/png" }),
      );
      const uploaded = await handleNativeCMS(
        new Request(`${config.origin}/api/cms`, {
          method: "POST",
          headers: { origin: config.origin },
          body: form,
        }),
        actor,
      );
      const media = await uploaded.json();
      assert.equal(uploaded.status, 201, JSON.stringify(media));
      assert.match(media.doc.filename, /^[a-f0-9-]{36}\.png$/);
      const { GET: readMedia } =
        await import("../src/app/api/media/file/[filename]/route.ts");
      const response = await readMedia(
        new Request(`${config.origin}${media.doc.url}`),
        { params: Promise.resolve({ filename: media.doc.filename }) },
      );
      assert.equal(response.status, 200);
      assert.deepEqual(Buffer.from(await response.arrayBuffer()), bytes);
      // Runtime credentials must never be able to create schema or tables.
      const result = await payload.db.pool.query(
        "SELECT has_schema_privilege(current_user,'app','CREATE') AS can_create, rolcreatedb,rolsuper FROM pg_roles WHERE rolname=current_user",
      );
      assert.equal(result.rows[0].can_create, false);
      assert.equal(result.rows[0].rolcreatedb, false);
      assert.equal(result.rows[0].rolsuper, false);
    } finally {
      await payload.destroy();
    }
  },
);
