import test from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import type { Socket } from "node:net";
import { buildConfig } from "payload";
import { cmsFields } from "@webdock/cms-ui/schema";
import { media } from "../src/lib/content.ts";

const storageModule = (file: string) =>
  import(new URL(file, import.meta.resolve("@payloadcms/storage-s3")).href);
const cloudModule = (file: string) =>
  import(
    new URL(file, import.meta.resolve("@payloadcms/plugin-cloud-storage")).href
  );
const collection = {
  slug: "media",
  fields: [],
  upload: {},
  labels: { singular: "Media" },
};
const doc = { id: 1, filename: "fixture.png", prefix: "", _objectKey: null };
const logger = { error() {} };

test("native editor omits integrity and storage identity fields from sanitized config", async () => {
  const config = await buildConfig({
    secret: "s".repeat(32),
    collections: [media("/tmp/unused-cms-media")],
    db: { init: () => ({}) } as never,
    admin: { disable: true },
    telemetry: false,
  });
  const fields = cmsFields(
    config.collections.find((c) => c.slug === "media")!.fields,
  );
  assert.equal(
    fields.some((f) => f.name === "contentSha256"),
    false,
  );
});

test("provider delete failure rejects the actual cloud hook with the exact object reference", async () => {
  const { getAfterDeleteHook } = await cloudModule("./hooks/afterDelete.js");
  const paths: string[] = [];
  const denied = Object.assign(new Error("Denied fixture purge"), {
    name: "AccessDenied",
  });
  const hook = getAfterDeleteHook({
    adapter: {
      handleDelete: async ({
        storageFilePath,
      }: {
        storageFilePath: string;
      }) => {
        paths.push(storageFilePath);
        throw denied;
      },
    },
    collection,
    collectionPrefix: "cms/owned/media",
    useCompositePrefixes: true,
  });
  await assert.rejects(
    hook({ doc, req: { payload: { logger } } }),
    (err) => err === denied,
  );
  assert.deepEqual(paths, ["cms/owned/media/fixture.png"]);
  assert.equal(doc.filename, "fixture.png");
});

for (const phase of ["HEAD", "GET", "body", "already-aborted"]) {
  test(
    `S3 ${phase} cancellation settles and releases the provider socket`,
    { timeout: 3000 },
    async () => {
      const { S3 } = await import("@aws-sdk/client-s3");
      const { getFile } = await storageModule("./getFile.js");
      const sockets = new Set<Socket>();
      let requests = 0;
      const server = createServer((req, res) => {
        requests++;
        if (phase === "HEAD" || phase === "already-aborted") return;
        if (req.method === "HEAD") {
          res.writeHead(200, {
            "content-length": "3",
            "content-type": "image/png",
          });
          res.end();
        } else if (phase === "body") {
          res.writeHead(200, {
            "content-length": "3",
            "content-type": "image/png",
          });
          res.write("a");
        }
      });
      server.on("connection", (socket) => {
        sockets.add(socket);
        socket.on("close", () => sockets.delete(socket));
      });
      await new Promise<void>((resolve) =>
        server.listen(0, "127.0.0.1", resolve),
      );
      const address = server.address() as { port: number };
      const client = new S3({
        endpoint: `http://127.0.0.1:${address.port}`,
        region: "test",
        credentials: {
          accessKeyId: "owned-fixture",
          secretAccessKey: "owned-fixture",
        },
        forcePathStyle: true,
        maxAttempts: 1,
        requestHandler: {
          connectionTimeout: 200,
          requestTimeout: 2000,
          throwOnRequestTimeout: true,
        },
      });
      const controller = new AbortController();
      if (phase === "already-aborted") controller.abort();
      const timer = setTimeout(() => controller.abort(), 100);
      try {
        const operation = (async () => {
          const response = await getFile({
            client,
            bucket: "owned-fixture",
            collection,
            doc,
            filename: doc.filename,
            signedDownloads: false,
            req: {
              headers: new Headers(),
              signal: controller.signal,
              payload: { logger },
            },
          });
          if (response.status === 200)
            await response.arrayBuffer().catch(() => {});
          return response.status;
        })();
        const result = await Promise.race([
          operation,
          new Promise((resolve) => setTimeout(() => resolve("pending"), 500)),
        ]);
        assert.notEqual(result, "pending");
        if (phase === "already-aborted") assert.equal(requests, 0);
        for (let i = 0; i < 20 && sockets.size; i++)
          await new Promise((resolve) => setTimeout(resolve, 10));
        assert.equal(sockets.size, 0);
      } finally {
        clearTimeout(timer);
        client.destroy();
        for (const socket of sockets) socket.destroy();
        await new Promise<void>((resolve) => server.close(() => resolve()));
      }
    },
  );
}

