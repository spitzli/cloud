import { clerkMiddleware } from "@clerk/nextjs/server";
import { canonicalClerkRequest, resumeClerkRequest } from "./clerk-request-origin";
import {
  NextResponse,
  type NextRequest,
  type NextFetchEvent,
} from "next/server";
import {
  browserAuthConfiguration,
  browserAuthPageURL,
  safeBrowserReturn,
} from "./browser-auth-config";
type Credentials = {
  cookie: string;
  bearerToken?: string;
  consoleSession?: { token: string; serviceKey: string };
};
export function createBrowserProxy(
  run: (request: NextRequest, credentials: Credentials) => Promise<Response>,
) {
  if (browserAuthConfiguration.provider === "better-auth")
    return (request: NextRequest) =>
      run(request, { cookie: request.headers.get("cookie") ?? "" });
  if (
    !(
      process.env.CLERK_PUBLISHABLE_KEY ||
      process.env.NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY
    ) ||
    !process.env.CLERK_SECRET_KEY
  )
    throw Error("Clerk browser keys must be configured");
  const configured = browserAuthConfiguration;
  const clerk = clerkMiddleware(
    async (auth, request) => {
      if (request.nextUrl.pathname === "/api/sso/login") {
        const target = new URL("/login", request.url);
        target.searchParams.set(
          "returnTo",
          safeBrowserReturn(request.nextUrl.searchParams.get("returnTo")),
        );
        return NextResponse.redirect(target);
      }
      if (request.nextUrl.pathname.startsWith("/api/"))
        return NextResponse.next();
      const token = await (await auth()).getToken();
      const serviceKey = process.env.WEBDOCK_CONSOLE_SERVICE_KEY;
      if (
        token &&
        configured.transport === "console" &&
        (!serviceKey || serviceKey.length < 32)
      )
        throw Error("Console delegation is not configured.");
      return run(request, {
        cookie: "",
        ...(token
          ? { consoleSession: { token, serviceKey: serviceKey! } }
          : {}),
      });
    },
    {
      publishableKey:
        process.env.CLERK_PUBLISHABLE_KEY ||
        process.env.NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY,
      // The static secret comes from Clerk's server environment, not dynamic-key propagation.
      signInUrl: browserAuthPageURL("/login"),
      signUpUrl: browserAuthPageURL("/sign-up"),
      authorizedParties: configured.authorizedParties,
      audience: configured.audience,
    },
  );
  return async (request: NextRequest, event: NextFetchEvent) => {
    const canonicalOrigin = process.env.NEXT_PUBLIC_SERVER_URL || process.env.WEBDOCK_CONSOLE_ORIGIN;
    if (canonicalOrigin) request = canonicalClerkRequest(request, canonicalOrigin, {
      allowLocalHTTP: "allowLocalHTTP" in configured && configured.allowLocalHTTP === true,
      environment: process.env.NODE_ENV,
    });
    const response = await clerk(request, event);
    return response instanceof Response ? resumeClerkRequest(request.url, response) : response;
  };
}
