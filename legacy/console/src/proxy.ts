import { NextResponse, type NextRequest } from "next/server";
import { requestAPI } from "@webdock/api-client";
import type { ModuleView } from "@webdock/module-sdk";
import { guardModuleRoute } from "./lib/module-route-guard";
import { createBrowserProxy } from "./lib/browser-auth-proxy";
export default createBrowserProxy(
  async (
    request: NextRequest,
    credentials: {
      cookie: string;
      bearerToken?: string;
      consoleSession?: { token: string; serviceKey: string };
    },
  ) => {
    const blocked = await guardModuleRoute(
      request,
      () =>
        requestAPI<{ data?: { installation?: ModuleView } }>({
          apiOrigin: process.env.WEBDOCK_API_ORIGIN || "",
          uiOrigin:
            process.env.WEBDOCK_STUDIO_ORIGIN ||
            process.env.NEXT_PUBLIC_SERVER_URL ||
            "https://studio.webdock.dev",
          ...credentials,
          path: credentials.consoleSession ? "/v1/console" : "/v1/studio",
          body: { operation: "session", args: [] },
        }),
      !!credentials.consoleSession,
    );
    return blocked ?? NextResponse.next();
  },
);
export const config = {
  matcher: ["/((?!_next|favicon.ico|.*\\.(?:png|jpg|svg|ico|woff2?|ttf)$).*)"],
};
