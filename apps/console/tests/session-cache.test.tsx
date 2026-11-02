import { I18nProvider } from "@webdock/i18n/react";
import assert from "node:assert/strict";
import { after, test } from "node:test";
import { act } from "react";
import { createRoot } from "react-dom/client";
import {
  QueryClient,
  QueryClientProvider,
  notifyManager,
  defaultScheduler,
  useQuery,
} from "@tanstack/react-query";
import { Window } from "happy-dom";
import { ApiClient, ApiError, type Session } from "../src/api";
import { ApiContext } from "../src/context";
import { AccountApp } from "../src/AccountUI";
import {
  retirePrivateCache,
  sessionCacheScope,
  useSessionCacheIsolation,
} from "../src/session-cache";

// Keep TanStack notifications inside React act's microtask flush, not a later timer.
notifyManager.setScheduler(queueMicrotask);
after(() => {
  notifyManager.setScheduler(defaultScheduler);
});

const account = (id: string): Session => ({
  user: {
    id,
    name: id,
    email: `${id}@example.test`,
    role: "operator",
    emailVerified: true,
    twoFactorEnabled: true,
  },
  session: {
    id: `session-${id}`,
    userId: id,
    expiresAt: "2099-01-01T00:00:00Z",
  },
});
const privateKeys = [
  ["registry"],
  ["tenants"],
  ["sites"],
  ["databases", "1"],
  ["database-management", "1"],
  ["database-capacity"],
  ["plugins"],
  ["account-passkeys"],
  ["account-sessions"],
  ["account-sites"],
  ["consent", "request"],
  ["invitation", "1"],
];
function PrivateView({
  identity,
  renders,
}: {
  identity: string;
  renders: string[];
}) {
  const value = useQuery({
    queryKey: ["tenants"],
    queryFn: () => Promise.resolve(`private-${identity}`),
    staleTime: Infinity,
  });
  renders.push(`${identity}:${value.data ?? "pending"}`);
  return <p>{value.data ?? "pending"}</p>;
}
interface Authority {
  session: Session | null;
  error?: Error;
  verifiedAt?: number;
}
function Harness({
  authority,
  renders,
}: {
  authority: Authority;
  renders: string[];
}) {
  const ready = useSessionCacheIsolation(
    authority.error ? null : authority.session,
    authority.verifiedAt,
  );
  if (!ready) return <p>Isolating session</p>;
  return authority.session && !authority.error ? (
    <PrivateView identity={authority.session.user.id} renders={renders} />
  ) : (
    <p>Signed out</p>
  );
}
async function flush(action: () => void) {
  await act(async () => {
    action();
    await Promise.resolve();
  });
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
}
function setup() {
  const browser = new Window({ url: "https://console.example/account" });
  Object.assign(globalThis, {
    window: browser, navigator: browser.navigator,
    location: browser.location,
    document: browser.document,
    HTMLElement: browser.HTMLElement,
    FormData: browser.FormData,
    Event: browser.Event,
    IS_REACT_ACT_ENVIRONMENT: true,
  });
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false, gcTime: Infinity },
      mutations: { retry: false },
    },
  });
  return {
    host,
    client,
    root,
    close: async () => {
      await flush(() => {
        root.unmount();
      });
      client.clear();
      await browser.happyDOM.abort();
    },
  };
}

await test("a fresh private query cannot render for the next principal, including the first render", async () => {
  const fixture = setup();
  const renders: string[] = [];
  const render = (authority: Authority) =>
    flush(() => {
      fixture.root.render(
        <QueryClientProvider client={fixture.client}>
          <Harness authority={authority} renders={renders} />
        </QueryClientProvider>,
      );
    });
  try {
    await render({ session: account("A") });
    assert.equal(fixture.host.textContent, "private-A");
    await render({ session: account("B") });
    assert.equal(fixture.host.textContent, "private-B");
    assert.ok(
      !renders.includes("B:private-A"),
      `Old data rendered: ${renders.join(", ")}`,
    );
  } finally {
    await fixture.close();
  }
});

