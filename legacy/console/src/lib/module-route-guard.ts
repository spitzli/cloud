import { moduleForPath } from "@webdock/module-sdk/selection";
import type { ModuleView } from "@webdock/module-sdk";
export async function guardModuleRoute(
  request: Request,
  readSession: () => Promise<{
    status: number;
    body: { data?: { installation?: ModuleView } };
  }>,
  clerkSession = false,
): Promise<Response | undefined> {
  const url = new URL(request.url),
    module = moduleForPath(url.pathname);
  if (!module) return;
  const headers = { "cache-control": "no-store" };
  try {
    const result = await readSession();
    if (result.status === 401)
      return new Response(null, {
        status: 307,
        headers: {
          ...headers,
          location:
            "/api/sso/login?returnTo=" +
            encodeURIComponent(url.pathname + url.search),
        },
      });
    if (clerkSession && result.status === 403)
      return new Response(null, { status: 307, headers: { ...headers, location: "/access-pending" } });
    if (result.status !== 200)
      return new Response("Service unavailable", { status: 503, headers });
    if (
      !["enabled", "draining"].includes(
        result.body.data?.installation?.modules[module] ?? "",
      )
    )
      return new Response("Not found", { status: 404, headers });
  } catch {
    return new Response("Service unavailable", { status: 503, headers });
  }
}
