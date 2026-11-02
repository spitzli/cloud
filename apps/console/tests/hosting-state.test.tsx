import "./router-test-environment";
import assert from "node:assert/strict";
import { after, test } from "node:test";
import { act, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import {
  QueryClient,
  QueryClientProvider,
  notifyManager,
  defaultScheduler,
} from "@tanstack/react-query";
import {
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
  Outlet,
  RouterProvider,
} from "@tanstack/react-router";
import { Window } from "happy-dom";
import { I18nProvider } from "@webdock/i18n/react";
import { commandSchema } from "@webdock/hosting-contracts";
import { ApiClient, type Session } from "../src/api";
import { ApiContext, SessionContext } from "../src/context";
import { appSchema } from "../src/hosting-contracts";
import { HostingApplication } from "../src/HostingApplication";
import { useHostingContext } from "../src/hosting-context";
import { Shell } from "../src/Shell";
notifyManager.setScheduler(queueMicrotask);
after(() => {
  notifyManager.setScheduler(defaultScheduler);
});
const session: Session = {
  user: {
    id: "actor",
    name: "Operator",
    email: "operator@example.test",
    role: "operator",
    emailVerified: true,
  },
  session: {
    id: "session",
    userId: "actor",
    expiresAt: "2099-01-01T00:00:00Z",
  },
};
function application() {
  return appSchema.parse({
    id: "301",
    name: "Private application",
    projectID: "201",
    lifecycle: "deployed",
    revision: 1,
    observedRevision: 1,
    status: "ready",
    operationID: null,
    lastError: null,
    logs: null,
    environmentNames: [],
    subscriptionRevision: 3,
    operator: true,
    canManage: true,
    canRequestLogs: true,
    spec: {
      template: "custom",
      image: `registry.example/app@sha256:${"a".repeat(64)}`,
      args: [],
      port: 8080,
      healthPath: "/",
      cpuMillicores: 100,
      memoryBytes: 67108864,
      ephemeralBytes: 67108864,
      replicas: 1,
      volumeBytes: 0,
    },
  });
}
const project = {
  projectID: "201",
  name: "Private project",
  customerID: "101",
  customerName: "Private customer",
  operator: true,
  configured: true,
  provider: "docker",
};
const operation = {
  id: "801",
  action: "apps.stop",
  status: "succeeded",
  customerID: "101",
  projectID: "201",
  createdAt: "2026-10-20T10:00:00Z",
};
async function flush(action: () => void) {
  await act(async () => {
    action();
    await Promise.resolve();
  });
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
}
async function until(check: () => boolean) {
  for (let attempt = 0; attempt < 60; attempt++) {
    if (check()) return;
    await flush(() => {
      /* Drain query and router notifications. */
    });
  }
  assert.ok(check(), "Expected React state did not settle");
}
function fixture(fetcher: typeof fetch) {
  const browser = new Window({
    url: "https://console.example/hosting/apps/301",
  });
  Object.assign(globalThis, {
    window: browser, navigator: browser.navigator,
    self: browser,
    scrollTo: browser.scrollTo.bind(browser),
    document: browser.document,
    location: browser.location,
    HTMLElement: browser.HTMLElement,
    Event: browser.Event,
    FormData: browser.FormData,
    IS_REACT_ACT_ENVIRONMENT: true,
  });
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false, staleTime: Infinity, gcTime: Infinity },
      mutations: { retry: false },
    },
  });
  const api = new ApiClient(
    {
      apiOrigin: "https://console.example",
      authOrigin: "https://console.example",
      enabledFeatures: ["hosting"],
    },
    fetcher,
  );
  return {
    host,
    client,
    render: async (element: ReactNode, shell = false) => {
      const parent = createRootRoute({
        component: () => (
          <ApiContext.Provider value={api}>
            <SessionContext.Provider value={session}>
              <I18nProvider locale="en" preference="en">
                {shell ? <Shell /> : <Outlet />}
              </I18nProvider>
            </SessionContext.Provider>
          </ApiContext.Provider>
        ),
      });
      const route = createRoute({
        getParentRoute: () => parent,
        path: "/hosting/apps/$appID",
        component: () => element,
      });
      const router = createRouter({
        routeTree: parent.addChildren([route]),
        history: createMemoryHistory({ initialEntries: ["/hosting/apps/301"] }),
        isServer: false,
      });
      await router.load();
      await flush(() => {
        root.render(
          <QueryClientProvider client={client}>
            <RouterProvider router={router} />
          </QueryClientProvider>,
        );
      });
    },
    close: async () => {
      await flush(() => {
        root.unmount();
      });
      client.clear();
      await browser.happyDOM.abort();
    },
  };
}
function button(host: HTMLElement, label: string) {
  const value = [...host.querySelectorAll<HTMLButtonElement>("button")].find(
    (item) => item.textContent === label,
  );
  assert.ok(value, `Missing button: ${label}`);
  return value;
}
function deferred<T>() {
  let resolve: (value: T) => void = () => {
    throw new Error("Promise not initialized");
  };
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}
await test("overview controls wait for terminal confirmation and a fresh app before accepting another action", async () => {
  const state = application();
  let status = "running";
  let holdRefresh = false;
  const refresh = deferred<Response>();
  const submitted: number[] = [];
  const fetcher: typeof fetch = (_input, options) => {
    const body: unknown = JSON.parse(
      typeof options?.body === "string" ? options.body : "{}",
    );
    const command = commandSchema.parse(body);
    if (command.action === "apps.get")
      return holdRefresh
        ? refresh.promise.then((response) => response.clone())
        : Promise.resolve(Response.json(state));
    if (command.action === "operations.get")
      return Promise.resolve(Response.json({ ...operation, status }));
    if (command.action === "apps.stop" || command.action === "apps.start") {
      submitted.push(command.revision);
      state.revision++;
      state.operationID = "801";
      state.status = "pending";
      state.spec.replicas = command.action === "apps.stop" ? 0 : 1;
      return Promise.resolve(
        Response.json({
          id: "801",
          appID: "301",
          status: "queued",
          revision: state.revision,
        }),
      );
    }
    return Promise.resolve(Response.json({}, { status: 400 }));
  };
  const view = fixture(fetcher);
  try {
    await view.render(<HostingApplication appID="301" />);
    await until(() => view.host.textContent.includes("Stop application"));
    await flush(() => {
      button(view.host, "Stop application").click();
    });
    const form = view.host.querySelector("dialog form");
    assert.ok(form);
    await flush(() => {
      form.dispatchEvent(
        new Event("submit", { bubbles: true, cancelable: true }),
      );
    });
    await until(
      () =>
        view.client.getQueryData<{ status: string }>([
          "hosting-operation",
          "801",
        ])?.status === "running",
    );
    assert.equal(
      button(view.host, "Start application").disabled,
      true,
      "pending operation blocks another action",
    );
    holdRefresh = true;
    status = "succeeded";
    await act(async () => {
      await view.client.invalidateQueries({
        queryKey: ["hosting-operation", "801"],
        exact: true,
      });
    });
    await flush(() => {
      /* A terminal operation alone must not unlock stale application data. */
    });
    assert.equal(button(view.host, "Start application").disabled, true);
    state.observedRevision = state.revision;
    state.status = "stopped";
    await flush(() => {
      refresh.resolve(Response.json(state));
    });
    await until(() => !button(view.host, "Start application").disabled);
    holdRefresh = false;
    await flush(() => {
      button(view.host, "Start application").click();
    });
    const next = view.host.querySelector("dialog form");
    assert.ok(next);
    await flush(() => {
      next.dispatchEvent(
        new Event("submit", { bubbles: true, cancelable: true }),
      );
    });
    assert.deepEqual(
      submitted,
      [1, 2],
      "next action uses the refreshed revision without navigation",
    );
  } finally {
    await view.close();
  }
});
await test("required context lookup denial withholds every cached private value and breadcrumb", async () => {
  for (const action of [
    "apps.get",
    "projects.get",
    "operations.get",
    "tenants",
  ] as const) {
    let denied = false;
    const fetcher: typeof fetch = (input, options) => {
      assert.ok(input instanceof URL);
      if (input.pathname.endsWith("/tenants"))
        return Promise.resolve(
          Response.json([{ id: "101", name: "Private customer" }], {
            status: denied && action === "tenants" ? 403 : 200,
          }),
        );
      const body: unknown = JSON.parse(
        typeof options?.body === "string" ? options.body : "{}",
      );
      const command = commandSchema.parse(body);
      if (denied && command.action === action)
        return Promise.resolve(Response.json({}, { status: 403 }));
      if (command.action === "apps.get")
        return Promise.resolve(Response.json(application()));
      if (command.action === "projects.get")
        return Promise.resolve(Response.json(project));
      return Promise.resolve(Response.json(operation));
    };
    const view = fixture(fetcher);
    view.client.setQueryData(["hosting", "app", "301"], application());
    view.client.setQueryData(["hosting", "project", "201"], project);
    view.client.setQueryData(["hosting-operation", "801"], operation);
    view.client.setQueryData(
      ["tenants"],
      [{ id: "101", name: "Private customer" }],
    );
    const contexts: ReturnType<typeof useHostingContext>[] = [];
    const path =
      action === "operations.get"
        ? "/hosting/operations/801"
        : action === "tenants"
          ? "/hosting"
          : "/hosting/apps/301";
    function Probe() {
      const context = useHostingContext(path, "101");
      contexts.push(context);
      return <p>{context.label}</p>;
    }
    const keys = {
      "apps.get": ["hosting", "app", "301"],
      "projects.get": ["hosting", "project", "201"],
      "operations.get": ["hosting-operation", "801"],
      tenants: ["tenants"],
    };
    const key = keys[action];
    try {
      await view.render(<Probe />);
      assert.equal(contexts.at(-1)?.customer?.name, "Private customer");
      denied = true;
      await act(async () => {
        await view.client.invalidateQueries({ queryKey: key, exact: true });
      });
      await until(() => contexts.at(-1)?.unavailable === true);
      assert.ok(
        view.client.getQueryData(key),
        "the reproduction retains stale TanStack data",
      );
      const context = contexts.at(-1);
      assert.ok(context);
      assert.equal(context.customer, undefined);
      assert.equal(context.project, undefined);
      assert.equal(context.application, undefined);
      assert.ok(
        context.breadcrumbs.every((value) => !value.includes("Private")),
      );
    } finally {
      await view.close();
    }
  }
});
await test("Shell removes old private names and links after a required project read becomes forbidden", async () => {
  let denied = false;
  const fetcher: typeof fetch = (_input, options) => {
    const body: unknown = JSON.parse(
      typeof options?.body === "string" ? options.body : "{}",
    );
    const command = commandSchema.parse(body);
    if (command.action === "apps.get")
      return Promise.resolve(Response.json(application()));
    return Promise.resolve(
      Response.json(project, { status: denied ? 403 : 200 }),
    );
  };
  const view = fixture(fetcher);
  try {
    await view.render(<p>Application view</p>, true);
    await until(() => view.host.textContent.includes("Private project"));
    denied = true;
    await act(async () => {
      await view.client.invalidateQueries({
        queryKey: ["hosting", "project", "201"],
        exact: true,
      });
    });
    await until(() => view.host.textContent.includes("Workspace unavailable"));
    assert.doesNotMatch(
      view.host.textContent,
      /Private customer|Private project|Private application/,
    );
    assert.equal(
      view.host.querySelector('a[href="/hosting/projects/201"]'),
      null,
    );
    assert.equal(view.host.querySelector('a[href="/hosting/apps/301"]'), null);
  } finally {
    await view.close();
  }
});
