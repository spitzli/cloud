import test from "node:test";
import assert from "node:assert/strict";
import { randomBytes, randomUUID, createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
import sharp from "sharp";
import { createBridgeTransport } from "@webdock/cms-bridge/client";
const phase = process.env.CMS_IMAGE_ACCEPTANCE_PHASE;
const expectedPath = "/data/acceptance-image.json";
test(
  "immutable image serves exact 10 MiB media and retains page/media across recreation",
  {
    skip: !["write", "read", "binding-rejection"].includes(phase ?? ""),
    timeout: 120_000,
  },
  async () => {
    const origin = new URL(process.env.CMS_IMAGE_TEST_ORIGIN!);
    assert.ok(
      ["cms", "localhost", "127.0.0.1"].includes(origin.hostname),
      "Disposable fixture origin only.",
    );
    if (phase === "binding-rejection") {
      for (let i = 0; i < 3; i++) {
        let response: Response | undefined;
        for (let attempt = 0; attempt < 100; attempt++) {
          try {
            response = await fetch(new URL("/api/health", origin), {
              signal: AbortSignal.timeout(5000),
            });
            break;
          } catch {}
          await new Promise((resolve) => setTimeout(resolve, 100));
        }
        assert.equal(response?.status, 503);
        assert.deepEqual(await response!.json(), { status: "unavailable" });
      }
      return;
    }
    let ready = false;
    for (let count = 0; count < 100; count++) {
      try {
        ready = (await fetch(new URL("/api/health", origin))).ok;
      } catch {}
      if (ready) break;
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    assert.equal(ready, true, "Image health must verify DB and media volume.");
    const hash = (bytes: Buffer) =>
      createHash("sha256").update(bytes).digest("hex");
    if (process.env.CMS_LEGACY_ACCEPTANCE === "true") {
      const legacy = JSON.parse(
        await readFile("/data/transfers/legacy-media.json", "utf8"),
      );
      const response = await fetch(
        new URL(`/api/media/file/${legacy.filename}`, origin),
      );
      assert.equal(response.status, 200);
      assert.equal(
        hash(Buffer.from(await response.arrayBuffer())),
        legacy.hash,
      );
    }
    if (phase === "read") {
      const expected = JSON.parse(await readFile(expectedPath, "utf8"));
      const page = await fetch(
        new URL(`/api/content/pages/${expected.slug}`, origin),
      );
      assert.equal(page.status, 200);
      assert.equal((await page.json()).title, expected.title);
      const image = await fetch(
        new URL(`/api/media/file/${expected.filename}`, origin),
      );
      assert.equal(image.status, 200);
      assert.equal(
        hash(Buffer.from(await image.arrayBuffer())),
        expected.sha256,
      );
      return;
    }
    let role = "admin",
      frames = 0;
    const authority = () => ({
      authorization: `Bearer ${process.env.CMS_BRIDGE_SECRET}`,
      "x-webdock-instance": process.env.CMS_INSTANCE_ID!,
      "x-webdock-project": process.env.CMS_PROJECT_ID!,
      "x-webdock-subject": `image-${process.env.CMS_INSTANCE_ID}`,
      "x-webdock-role": role,
      "x-webdock-grant-epoch": "1",
    });
    const transport = createBridgeTransport(async (request) => {
      frames++;
      const response = await fetch(new URL(request.path, origin), {
        method: request.method,
        headers: {
          ...authority(),
          ...(request.body ? { "content-type": "application/json" } : {}),
        },
        ...(request.body ? { body: JSON.stringify(request.body) } : {}),
      });
      return { status: response.status, body: await response.json() };
    });
    const save = async (body: unknown) => {
      const response = await transport("/api/cms", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
      });
      const result = await response.json();
      assert.ok(response.ok, JSON.stringify(result));
      return result.doc;
    };
    const slug = `image-${randomUUID()}`,
      title = "Independent image acceptance";
    const draft = await save({
      module: "pages",
      id: "new",
      locale: "en",
      mode: "draft",
      data: {
        title,
        slug,
        content: [{ blockType: "text", text: "Chunked bridge acceptance" }],
      },
    });
    assert.equal(
      (await fetch(new URL(`/api/content/pages/${slug}`, origin))).status,
      404,
    );
    await save({
      module: "pages",
      id: String(draft.id),
      locale: "en",
      mode: "publish",
      updatedAt: draft.updatedAt,
      data: {
        title,
        slug,
        content: [{ blockType: "text", text: "Chunked bridge acceptance" }],
      },
    });
    assert.equal(
      (await fetch(new URL(`/api/content/pages/${slug}`, origin))).status,
      200,
    );
    role = "reader";
    assert.equal(
      (await (await transport("/api/cms")).json()).user.role,
      "reader",
    );
    await assert.rejects(
      transport("/api/cms", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: "{}",
      }),
      /Read only/,
    );
    role = "admin";
    const png = await sharp(randomBytes(1800 * 1800 * 3), {
      raw: { width: 1800, height: 1800, channels: 3 },
    })
      .png()
      .toBuffer();
    assert.ok(png.length < 10 * 1024 * 1024);
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
        data: { alt: "Exact ten MiB raster" },
      }),
    );
    form.set(
      "file",
      new File([new Uint8Array(bytes)], "../../injected.png", {
        type: "image/png",
      }),
    );
    const uploaded = await transport("/api/cms", {
      method: "POST",
      body: form,
    });
    const media = await uploaded.json();
    assert.equal(uploaded.status, 201, JSON.stringify(media));
    assert.match(media.doc.filename, /^[a-f0-9-]{36}\.png$/);
    const descriptor = await (
      await transport(`/api/cms?module=media&id=${media.doc.id}&locale=en`)
    ).json();
    assert.equal(
      descriptor.fields.some((field: { name: string }) =>
        ["contentSha256", "prefix", "_objectKey"].includes(field.name),
      ),
      false,
    );
    const image = await fetch(
      new URL(`/api/media/file/${media.doc.filename}`, origin),
    );
    assert.equal(image.status, 200);
    assert.equal(hash(Buffer.from(await image.arrayBuffer())), hash(bytes));
    assert.equal((await fetch(new URL("/api/cms", origin))).status, 401);
    assert.equal((await fetch(new URL("/api/pages", origin))).status, 404);
    assert.equal(
      (
        await fetch(new URL("/api/webdock/cms?request=e30", origin), {
          headers: { ...authority(), "x-webdock-instance": "other-binding" },
        })
      ).status,
      403,
    );
    await writeFile(
      expectedPath,
      JSON.stringify({
        slug,
        title,
        filename: media.doc.filename,
        sha256: hash(bytes),
      }),
      { mode: 0o600 },
    );
    assert.ok(
      frames > 800,
      "The image must exercise chunked transport, not a direct upload shortcut.",
    );
  },
);
