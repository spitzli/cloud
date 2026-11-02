import assert from "node:assert/strict";
import test, { afterEach } from "node:test";
import { mockMethod, restoreMocks } from "../helpers/node-test";
import { toNextJsHandler } from "better-auth/next-js";
import { lazyAuth } from "../../src/modules/auth/lazy-auth";
afterEach(restoreMocks);

test("Next.js route registration stays lazy and later requests recover after failed startup", async () => {
  let creations = 0;
  const failure = new Error("startup connection timeout");
  const auth = lazyAuth(() => {
    const context =
      ++creations === 1 ? Promise.reject(failure) : Promise.resolve("ready");
    return {
      $context: context,
      handler: async () => new Response(await context),
    };
  });
  const { GET } = toNextJsHandler(auth);
  assert.equal(creations, 0);
  await assert.rejects(
    GET(new Request("https://auth.example.test/api/auth/get-session")),
    (error) => error === failure,
  );
  assert.equal(creations, 1);
  assert.equal(
    await (
      await GET(new Request("https://auth.example.test/api/auth/get-session"))
    ).text(),
    "ready",
  );
  assert.equal(creations, 2);
});

test("handler mocks work through the Next.js adapter and restore the original handler", async (t) => {
  const auth = lazyAuth(() => ({
    $context: Promise.resolve("ready"),
    handler: async () => new Response("original"),
  }));
  const { GET } = toNextJsHandler(auth);
  const mock = mockMethod(
    auth,
    "handler",
    async () => new Response("fixture"),
  );
  assert.equal(
    await (
      await GET(new Request("https://auth.example.test/api/auth/session"))
    ).text(),
    "fixture",
  );
  mock.mock.restore();
  assert.equal(
    await (
      await GET(new Request("https://auth.example.test/api/auth/session"))
    ).text(),
    "original",
  );
});

test("explicit immutable properties obey proxy invariants", () => {
  let creations = 0;
  const auth = lazyAuth(() => {
    creations++;
    return { $context: Promise.resolve("ready"), marker: "original" };
  });
  Object.defineProperty(auth, "marker", {
    value: "override",
    configurable: false,
    writable: false,
  });
  assert.equal(auth.marker, "override");
  assert.equal("marker" in auth, true);
  assert.equal(
    Object.getOwnPropertyDescriptor(auth, "marker")?.configurable,
    false,
  );
  assert.equal(creations, 0);
});

test("auth initializes only on first access and reuses a successful instance", async () => {
  let creations = 0;
  const instance = {
    $context: Promise.resolve("ready"),
    api: { value: 42 },
    handler: () => "handled",
  };
  const auth = lazyAuth(() => {
    creations++;
    return instance;
  });
  assert.equal(creations, 0);
  // Better Auth's Next.js integration checks this before calling the handler.
  assert.equal("handler" in auth, true);
  assert.equal(auth.handler(), "handled");
  assert.equal(await auth.$context, "ready");
  assert.equal(auth.api, instance.api);
  assert.equal(creations, 1);
});

test("failed initialization reaches the caller once and the next access can recover", async () => {
  let creations = 0;
  const failure = new Error("connection timeout");
  let reject!: (error: Error) => void;
  const firstContext = new Promise<string>((_resolve, fail) => {
    reject = fail;
  });
  const auth = lazyAuth(() => {
    const context = ++creations === 1 ? firstContext : Promise.resolve("ready");
    return { $context: context, api: { read: () => context } };
  });
  const oldAPI = auth.api;
  const firstCall = oldAPI.read();
  const failed = assert.rejects(firstCall, (error) => error === failure);
  reject(failure);
  await failed;
  assert.equal(
    creations,
    1,
    "failure must not automatically retry the operation",
  );
  assert.equal(await auth.api.read(), "ready");
  assert.equal(creations, 2);
  // A caller still holding the rejected instance cannot poison its replacement.
  await assert.rejects(oldAPI.read(), (error) => error === failure);
  assert.equal(await auth.api.read(), "ready");
  assert.equal(creations, 2);
});

test("an unconsumed context rejection is observed and permits the next request", async () => {
  let creations = 0;
  const auth = lazyAuth(() => ({
    $context:
      ++creations === 1
        ? Promise.reject(new Error("initialization failed"))
        : Promise.resolve("ready"),
    api: {},
  }));
  void auth.api;
  // node:test fails if an unhandled rejection occurs, including after a test ends.
  await new Promise<void>((resolve) => setImmediate(resolve));
  assert.equal(creations, 1);
  assert.equal(await auth.$context, "ready");
  assert.equal(creations, 2);
});

test("a synchronous factory failure remains visible and does not cache a broken instance", async () => {
  let creations = 0;
  const auth = lazyAuth(() => {
    if (++creations === 1) throw new Error("invalid initialization");
    return { $context: Promise.resolve("ready") };
  });
  assert.throws(() => auth.$context, /invalid initialization/);
  assert.equal(await auth.$context, "ready");
  assert.equal(creations, 2);
});

test("an API failure after startup neither repeats the operation nor resets auth", async () => {
  let creations = 0,
    writes = 0;
  const auth = lazyAuth(() => {
    creations++;
    return {
      $context: Promise.resolve("ready"),
      api: {
        write: async () => {
          writes++;
          throw new Error("write failed");
        },
      },
    };
  });
  await auth.$context;
  await assert.rejects(auth.api.write(), /write failed/);
  assert.equal(await auth.$context, "ready");
  assert.equal(writes, 1);
  assert.equal(creations, 1);
});
