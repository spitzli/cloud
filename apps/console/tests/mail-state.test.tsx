import assert from "node:assert/strict";
import { after, test } from "node:test";
import { act } from "react";
import { createRoot } from "react-dom/client";
import {
  QueryClient,
  QueryClientProvider,
  notifyManager,
  defaultScheduler,
} from "@tanstack/react-query";
import { Window } from "happy-dom";
import { ApiClient, type Session } from "../src/api";
import { ApiContext, SessionContext } from "../src/context";
import { MailCustomer } from "../src/Mail";
import { commandSchema } from "../src/mail-contracts";
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
function service(customerID = "101") {
  return {
    service: { customerID, enabled: false, state: "disabled", revision: "0" },
    mailboxesAvailable: true,
    sendingAvailable: true,
    canActivate: true,
    canSuspend: false,
    canReconcile: false,
    pendingReview: false,
  };
}
const directory = {
  customerID: "101",
  operator: true,
  canManage: true,
  canConfigure: true,
  configuration: { revision: "2", appliedRevision: "0", state: "pending" },
  mailboxes: [
    {
      id: "401",
      name: "Private mailbox",
      address: "private@example.test",
      state: "pending",
      enabled: true,
      quotaBytes: "1000000000",
      canOpen: false,
    },
  ],
  domains: [],
  verifiedDomains: [],
  people: [],
  grants: [],
};
async function flush(action: () => void = () => undefined) {
  await act(async () => {
    action();
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
}
async function until(check: () => boolean) {
  for (let n = 0; n < 60; n++) {
    if (check()) return;
    await flush();
  }
  assert.ok(check(), "UI did not settle");
}
function button(host: HTMLElement, text: string) {
  const found = [...host.querySelectorAll("button")].find(
    (value) => value.textContent === text,
  );
  assert.ok(found, `Missing ${text}`);
  return found;
}
function fixture(fetcher: typeof fetch, preview = false) {
  const browser = new Window({ url: "https://console.example/mail" });
  Object.assign(globalThis, {
    window: browser, navigator: browser.navigator,
    document: browser.document,
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
      queries: { retry: false, staleTime: Infinity },
      mutations: { retry: false },
    },
  });
  const api = new ApiClient(
    {
      apiOrigin: "https://console.example",
      authOrigin: "https://console.example",
      enabledFeatures: ["mail"],
    },
    fetcher,
  );
  return {
    host,
    client,
    render: async (customer = "101") =>
      flush(() => {
        root.render(
          <QueryClientProvider client={client}>
            <ApiContext.Provider value={api}>
              <SessionContext.Provider
                value={{ ...session, user: { ...session.user, preview } }}
              >
                <MailCustomer key={customer} customerID={customer} />
              </SessionContext.Provider>
            </ApiContext.Provider>
          </QueryClientProvider>,
        );
      }),
    close: async () => {
      await flush(() => {
        root.unmount();
      });
      client.clear();
      await browser.happyDOM.abort();
    },
  };
}
await test("unknown lifecycle outcome is not retried or unlocked by an unchanged read and survives workspace navigation", async () => {
  const current = service();
  let writes = 0;
  const view = fixture((_url, options) => {
    const command = commandSchema.parse(
      JSON.parse(typeof options?.body === "string" ? options.body : "{}"),
    );
    if (command.action === "manageTenantMailService") {
      writes++;
      return Promise.resolve(Response.json({}, { status: 503 }));
    }
    if (command.action === "getTenantMailboxes")
      return Promise.resolve(
        Response.json({
          data: { ...directory, customerID: command.customerID },
        }),
      );
    return Promise.resolve(
      Response.json({
        data: command.customerID === "101" ? current : service("102"),
      }),
    );
  });
  try {
    await view.render();
    await until(() => view.host.textContent.includes("Activate incoming mail"));
    await flush(() => {
      button(view.host, "Activate incoming mail").click();
    });
    await flush(() => {
      button(view.host, "Confirm activate").click();
    });
    await until(() =>
      view.host.textContent.includes("Request outcome is unknown"),
    );
    await flush(() => {
      button(view.host, "Refresh Mail").click();
    });
    assert.equal(button(view.host, "Activate incoming mail").disabled, true);
    assert.equal(writes, 1);
    await view.render("102");
    await until(() => !button(view.host, "Activate incoming mail").disabled);
    await view.render("101");
    await until(() =>
      view.host.textContent.includes("Request outcome is unknown"),
    );
    assert.equal(button(view.host, "Activate incoming mail").disabled, true);
    Object.assign(current.service, {
      enabled: true,
      state: "ready",
      revision: "1",
    });
    current.canSuspend = true;
    current.canActivate = false;
    await flush(() => {
      button(view.host, "Refresh Mail").click();
    });
    await until(
      () => !view.host.textContent.includes("Request outcome is unknown"),
    );
    assert.equal(button(view.host, "Suspend incoming mail").disabled, false);
    assert.equal(writes, 1);
    await flush(() => {
      button(view.host, "Suspend incoming mail").click();
    });
    assert.match(view.host.textContent, /retained/);
  } finally {
    await view.close();
  }
});
await test("preview prohibits writes and pending mailbox configuration never appears applied", async () => {
  const view = fixture((_url, options) => {
    const command = commandSchema.parse(
      JSON.parse(typeof options?.body === "string" ? options.body : "{}"),
    );
    assert.notEqual(command.action, "manageTenantMailService");
    return Promise.resolve(
      Response.json({
        data: command.action === "getTenantMailService" ? service() : directory,
      }),
    );
  }, true);
  try {
    await view.render();
    await until(() => view.host.textContent.includes("Private mailbox"));
    assert.equal(button(view.host, "Activate incoming mail").disabled, true);
    assert.match(view.host.textContent, /Applied revision 0/);
    assert.match(view.host.textContent, /not yet applied/);
  } finally {
    await view.close();
  }
});
await test("denied reads hide cached mailbox and service data", async () => {
  let deny = false;
  const view = fixture((_url, options) => {
    const command = commandSchema.parse(
      JSON.parse(typeof options?.body === "string" ? options.body : "{}"),
    );
    return Promise.resolve(
      Response.json(
        {
          data:
            command.action === "getTenantMailService" ? service() : directory,
        },
        { status: deny ? 403 : 200 },
      ),
    );
  });
  try {
    await view.render();
    await until(() => view.host.textContent.includes("Private mailbox"));
    deny = true;
    await act(async () => {
      await view.client.invalidateQueries({ queryKey: ["mail"] });
    });
    await until(() => view.host.textContent.includes("Unable to load"));
    assert.doesNotMatch(
      view.host.textContent,
      /Private mailbox|private@example.test|Activate incoming mail/,
    );
  } finally {
    await view.close();
  }
});
await test("revision conflict requires explicit fresh review before a new lifecycle request", async () => {
  const current = service();
  const revisions: string[] = [];
  const view = fixture((_url, options) => {
    const command = commandSchema.parse(
      JSON.parse(typeof options?.body === "string" ? options.body : "{}"),
    );
    if (command.action === "getTenantMailboxes")
      return Promise.resolve(Response.json({ data: directory }));
    if (command.action === "getTenantMailService")
      return Promise.resolve(Response.json({ data: current }));
    assert.equal(command.action, "manageTenantMailService");
    revisions.push(command.input.revision);
    if (revisions.length === 1) {
      current.service.revision = "1";
      return Promise.resolve(Response.json({}, { status: 409 }));
    }
    Object.assign(current.service, {
      enabled: true,
      state: "pending",
      revision: "2",
    });
    return Promise.resolve(Response.json({ data: current }));
  });
  try {
    await view.render();
    await until(() => view.host.textContent.includes("Activate incoming mail"));
    await flush(() => {
      button(view.host, "Activate incoming mail").click();
    });
    await flush(() => {
      button(view.host, "Confirm activate").click();
    });
    await until(() =>
      view.host.textContent.includes("Request was not accepted"),
    );
    assert.equal(button(view.host, "Activate incoming mail").disabled, true);
    await flush(() => {
      button(view.host, "Refresh Mail").click();
    });
    await until(() => !button(view.host, "Activate incoming mail").disabled);
    await flush(() => {
      button(view.host, "Activate incoming mail").click();
    });
    await flush(() => {
      button(view.host, "Confirm activate").click();
    });
    await until(() => view.host.textContent.includes("Request recorded"));
    assert.deepEqual(revisions, ["0", "1"]);
    assert.equal(button(view.host, "Activate incoming mail").disabled, true);
    assert.match(view.host.textContent, /change is queued/);
  } finally {
    await view.close();
  }
});

await test("successful same-direction no-op does not permanently block suspension", async () => {
  const current = service();
  Object.assign(current.service, {
    enabled: true,
    state: "ready",
    revision: "1",
  });
  current.canSuspend = true;
  const view = fixture((_url, options) => {
    const command = commandSchema.parse(
      JSON.parse(typeof options?.body === "string" ? options.body : "{}"),
    );
    return Promise.resolve(
      Response.json({
        data: command.action === "getTenantMailboxes" ? directory : current,
      }),
    );
  });
  try {
    await view.render();
    await until(() => view.host.textContent.includes("Activate incoming mail"));
    assert.equal(
      button(view.host, "Activate incoming mail").disabled,
      true,
      "already-enabled direction still offers a no-op activation",
    );
    assert.equal(button(view.host, "Suspend incoming mail").disabled, false);
  } finally {
    await view.close();
  }
});
await test("accepted operation requiring review allows explicitly authorized reconcile", async () => {
  const current = service();
  const view = fixture((_url, options) => {
    const command = commandSchema.parse(
      JSON.parse(typeof options?.body === "string" ? options.body : "{}"),
    );
    if (command.action === "manageTenantMailService")
      Object.assign(current.service, {
        enabled: true,
        state: "pending",
        revision: "1",
      });
    return Promise.resolve(
      Response.json({
        data: command.action === "getTenantMailboxes" ? directory : current,
      }),
    );
  });
  try {
    await view.render();
    await until(() => view.host.textContent.includes("Activate incoming mail"));
    await flush(() => {
      button(view.host, "Activate incoming mail").click();
    });
    await flush(() => {
      button(view.host, "Confirm activate").click();
    });
    await until(() => view.host.textContent.includes("Request recorded"));
    Object.assign(current.service, { state: "needs_review" });
    Object.assign(current, { canReconcile: true, pendingReview: true });
    await flush(() => {
      button(view.host, "Refresh Mail").click();
    });
    await until(() =>
      view.host.textContent.includes("An earlier operation needs review"),
    );
    await until(() => !button(view.host, "Reconcile incoming mail").disabled);
    assert.equal(
      button(view.host, "Reconcile incoming mail").disabled,
      false,
      "acknowledged failed operation blocked explicit reconciliation",
    );
  } finally {
    await view.close();
  }
});