test(
  "configured S3 adapter bounds stalled HEAD and body without a caller signal",
  { timeout: 35000 },
  async () => {
    const { mediaStorageAdapter } = await import("../src/lib/media-storage.ts");
    const { loadCMSConfig } = await import("../src/lib/config.ts");
    await Promise.all(
      ["HEAD", "body"].map(async (phase) => {
        const sockets = new Set<Socket>();
        let requests = 0;
        const server = createServer((req, res) => {
          requests++;
          if (phase === "HEAD") return;
          res.writeHead(200, {
            "content-length": "3",
            "content-type": "image/png",
          });
          if (req.method === "HEAD") res.end();
          else res.write("a");
        });
        server.on("connection", (socket) => {
          sockets.add(socket);
          socket.on("close", () => sockets.delete(socket));
        });
        await new Promise<void>((resolve) =>
          server.listen(0, "127.0.0.1", resolve),
        );
        const address = server.address() as { port: number };
        const config = loadCMSConfig({
          CMS_INSTANCE_ID: "timeout-fixture",
          CMS_PROJECT_ID: "timeout-project",
          CMS_ENVIRONMENT_ID: "test",
          CMS_ORIGIN: "https://cms.invalid",
          CMS_SITE_NAME: "Fixture",
          PAYLOAD_SECRET: "p".repeat(32),
          CMS_BRIDGE_SECRET: "b".repeat(32),
          DATABASE_URL: "postgresql://offline:offline@127.0.0.1/offline",
          CMS_MEDIA_STORAGE: "s3",
          CMS_MEDIA_S3_RESOURCE_ID: "bbba0aa0-2f38-4fb0-a870-81808ccbd1b0",
          CMS_MEDIA_S3_ENDPOINT: `http://127.0.0.1:${address.port}`,
          CMS_MEDIA_S3_LOOPBACK_TEST: "true",
          CMS_MEDIA_S3_BUCKET: "timeout-fixture",
          CMS_MEDIA_S3_REGION: "test",
          CMS_MEDIA_S3_ACCESS_KEY_ID: "owned-fixture",
          CMS_MEDIA_S3_SECRET_ACCESS_KEY: "owned-fixture",
        });
        try {
          const configured = await mediaStorageAdapter(config).init({
            collections: [media("/tmp/unused-cms-media")],
            secret: "s".repeat(32),
            db: { init: () => ({}) } as never,
          });
          const upload = configured.collections![0].upload;
          assert.ok(upload && typeof upload === "object");
          const handler = upload.handlers![0];
          const started = performance.now();
          const operation = (async () => {
            const response = await handler(
              { headers: new Headers(), payload: { logger } } as never,
              { doc, params: { collection: "media", filename: doc.filename } },
            );
            assert.ok(response);
            if (phase === "body") {
              assert.equal(response.status, 200);
              await assert.rejects(response.arrayBuffer());
            } else assert.equal(response.status, 500);
          })();
          await Promise.race([
            operation,
            new Promise((_, reject) =>
              setTimeout(
                () =>
                  reject(Error("Provider operation exceeded hard deadline")),
                32000,
              ),
            ),
          ]);
          assert.ok(performance.now() - started < 31500);
          assert.ok(requests > 0);
          for (let i = 0; i < 50 && sockets.size; i++)
            await new Promise((resolve) => setTimeout(resolve, 10));
          assert.equal(sockets.size, 0);
        } finally {
          for (const socket of sockets) socket.destroy();
          await new Promise<void>((resolve) => server.close(() => resolve()));
        }
      }),
    );
  },
);

test("abort racing with GET body arrival destroys the stream without an unhandled error", async () => {
  const { PassThrough } = await import("node:stream");
  const { getFile } = await storageModule("./getFile.js");
  const controller = new AbortController();
  const body = new PassThrough();
  const response = await getFile({
    client: {
      headObject: async () => ({ ContentLength: 3 }),
      getObject: async () => {
        controller.abort();
        return { Body: body };
      },
    },
    bucket: "owned-fixture",
    collection,
    doc,
    filename: doc.filename,
    signedDownloads: false,
    req: {
      headers: new Headers(),
      signal: controller.signal,
      payload: { logger },
    },
  });
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(response.status, 500);
  assert.equal(body.destroyed, true);
});