await test("null and 401 retire every private namespace while preserving session/runtime queries", async () => {
  const fixture = setup();
  const render = (authority: Authority) =>
    flush(() => {
      fixture.root.render(
        <QueryClientProvider client={fixture.client}>
          <Harness authority={authority} renders={[]} />
        </QueryClientProvider>,
      );
    });
  try {
    for (const authority of [
      { session: null },
      { session: account("A"), error: new ApiError(401, "Expired") },
    ]) {
      await render({ session: account("A") });
      for (const key of privateKeys)
        fixture.client.setQueryData(key, "A-private");
      fixture.client.setQueryData(["session"], { data: null });
      fixture.client.setQueryData(["account-session"], null);
      fixture.client.setQueryData(["runtime-config", 0], {
        surface: "console",
      });
      await render(authority);
      assert.equal(fixture.host.textContent, "Signed out");
      for (const key of privateKeys)
        assert.equal(fixture.client.getQueryData(key), undefined);
      assert.deepEqual(fixture.client.getQueryData(["session"]), {
        data: null,
      });
      assert.equal(fixture.client.getQueryData(["account-session"]), null);
      assert.deepEqual(fixture.client.getQueryData(["runtime-config", 0]), {
        surface: "console",
      });
    }
  } finally {
    await fixture.close();
  }
});

await test("session, role, preview and security transitions purge fresh private data", async () => {
  const fixture = setup();
  const original = account("A");
  const variants: Session[] = [
    { ...original, session: { ...original.session, id: "rotated" } },
    { ...original, user: { ...original.user, role: "reader" } },
    { ...original, user: { ...original.user, preview: true } },
    { ...original, user: { ...original.user, preview: true, acting: true, customerScope: "owner-epoch-a" } },
    { ...original, user: { ...original.user, preview: true, acting: true, customerScope: "owner-epoch-b" } },
    { ...original, user: { ...original.user, emailVerified: false } },
    { ...original, user: { ...original.user, mustChangePassword: true } },
    { ...original, user: { ...original.user, twoFactorEnabled: false } },
  ];
  const render = (session: Session, verifiedAt = 1) =>
    flush(() => {
      fixture.root.render(
        <QueryClientProvider client={fixture.client}>
          <Harness authority={{ session, verifiedAt }} renders={[]} />
        </QueryClientProvider>,
      );
    });
  try {
    for (const session of variants) {
      await render(original);
      fixture.client.setQueryData(["database-management"], "old private plan");
      assert.notEqual(sessionCacheScope(original), sessionCacheScope(session));
      await render(session);
      assert.equal(
        fixture.client.getQueryData(["database-management"]),
        undefined,
      );
    }
    const preview = { ...original, user: { ...original.user, preview: true } };
    await render(preview, 1);
    fixture.client.setQueryData(
      ["database-management"],
      "previous preview target",
    );
    await render(preview, 2);
    assert.equal(
      fixture.client.getQueryData(["database-management"]),
      undefined,
    );
    fixture.client.setQueryData(["database-management"], "same scope");
    await render(preview, 2);
    assert.equal(
      fixture.client.getQueryData(["database-management"]),
      "same scope",
    );
  } finally {
    await fixture.close();
  }
});

await test("cancelled old queries and retired mutations cannot repopulate the new cache", async () => {
  const client = new QueryClient();
  const oldResponse = deferred();
  const mutationResponse = deferred();
  let signal: AbortSignal | undefined;
  const request = client
    .query({
      queryKey: ["databases"],
      queryFn: (context) => {
        signal = context.signal;
        return oldResponse.promise;
      },
    })
    .catch(() => undefined);
  const mutation = client.getMutationCache().build(client, {
    mutationFn: () => mutationResponse.promise,
    onSuccess: (value) => {
      client.setQueryData(["databases"], value);
    },
    onSettled: () => {
      client.setQueryData(["account-passkeys"], "old callback");
    },
  });
  const pendingMutation = mutation.execute(undefined);
  await Promise.resolve();
  await retirePrivateCache(client);
  assert.equal(signal?.aborted, true);
  assert.equal(client.getMutationCache().getAll().length, 0);
  client.setQueryData(["databases"], "B-private");
  oldResponse.resolve("A-private");
  mutationResponse.resolve("A-private");
  await Promise.all([request, pendingMutation]);
  assert.equal(client.getQueryData(["databases"]), "B-private");
  assert.equal(client.getQueryData(["account-passkeys"]), undefined);
  assert.equal(client.getMutationCache().getAll().length, 0);
  client.clear();
});

