import { test } from "node:test";
import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import { createElement as h, act } from "react";
import { createRoot } from "react-dom/client";
import { Window } from "happy-dom";
import { I18nProvider } from "@spitzli/i18n/react";
import {
  DatabaseActionsProvider,
  type DatabaseUIActions,
} from "../src/ui/actions";
import { DatabaseCredentialPanel } from "../src/ui/database-details";
import { resourceScope as hostingUIScope } from "@spitzli/console-kit/resource-scope";
test("Database UI resource keys isolate customer/project/database state without encoding secrets or request revisions", () => {
  const scope = {
    command: {
      action: "apps.update",
      customerID: "123",
      projectID: "456",
      appID: "789",
      idempotencyKey: "private-one",
      revision: 1,
      password: "secret",
    },
  };
  assert.equal(
    hostingUIScope(scope),
    hostingUIScope({
      command: {
        ...scope.command,
        revision: 2,
        password: "other",
        idempotencyKey: "new",
      },
    }),
  );
  assert.notEqual(
    hostingUIScope(scope),
    hostingUIScope({ command: { ...scope.command, customerID: "other" } }),
  );
  assert.notEqual(
    hostingUIScope({
      options: { project: { projectID: "123", customerID: "456" } },
    }),
    hostingUIScope({
      options: { project: { projectID: "999", customerID: "456" } },
    }),
  );
  assert.ok(!hostingUIScope(scope).includes("private-one"));
  assert.ok(!hostingUIScope(scope).includes("secret"));
});
test("revealed database credentials disappear on resource switch before another action runs", async () => {
  const window = new Window();
  Object.assign(globalThis, {
    window,
    document: window.document,
    IS_REACT_ACT_ENVIRONMENT: true,
  });
  const container = window.document.createElement("div");
  window.document.body.append(container);
  const root = createRoot(container as unknown as HTMLElement);
  const requested: string[] = [];
  const actions: DatabaseUIActions = {
    prepareDatabaseOffer: async () => ({}),
    acceptDatabaseOffer: async () => ({}),
    createDatabasePlan: async () => ({}),
    saveDatabase: async () => ({}),
    launchDatabase: async () => ({ error: "fixture" }),
    launchManagedDatabase: async () => ({ error: "fixture" }),
    revealDatabaseCredentials: async (id) => {
      requested.push(id);
      return {
        credentials: {
          databaseID: id,
          databaseName: "fixture",
          host: "private-host-" + id,
          directPort: 30001,
          pooledPort: 30002,
          credentialRevision: 1,
          application: { username: "user", password: "secret-" + id },
          migration: { username: "migration", password: "migration-" + id },
          sslmode: "verify-full",
          caCertificate: "fixture-ca",
        },
      };
    },
  };
  const render = (id: string) =>
    h(
      I18nProvider,
      { locale: "en", preference: "en" },
      h(DatabaseActionsProvider, {
        actions,
        children: h(DatabaseCredentialPanel, { databaseID: id }),
      }),
    );
  try {
    await act(async () => root.render(render("123")));
    await act(async () => container.querySelector("button")!.click());
    assert.deepEqual(requested, ["123"]);
    assert.match(container.textContent, /private-host-123/);
    await act(async () => root.render(render("456")));
    assert.doesNotMatch(
      container.textContent,
      /private-host-123|secret-123|migration-123/,
    );
    assert.match(container.textContent, /Reveal credentials/);
    assert.deepEqual(requested, ["123"]);
  } finally {
    await act(async () => root.unmount());
    await window.happyDOM.close();
  }
});
test("Database module UI and console-kit depend on public UI ports, never application or server runtime imports", async () => {
  for (const directory of [
    new URL("../src/ui/", import.meta.url),
    new URL("../../../packages/console-kit/src/", import.meta.url),
  ])
    for (const file of await readdir(directory)) {
      if (!/\.tsx?$/.test(file)) continue;
      const source = await readFile(new URL(file, directory), "utf8");
      assert.doesNotMatch(
        source,
        /(?:from\s+|import\s*\()["'][^"']*(?:@\/|apps\/|\/server\/|db\/runtime)/,
      );
      assert.doesNotMatch(source, /^["']use server["'];/m);
      assert.doesNotMatch(source, /process\.env/);
    }
});
