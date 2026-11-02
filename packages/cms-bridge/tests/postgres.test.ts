import assert from "node:assert/strict";
import { randomUUID, createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { test } from "node:test";
import pg from "pg";
import { drizzle } from "drizzle-orm/node-postgres";
import { PostgresBridgeTransfers } from "../src/postgres.ts";
import { up } from "../../../apps/website/src/migrations-instance/20261018_180000_cms_bridge.ts";
import { CHUNK_BYTES, MEDIA_BYTES } from "../src/types.ts";
import type { BridgeScope } from "../src/types.ts";

// Named steps share one disposable container; a failure names the step it happened in.
const step = async (name: string, run: () => Promise<void>) => {
  try {
    await run();
  } catch (error) {
    throw new Error(`${name} failed`, { cause: error });
  }
};
const scope: BridgeScope = {
  instance: "binding-1",
  project: "project-1",
  subject: "subject-1",
  role: "editor",
  grantEpoch: "generation-1",
};
const digest = (bytes: Buffer) =>
  createHash("sha256").update(bytes).digest("hex");
const descriptor = Buffer.from(
  JSON.stringify({ method: "GET", path: "/api/cms" }),
).toString("base64url");
const encodedRequest = (size = 30_000, media = false) =>
  Buffer.from(
    JSON.stringify({
      method: "POST",
      path: "/api/cms",
      contentType: media
        ? "multipart/form-data; boundary=test"
        : "application/json",
      bodyBase64: Buffer.alloc(size, 97).toString("base64"),
    }),
  );

test(
  "Postgres transfers survive independent handlers and enforce scoped durable bounds",
  { timeout: 120_000 },
  async () => {
    const container = `webdock-bridge-test-${randomUUID().slice(0, 8)}`;
    execFileSync(
      "docker",
      [
        "run",
        "--detach",
        "--rm",
        "--name",
        container,
        "-e",
        "POSTGRES_PASSWORD=fixture-only",
        "-p",
        "127.0.0.1::5432",
        "postgres:17-alpine",
      ],
      { stdio: "pipe" },
    );
    let pool: pg.Pool | undefined;
    let runtimePool: pg.Pool | undefined;
    try {
      for (let i = 0; i < 100; i++) {
        try {
          execFileSync(
            "docker",
            [
              "exec",
              container,
              "pg_isready",
              "-h",
              "127.0.0.1",
              "-U",
              "postgres",
            ],
            { stdio: "pipe" },
          );
          break;
        } catch {
          await new Promise((resolve) => setTimeout(resolve, 100));
        }
      }
      const port = execFileSync("docker", ["port", container, "5432/tcp"], {
        encoding: "utf8",
      })
        .trim()
        .split(":")
        .at(-1);
      pool = new pg.Pool({
        connectionString: `postgresql://postgres:fixture-only@127.0.0.1:${port}/postgres`,
        max: 4,
      });
      await pool.query(
        `CREATE ROLE webdock_website_owner; CREATE ROLE webdock_browser_website; CREATE SCHEMA webdock AUTHORIZATION webdock_website_owner; CREATE ROLE webdock_runtime LOGIN PASSWORD 'fixture-runtime'; GRANT USAGE ON SCHEMA webdock TO webdock_runtime; CREATE TABLE webdock.users (id serial PRIMARY KEY);`,
      );
      await up({ db: drizzle({ client: pool }) } as never);
      runtimePool = new pg.Pool({
        connectionString: `postgresql://webdock_runtime:fixture-runtime@127.0.0.1:${port}/postgres`,
        max: 4,
      });
      let clock = Date.now();
      const a = new PostgresBridgeTransfers(
        runtimePool,
        "webdock",
        () => clock,
      );
      const b = new PostgresBridgeTransfers(
        runtimePool,
        "webdock",
        () => clock,
      );
      const clear = () =>
        pool!.query("DELETE FROM webdock.cms_bridge_transfers");
      async function upload(bytes: Buffer, kind: "json" | "media" = "json") {
        const { transfer } = await a.begin(scope, {
          op: "begin",
          size: bytes.length,
          sha256: digest(bytes),
          kind,
        });
        for (let offset = 0; offset < bytes.length; offset += CHUNK_BYTES)
          await b.append(scope, {
            op: "append",
            transfer,
            offset,
            data: bytes
              .subarray(offset, offset + CHUNK_BYTES)
              .toString("base64"),
          });
        return transfer;
      }
      await step(
        "DML permission is granted only to the runtime role",
        async () => {
          const {
            rows: [row],
          } = await pool!.query(
            "SELECT has_table_privilege('webdock_runtime','webdock.cms_bridge_transfers','SELECT,INSERT,UPDATE,DELETE') AS allowed, has_table_privilege('public','webdock.cms_bridge_transfers','SELECT') AS public_read",
          );
          assert.equal(row.allowed, true);
          assert.equal(row.public_read, false);
        },
      );
      await step(
        "continuity, scoped denial, serialized append, digest, and replay",
        async () => {
          const bytes = encodedRequest(),
            transfer = await upload(bytes);
          const partial = await a.begin(scope, {
            op: "begin",
            size: bytes.length,
            sha256: digest(bytes),
            kind: "json",
          });
          await assert.rejects(
            b.append(scope, {
              op: "append",
              transfer: partial.transfer,
              offset: 0,
              data: "YQ==",
            }),
            /Incomplete chunk/,
          );
          for (const changed of [
            { instance: "other" },
            { project: "other" },
            { subject: "other" },
            { role: "admin" as const },
            { grantEpoch: "other" },
          ])
            await assert.rejects(
              b.commit(
                { ...scope, ...changed },
                transfer,
                "https://site.test",
                async () => Response.json({}),
              ),
              /unavailable|Unknown/,
            );
          const chunk = {
            op: "append" as const,
            transfer,
            offset: 0,
            data: bytes.subarray(0, CHUNK_BYTES).toString("base64"),
          };
          await Promise.all([a.append(scope, chunk), b.append(scope, chunk)]);
          await assert.rejects(
            a.append(scope, {
              ...chunk,
              data: Buffer.alloc(CHUNK_BYTES).toString("base64"),
            }),
            /Changed/,
          );
          let executions = 0;
          const execute = async (request: Request) => {
            executions++;
            assert.equal((await request.arrayBuffer()).byteLength, 30_000);
            return Response.json({ saved: true });
          };
          const result = await b.commit(
            scope,
            transfer,
            "https://site.test",
            execute,
          );
          assert.deepEqual(
            await a.commit(scope, transfer, "https://site.test", execute),
            result,
          );
          assert.equal(executions, 1);
          const read = await a.read(scope, transfer, 0);
          assert.equal(
            JSON.parse(Buffer.from(read.data, "base64").toString()).status,
            200,
          );
          assert.deepEqual(
            await b.commit(scope, transfer, "https://site.test", execute),
            result,
          );
          const bad = await a.begin(scope, {
            op: "begin",
            size: bytes.length,
            sha256: "0".repeat(64),
            kind: "json",
          });
          await assert.rejects(
            a.commit(scope, bad.transfer, "https://site.test", execute),
            /Incomplete/,
          );
          for (let offset = 0; offset < bytes.length; offset += CHUNK_BYTES)
            await a.append(scope, {
              op: "append",
              transfer: bad.transfer,
              offset,
              data: bytes
                .subarray(offset, offset + CHUNK_BYTES)
                .toString("base64"),
            });
          await assert.rejects(
            b.commit(scope, bad.transfer, "https://site.test", execute),
            /digest/,
          );
          await clear();
        },
      );
      await step(
        "executing is committed before effects and ambiguity never retries",
        async () => {
          const transfer = await upload(encodedRequest());
          let executions = 0;
          await assert.rejects(
            a.commit(scope, transfer, "https://site.test", async () => {
              executions++;
              await assert.rejects(
                b.commit(scope, transfer, "https://site.test", async () => {
                  executions++;
                  return Response.json({});
                }),
                /reconciliation_required/,
              );
              throw Error("connection lost after mutation");
            }),
            /connection lost/,
          );
          await assert.rejects(
            b.commit(scope, transfer, "https://site.test", async () => {
              executions++;
              return Response.json({});
            }),
            /reconciliation_required/,
          );
          assert.equal(executions, 1);
          await clear();
        },
      );
      await step(
        "reader GET releases final response and errors release allocation",
        async () => {
          const reader = { ...scope, role: "reader" as const };
          await assert.rejects(
            a.begin(reader, {
              op: "begin",
              size: 1,
              sha256: "0".repeat(64),
              kind: "json",
            }),
            /Read only/,
          );
          const result = await a.get(
            reader,
            descriptor,
            "https://site.test",
            async () => Response.json({ read: true }),
          );
          await assert.rejects(
            b.read(scope, result.transfer, 0),
            /unavailable|Unknown/,
          );
          assert.equal((await b.read(reader, result.transfer, 0)).done, true);
          await assert.rejects(
            a.read(reader, result.transfer, 0),
            /unavailable|Unknown/,
          );
          await assert.rejects(
            a.get(reader, descriptor, "https://site.test", async () => {
              throw Error("read failure");
            }),
            /read failure/,
          );
          assert.equal(
            (
              await pool!.query(
                "SELECT count(*) FROM webdock.cms_bridge_transfers",
              )
            ).rows[0].count,
            "0",
          );
        },
      );
      await step(
        "active quota is atomic across handlers, expired rows are reclaimed",
        async () => {
          const input = {
            op: "begin" as const,
            size: 1,
            sha256: "0".repeat(64),
            kind: "json" as const,
          };
          const results = await Promise.allSettled(
            Array.from({ length: 17 }, (_, i) =>
              (i % 2 ? a : b).begin(scope, input),
            ),
          );
          assert.equal(
            results.filter((r) => r.status === "fulfilled").length,
            16,
          );
          const id = (
            results.find(
              (r) => r.status === "fulfilled",
            ) as PromiseFulfilledResult<{ transfer: string }>
          ).value.transfer;
          clock += 600_001;
          await assert.rejects(
            a.append(scope, {
              op: "append",
              transfer: id,
              offset: 0,
              data: "YQ==",
            }),
            /unavailable/,
          );
          await b.begin(scope, input);
          assert.equal(
            (
              await pool!.query(
                "SELECT count(*) FROM webdock.cms_bridge_transfers",
              )
            ).rows[0].count,
            "1",
          );
          await clear();
        },
      );
      await step("receipts and reserved storage are bounded", async () => {
        const bytes = encodedRequest(1);
        for (let i = 0; i < 127; i++) {
          const id = await upload(bytes);
          await a.commit(scope, id, "https://site.test", async () =>
            Response.json({}),
          );
        }
        const last = await upload(bytes);
        await assert.rejects(
          b.begin(scope, {
            op: "begin",
            size: bytes.length,
            sha256: digest(bytes),
            kind: "json",
          }),
          /capacity/,
        );
        await b.commit(scope, last, "https://site.test", async () =>
          Response.json({}),
        );
        await assert.rejects(
          a.begin(scope, {
            op: "begin",
            size: bytes.length,
            sha256: digest(bytes),
            kind: "json",
          }),
          /capacity/,
        );
        await clear();
        const big = encodedRequest(MEDIA_BYTES, true);
        let allocated = 0;
        while (allocated < 16) {
          try {
            await a.begin(scope, {
              op: "begin",
              size: big.length,
              sha256: digest(big),
              kind: "media",
            });
            allocated++;
          } catch (error) {
            assert.match(String(error), /capacity/);
            break;
          }
        }
        assert.ok(allocated > 0 && allocated < 16);
        await clear();
      });
      await step(
        "large base64 requests and responses use bounded chunks",
        async () => {
          const bytes = encodedRequest(MEDIA_BYTES, true),
            id = await upload(bytes, "media");
          const result = await b.commit(
            scope,
            id,
            "https://site.test",
            async (request) => {
              assert.equal(
                (await request.arrayBuffer()).byteLength,
                MEDIA_BYTES,
              );
              return new Response(Buffer.alloc(100_000, 7), {
                headers: { "content-type": "image/png" },
              });
            },
          );
          const chunks: Buffer[] = [];
          for (let offset = 0; offset < result.size;) {
            const chunk = await a.read(scope, id, offset);
            chunks.push(Buffer.from(chunk.data, "base64"));
            offset = chunk.offset;
          }
          assert.equal(digest(Buffer.concat(chunks)), result.sha256);
          assert.equal(
            Buffer.from(
              JSON.parse(Buffer.concat(chunks).toString()).bodyBase64,
              "base64",
            ).length,
            100_000,
          );
          await clear();
        },
      );
    } finally {
      await runtimePool?.end();
      await pool?.end();
      execFileSync("docker", ["rm", "-f", container], { stdio: "pipe" });
    }
  },
);
