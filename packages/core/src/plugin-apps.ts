import { z } from "zod";
import {
  PluginError,
  managedAppOriginSchema,
  managedAppProvisioningSchema,
  managedAppProvisioningListSchema,
  managedAppSchema,
  managedAppOperationSchema,
  managedAppsRequestSchema,
  type ManagedAppsRequest,
} from "@spitzli/plugin-sdk/contracts";
export type ManagedAppsScope = {
  pluginID: string;
  customerID: string;
  projectID: string;
  operationID: string;
  subject: string;
  sessionID: string;
  pluginGeneration?: number;
  grantRevision?: number;
  consoleIdentity?: {
    issuer: string;
    userID: string;
    sessionID: string;
    organizationID: string | null;
  };
};
const bindingSchema = z.strictObject({
  appID: z.string().regex(/^[0-9]{1,20}$/),
  kind: z.string().regex(/^[a-z][a-z0-9-]{0,63}$/),
  origin: managedAppOriginSchema,
});
export type ManagedAppBinding = z.infer<typeof bindingSchema>;
export type ManagedAppsProvisioningRequest = Extract<
  ManagedAppsRequest,
  { operation: "provision" | "provisioningStatus" | "provisioningList" }
>;
export type ManagedAppsProvisioningHandler = (
  scope: ManagedAppsScope,
  request: ManagedAppsProvisioningRequest,
) => Promise<unknown>;
export type ManagedAppsHandler = (
  scope: ManagedAppsScope,
  binding: ManagedAppBinding,
  request: Exclude<
    ManagedAppsRequest,
    {
      operation:
        "list" | "provision" | "provisioningStatus" | "provisioningList";
    }
  >,
) => Promise<unknown>;
export async function dispatchManagedApps(
  scope: ManagedAppsScope,
  rawBindings: ManagedAppBinding[],
  request: Exclude<ManagedAppsRequest, ManagedAppsProvisioningRequest>,
  handler?: ManagedAppsHandler,
) {
  if (!handler)
    throw new PluginError(
      "capability_unavailable",
      "Managed apps are unavailable",
    );
  const parsed = z
    .array(bindingSchema)
    .max(100)
    .refine(
      (items) => new Set(items.map((item) => item.appID)).size === items.length,
    )
    .safeParse(rawBindings);
  if (!parsed.success)
    throw new PluginError("unavailable", "Invalid managed app binding");
  const bindings = parsed.data;
  const read = async (binding: ManagedAppBinding) => {
    const value = managedAppSchema.safeParse(
      await handler(scope, binding, {
        operation: "get",
        input: { app_id: binding.appID },
      }),
    );
    if (
      !value.success ||
      value.data.app_id !== binding.appID ||
      value.data.origin !== binding.origin ||
      value.data.kind !== binding.kind ||
      value.data.console_path !== `/hosting/apps/${binding.appID}`
    )
      throw new PluginError("unavailable", "Invalid managed app result");
    return value.data;
  };
  if (request.operation === "list")
    return { apps: await Promise.all(bindings.map(read)) };
  const binding = bindings.find((item) => item.appID === request.input.app_id);
  if (!binding)
    throw new PluginError(
      "permission_denied",
      "App is not bound to this project",
    );
  if (request.operation === "get") return read(binding);
  const value = managedAppOperationSchema.safeParse(
    await handler(scope, binding, request),
  );
  if (!value.success || value.data.app_id !== binding.appID)
    throw new PluginError("unavailable", "Invalid managed app operation");
  return value.data;
}
export function createPluginApps(authority: {
  run<T>(
    request: Request,
    capability:
      "plugin.apps.read" | "plugin.apps.manage" | "plugin.apps.provision",
    fn: (scope: ManagedAppsScope, bindings: ManagedAppBinding[]) => Promise<T>,
  ): Promise<T>;
  handler(): ManagedAppsHandler | undefined;
  provisioningHandler?(): ManagedAppsProvisioningHandler | undefined;
}) {
  return {
    async request(request: Request, raw: unknown) {
      const parsed = managedAppsRequestSchema.safeParse(raw);
      if (!parsed.success)
        throw new PluginError("invalid_input", "Invalid managed app request");
      const input = parsed.data;
      if (
        input.operation === "provision" ||
        input.operation === "provisioningStatus" ||
        input.operation === "provisioningList"
      ) {
        return authority.run(
          request,
          input.operation === "provision"
            ? "plugin.apps.provision"
            : "plugin.apps.read",
          async (scope) => {
            const handler = authority.provisioningHandler?.();
            if (!handler)
              throw new PluginError(
                "capability_unavailable",
                "Application provisioning is unavailable",
              );
            const rawResult = await handler(scope, input);
            if (input.operation === "provisioningList") {
              const list =
                managedAppProvisioningListSchema.safeParse(rawResult);
              if (!list.success)
                throw new PluginError(
                  "unavailable",
                  "Invalid provisioning list",
                );
              return list.data;
            }
            const result = managedAppProvisioningSchema.safeParse(rawResult);
            if (
              !result.success ||
              (input.operation === "provisioningStatus" &&
                result.data.provisioning_id !== input.input.provisioning_id) ||
              (input.operation === "provision" &&
                result.data.template_id !== input.input.template_id)
            )
              throw new PluginError(
                "unavailable",
                "Invalid provisioning result",
              );
            return result.data;
          },
        );
      }
      return authority.run(
        request,
        parsed.data.operation === "command"
          ? "plugin.apps.manage"
          : "plugin.apps.read",
        (scope, bindings) =>
          dispatchManagedApps(scope, bindings, input, authority.handler()),
      );
    },
  };
}
