import { PlatformError, getPlatformSettings, savePlatformSettings } from "../platform/platform-settings";
import { PluginError } from "@webdock/plugin-sdk/contracts";
import { listRuntimePlugins, invokeRuntimePlugin } from "../plugins/runtime";
import { pluginSources } from "../plugins/sources";
import { PluginSourceError } from "../plugins/sources-policy";
import { githubView } from "../integrations/github/view";
import { consoleAuthConfiguration } from "../auth/console-configuration";
import { studioOperationDomain } from "./operation-domain";
import {
  browserAuthProvider,
  clerkBrowserSession,
} from "../auth/clerk-runtime";
import { ClerkSessionUnavailable } from "../auth/clerk-bridge";
import {
  withTrustedLocalSession,
  selectedIdentityDirectory,
  trustedLocalSessionKind,
} from "../auth/principal";
import {
  requireInstalledOperation,
  publicInstallation,
} from "../../installation";
import { ModuleError } from "@webdock/module-sdk";
import {
  currentConsolePrincipal,
  withConsolePrincipal,
} from "../auth/principal";
import {
  legacyClerkMemberships,
  requireConsolePrincipal,
} from "../auth/clerk-provider";
import { ConsoleAuthError } from "../auth/clerk-identity";
import { introspectToken } from "../auth/introspection";
import { ZodError } from "zod";

import { registryService } from "../registry/context";
import { RegistryError } from "../registry/service";
import { currentDatabaseScope } from "@webdock/database-runtime";
type Shape = ((value: unknown) => boolean)[];
let contribution: { shapes: Record<string, Shape>; dispatch: (operation: string, args: unknown[], headers: Headers, userID: string) => Promise<unknown>; publicError: (error: unknown) => { kind: string; message: string; status: number } | undefined } | undefined;
export function configureStudioProducts(value: NonNullable<typeof contribution>) { contribution = value; }

import {
  loadProductOperation,
  type ProductOperation,
} from "./product-operation";
import { withNativeHeaders } from "./request-context";
import {
  getConnections,
  registerConnection,
  disableConnection,
} from "../auth/connections";
// Server-only delegation boundary: native cookies never leave this module.
import { APIError, serializeSignedCookie } from "better-call";

import { database, authDatabase } from "../../db/runtime";
import { withDatabaseScope } from "@webdock/database-runtime";
import {
  currentTenantPreview,
  startTenantPreview,
  exitTenantPreview,
  dispatchTenantPreview,
} from "../access/tenant-preview";
import {
  AccessError,
  accountSites,
  listAccess,
  manageAccess,
  requireAccessOperator,
} from "../access/access-management";
import {
  listTenants,
  listTenantInvitations,
  getTenant,
  manageTenant,
} from "../access/tenants";
const bodyLimit = 128 * 1024;
class RequestError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
  }
}
function fail(status: number, message: string): never {
  throw new RequestError(status, message);
}
function invalid(): never {
  return fail(400, "Invalid Studio request.");
}
function unauthorized(): never {
  return fail(401, "Sign in to Studio again.");
}
const isObject = (value: unknown): value is Record<string, unknown> =>
  value !== null && typeof value === "object" && !Array.isArray(value);
const isString = (value: unknown): value is string =>
  typeof value === "string" && value.length <= 32768;
const isInput = (value: unknown): value is Record<string, string> =>
  isObject(value) &&
  Object.keys(value).length <= 100 &&
  Object.values(value).every(isString);
const shapes: Record<string, ((value: unknown) => boolean)[]> = {
  "plugins.list": [isString],
  "plugins.invoke": [isObject],
  "plugins.sources": [isObject],
  "plugins.github": [isInput],







  "registry.session": [],
  "registry.query": [isString, isObject],
  "registry.customer": [(value) => value === null || isString(value), isObject],
  "registry.project": [(value) => value === null || isString(value), isObject],
  "registry.archive": [
    isString,
    isString,
    (value) => typeof value === "boolean",
  ],
  getConnections: [],
  registerConnection: [isInput],
  disableConnection: [isString],
  startTenantPreview: [isString],
  exitTenantPreview: [],
  session: [],
  requireAccessOperator: [],
  listAccess: [isString],
  manageAccess: [isInput],
  accountSites: [],
  listTenants: [],
  listTenantInvitations: [],
  getTenant: [isString],
  manageTenant: [isString, isInput],
  getPlatformSettings: [],
  savePlatformSettings: [isInput],









};

