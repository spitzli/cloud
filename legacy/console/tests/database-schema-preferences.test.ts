import test from "node:test";
import assert from "node:assert/strict";
import type { TabularisClient } from "@tabularis/web-ui";
import { rememberDatabaseSchemas } from "../src/lib/database-schema-preferences";

const scope = {
  userID: "alice",
  bindingID: "123",
  kind: "managed" as const,
  connectionID: "connection-a",
};
function memory() {
  const values = new Map<string, string>();
  return {
    values,
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => {
      values.set(key, value);
    },
  };
}
function client(
  run: (command: string, request: unknown) => Promise<unknown> = async (
    command,
  ) =>
    command === "get_selected_schemas"
      ? []
      : command === "get_schema_preference"
        ? null
        : undefined,
) {
  return { call: run as TabularisClient["call"] };
}

test("selected Unicode schemas and active schema survive a new connection/session", async () => {
  const storage = memory(),
    first = client();
  rememberDatabaseSchemas(first, scope, () => storage);
  const schemas = ["public", "Büro 東京", "emoji_🌿"];
  await first.call("set_selected_schemas", {
    connectionId: scope.connectionID,
    schemas,
  });
  await first.call("set_schema_preference", {
    connectionId: scope.connectionID,
    schema: schemas[1],
  });
  const next = client();
  rememberDatabaseSchemas(
    next,
    { ...scope, connectionID: "new-session-connection" },
    () => storage,
  );
  assert.deepEqual(
    await next.call("get_selected_schemas", {
      connectionId: "new-session-connection",
    }),
    schemas,
  );
  assert.equal(
    await next.call("get_schema_preference", {
      connectionId: "new-session-connection",
    }),
    schemas[1],
  );
  assert.deepEqual(
    Object.keys(JSON.parse([...storage.values.values()][0])).sort(),
    ["activeSchema", "selectedSchemas"],
  );
  assert.ok(![...storage.values.keys()][0].includes(scope.connectionID));
});

test("preferences isolate user, database, and managed versus registered binding", async () => {
  const storage = memory(),
    first = client();
  rememberDatabaseSchemas(first, scope, () => storage);
  await first.call("set_selected_schemas", {
    connectionId: scope.connectionID,
    schemas: ["private"],
  });
  for (const other of [
    { userID: "bob" },
    { bindingID: "456" },
    { kind: "registered" as const },
  ]) {
    const isolated = client();
    rememberDatabaseSchemas(isolated, { ...scope, ...other }, () => storage);
    assert.deepEqual(
      await isolated.call("get_selected_schemas", {
        connectionId: scope.connectionID,
      }),
      [],
    );
  }
  assert.deepEqual(
    await first.call("get_selected_schemas", {
      connectionId: "different-connection",
    }),
    [],
  );
});

test("upstream is always called and failed saves, authorization and fetch errors propagate", async () => {
  const storage = memory();
  let failure: Error | undefined;
  const calls: string[] = [];
  const wrapped = client(async (command) => {
    calls.push(command);
    if (failure) throw failure;
    return command === "get_selected_schemas" ? [] : null;
  });
  rememberDatabaseSchemas(wrapped, scope, () => storage);
  await wrapped.call("set_selected_schemas", {
    connectionId: scope.connectionID,
    schemas: ["public"],
  });
  for (const error of [
    Object.assign(new Error("Unauthorized"), { status: 401 }),
    new TypeError("Failed to fetch"),
  ]) {
    failure = error;
    await assert.rejects(
      wrapped.call("get_selected_schemas", {
        connectionId: scope.connectionID,
      }),
      (cause) => cause === error,
    );
    await assert.rejects(
      wrapped.call("set_selected_schemas", {
        connectionId: scope.connectionID,
        schemas: ["failed"],
      }),
      (cause) => cause === error,
    );
    await assert.rejects(
      wrapped.call("set_schema_preference", {
        connectionId: scope.connectionID,
        schema: "failed",
      }),
      (cause) => cause === error,
    );
    await assert.rejects(
      wrapped.call("get_schema_preference", {
        connectionId: scope.connectionID,
      }),
      (cause) => cause === error,
    );
  }
  failure = undefined;
  assert.deepEqual(
    await wrapped.call("get_selected_schemas", {
      connectionId: scope.connectionID,
    }),
    ["public"],
  );
  assert.equal(
    await wrapped.call("get_schema_preference", {
      connectionId: scope.connectionID,
    }),
    null,
  );
  assert.equal(calls.length, 11);
});

test("malformed storage is ignored and successful saves replace it with schema-only preferences", async () => {
  const storage = memory(),
    wrapped = client(async (command) =>
      command === "get_selected_schemas" ? ["server"] : undefined,
    );
  rememberDatabaseSchemas(wrapped, scope, () => storage);
  await wrapped.call("set_selected_schemas", {
    connectionId: scope.connectionID,
    schemas: ["public"],
  });
  const key = [...storage.values.keys()][0];
  for (const raw of [
    "not-json",
    "null",
    "[]",
    '{"selectedSchemas":[1]}',
    '{"activeSchema":{}}',
    '{"selectedSchemas":["public"],"sql":"secret"}',
    '{"selectedSchemas":["\\u0000"]}',
  ]) {
    storage.setItem(key, raw);
    assert.deepEqual(
      await wrapped.call("get_selected_schemas", {
        connectionId: scope.connectionID,
      }),
      ["server"],
    );
  }
  await wrapped.call("set_schema_preference", {
    connectionId: scope.connectionID,
    schema: "東京",
  });
  assert.deepEqual(JSON.parse(storage.values.get(key)!), {
    activeSchema: "東京",
  });
});

test("disabled storage, quota errors and unrelated commands preserve upstream behavior", async () => {
  for (const getStorage of [
    () => {
      throw new Error("Storage disabled");
    },
    () => ({
      getItem: () => null,
      setItem: () => {
        throw new Error("Quota exceeded");
      },
    }),
  ]) {
    const wrapped = client(async (command) =>
      command === "get_selected_schemas" ? ["server"] : undefined,
    );
    rememberDatabaseSchemas(wrapped, scope, getStorage);
    await wrapped.call("set_selected_schemas", {
      connectionId: scope.connectionID,
      schemas: [],
    });
    assert.deepEqual(
      await wrapped.call("get_selected_schemas", {
        connectionId: scope.connectionID,
      }),
      ["server"],
    );
  }
  let storageReads = 0;
  const wrapped = client(async () => ["live"]);
  rememberDatabaseSchemas(wrapped, scope, () => {
    storageReads++;
    return memory();
  });
  assert.deepEqual(
    await wrapped.call("get_schemas", { connectionId: scope.connectionID }),
    ["live"],
  );
  assert.equal(storageReads, 0);
});