await test("AccountApp reuses the root client and retires account security data on session refresh", async () => {
  const fixture = setup();
  let current = "A";
  let sessionReads = 0;
  const fetcher: typeof fetch = (input) => {
    assert.ok(input instanceof URL);
    const path = input.pathname;
    if (path.endsWith("get-session")) {
      sessionReads++;
      return Promise.resolve(Response.json(account(current)));
    }
    if (path.endsWith("list-user-passkeys"))
      return Promise.resolve(
        Response.json([
          { id: current, name: `${current} passkey`, createdAt: null },
        ]),
      );
    if (path.endsWith("list-sessions"))
      return Promise.resolve(Response.json([]));
    return Promise.resolve(Response.json({}, { status: 404 }));
  };
  const api = new ApiClient(
    {
      apiOrigin: "https://console.example",
      authOrigin: "https://console.example",
      enabledFeatures: [],
    },
    fetcher,
  );
  try {
    await flush(() => {
      fixture.root.render(
        <QueryClientProvider client={fixture.client}>
          <ApiContext.Provider value={api}>
            <I18nProvider locale="en" preference="system"><AccountApp /></I18nProvider>
          </ApiContext.Provider>
        </QueryClientProvider>,
      );
    });
    await flush(() => {
      /* Flush query notifications after the initial authority barrier. */
    });
    await waitFor(() => fixture.host.querySelector("#passkey-A") !== null);
    assert.equal(
      fixture.host.querySelector<HTMLInputElement>("#passkey-A")?.value,
      "A passkey",
    );
    current = "B";
    await act(async () => {
      await fixture.client.invalidateQueries({ queryKey: ["account-session"] });
    });
    await flush(() => {
      /* Flush private reads after the new session is confirmed. */
    });
    await waitFor(() => fixture.host.querySelector("#passkey-B") !== null);
    assert.equal(
      fixture.host.querySelector<HTMLInputElement>("#passkey-B")?.value,
      "B passkey",
    );
    assert.equal(fixture.host.querySelector("#passkey-A"), null);
    assert.equal(sessionReads, 2);
    assert.equal(
      fixture.client.getQueryData<Session>(["account-session"])?.user.id,
      "B",
    );
  } finally {
    await fixture.close();
  }
});