async function readBody(request: Request, native = false) {
  if (request.method !== "POST") fail(405, "Use POST.");
  if (
    request.headers.get("content-type")?.split(";")[0].trim() !==
    "application/json"
  )
    invalid();
  if (Number(request.headers.get("content-length")) > bodyLimit)
    fail(413, "Studio request is too large.");
  if (!request.body) invalid();
  const reader = request.body.getReader(),
    chunks: Uint8Array[] = [];
  let length = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      length += value.byteLength;
      if (length > bodyLimit) {
        await reader.cancel();
        fail(413, "Studio request is too large.");
      }
      chunks.push(value);
    }
  } finally {
    reader.releaseLock();
  }
  let body: unknown;
  try {
    body = JSON.parse(Buffer.concat(chunks).toString("utf8"));
  } catch {
    if (length > bodyLimit) fail(413, "Studio request is too large.");
    invalid();
  }
  if (length > bodyLimit) fail(413, "Studio request is too large.");
  if (
    !isObject(body) ||
    Object.keys(body).some(
      (key) =>
        !(
          native ? ["operation", "args"] : ["operation", "args", "accessToken"]
        ).includes(key),
    ) ||
    !isString(body.operation) ||
    !(Object.hasOwn(shapes, body.operation) || Object.hasOwn(contribution?.shapes ?? {}, body.operation)) ||
    !Array.isArray(body.args) ||
    (!native && (!isString(body.accessToken) || !body.accessToken))
  )
    invalid();
  const { operation, args, accessToken } = body;
  if (length > bodyLimit) fail(413, "Studio request is too large.");
  const validators = shapes[operation] ?? contribution!.shapes[operation];
  if (
    args.length !== validators.length ||
    validators.some((validate, index) => !validate(args[index]))
  )
    invalid();
  return {
    operation,
    args,
    accessToken: typeof accessToken === "string" ? accessToken : "",
  };
}

type Dependencies = {
  introspect?: (request: Request) => Promise<Response>;
  browserHeaders?: Headers;
};
async function delegatedSession(
  request: Request,
  accessToken: string,
  dependencies: Dependencies,
) {
  const clientID = process.env.WEBDOCK_STUDIO_CLIENT_ID;
  const basic = request.headers.get("authorization") || "";
  if (
    !clientID ||
    !/^Basic [A-Za-z0-9+/]+=*$/i.test(basic) ||
    basic.length > 16384
  )
    unauthorized();
  const credentials = Buffer.from(basic.slice(6), "base64").toString("utf8");
  if (
    credentials.slice(0, credentials.indexOf(":")) !== clientID ||
    !credentials.slice(credentials.indexOf(":") + 1)
  )
    unauthorized();
  const origin = new URL(process.env.BETTER_AUTH_URL || "http://localhost:3125")
    .origin;
  // The native provider verifies the supplied confidential client and token.
  const response = await introspectToken(
    accessToken,
    clientID!,
    credentials.slice(credentials.indexOf(":") + 1),
    dependencies.introspect,
  );
  if (!response.ok) {
    if ([400, 401, 403].includes(response.status)) unauthorized();
    fail(
      response.status === 429 ? 429 : 503,
      "Studio is temporarily unavailable. Try again.",
    );
  }
  const token: unknown = await response.json();
  if (
    !isObject(token) ||
    token.active !== true ||
    token.disabled === true ||
    token.client_id !== clientID ||
    !isString(token.sub) ||
    !token.sub ||
    !isString(token.sid) ||
    !token.sid ||
    typeof token.exp !== "number" ||
    !Number.isFinite(token.exp) ||
    token.exp * 1000 <= Date.now()
  )
    unauthorized();
  const session = (
    await authDatabase.query(
      'SELECT token FROM webdock_auth.session WHERE id=$1 AND "userId"=$2 AND "expiresAt">now()',
      [token.sid, token.sub],
    )
  ).rows[0];
  if (!session) unauthorized();
  const { auth } = await import("../auth/auth");
  const context = await auth.$context;
  const cookie = await serializeSignedCookie(
    context.authCookies.sessionToken.name,
    session.token,
    context.secret,
  );
  const headers = new Headers({ Origin: origin, Cookie: cookie.split(";")[0] });
  const native = await auth.api.getSession({ headers });
  if (
    !native ||
    native.session.id !== token.sid ||
    native.user.id !== token.sub
  )
    unauthorized();
  const memberships = browserAuthProvider === "clerk"
    ? await legacyClerkMemberships(token.sub) : undefined;
  if (browserAuthProvider === "clerk" && !memberships) unauthorized();
  const reference = { subject: token.sub, sessionID: token.sid };
  const identity = browserAuthProvider === "clerk"
    ? await withDatabaseScope(
        {
          kind: "session", authProvider: "clerk", subject: token.sub, sessionID: token.sid,
          ...(memberships ? { memberships: { ...memberships, subject: token.sub, sessionID: token.sid } } : {}),
        },
        () => selectedIdentityDirectory.session(authDatabase, reference),
      )
    : undefined;
  if (browserAuthProvider === "clerk" && (!identity || identity.subject !== token.sub || identity.sessionID !== token.sid)) unauthorized();
  const user = { ...native.user, ...identity };
  if (user.banned || !user.emailVerified || user.mustChangePassword) unauthorized();
  const operator = user.role === "operator" && user.twoFactorEnabled === true;
  if (user.role !== "user" && !operator) unauthorized();
  const {
    id,
    name,
    email,
    role,
    emailVerified,
    twoFactorEnabled,
    mustChangePassword,
  } = user;
  return {
    headers,
    actor: { userID: id, sessionID: native.session.id },
    safe: {
      user: {
        id,
        name,
        email,
        role,
        emailVerified,
        twoFactorEnabled,
        mustChangePassword,
      },
      operator,
    },
  };
}

