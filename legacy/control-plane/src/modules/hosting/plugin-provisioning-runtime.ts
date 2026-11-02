import { isDeepStrictEqual } from "node:util";
import { isIP } from "node:net";
import { z } from "zod";
import { HostingError } from "@webdock/hosting-contracts";
import { managedAppOriginSchema } from "@webdock/plugin-sdk/contracts";
import type { SetupRow } from "@webdock/module-hosting/server/project-setup";
import {
  createPluginAppProvisioning,
  type ApprovedAppTemplate,
  type ApprovedAppTemplateHooks,
  type TemplateHookContext,
} from "./plugin-provisioning";
import { pluginProvisioningStore } from "./plugin-provisioning-store";
const bindingSchema = z.strictObject({
  appID: z.string().regex(/^[0-9]{1,20}$/),
  websiteBindingID: z.string().min(1).max(128),
  origin: managedAppOriginSchema,
  secretReference: z.string().min(1).max(256),
  pinnedAddresses: z
    .array(z.string().refine((ip) => isIP(ip) > 0))
    .min(1)
    .max(16),
  ca: z.string().max(65536).optional(),
});
export function createApprovedTemplateExecution(deps: {
  templates: ApprovedAppTemplate[];
  hooks: ApprovedAppTemplateHooks;
  store: Pick<
    typeof pluginProvisioningStore,
    "bySetup" | "assertLive" | "setBinding"
  >;
}) {
  async function context(setup: SetupRow): Promise<TemplateHookContext | null> {
    const record = await deps.store.bySetup(setup.id);
    if (!record) return null;
    if (
      record.projectID !== setup.project_id ||
      record.customerID !== setup.customer_id ||
      record.actor.subject !== setup.actor?.subject ||
      record.actor.sessionID !== setup.actor?.sessionID
    )
      throw new HostingError(
        403,
        "Provisioning identity changed. Resume setup.",
      );
    await deps.store.assertLive(record);
    const template = deps.templates.find((t) => t.id === record.templateID);
    if (!template)
      throw new HostingError(503, "Application template is unavailable.");
    if (
      !isDeepStrictEqual(setup.configuration, {
        ...template.configuration,
        name: record.name,
      })
    )
      throw new HostingError(
        409,
        "Approved application configuration changed. Review the template.",
      );
    return { record, setup, template, actor: record.actor };
  }
  return {
    async authorize(setup: SetupRow) {
      await context(setup);
    },
    async beforeRelease(setup: SetupRow) {
      const ctx = await context(setup);
      return ctx ? (await deps.hooks.beforeRelease(ctx)).ready : true;
    },
    async beforeReady(setup: SetupRow) {
      const ctx = await context(setup);
      if (!ctx) return true;
      if (ctx.record.binding) return true;
      const published = await deps.hooks.publish(ctx);
      if (!published) return false;
      const binding = bindingSchema.parse(published);
      if (binding.appID !== setup.resources.appID)
        throw new HostingError(
          409,
          "Published application does not match setup.",
        );
      await deps.store.assertLive(ctx.record);
      await deps.store.setBinding(ctx.record, binding);
      return true;
    },
  };
}
let configured:
  | { templates: ApprovedAppTemplate[]; hooks: ApprovedAppTemplateHooks }
  | undefined;
export function configureApprovedAppTemplates(
  templates: ApprovedAppTemplate[],
  hooks: ApprovedAppTemplateHooks,
) {
  configured = { templates, hooks };
  return createPluginAppProvisioning({
    templates,
    store: pluginProvisioningStore,
    executeHosting: async (actor, command) =>
      (await import("./service")).executeHosting(actor, command),
  }).handler;
}
const unavailable = async () => {
  throw new HostingError(
    503,
    "Approved application provisioning is unavailable.",
  );
};
function execution() {
  return createApprovedTemplateExecution({
    templates: configured?.templates ?? [],
    hooks: configured?.hooks ?? {
      beforeRelease: unavailable,
      publish: unavailable,
    },
    store: pluginProvisioningStore,
  });
}
export const authorizePluginSetup = (row: SetupRow) =>
  execution().authorize(row);
export const beforePluginSetupRelease = (row: SetupRow) =>
  execution().beforeRelease(row);
export const beforePluginSetupReady = (row: SetupRow) =>
  execution().beforeReady(row);
