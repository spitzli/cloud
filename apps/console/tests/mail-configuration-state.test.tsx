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
import { directorySchema, commandSchema } from "../src/mail-contracts";
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
function fixture(fetcher: typeof fetch, preview = false) {
  const browser = new Window({ url: "https://console.example/mail" });
  Object.assign(globalThis, {
    window: browser, navigator: browser.navigator,
    document: browser.document,
    HTMLElement: browser.HTMLElement,
    HTMLSelectElement: browser.HTMLSelectElement,
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

function configured() {
  return directorySchema.parse({
    data: {
      ...directory,
      configuration: { revision: "2", appliedRevision: "1", state: "pending" },
      domains: [
        { id: "301", name: "example.test", state: "ready", dkimKeys: [] },
      ],
      verifiedDomains: [{ id: "201", domain: "new.example.test" }],
      people: [{ id: "501", name: "Member", email: "member@example.test" }],
      mailboxes: [{ ...directory.mailboxes[0], state: "ready" }],
      grants: [
        { mailboxID: "401", subject: "501", enabled: true, revision: "1" },
      ],
    },
  }).data;
}

await test("lifecycle observations refresh mailbox permissions without a manual reload", async () => {
  const current = service();
  current.service = { customerID: "101", enabled: true, state: "pending", revision: "1" };
  const boxes = configured();
  boxes.configuration = null;
  boxes.canConfigure = false;
  let directoryReads = 0;
  const view = fixture((_url, options) => {
    const command = commandSchema.parse(
      JSON.parse(typeof options?.body === "string" ? options.body : "{}"),
    );
    if (command.action === "getTenantMailboxes") {
      directoryReads++;
      return Promise.resolve(Response.json({ data: boxes }));
    }
    assert.equal(command.action, "getTenantMailService");
    return Promise.resolve(Response.json({ data: current }));
  });
  const disabled = () =>
    view.host.querySelector<HTMLFieldSetElement>(
      'form[aria-label="Attach incoming domain"] fieldset',
    )?.disabled;
  try {
    await view.render();
    await until(() => disabled() === true);
    const initialReads = directoryReads;
    current.service.state = "ready";
    boxes.canConfigure = true;
    await act(async () => {
      await view.client.invalidateQueries({ queryKey: ["mail", "session", "101", "service"] });
    });
    await until(() => disabled() === false);
    assert.ok(directoryReads > initialReads);
    current.service.state = "suspended";
    current.service.revision = "2";
    boxes.canConfigure = false;
    await act(async () => {
      await view.client.invalidateQueries({ queryKey: ["mail", "session", "101", "service"] });
    });
    await until(() => disabled() === true);
  } finally {
    await view.close();
  }
});
function submit(
  host: HTMLElement,
  label: string,
  values: Record<string, string> = {},
) {
  const form = host.querySelector<HTMLFormElement>(
    `form[aria-label="${label}"]`,
  );
  assert.ok(form, `Missing form ${label}`);
  for (const [name, value] of Object.entries(values)) {
    const control = form.querySelector<HTMLInputElement | HTMLSelectElement>(
      `[name="${name}"]`,
    );
    assert.ok(control);
    control.value = value;
    if (
      control instanceof window.HTMLInputElement &&
      control.type === "checkbox"
    )
      control.checked = value === "yes";
  }
  form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
}
await test("lost mailbox creation stays blocked across workspace navigation while access revocation remains available", async () => {
  const data = configured();
  let creates = 0;
  let revokes = 0;
  const view = fixture((_url, options) => {
    const command = commandSchema.parse(
      JSON.parse(typeof options?.body === "string" ? options.body : "{}"),
    );
    if (command.action === "getTenantMailService")
      return Promise.resolve(
        Response.json({ data: service(command.customerID) }),
      );
    if (command.action === "getTenantMailboxes")
      return Promise.resolve(
        Response.json({ data: { ...data, customerID: command.customerID } }),
      );
    assert.equal(command.action, "manageTenantMailboxes");
    if (command.input.action === "mailbox.create") {
      creates++;
      return Promise.resolve(Response.json({}, { status: 503 }));
    }
    assert.equal(command.input.action, "revoke");
    revokes++;
    data.grants = data.grants.map((grant) => ({
      ...grant,
      enabled: false,
      revision: "2",
    }));
    return Promise.resolve(Response.json({ data: { ok: true } }));
  });
  try {
    await view.render();
    await until(() => view.host.textContent.includes("Create mailbox"));
    await flush(() => {
      submit(view.host, "Create mailbox", {
        domainID: "301",
        localPart: "support",
        name: "Support",
        quotaBytes: "1048576",
      });
      submit(view.host, "Create mailbox", {
        domainID: "301",
        localPart: "support",
        name: "Support",
        quotaBytes: "1048576",
      });
    });
    await until(() =>
      view.host.textContent.includes("Configuration outcome is unknown"),
    );
    await view.render("102");
    await until(() => view.host.textContent.includes("Create mailbox"));
    await view.render("101");
    await until(() =>
      view.host.textContent.includes("Configuration outcome is unknown"),
    );
    await flush(() => {
      submit(view.host, "Create mailbox", {
        domainID: "301",
        localPart: "support",
        name: "Support",
        quotaBytes: "1048576",
      });
    });
    assert.equal(creates, 1);
    data.canConfigure = false;
    await act(async () => {
      await view.client.invalidateQueries({ queryKey: ["mail"] });
    });
    await flush(() => {
      submit(view.host, "Revoke mailbox access", {
        grant: "401:501",
        confirm: "yes",
      });
    });
    await until(() => revokes === 1);
    assert.match(view.host.textContent, /Configuration outcome is unknown/);
    data.mailboxes.push({
      id: "402",
      name: "Support",
      address: "support@example.test",
      quotaBytes: "1048576",
      state: "pending",
      enabled: true,
      canOpen: false,
    });
    await act(async () => {
      await view.client.invalidateQueries({ queryKey: ["mail"] });
    });
    await until(
      () => !view.host.textContent.includes("Configuration outcome is unknown"),
    );
    assert.equal(creates, 1);
    assert.equal(revokes, 1);
  } finally {
    await view.close();
  }
});
await test("preview prevents configuration writes even when stale server flags allow management", async () => {
  let writes = 0;
  const view = fixture((_url, options) => {
    const command = commandSchema.parse(
      JSON.parse(typeof options?.body === "string" ? options.body : "{}"),
    );
    if (command.action === "manageTenantMailboxes") writes++;
    return Promise.resolve(
      Response.json({
        data:
          command.action === "getTenantMailService" ? service() : configured(),
      }),
    );
  }, true);
  try {
    await view.render();
    await until(() => view.host.textContent.includes("Create mailbox"));
    await flush(() => {
      submit(view.host, "Attach incoming domain", { senderDomainID: "201" });
    });
    await flush(() => {
      submit(view.host, "Revoke mailbox access", {
        grant: "401:501",
        confirm: "yes",
      });
    });
    assert.equal(writes, 0);
  } finally {
    await view.close();
  }
});
await test("configuration write denial retires old private directory before retrying current reads", async () => {
  let denied = false;
  const view = fixture((_url, options) => {
    const command = commandSchema.parse(
      JSON.parse(typeof options?.body === "string" ? options.body : "{}"),
    );
    if (command.action === "manageTenantMailboxes") denied = true;
    return Promise.resolve(
      Response.json(
        {
          data:
            command.action === "getTenantMailService"
              ? service()
              : configured(),
        },
        { status: denied ? 403 : 200 },
      ),
    );
  });
  try {
    await view.render();
    await until(() => view.host.textContent.includes("Private mailbox"));
    await flush(() => {
      submit(view.host, "Attach incoming domain", { senderDomainID: "201" });
    });
    await until(() => view.host.textContent.includes("Unable to load"));
    assert.doesNotMatch(
      view.host.textContent,
      /Private mailbox|private@example.test|Member/,
    );
    assert.equal(
      view.client.getQueryData(["mail", "session", "101", "directory"]),
      undefined,
    );
  } finally {
    await view.close();
  }
});
await test("operator retry conflict preserves the stop until explicit refresh and then uses current revision", async () => {
  const data = configured();
  data.canRetryConfiguration = true;
  data.configuration = {
    revision: "2",
    appliedRevision: "1",
    state: "needs_review",
  };
  const revisions: string[] = [];
  const view = fixture((_url, options) => {
    const command = commandSchema.parse(
      JSON.parse(typeof options?.body === "string" ? options.body : "{}"),
    );
    if (command.action === "getTenantMailService")
      return Promise.resolve(Response.json({ data: service() }));
    if (command.action === "getTenantMailboxes")
      return Promise.resolve(Response.json({ data }));
    assert.equal(command.action, "manageTenantMailboxes");
    assert.equal(command.input.action, "configuration.retry");
    revisions.push(command.input.revision);
    if (revisions.length === 1) {
      data.configuration = {
        revision: "5",
        appliedRevision: "1",
        state: "needs_review",
      };
      return Promise.resolve(Response.json({}, { status: 409 }));
    }
    data.configuration = {
      revision: "6",
      appliedRevision: "1",
      state: "pending",
    };
    return Promise.resolve(Response.json({ data: { ok: true } }));
  });
  function click(label: string) {
    const found = [...view.host.querySelectorAll("button")].find(
      (button) => button.textContent === label,
    );
    assert.ok(found);
    found.click();
  }
  try {
    await view.render();
    await until(() => view.host.textContent.includes("Retry configuration"));
    await flush(() => {
      click("Retry configuration");
    });
    await until(() =>
      view.host.textContent.includes("Configuration request rejected"),
    );
    assert.equal(revisions.length, 1);
    await flush(() => {
      click("Refresh configuration");
    });
    await until(
      () => !view.host.textContent.includes("Configuration request rejected"),
    );
    await flush(() => {
      click("Retry configuration");
    });
    await until(() => revisions.length === 2);
    assert.deepEqual(revisions, ["2", "5"]);
    assert.match(view.host.textContent, /Applied revision 1/);
  } finally {
    await view.close();
  }
});
await test("fresh succeeded restriction-only reads retain the unapplied revision gap and obey server retry eligibility", async () => {
  const data = configured();
  let reads = 0;
  const view = fixture((_url, options) => {
    const command = commandSchema.parse(
      JSON.parse(typeof options?.body === "string" ? options.body : "{}"),
    );
    if (command.action === "getTenantMailService")
      return Promise.resolve(Response.json({ data: service() }));
    if (command.action === "getTenantMailboxes") {
      reads++;
      if (reads >= 2)
        data.configuration = {
          revision: "2",
          appliedRevision: "1",
          state: "succeeded",
        };
    }
    return Promise.resolve(Response.json({ data }));
  });
  try {
    await view.render();
    await until(() => view.host.textContent.includes("Private mailbox"));
    await act(async () => {
      await view.client.invalidateQueries({ queryKey: ["mail"] });
    });
    assert.ok(reads >= 2, "refresh must read the real directory endpoint");
    await until(() => view.host.textContent.includes("succeeded"));
    assert.match(view.host.textContent, /succeeded/);
    assert.match(view.host.textContent, /not yet applied/);
    assert.doesNotMatch(view.host.textContent, /Retry configuration/);
    data.canRetryConfiguration = true;
    await act(async () => {
      await view.client.invalidateQueries({ queryKey: ["mail"] });
    });
    await until(() => view.host.textContent.includes("Retry configuration"));
    assert.match(view.host.textContent, /not yet applied/);
  } finally {
    await view.close();
  }
});
await test("changing a grant recipient requires fresh explicit content-access confirmation", async () => {
  const data = configured();
  data.people.push({
    id: "502",
    name: "Second member",
    email: "second@example.test",
  });
  data.grants = [];
  let writes = 0;
  const view = fixture((_url, options) => {
    const command = commandSchema.parse(
      JSON.parse(typeof options?.body === "string" ? options.body : "{}"),
    );
    if (command.action === "manageTenantMailboxes") writes++;
    return Promise.resolve(
      Response.json({
        data: command.action === "getTenantMailService" ? service() : data,
      }),
    );
  });
  try {
    await view.render();
    await until(() => view.host.textContent.includes("Grant mailbox access"));
    const form = view.host.querySelector(
      'form[aria-label="Grant mailbox access"]',
    );
    assert.ok(form);
    const mailbox = form.querySelector<HTMLSelectElement>('[name="mailboxID"]');
    const subject = form.querySelector<HTMLSelectElement>('[name="subject"]');
    const confirm = form.querySelector<HTMLInputElement>('[name="confirm"]');
    assert.ok(mailbox);
    assert.ok(subject);
    assert.ok(confirm);
    mailbox.value = "401";
    subject.value = "501";
    confirm.checked = true;
    await flush(() => {
      subject.value = "502";
      subject.dispatchEvent(new Event("change", { bubbles: true }));
    });
    await flush(() => {
      form.dispatchEvent(
        new Event("submit", { bubbles: true, cancelable: true }),
      );
    });
    assert.equal(
      writes,
      0,
      "confirmation for the prior recipient must not authorize a new grant",
    );
    assert.equal(confirm.checked, false);
  } finally {
    await view.close();
  }
});