async function dispatch(
  operation: string,
  args: unknown[],
  headers: Headers,
  safe:
    | Awaited<ReturnType<typeof delegatedSession>>["safe"]
    | Awaited<ReturnType<typeof clerkBrowserSession>>["safe"],
  product?: ProductOperation,
) {
  if (product) return product.dispatch(operation, args, headers, safe.user.id);
  if (contribution && Object.hasOwn(contribution.shapes, operation))
    return contribution.dispatch(operation, args, headers, safe.user.id);
  // Shape checks happen before dispatch; every original service still checks live permissions.
  const text = args[0] as string,
    input = args[0] as Record<string, string>,
    tenantInput = args[1] as Record<string, string>;
  if (operation === "plugins.list")
    return listRuntimePlugins(args[0] as string);
  if (operation === "plugins.invoke") return invokeRuntimePlugin(args[0]);
  if (operation === "plugins.sources") return pluginSources(args[0]);
  if (operation === "plugins.github") return githubView(input);
  switch (operation) {
    case "session":
      return safe;
    case "requireAccessOperator":
      await requireAccessOperator(headers);
      return safe;
    case "listAccess":
      return listAccess(headers, text);
    case "manageAccess":
      return manageAccess(headers, input);
    case "accountSites":
      return accountSites(headers);
    case "listTenants":
      return listTenants(headers);
    case "listTenantInvitations":
      return listTenantInvitations(headers);
    case "getTenant":
      return getTenant(headers, text);
    case "manageTenant":
      return manageTenant(headers, text, tenantInput);

  }
  // Even platform reads and connection tests require current root authorization.
  await requireAccessOperator(headers);
  if (operation.startsWith("registry.")) {
    const scope = currentDatabaseScope();
    if (scope?.kind !== "session")
      throw new AccessError("Authenticated session required.");
    const registry = await registryService(),
      actor = { subject: scope.subject, sessionID: scope.sessionID };
    switch (operation) {
      case "registry.session":
        return registry.operator(actor);
      case "registry.query":
        return registry.query(actor, text, args[1] as Record<string, unknown>);
      case "registry.customer":
        return registry.writeCustomer(actor, args[0] as string | null, args[1]);
      case "registry.project":
        return registry.writeProject(actor, args[0] as string | null, args[1]);
      case "registry.archive":
        if (!["customers", "projects"].includes(text)) invalid();
        return registry.archive(
          actor,
          text as "customers" | "projects",
          args[1] as string,
          args[2] as boolean,
        );
    }
  }
  switch (operation) {
    case "getConnections":
      return getConnections(headers);
    case "registerConnection":
      return registerConnection(headers, input);
    case "disableConnection":
      return disableConnection(headers, text);
    case "getPlatformSettings":
      return getPlatformSettings();
    case "savePlatformSettings":
      return savePlatformSettings(safe.user.id, input);
    default:
      invalid();
  }
}

