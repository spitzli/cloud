import { HostingError, type HostingActor } from "@webdock/hosting-contracts";
import {
  PluginError,
  type PluginErrorCode,
} from "@webdock/plugin-sdk/contracts";
import type { ManagedAppsHandler } from "@webdock/core/plugin-apps";
/** Uses native Hosting authorization and its revision/idempotency fences. */
export function createManagedAppsHandler(
  executeHosting: (actor: HostingActor, command: unknown) => Promise<any>,
  requireCommand: (action: string) => void,
): ManagedAppsHandler {
  return async (scope, binding, request) => {
    const actor: HostingActor = {
      subject: scope.subject,
      sessionID: scope.sessionID,
      source: "studio",
      scopes:
        request.operation === "command"
          ? ["hosting:read", "hosting:write"]
          : ["hosting:read"],
    };
    try {
      requireCommand(
        request.operation === "command"
          ? `apps.${request.input.operation}`
          : "apps.get",
      );
      const app = await executeHosting(actor, {
        action: "apps.get",
        appID: binding.appID,
      });
      if (app.id !== binding.appID || app.projectID !== scope.projectID)
        throw new PluginError(
          "permission_denied",
          "App binding does not match this project",
        );
      if (request.operation === "command") {
        const result = await executeHosting(actor, {
          action: `apps.${request.input.operation}`,
          appID: binding.appID,
          revision: request.input.revision,
          subscriptionRevision: request.input.subscription_revision,
          idempotencyKey: scope.operationID,
        });
        return {
          operation_id: result.id,
          app_id: result.appID,
          status: result.status,
        };
      }
      return {
        app_id: app.id,
        name: app.name,
        kind: binding.kind,
        status: app.status,
        revision: app.revision,
        observed_revision: app.observedRevision,
        subscription_revision: app.subscriptionRevision,
        operation_id: app.operationID,
        origin: binding.origin,
        console_path: `/hosting/apps/${binding.appID}`,
      };
    } catch (error) {
      if (error instanceof PluginError) throw error;
      const code =
        error instanceof HostingError
          ? ((
              {
                400: "invalid_input",
                401: "permission_denied",
                403: "permission_denied",
                404: "not_found",
                409: "conflict",
              } as Record<number, PluginErrorCode>
            )[error.status] ?? "unavailable")
          : "unavailable";
      throw new PluginError(
        code,
        code === "conflict"
          ? "App changed. Refresh and try again."
          : "Managed app operation unavailable",
      );
    }
  };
}
