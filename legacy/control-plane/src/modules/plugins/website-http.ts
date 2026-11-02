import { isWebsiteHTTPAction } from "@webdock/core/plugin-runtime-policy";
import type { Capability, PluginManifest } from "@webdock/plugin-sdk/contracts";
import { PluginError } from "@webdock/plugin-sdk/contracts";
import type { HTTPBinding } from "@webdock/core/plugin-http";

type WebsiteScope = {
  subject: string;
  customerID: string;
  projectID: string;
  generation: number;
  grantRevision: number;
};
type Website = { enabled: boolean; customerID: string; redirectURIs: unknown };
type Dependencies = {
  website(bindingID: string): Promise<Website | null>;
  claims(
    subject: string,
    bindingID: string,
  ): Promise<{ disabled?: boolean; webdock_role?: string }>;
};
const roles = ["reader", "editor", "admin", "operator"];
function denied(): never {
  throw new PluginError("permission_denied", "Website access is unavailable");
}

/** Only trusted host bindings enter here; authority is refreshed for each HTTP exchange. */
export async function authorizeWebsiteHTTPBinding(
  scope: WebsiteScope,
  bindingID: string,
  binding: HTTPBinding,
  method: string,
  dependencies: Dependencies,
): Promise<HTTPBinding> {
  if (
    !/^[1-9][0-9]{0,19}$/.test(bindingID) ||
    !scope.subject ||
    scope.subject.length > 256 ||
    /[\x00-\x20\x7f-\uffff]/.test(scope.subject) ||
    !/^[1-9][0-9]{0,19}$/.test(scope.projectID) ||
    !Number.isSafeInteger(scope.generation) ||
    scope.generation < 1 ||
    !Number.isSafeInteger(scope.grantRevision) ||
    scope.grantRevision < 0
  )
    denied();
  const site = await dependencies.website(bindingID);
  if (
    !site?.enabled ||
    site.customerID !== scope.customerID ||
    !Array.isArray(site.redirectURIs)
  )
    denied();
  // The SSO callback establishes the exact website origin, not an arbitrary redirect URL.
  if (
    !site.redirectURIs.some((value) => {
      if (typeof value !== "string") return false;
      try {
        const url = new URL(value);
        return (
          url.protocol === "https:" &&
          !url.username &&
          !url.password &&
          url.origin === binding.origin &&
          url.pathname === "/api/sso/callback" &&
          !url.search &&
          !url.hash
        );
      } catch {
        return false;
      }
    })
  )
    denied();
  const claims = await dependencies.claims(scope.subject, bindingID);
  if (
    claims.disabled ||
    !roles.includes(claims.webdock_role ?? "") ||
    (method !== "GET" && claims.webdock_role === "reader")
  )
    denied();
  const headers: Record<string, string> = {};
  for (const [key, value] of Object.entries(binding.headers)) {
    const normalized = key.toLowerCase();
    if (normalized === "cookie" || normalized.startsWith("x-webdock-"))
      continue;
    headers[normalized] = value;
  }
  return {
    ...binding,
    headers: {
      ...headers,
      "x-webdock-instance": bindingID,
      "x-webdock-project": scope.projectID,
      "x-webdock-subject": scope.subject,
      "x-webdock-role": claims.webdock_role!,
      "x-webdock-grant-epoch": `${scope.generation}:${scope.grantRevision}`,
    },
  };
}

/** Shared by initial action dispatch, each host service call, and late-result validation. */
export async function authorizePluginHTTPAction(
  scope: WebsiteScope,
  action: PluginManifest["actions"][number],
  project: {
    active: boolean;
    allowed: boolean;
    preview: boolean;
    canWrite: boolean;
  },
  dependencies: Dependencies & {
    resolve(
      name: string,
    ): Promise<(HTTPBinding & { websiteBindingID?: string }) | null>;
  },
  request: {
    capability?: Capability;
    http?: { name: string; method: string };
  } = {},
): Promise<HTTPBinding | undefined> {
  if (!project.active || !project.allowed || project.preview) denied();
  if (!action.httpBinding) {
    if (action.intent === "write" && !project.canWrite) denied();
    return undefined;
  }
  if (
    !isWebsiteHTTPAction(action) ||
    (request.capability &&
      (!action.capabilities.includes(request.capability) ||
        !["plugin.http.read", "plugin.http.request"].includes(
          request.capability,
        ))) ||
    (request.http && request.http.name !== action.httpBinding)
  )
    denied();
  const binding = await dependencies.resolve(action.httpBinding);
  if (!binding?.websiteBindingID) denied();
  return authorizeWebsiteHTTPBinding(
    scope,
    binding.websiteBindingID,
    binding,
    action.intent === "write" ? "POST" : (request.http?.method ?? "GET"),
    dependencies,
  );
}