function deferred<T = string>() {
  let resolve: (value: T) => void = () => {
    throw new Error("Promise not initialized");
  };
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

await test("account password mutation still invalidates the root session across security changes", async () => {
  const fixture = setup();
  let mustChangePassword = true;
  let sessionReads = 0;
  const fetcher: typeof fetch = (input) => {
    assert.ok(input instanceof URL);
    if (input.pathname.endsWith("get-session")) {
      sessionReads++;
      const session = account("A");
      session.user.mustChangePassword = mustChangePassword;
      session.session.id = mustChangePassword
        ? "initial-password-session"
        : "changed-password-session";
      return Promise.resolve(Response.json(session));
    }
    if (input.pathname.endsWith("change-password")) {
      mustChangePassword = false;
      return Promise.resolve(Response.json({ status: true }));
    }
    return Promise.resolve(Response.json([]));
  };
  const api = new ApiClient(
    {
      apiOrigin: "https://console.example",
      authOrigin: "https://console.example",
      enabledFeatures: [],
    },
    fetcher,
  );
  try {
    await flush(() => {
      fixture.root.render(
        <QueryClientProvider client={fixture.client}>
          <ApiContext.Provider value={api}>
            <I18nProvider locale="en" preference="system"><AccountApp /></I18nProvider>
          </ApiContext.Provider>
        </QueryClientProvider>,
      );
    });
    assert.match(fixture.host.textContent, /Set your own password to continue/);
    const form = fixture.host.querySelector<HTMLFormElement>(
      ".security-password form",
    );
    assert.ok(form);
    for (const input of form.querySelectorAll<HTMLInputElement>("input"))
      input.value = "new-password-12345";
    await flush(() => {
      form.dispatchEvent(
        new Event("submit", { bubbles: true, cancelable: true }),
      );
    });
    await flush(() => {
      /* Finish the session refetch triggered by the mutation callback. */
    });
    assert.equal(sessionReads, 2);
    assert.equal(
      fixture.client.getQueryData<Session>(["account-session"])?.user
        .mustChangePassword,
      false,
    );
    assert.doesNotMatch(
      fixture.host.textContent,
      /Set your own password to continue/,
    );
    assert.match(fixture.host.textContent, /Password updated\./);
    await flush(() => {
      fixture.client.setQueryData(["account-session"], account("B"));
    });
    assert.doesNotMatch(fixture.host.textContent, /Password updated\./);
    await flush(() => {
      fixture.client.setQueryData(["account-session"], account("A"));
    });
    assert.doesNotMatch(fixture.host.textContent, /Password updated\./);
  } finally {
    await fixture.close();
  }
});

await test("cached account authority cannot redirect before successful current-session verification", async () => {
  for (const status of [200, 401]) {
    const fixture = setup();
    const response = deferred<Response>();
    const redirects: string[] = [];
    const target = "/api/auth/oauth2/authorize?client_id=test";
    Object.assign(globalThis, {
      location: {
        pathname: "/account",
        origin: "https://console.example",
        search: `?returnTo=${encodeURIComponent(target)}`,
        replace: (url: string) => {
          redirects.push(url);
        },
      },
    });
    fixture.client.setQueryData(["account-session"], account("A"));
    const api = new ApiClient(
      {
        apiOrigin: "https://console.example",
        authOrigin: "https://console.example",
        enabledFeatures: [],
      },
      () => response.promise,
    );
    try {
      await flush(() => {
        fixture.root.render(
          <QueryClientProvider client={fixture.client}>
            <ApiContext.Provider value={api}>
              <I18nProvider locale="en" preference="system"><AccountApp /></I18nProvider>
            </ApiContext.Provider>
          </QueryClientProvider>,
        );
      });
      assert.deepEqual(redirects, []);
      assert.match(fixture.host.textContent, /Opening your account/);
      await flush(() => {
        response.resolve(
          Response.json(status === 200 ? account("B") : {}, { status }),
        );
      });
      assert.deepEqual(redirects, status === 200 ? [target] : []);
    } finally {
      await fixture.close();
    }
  }
});

await test("reauthentication proof cannot remove a different principal's prompt or max_age", async () => {
  const fixture = setup();
  const redirects: string[] = [];
  const target =
    "/api/auth/oauth2/authorize?client_id=test&prompt=login&max_age=0";
  Object.assign(globalThis, {
    location: {
      pathname: "/account",
      origin: "https://console.example",
      search: `?returnTo=${encodeURIComponent(target)}`,
      replace: (url: string) => {
        redirects.push(url);
      },
    },
  });
  let current = account("A");
  const fetcher: typeof fetch = (input) => {
    assert.ok(input instanceof URL);
    if (input.pathname.endsWith("get-session"))
      return Promise.resolve(Response.json(current));
    if (input.pathname.endsWith("sign-in/email")) {
      current = {
        ...current,
        session: { ...current.session, id: "reauthenticated-A" },
      };
      return Promise.resolve(Response.json({ status: true }));
    }
    return Promise.resolve(Response.json([]));
  };
  const api = new ApiClient(
    {
      apiOrigin: "https://console.example",
      authOrigin: "https://console.example",
      enabledFeatures: [],
    },
    fetcher,
  );
  try {
    await flush(() => {
      fixture.root.render(
        <QueryClientProvider client={fixture.client}>
          <ApiContext.Provider value={api}>
            <I18nProvider locale="en" preference="system"><AccountApp /></I18nProvider>
          </ApiContext.Provider>
        </QueryClientProvider>,
      );
    });
    assert.match(fixture.host.textContent, /Sign in to Spitzli Cloud/);
    const form = fixture.host.querySelector<HTMLFormElement>("form");
    assert.ok(form);
    for (const input of form.querySelectorAll<HTMLInputElement>("input"))
      input.value =
        input.name === "email" ? "A@example.test" : "password-12345";
    await flush(() => {
      form.dispatchEvent(
        new Event("submit", { bubbles: true, cancelable: true }),
      );
    });
    assert.deepEqual(redirects, ["/api/auth/oauth2/authorize?client_id=test"]);
    current = account("B");
    await act(async () => {
      await fixture.client.invalidateQueries({ queryKey: ["account-session"] });
    });
    await flush(() => {
      /* Commit the new principal's authority barrier. */
    });
    assert.deepEqual(redirects, ["/api/auth/oauth2/authorize?client_id=test"]);
    assert.match(fixture.host.textContent, /Sign in to Spitzli Cloud/);
  } finally {
    await fixture.close();
  }
});

async function waitFor(condition: () => boolean) {
  for (let attempt = 0; attempt < 20; attempt++) {
    if (condition()) return;
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 1));
    });
  }
  assert.ok(condition(), "React did not render the settled query result");
}

