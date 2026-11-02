import { afterEach, expect, test } from "bun:test";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import Elysia from "elysia";
import { loadRoutes } from "../src/route-preflight";
const dirs: string[] = [];
const route = `import Elysia from 'elysia'; export default new Elysia().get('', () => 'private');`;
async function fixture(files: Record<string, string>) {
  const dir = await mkdtemp(join(import.meta.dir, ".routes-"));
  dirs.push(dir);
  for (const [name, body] of Object.entries(files)) {
    const path = join(dir, name);
    await mkdir(join(path, ".."), { recursive: true });
    await writeFile(path, body);
  }
  return dir;
}
afterEach(async () => {
  await Promise.all(
    dirs.splice(0).map((dir) => rm(dir, { recursive: true, force: true })),
  );
});
test("startup rejects a missing or empty required route tree", async () => {
  const dir = await fixture({});
  await expect(loadRoutes(join(dir, "missing"))).rejects.toThrow();
  await expect(loadRoutes(dir)).rejects.toThrow("empty");
});
test("a broken controller cannot silently remove the security boundary", async () => {
  const dir = await fixture({
    "(secure).ts": `throw Error('broken guard');`,
    "(secure)/secret.ts": route,
  });
  await expect(loadRoutes(dir)).rejects.toThrow();
});
test("invalid module export prevents readiness", async () => {
  for (const body of ["export const value = 1;", "export default {};"]) {
    await expect(
      loadRoutes(await fixture({ "secret.ts": body })),
    ).rejects.toThrow("Elysia");
  }
});
test("duplicates are rejected before FSR discards a handler", async () => {
  await expect(
    loadRoutes(await fixture({ "users.ts": route, "users/index.ts": route })),
  ).rejects.toThrow("collision");
  await expect(
    loadRoutes(
      await fixture({ "users/[id].ts": route, "users/[slug].ts": route }),
    ),
  ).rejects.toThrow("collision");
});
test("unsupported paths and hidden files fail rather than disappearing", async () => {
  for (const path of ["v[id].ts", "[...rest].ts", ".well-known/openid.ts"]) {
    await expect(loadRoutes(await fixture({ [path]: route }))).rejects.toThrow(
      "path",
    );
  }
});
test("static parent guards cover ghost directories and all supported methods", async () => {
  const dir = await fixture({
    "(secure)/secret.ts": `import Elysia from 'elysia'; export default new Elysia().get('',()=> 'private').post('',()=> 'private');`,
  });
  const { app } = await loadRoutes(dir);
  const guarded = new Elysia()
    .guard({
      beforeHandle: ({ headers }) => {
        if (headers.authorization !== "fixture")
          return new Response(null, { status: 401 });
      },
    })
    .use(app);
  for (const method of ["GET", "POST"]) {
    expect(
      (await guarded.handle(new Request("http://localhost/secret", { method })))
        .status,
    ).toBe(401);
    expect(
      await (
        await guarded.handle(
          new Request("http://localhost/secret", {
            method,
            headers: { authorization: "fixture" },
          }),
        )
      ).text(),
    ).toBe("private");
  }
});
test("FSR keeps static and catch-all route behavior with an absolute root", async () => {
  const dir = await fixture({
    "users/me.ts": route.replace("private", "me"),
    "users/[id].ts": route,
    "files/[...].ts": route,
  });
  const { app, manifest } = await loadRoutes(dir);
  expect(
    await (await app.handle(new Request("http://localhost/users/me"))).text(),
  ).toBe("me");
  expect(
    (await app.handle(new Request("http://localhost/files/a/b"))).status,
  ).toBe(200);
  expect(manifest).toEqual(
    expect.arrayContaining([
      { method: "GET", path: "/users/:id", source: "users/[id].ts" },
    ]),
  );
});