export function handleStudioRequest(
  request: Request,
  dependencies: Dependencies = {},
): Promise<Response> {
  return handleRequest(request, dependencies, false);
}
export function handleNativeStudioRequest(request: Request): Promise<Response> {
  return currentConsolePrincipal() ||
    browserAuthProvider === "better-auth" ||
    trustedLocalSessionKind(request.headers) === "clerk"
    ? handleRequest(request, {}, true)
    : Promise.resolve(new Response(null, { status: 404 }));
}
export function handleClerkStudioRequest(request: Request): Promise<Response> {
  if (browserAuthProvider !== "clerk" || !clerkBrowserSession)
    return Promise.resolve(new Response(null, { status: 404 }));
  return handleRequest(request, {}, true, true);
}
async function nativeSession(
  headers: Headers,
): Promise<Awaited<ReturnType<typeof delegatedSession>>> {
  const principal = currentConsolePrincipal();
  if (principal)
    return {
      headers,
      actor: { userID: principal.userID, sessionID: principal.sessionID },
      safe: {
        user: {
          id: principal.userID,
          name: principal.name,
          email: principal.email,
          role: principal.operator ? "operator" : "user",
          emailVerified: true,
          twoFactorEnabled: false,
          mustChangePassword: false,
        },
        operator: principal.operator,
      },
    };
  const { auth } = await import("../auth/auth");
  const native = await auth.api.getSession({ headers });
  if (!native) unauthorized();
  const identity = browserAuthProvider === "clerk"
    ? await selectedIdentityDirectory.session(authDatabase, { subject: native.user.id, sessionID: native.session.id })
    : undefined;
  if (browserAuthProvider === "clerk" && (!identity || identity.subject !== native.user.id || identity.sessionID !== native.session.id)) unauthorized();
  const user = { ...native.user, ...identity };
  if (user.banned || !user.emailVerified || user.mustChangePassword) unauthorized();
  const operator = user.role === "operator" && user.twoFactorEnabled === true;
  if (user.role !== "user" && !operator) unauthorized();
  const {
    id,
    name,
    email,
    role,
    emailVerified,
    twoFactorEnabled,
    mustChangePassword,
  } = user;
  return {
    headers,
    actor: { userID: id, sessionID: native.session.id },
    safe: {
      user: {
        id,
        name,
        email,
        role,
        emailVerified,
        twoFactorEnabled,
        mustChangePassword,
      },
      operator,
    },
  };
}
async function handleRequest(
  request: Request,
  dependencies: Dependencies,
  native: boolean,
  clerk = false,
): Promise<Response> {
  const responseHeaders = { "Cache-Control": "no-store", Pragma: "no-cache" };
  let product: ProductOperation | undefined;
  try {
    if (
      native &&
      !currentConsolePrincipal() &&
      request.headers.get("origin") !==
        new URL(
          (clerk
            ? process.env.WEBDOCK_SSO_APP_ORIGIN
            : process.env.BETTER_AUTH_URL) || "http://localhost:3125",
        ).origin
    )
      fail(403, "Invalid origin.");
    const { operation, args, accessToken } = await readBody(request, native);
    const { headers, actor, safe } = clerk
      ? await clerkBrowserSession!(request.headers)
      : native
        ? await nativeSession(request.headers)
        : await delegatedSession(request, accessToken, dependencies);
    requireInstalledOperation(operation, args);
    if (process.env.WEBDOCK_API_DOMAINS !== undefined) {
      const domain = studioOperationDomain(operation, args);
      if (!process.env.WEBDOCK_API_DOMAINS.split(",").includes(domain))
        fail(503, "This service is temporarily unavailable.");
    }
    product = await loadProductOperation(operation, args);
    const memberships = currentConsolePrincipal()
      ? undefined
      : await legacyClerkMemberships(actor.userID);
    return await withTrustedLocalSession(
      headers,
      { subject: actor.userID, sessionID: actor.sessionID },
      clerk ? "clerk" : (trustedLocalSessionKind(headers) ?? "legacy"),
      () =>
        withDatabaseScope(
          {
            kind: "session",
            authProvider: browserAuthProvider,
            subject: actor.userID,
            sessionID: actor.sessionID,
            ...(memberships ? { memberships: { ...memberships, subject: actor.userID, sessionID: actor.sessionID } } : {}),
          },
          async () => {
            // The context comes from the authenticated central session, never a caller hint.
            // An expired context remains present and restrictive until explicit exit.
            const context = await currentTenantPreview(actor);
            let data: unknown;
            if (
              operation === "hosting.command" ||
              operation === "database.command"
            )
              data = await withNativeHeaders(
                headers,
                () => dispatch(operation, args, headers, safe, product),
                dependencies.browserHeaders || request.headers,
                accessToken,
              );
            else if (operation === "exitTenantPreview")
              data = await exitTenantPreview(actor);
            else if (operation === "session" && context)
              data = {
                ...safe,
                user: { ...safe.user, role: "user" },
                operator: false,
                preview: context.preview,
              };
            else if (context) {
              try {
                data = await dispatchTenantPreview(
                  context,
                  operation,
                  args,
                  headers,
                );
              } catch (error) {
                if (error instanceof AccessError) {
                  // readBody already restricted operation to our fixed shapes registry.
                  // Log the actual actor and scoped tenant, never arguments or tokens.
                  await database.query(
                    "INSERT INTO webdock_auth.access_event(actor_id,action,target_id,outcome) VALUES($1,$2,$3,'denied')",
                    [
                      actor.userID,
                      `tenant-preview-denied:${operation}`,
                      context.preview.customerID,
                    ],
                  );
                }
                throw error;
              }
            } else if (operation === "startTenantPreview")
              data = await startTenantPreview(actor, args[0] as string);
            else
              data = await withNativeHeaders(
                headers,
                () => dispatch(operation, args, headers, safe, product),
                dependencies.browserHeaders || request.headers,
                accessToken,
              );
            return Response.json(
              {
                data:
                  operation === "session" && data
                    ? {
                        ...(data as object),
                        installation: publicInstallation(),
                      }
                    : (data ?? null),
              },
              { headers: responseHeaders },
            );
          },
        ),
    );
  } catch (error) {
    if(error instanceof PluginSourceError)return Response.json({error:{kind:"PluginSourceError",code:error.code,message:error.message}},{status:error.status,headers:responseHeaders});
    const publicError = product?.publicError(error) ?? contribution?.publicError(error);
    if (publicError)
      return Response.json(
        { error: { kind: publicError.kind, message: publicError.message } },
        { status: publicError.status, headers: responseHeaders },
      );
    if (error instanceof ClerkSessionUnavailable)
      return Response.json(
        {
          error: {
            kind: "PortalSessionMissing",
            message:
              "Sign in with the configured provider and complete account setup.",
          },
        },
        { status: 401, headers: responseHeaders },
      );
    if (error instanceof ModuleError)
      return Response.json(
        {
          error: {
            kind: "ModuleError",
            code: error.code,
            message: error.message,
          },
        },
        { status: error.status, headers: responseHeaders },
      );
    if (error instanceof ConsoleAuthError)
      return Response.json(
        { error: { kind: "AccessError", message: error.message } },
        { status: error.status, headers: responseHeaders },
      );
    if (error instanceof ZodError)
      return Response.json(
        {
          error: {
            kind: "StudioError",
            message: "Check the supplied details and try again.",
          },
        },
        { status: 400, headers: responseHeaders },
      );
    if (error instanceof PluginError)
      return Response.json(
        { error: { kind: "PluginError", message: error.message } },
        {
          status:
            error.code === "permission_denied"
              ? 403
              : error.code === "invalid_input"
                ? 400
                : 503,
        },
      );
    if (error instanceof RegistryError)
      return Response.json(
        { error: { kind: "RegistryError", message: error.message } },
        { status: error.status, headers: responseHeaders },
      );
    const known = (
      [
        ["AccessError", AccessError],
        ["PlatformError", PlatformError],
      ] as const
    ).find(([, type]) => error instanceof type);
    const status =
      error instanceof RequestError
        ? error.status
        : error instanceof AccessError || error instanceof APIError
          ? 403
          : known
            ? 400
            : 500;
    const kind = known?.[0] || (status < 500 ? "AccessError" : "Error");
    const message =
      error instanceof RequestError || (known && error instanceof Error)
        ? error.message
        : status === 403
          ? "This action is unavailable for your account or tenant."
          : status === 400
            ? "Could not complete this action. Check the supplied details and current settings."
            : "Studio is temporarily unavailable. Try again.";
    // Domain error classes contain reviewed public validation messages. Raw provider,
    // database and native authentication errors may contain secrets; never expose them.
    return Response.json(
      { error: { kind, message } },
      { status, headers: responseHeaders },
    );
  }
}

/** Separate transport: no opaque-token or legacy-cookie fallback. */
export async function handleConsoleRequest(
  request: Request,
): Promise<Response> {
  if (!consoleAuthConfiguration().enabled)
    return Response.json(
      {
        error: {
          kind: "AccessError",
          message: "Clerk Console is not activated.",
        },
      },
      { status: 503, headers: { "cache-control": "no-store" } },
    );
  try {
    const principal = await requireConsolePrincipal(request);
    return await withConsolePrincipal(principal, () =>
      handleRequest(request, {}, true),
    );
  } catch (error) {
    const status = error instanceof ConsoleAuthError ? error.status : 503;
    return Response.json(
      {
        error: {
          kind: "AccessError",
          message:
            error instanceof ConsoleAuthError
              ? error.message
              : "Console is temporarily unavailable.",
        },
      },
      { status, headers: { "cache-control": "no-store" } },
    );
  }
}