/** Acknowledgement is only offered after the one-time codes were downloaded or copied. */
async function keepRecoveryCodes(host: HTMLElement) {
  const button = (label: string) =>
    [...host.querySelectorAll<HTMLButtonElement>("button")].find((item) => item.textContent === label);
  const done = button("I saved my recovery codes");
  assert.ok(done);
  assert.equal(done.disabled, true);
  const download = button("Download codes");
  assert.ok(download);
  await flush(() => {
    download.click();
  });
  assert.equal(done.disabled, false);
  return done;
}
async function submitAccountForm(
  host: HTMLElement,
  buttonText: string,
  value: string,
) {
  const button = [...host.querySelectorAll<HTMLButtonElement>("button")].find(
    (item) => item.textContent === buttonText,
  );
  const form = button?.closest("form");
  assert.ok(form);
  for (const input of form.querySelectorAll<HTMLInputElement>("input"))
    input.value = value;
  await flush(() => {
    form.dispatchEvent(
      new Event("submit", { bubbles: true, cancelable: true }),
    );
  });
}

await test("MFA polling preserves owned recovery codes and cannot continue SSO before acknowledgement", async () => {
  const fixture = setup();
  const redirects: string[] = [];
  const target = "/api/auth/oauth2/authorize?client_id=test";
  Object.assign(globalThis, {
    location: {
      pathname: "/account",
      origin: "https://console.example",
      search: `?returnTo=${encodeURIComponent(target)}`,
      replace: (url: string) => {
        redirects.push(url);
      },
    },
  });
  const current = account("A");
  current.user.twoFactorEnabled = false;
  const fetcher: typeof fetch = (input) => {
    assert.ok(input instanceof URL);
    if (input.pathname.endsWith("get-session"))
      return Promise.resolve(Response.json(current));
    if (input.pathname.endsWith("two-factor/enable"))
      return Promise.resolve(
        Response.json({
          totpURI: "otpauth://totp/Test?secret=JBSWY3DPEHPK3PXP",
          backupCodes: ["fixture-recovery-one", "fixture-recovery-two"],
        }),
      );
    if (input.pathname.endsWith("two-factor/verify-totp")) {
      current.user.twoFactorEnabled = true;
      return Promise.resolve(Response.json({ status: true }));
    }
    return Promise.resolve(Response.json([]));
  };
  const api = new ApiClient(
    {
      apiOrigin: "https://console.example",
      authOrigin: "https://console.example",
      enabledFeatures: [],
    },
    fetcher,
  );
  try {
    await flush(() => {
      fixture.root.render(
        <QueryClientProvider client={fixture.client}>
          <ApiContext.Provider value={api}>
            <I18nProvider locale="en" preference="system"><AccountApp /></I18nProvider>
          </ApiContext.Provider>
        </QueryClientProvider>,
      );
    });
    await submitAccountForm(
      fixture.host,
      "Set up authenticator",
      "password-12345",
    );
    fixture.client.setQueryData(["database-capacity"], "old-private-data");
    await submitAccountForm(fixture.host, "Verify and enable", "123456");
    assert.match(fixture.host.textContent, /fixture-recovery-one/);
    await act(async () => {
      await fixture.client.invalidateQueries({ queryKey: ["account-session"] });
    });
    await flush(() => {
      /* Simulate the account-session poll confirming the enabled authenticator. */
    });
    assert.match(fixture.host.textContent, /fixture-recovery-one/);
    assert.equal(fixture.client.getQueryData(["database-capacity"]), undefined);
    assert.deepEqual(redirects, []);
    const done = await keepRecoveryCodes(fixture.host);
    await flush(() => {
      done.click();
    });
    assert.doesNotMatch(fixture.host.textContent, /fixture-recovery-one/);
    assert.deepEqual(redirects, [target]);
  } finally {
    await fixture.close();
  }
});

