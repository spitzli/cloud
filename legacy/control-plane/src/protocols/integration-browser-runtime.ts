import { withDatabaseScope } from "@webdock/database-runtime";
import { ModuleError } from "@webdock/module-sdk";
import type { ApiConfig } from "../config";
import { consoleAuthConfiguration } from "../modules/auth/console-configuration";
import {
  requireConsoleBrowserPrincipal,
  requireConsolePrincipal,
} from "../modules/auth/clerk-provider";
import {
  withConsolePrincipal,
  withTrustedLocalSession,
} from "../modules/auth/principal";
import { ConsoleAuthError } from "../modules/auth/clerk-identity";
import {
  browserAuthProvider,
  clerkBrowserSession,
} from "../modules/auth/clerk-runtime";
import { ClerkSessionUnavailable } from "../modules/auth/clerk-bridge";
import { createPortalSession } from "../modules/portal/session";
import { withNativeHeaders } from "../modules/portal/request-context";
import { createIntegrationCallbackHandoff } from "./integration-callback-handoff";
import { createIntegrationBrowserRunner } from "./integration-browser";

export function createIntegrationBrowserRuntime(
  config: ApiConfig,
  requireAdmission?: (intent: "access" | "provision") => void,
) {
  const sso =
    browserAuthProvider === "better-auth" ? createPortalSession() : null;
  const secret =
    process.env.BETTER_AUTH_SECRET ||
    process.env.WEBDOCK_SSO_COOKIE_SECRET ||
    "";
  // An unconfigured GitHub integration must not make a fresh Clerk Core fail startup.
  const handoff =
    browserAuthProvider === "clerk" && secret.length >= 32
      ? createIntegrationCallbackHandoff({
          origin: config.studioOrigin,
          consoleOrigin: config.consoleOrigin ?? config.studioOrigin,
          secret,
        })
      : null;
  const runBrowser = createIntegrationBrowserRunner({
    refreshCallback: (request) => handoff?.begin(request) ?? null,
    provider: browserAuthProvider,
    origin: config.studioOrigin,
    requireAdmission,
    async clerk(headers) {
      try {
        const session = await clerkBrowserSession(headers);
        return {
          headers: session.headers,
          reference: {
            subject: session.actor.userID,
            sessionID: session.actor.sessionID,
          },
        };
      } catch (error) {
        if (error instanceof ClerkSessionUnavailable) return null;
        throw error;
      }
    },
    async legacy(headers) {
      const session = await sso!.getDelegatedSession(headers);
      return session?.sessionID
        ? {
            headers,
            reference: {
              subject: session.subject,
              sessionID: session.sessionID,
            },
            accessToken: session.accessToken,
          }
        : null;
    },
    withIdentity(identity, browser, kind, handler) {
      const dispatch = () =>
        withDatabaseScope({ kind: "session", ...identity.reference }, () =>
          withNativeHeaders(
            identity.headers,
            handler,
            browser,
            identity.accessToken,
          ),
        );
      return kind === "clerk"
        ? withTrustedLocalSession(
            identity.headers,
            identity.reference,
            kind,
            dispatch,
          )
        : dispatch();
    },
  });
  const run: typeof runBrowser = async (request, intent, handler, options) => {
    if (!consoleAuthConfiguration().enabled)
      return runBrowser(request, intent, handler, options);
    if (
      request.method !== "GET" &&
      request.headers.get("origin") !== config.studioOrigin
    )
      return new Response(null, {
        status: 403,
        headers: { "cache-control": "no-store" },
      });
    try {
      requireAdmission?.(intent);
      const principal = await (options?.refreshed
        ? requireConsolePrincipal(request)
        : requireConsoleBrowserPrincipal(request));
      return await withConsolePrincipal(principal, () =>
        withNativeHeaders(
          request.headers,
          () => handler(request),
          request.headers,
        ),
      );
    } catch (error) {
      if (
        error instanceof ConsoleAuthError &&
        error.status === 401 &&
        request.method === "GET" &&
        !options?.refreshed
      ) {
        const refresh = handoff?.begin(request);
        if (refresh) return refresh;
      }
      return new Response(null, {
        status:
          error instanceof ConsoleAuthError || error instanceof ModuleError
            ? error.status
            : 503,
        headers: { "cache-control": "no-store" },
      });
    }
  };
  return { run, handoff };
}
