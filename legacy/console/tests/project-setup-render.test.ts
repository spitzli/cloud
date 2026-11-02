import test from "node:test";
import assert from "node:assert/strict";
import { act, createElement as h, useState } from "react";
import { createRoot } from "react-dom/client";
import { renderToStaticMarkup } from "react-dom/server";
import { Window } from "happy-dom";
import { I18nProvider } from "@webdock/i18n/react";
import { SetupSourceFields } from "../src/components/hosting/setup-source-fields";
import { SetupReview } from "../src/components/hosting/setup-review";
import { defaultSetupConfiguration } from "../src/lib/project-setup-view";
import type {
  SetupOptions,
  SetupView,
  SetupConfiguration,
} from "@webdock/hosting-contracts";
const options: SetupOptions = {
  enabled: true,
  project: {
    projectID: "11",
    customerID: "12",
    name: "Fixture",
    customerName: "Fixture",
    configured: true,
    provider: "k3s",
    canManage: true,
    canAssignHosting: false,
  },
  applications: [],
  clusters: [],
  connections: [
    {
      id: "13",
      accountLogin: "fixture",
      state: "active",
      repositories: [
        {
          repositoryID: "14",
          fullName: "fixture/Language",
          defaultBranch: "main",
        },
        { repositoryID: "15", fullName: "fixture/api", defaultBranch: "trunk" },
      ],
    },
  ],
  builds: { isolated: true, githubActions: false },
  databases: [],
  plans: [],
  mail: [],
  allowances: {
    effective: {
      apps: 10,
      cpuMillicores: 4000,
      memoryBytes: 4294967296,
      ephemeralBytes: 4294967296,
      volumeBytes: 0,
      replicasPerApp: 2,
      concurrentDeployments: 2,
    },
    subscriptionRevision: 1,
  },
};
test("repository selection follows its default branch and preserves customer-authored names", async () => {
  const window = new Window();
  Object.assign(globalThis, {
    window,
    document: window.document,
    IS_REACT_ACT_ENVIRONMENT: true,
  });
  const container = window.document.createElement("div");
  window.document.body.append(container);
  const root = createRoot(container as unknown as HTMLElement);
  function Harness() {
    const [configuration, set] = useState<SetupConfiguration>({
      ...defaultSetupConfiguration([]),
      source: {
        kind: "git" as const,
        connectionID: "13",
        repositoryID: "14",
        branch: "main",
        rootDirectory: ".",
        dockerfile: "Dockerfile",
        buildProvider: "isolated" as const,
        workflowPath: ".github/workflows/webdock.yml",
        artifactPrefix: "webdock",
      },
    });
    return h(SetupSourceFields, { configuration, options, onChange: set });
  }
  try {
    await act(async () =>
      root.render(
        h(I18nProvider, { locale: "de", preference: "de" }, h(Harness)),
      ),
    );
    const repository = container.querySelectorAll("select")[1];
    assert.ok(repository);
    assert.ok(container.textContent.includes("fixture/Language"));
    await act(async () => {
      repository.value = "15";
      repository.dispatchEvent(new window.Event("change", { bubbles: true }));
    });
    assert.equal(
      container.querySelector('input[maxlength="200"]')?.getAttribute("value"),
      "trunk",
    );
    assert.equal(
      container
        .querySelector('input[type="checkbox"]')
        ?.hasAttribute("checked"),
      false,
    );
  } finally {
    await act(async () => root.unmount());
    await window.happyDOM.close();
  }
});
test("review renders explicit unchecked price consent and only secret names", () => {
  const view: SetupView = {
    id: "16",
    projectID: "11",
    revision: 1,
    state: "awaiting-confirmation",
    configuration: defaultSetupConfiguration([]),
    environmentNames: ["PRIVATE_TOKEN"],
    resources: {},
    steps: [],
    reason: null,
    preview: {
      id: "00000000-0000-4000-8000-000000000000",
      revision: 1,
      expiresAt: "2026-10-15T12:00:00Z",
      sourceSHA: null,
      image: null,
      subscriptionRevision: 1,
      blockers: [],
      offers: [
        {
          id: "42",
          name: "Fixture database",
          monthlyPriceCents: 590,
          currency: "EUR",
          terms: "Fixture terms",
          expiresAt: "2026-10-15T12:00:00Z",
          backupStatus: "not-configured",
        },
      ],
    },
  };
  const html = renderToStaticMarkup(
    h(
      I18nProvider,
      { locale: "en", preference: "en" },
      h(SetupReview, { view, options, accepted: [], onAccept: () => {} }),
    ),
  );
  assert.match(html, /name="acceptedOfferIDs" value="42"/);
  assert.doesNotMatch(html, /checked=""/);
  assert.match(html, /Fixture terms/);
  assert.doesNotMatch(html, /password|secret_encrypted|environment_encrypted/);
});