await test("owned recovery codes clear permanently on logout, revocation, user/session replacement and preview", async () => {
  const original = account("A");
  const transitions = [
    { session: null, status: 200 },
    { session: original, status: 401 },
    { session: account("B"), status: 200 },
    {
      session: {
        ...original,
        session: { ...original.session, id: "replacement-session" },
      },
      status: 200,
    },
    {
      session: { ...original, user: { ...original.user, preview: true } },
      status: 200,
    },
    {
      session: {
        ...original,
        user: { ...original.user, twoFactorEnabled: false },
      },
      status: 200,
    },
  ];
  for (const transition of transitions) {
    const fixture = setup();
    let current: { session: Session | null; status: number } = {
      session: original,
      status: 200,
    };
    const fetcher: typeof fetch = (input) => {
      assert.ok(input instanceof URL);
      if (input.pathname.endsWith("get-session"))
        return Promise.resolve(
          Response.json(current.session, { status: current.status }),
        );
      if (input.pathname.endsWith("generate-backup-codes"))
        return Promise.resolve(
          Response.json({ backupCodes: ["owned-fixture-recovery-code"] }),
        );
      return Promise.resolve(Response.json([]));
    };
    const api = new ApiClient(
      {
        apiOrigin: "https://console.example",
        authOrigin: "https://console.example",
        enabledFeatures: [],
      },
      fetcher,
    );
    try {
      await flush(() => {
        fixture.root.render(
          <QueryClientProvider client={fixture.client}>
            <ApiContext.Provider value={api}>
              <I18nProvider locale="en" preference="system"><AccountApp /></I18nProvider>
            </ApiContext.Provider>
          </QueryClientProvider>,
        );
      });
      await submitAccountForm(
        fixture.host,
        "Generate new recovery codes",
        "password-12345",
      );
      assert.match(fixture.host.textContent, /owned-fixture-recovery-code/);
      current = transition;
      await act(async () => {
        await fixture.client.invalidateQueries({
          queryKey: ["account-session"],
        });
      });
      await flush(() => {
        /* Commit revocation of the ephemeral secret owner. */
      });
      assert.doesNotMatch(
        fixture.host.textContent,
        /owned-fixture-recovery-code/,
      );
      current = { session: original, status: 200 };
      await act(async () => {
        await fixture.client.invalidateQueries({
          queryKey: ["account-session"],
        });
      });
      await flush(() => {
        /* Returning to the previous identity must not restore old codes. */
      });
      assert.doesNotMatch(
        fixture.host.textContent,
        /owned-fixture-recovery-code/,
      );
    } finally {
      await fixture.close();
    }
  }
});

