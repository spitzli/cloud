import type { ModuleView } from "@webdock/module-sdk";
import { moduleForOperation } from "@webdock/module-sdk/selection";
import { browserAuthConfiguration } from "./browser-auth-config";
import { browserRequestCredentials } from "./browser-auth";
import { getConsoleIdentity, clerkConsoleEnabled } from "./console-identity";
import { headers } from "next/headers";
import { redirect } from "next/navigation";
import "server-only";
import { cache } from "react";
import { requestAPI } from "@webdock/api-client";
import { AccessError, StudioError, studioError } from "./studio-errors";

export async function studioCall<T>(
  operation: string,
  args: unknown[] = [],
): Promise<T> {
  const delegated = browserAuthConfiguration.transport === "console";
  const consoleIdentity = delegated ? await getConsoleIdentity() : null;
  const credentials = delegated
    ? { cookie: (await headers()).get("cookie") || "" }
    : await browserRequestCredentials();
  if (
    (delegated && !consoleIdentity)
  )
    throw new AccessError("Sign in to Console.", 401);
  const module = moduleForOperation(operation, args);
  if (module) {
    const session = await getStudioSession();
    if (
      !session?.installation ||
      !["enabled", "draining"].includes(
        session.installation.modules[module] ?? "",
      )
    )
      throw new StudioError("This product is unavailable.", 404);
  }
  if (!process.env.WEBDOCK_API_ORIGIN)
    throw new StudioError("Studio API is not configured.", 503);
  {
    let result: {
      status: number;
      body: { data?: T; error?: { kind?: string; message?: string } };
    };
    try {
      result = await requestAPI({
        apiOrigin: process.env.WEBDOCK_API_ORIGIN,
        uiOrigin:
          process.env.WEBDOCK_STUDIO_ORIGIN ||
          process.env.NEXT_PUBLIC_SERVER_URL ||
          "https://studio.webdock.dev",
        ...credentials,
        ...(consoleIdentity ? { consoleSession: consoleIdentity } : {}),
        path: consoleIdentity ? "/v1/console" : "/v1/studio",
        body: { operation, args },
      });
    } catch {
      throw new StudioError(
        "Studio could not reach account services. Refresh and check the current state before retrying.",
        503,
      );
    }
    if (
      (consoleIdentity || result.body.error?.kind === "PortalSessionMissing") &&
      result.status === 401
    )
      throw new AccessError("Sign in to Studio to continue.", 401);
    if (result.status < 200 || result.status >= 300 || result.body.error)
      throw studioError(
        result.body.error?.kind || "StudioError",
        result.body.error?.message || "This operation is unavailable.",
        result.status,
      );
    return result.body.data as T;
  }
}
export type TenantPreview = {
  customerID: string;
  customerName: string;
  expiresAt: string;
  status: "active" | "expired";
  readOnly: true;
  role: "admin";
};
export type StudioSession = {
  installation?: ModuleView;
  preview?: TenantPreview;
  user: {
    id: string;
    name: string;
    email: string;
    role: "operator" | "user";
    emailVerified: boolean;
    twoFactorEnabled: boolean;
    mustChangePassword: boolean;
  };
  operator: boolean;
};
const readStudioSession = cache(async (): Promise<StudioSession | null> => {
  try {
    return await studioCall<StudioSession>("session");
  } catch (error) {
    if (error instanceof AccessError && error.status === 401) return null;
    throw error;
  }
});

export async function getStudioSession(): Promise<StudioSession | null> {
  try {
    return await readStudioSession();
  } catch (error) {
    if (
      clerkConsoleEnabled() &&
      error instanceof StudioError &&
      error.status === 403
    )
      redirect("/access-pending");
    throw error;
  }
}
// Public account and pending-access pages must render without a business session.
export async function getStudioInstallation(): Promise<ModuleView> {
  try {
    return (
      (await readStudioSession())?.installation ?? {
        revision: "unavailable",
        modules: {},
      }
    );
  } catch (error) {
    if (
      clerkConsoleEnabled() &&
      error instanceof StudioError &&
      error.status === 403
    )
      return { revision: "unavailable", modules: {} };
    throw error;
  }
}
