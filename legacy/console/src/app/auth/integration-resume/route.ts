import { auth } from "@clerk/nextjs/server";
import { resumeIntegrationCallback } from "@webdock/api-client/integration-resume";
import { browserAuthConfiguration } from "@/lib/browser-auth-config";
export async function GET(request: Request): Promise<Response> {
  if (browserAuthConfiguration.provider !== "clerk")
    return new Response(null, { status: 404 });
  const token = await (await auth()).getToken();
  return resumeIntegrationCallback({
    apiOrigin: process.env.WEBDOCK_API_ORIGIN ?? "",
    uiOrigin:
      process.env.WEBDOCK_STUDIO_ORIGIN ||
      process.env.NEXT_PUBLIC_SERVER_URL ||
      "https://studio.webdock.dev",
    consoleOrigin:
      process.env.WEBDOCK_CONSOLE_ORIGIN ||
      process.env.WEBDOCK_STUDIO_ORIGIN ||
      process.env.NEXT_PUBLIC_SERVER_URL ||
      "https://studio.webdock.dev",
    cookie: request.headers.get("cookie") ?? "",
    bearerToken: token,
    serviceKey: process.env.WEBDOCK_CONSOLE_SERVICE_KEY,
  });
}