async function forcedSetup(
  force: string,
  changePassword: boolean,
  rotateMfa: boolean,
) {
  const fixture = setup();
  const redirects: string[] = [];
  const target = `/api/auth/oauth2/authorize?client_id=test&${force}`;
  Object.assign(globalThis, {
    location: {
      pathname: "/account",
      origin: "https://console.example",
      search: `?returnTo=${encodeURIComponent(target)}`,
      replace: (url: string) => {
        redirects.push(url);
      },
    },
  });
  let current = account("A");
  current.user.twoFactorEnabled = false;
  current.user.mustChangePassword = changePassword;
  const fetcher: typeof fetch = (input) => {
    assert.ok(input instanceof URL);
    if (input.pathname.endsWith("get-session"))
      return Promise.resolve(Response.json(current));
    if (input.pathname.endsWith("sign-in/email")) {
      current = {
        ...current,
        session: { ...current.session, id: "forced-initial-login" },
      };
      return Promise.resolve(Response.json({ status: true }));
    }
    if (input.pathname.endsWith("change-password")) {
      current = {
        ...current,
        user: { ...current.user, mustChangePassword: false },
        session: { ...current.session, id: "forced-password-rotation" },
      };
      return Promise.resolve(Response.json({ status: true }));
    }
    if (input.pathname.endsWith("two-factor/enable"))
      return Promise.resolve(
        Response.json({
          totpURI: "otpauth://totp/Test?secret=JBSWY3DPEHPK3PXP",
          backupCodes: ["forced-setup-recovery-code"],
        }),
      );
    if (input.pathname.endsWith("two-factor/verify-totp")) {
      current = {
        ...current,
        user: { ...current.user, twoFactorEnabled: true },
        session: {
          ...current.session,
          id: rotateMfa ? "forced-mfa-rotation" : current.session.id,
        },
      };
      return Promise.resolve(Response.json({ status: true }));
    }
    return Promise.resolve(Response.json([]));
  };
  const api = new ApiClient(
    {
      apiOrigin: "https://console.example",
      authOrigin: "https://console.example",
      enabledFeatures: [],
    },
    fetcher,
  );
  try {
    await flush(() => {
      fixture.root.render(
        <QueryClientProvider client={fixture.client}>
          <ApiContext.Provider value={api}>
            <I18nProvider locale="en" preference="system"><AccountApp /></I18nProvider>
          </ApiContext.Provider>
        </QueryClientProvider>,
      );
    });
    await submitAccountForm(fixture.host, "Sign in", "password-12345");
    if (changePassword) {
      await submitAccountForm(
        fixture.host,
        "Update password",
        "new-password-12345",
      );
      assert.match(fixture.host.textContent, /Password updated\./);
    }
    await submitAccountForm(
      fixture.host,
      "Set up authenticator",
      "password-12345",
    );
    await submitAccountForm(fixture.host, "Verify and enable", "123456");
    assert.match(fixture.host.textContent, /forced-setup-recovery-code/);
    assert.deepEqual(redirects, []);
    await act(async () => {
      await fixture.client.invalidateQueries({ queryKey: ["account-session"] });
    });
    await flush(() => {
      /* A verified background refresh must not hide unacknowledged codes. */
    });
    assert.match(fixture.host.textContent, /forced-setup-recovery-code/);
    assert.deepEqual(redirects, []);
    const done = await keepRecoveryCodes(fixture.host);
    await flush(() => {
      done.click();
    });
    assert.deepEqual(redirects, ["/api/auth/oauth2/authorize?client_id=test"]);
    assert.doesNotMatch(fixture.host.textContent, /forced-setup-recovery-code/);
    current = {
      ...current,
      session: { ...current.session, id: "unrelated-replacement-session" },
    };
    await act(async () => {
      await fixture.client.invalidateQueries({ queryKey: ["account-session"] });
    });
    await flush(() => {
      /* A different session cannot reuse the explicit setup proof. */
    });
    assert.match(fixture.host.textContent, /Sign in to Spitzli Cloud/);
    assert.equal(redirects.length, 1);
  } finally {
    await fixture.close();
  }
}

await test("forced login preserves same-session MFA codes until acknowledgement", async () => {
  await forcedSetup("prompt=login", false, false);
});
await test("forced initial setup survives password and native MFA session rotation", async () => {
  await forcedSetup("prompt=login", true, true);
});
await test("max_age initial setup continues only after owned rotating-MFA code acknowledgement", async () => {
  await forcedSetup("max_age=0", true, true);
});

