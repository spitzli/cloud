import {
  PluginError,
  type ManagedAppProvisioning,
} from "@webdock/plugin-sdk/contracts";
import type {
  ManagedAppsScope,
  ManagedAppsProvisioningRequest,
} from "@webdock/core/plugin-apps";
import type {
  HostingActor,
  SetupConfiguration,
  SetupView,
} from "@webdock/hosting-contracts";
import type { SetupRow } from "@webdock/module-hosting/server/project-setup";
export type ProvisionedAppBinding = {
  appID: string;
  websiteBindingID: string;
  origin: string;
  secretReference: string;
  pinnedAddresses: string[];
  ca?: string;
};
export type ProvisioningRecord = {
  id: string;
  pluginID: string;
  customerID: string;
  projectID: string;
  templateID: string;
  name: string;
  actor: HostingActor;
  pluginGeneration: number;
  grantRevision: number;
  setupID: string | null;
  binding: ProvisionedAppBinding | null;
  templateState: Record<string, unknown>;
};
export type ApprovedAppTemplate = {
  id: string;
  configuration: Omit<SetupConfiguration, "name">;
  environment(
    scope: ManagedAppsScope,
    provisioningID: string,
    name: string,
  ): Promise<{ name: string; value: string }[]>;
};
export type TemplateHookContext = {
  record: ProvisioningRecord;
  setup: SetupRow;
  template: ApprovedAppTemplate;
  actor: HostingActor;
};
export type ApprovedAppTemplateHooks = {
  beforeRelease(context: TemplateHookContext): Promise<{ ready: boolean }>;
  publish(context: TemplateHookContext): Promise<ProvisionedAppBinding | null>;
};
export type ProvisioningStatusView = Pick<
  SetupView,
  "state" | "resources" | "steps"
>;
export type ProvisioningStore = {
  status(
    record: ProvisioningRecord,
  ): Promise<ProvisioningStatusView | undefined>;
  list(
    scope: Pick<ManagedAppsScope, "pluginID" | "customerID" | "projectID">,
  ): Promise<ProvisioningRecord[]>;
  reserve(
    scope: ManagedAppsScope,
    input: Extract<
      ManagedAppsProvisioningRequest,
      { operation: "provision" }
    >["input"],
  ): Promise<ProvisioningRecord>;
  get(
    scope: Pick<ManagedAppsScope, "pluginID" | "customerID" | "projectID">,
    id: string,
  ): Promise<ProvisioningRecord | null>;
  attachSetup(record: ProvisioningRecord, setupID: string): Promise<void>;
  assertLive(
    scope: Pick<
      ManagedAppsScope,
      | "pluginID"
      | "customerID"
      | "projectID"
      | "pluginGeneration"
      | "grantRevision"
    >,
    capability?: "plugin.apps.read" | "plugin.apps.provision",
  ): Promise<void>;
};
function status(
  record: ProvisioningRecord,
  setup?: ProvisioningStatusView,
): ManagedAppProvisioning {
  const state = setup?.state;
  return {
    provisioning_id: record.id,
    template_id: record.templateID,
    state:
      state === "ready"
        ? record.binding
          ? "ready"
          : "running"
        : state === "blocked" || state === "failed"
          ? state
          : state === "running"
            ? "running"
            : "queued",
    ...(setup?.resources.appID ? { app_id: setup.resources.appID } : {}),
    ...(state === "ready" && record.binding
      ? { origin: record.binding.origin }
      : {}),
    steps: (setup?.steps ?? []).map(({ name, state }) => ({ name, state })),
    ...(state === "blocked" || state === "failed"
      ? {
          reason:
            "Setup needs attention. Resume provisioning after resolving its Hosting requirements.",
        }
      : {}),
  };
}
export function createPluginAppProvisioning(deps: {
  templates: ApprovedAppTemplate[];
  store: ProvisioningStore;
  executeHosting(actor: HostingActor, command: unknown): Promise<any>;
}) {
  const templates = new Map(deps.templates.map((t) => [t.id, t]));
  if (templates.size !== deps.templates.length)
    throw new Error("Duplicate approved application template");
  return {
    async handler(
      scope: ManagedAppsScope,
      request: ManagedAppsProvisioningRequest,
    ): Promise<
      ManagedAppProvisioning | { provisionings: ManagedAppProvisioning[] }
    > {
      await deps.store.assertLive(
        scope,
        request.operation === "provision"
          ? "plugin.apps.provision"
          : "plugin.apps.read",
      );
      if (request.operation === "provisioningList") {
        const records = await deps.store.list(scope);
        const provisionings = [];
        for (const record of records) {
          const setup = await deps.store.status(record);
          provisionings.push(status(record, setup));
        }
        return { provisionings };
      }
      if (request.operation === "provisioningStatus") {
        const record = await deps.store.get(
          scope,
          request.input.provisioning_id,
        );
        if (!record)
          throw new PluginError("not_found", "Provisioning is unavailable");
        const setup = await deps.store.status(record);
        return status(record, setup);
      }
      const template = templates.get(request.input.template_id);
      if (!template)
        throw new PluginError(
          "permission_denied",
          "Application template is not approved",
        );
      if (
        !Number.isSafeInteger(scope.pluginGeneration) ||
        !Number.isSafeInteger(scope.grantRevision)
      )
        throw new PluginError(
          "permission_denied",
          "Provisioning authority is incomplete",
        );
      const record = await deps.store.reserve(scope, request.input);
      const call = async (command: unknown) => {
        await deps.store.assertLive(scope);
        return deps.executeHosting(record.actor, command);
      };
      let setup: SetupView;
      if (!record.setupID) {
        setup = await call({
          action: "setup.save",
          projectID: scope.projectID,
          revision: 0,
          configuration: { ...template.configuration, name: record.name },
          environment: await template.environment(
            scope,
            record.id,
            record.name,
          ),
          idempotencyKey: `plugin-${record.id}-save`,
        });
        await deps.store.attachSetup(record, setup.id);
        record.setupID = setup.id;
      } else
        setup = await call({
          action: "setup.get",
          projectID: scope.projectID,
          setupID: record.setupID,
        });
      if (
        setup.state === "draft" ||
        setup.state === "awaiting-confirmation" ||
        ((setup.state === "blocked" || setup.state === "failed") &&
          template.configuration.database.mode === "new" &&
          !setup.resources.databaseID &&
          (!setup.preview ||
            new Date(setup.preview.expiresAt).getTime() <= Date.now()))
      ) {
        setup = await call({
          action: "setup.preview",
          projectID: scope.projectID,
          setupID: setup.id,
          revision: setup.revision,
        });
        if (!setup.preview || setup.preview.blockers.length)
          return {
            ...status(record, setup),
            state: "blocked",
            reason:
              "Hosting admission requirements must be resolved before provisioning.",
          };
        setup = await call({
          action: "setup.confirm",
          projectID: scope.projectID,
          setupID: setup.id,
          revision: setup.revision,
          previewID: setup.preview.id,
          acceptedOfferIDs: setup.preview.offers.map((o) => o.id),
          idempotencyKey: `plugin-${record.id}-confirm-${setup.preview.id}`,
        });
      } else if (setup.state === "blocked" || setup.state === "failed")
        setup = await call({
          action: "setup.resume",
          projectID: scope.projectID,
          setupID: setup.id,
          revision: setup.revision,
          idempotencyKey: `plugin-${record.id}-resume-${scope.operationID}`,
        });
      return status(record, setup);
    },
  };
}