await test("MFA transition rejects mismatched readback and aborts an A-to-B race without publishing codes or proof", async () => {
  for (const failure of ["user", "role", "preview", "unavailable", "race"]) {
    const fixture = setup();
    const response = deferred<Response>();
    const redirects: string[] = [];
    const target = "/api/auth/oauth2/authorize?client_id=test&prompt=login";
    Object.assign(globalThis, {
      location: {
        pathname: "/account",
        origin: "https://console.example",
        search: `?returnTo=${encodeURIComponent(target)}`,
        replace: (url: string) => {
          redirects.push(url);
        },
      },
    });
    const before = account("A");
    before.user.twoFactorEnabled = false;
    const after = account("A");
    after.session.id = "confirmed-mfa-session";
    let verifying = false;
    let readbackSignal: AbortSignal | null | undefined;
    const fetcher: typeof fetch = (input, options) => {
      assert.ok(input instanceof URL);
      if (input.pathname.endsWith("get-session")) {
        if (!verifying) return Promise.resolve(Response.json(before));
        readbackSignal = options?.signal;
        return response.promise;
      }
      if (input.pathname.endsWith("two-factor/enable"))
        return Promise.resolve(
          Response.json({
            totpURI: "otpauth://totp/Test?secret=JBSWY3DPEHPK3PXP",
            backupCodes: ["never-publish-this-fixture-code"],
          }),
        );
      if (input.pathname.endsWith("two-factor/verify-totp")) {
        verifying = true;
        return Promise.resolve(Response.json({ status: true }));
      }
      if (input.pathname.endsWith("sign-in/email"))
        return Promise.resolve(Response.json({ status: true }));
      return Promise.resolve(Response.json([]));
    };
    const api = new ApiClient(
      {
        apiOrigin: "https://console.example",
        authOrigin: "https://console.example",
        enabledFeatures: [],
      },
      fetcher,
    );
    try {
      await flush(() => {
        fixture.root.render(
          <QueryClientProvider client={fixture.client}>
            <ApiContext.Provider value={api}>
              <I18nProvider locale="en" preference="system"><AccountApp /></I18nProvider>
            </ApiContext.Provider>
          </QueryClientProvider>,
        );
      });
      await submitAccountForm(fixture.host, "Sign in", "password-12345");
      await submitAccountForm(
        fixture.host,
        "Set up authenticator",
        "password-12345",
      );
      await submitAccountForm(fixture.host, "Verify and enable", "123456");
      assert.ok(readbackSignal);
      assert.doesNotMatch(
        fixture.host.textContent,
        /never-publish-this-fixture-code/,
      );
      if (failure === "race") {
        await flush(() => {
          fixture.client.setQueryData(["account-session"], account("B"));
        });
        assert.equal(readbackSignal.aborted, true);
      }
      const candidate = mfaReadback(failure, after);
      await flush(() => {
        response.resolve(candidate);
      });
      assert.doesNotMatch(
        fixture.host.textContent,
        /never-publish-this-fixture-code/,
      );
      assert.deepEqual(redirects, []);
      if (failure === "race") {
        assert.equal(
          fixture.client.getQueryData<Session>(["account-session"])?.user.id,
          "B",
        );
      } else {
        assert.match(
          fixture.host.textContent,
          /current session could not be verified/,
        );
        assert.equal(
          fixture.client.getQueryData<Session>(["account-session"])?.session.id,
          before.session.id,
        );
      }
    } finally {
      await fixture.close();
    }
  }
});

function mfaReadback(failure: string, session: Session) {
  switch (failure) {
    case "user":
      return Response.json(account("B"));
    case "role":
      return Response.json({
        ...session,
        user: { ...session.user, role: "reader" },
      });
    case "preview":
      return Response.json({
        ...session,
        user: { ...session.user, preview: true },
      });
    case "unavailable":
      return Response.json({}, { status: 503 });
    default:
      return Response.json(session);
  }
}
