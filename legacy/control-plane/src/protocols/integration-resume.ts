import type { createIntegrationCallbackHandoff } from "./integration-callback-handoff";
type Callback = (
  request: Request,
  options: { refreshed: true },
) => Promise<Response>;
const productCallbacks = new Map<string, Record<string, Callback>>();
/** Full distribution contributes already-authenticated, product-gated callbacks. */
export function configureProductIntegrationCallbacks(
  origin: string,
  callbacks: Record<string, Callback>,
) {
  productCallbacks.set(origin, { ...callbacks });
}
export function createIntegrationResumeHandler(
  origin: string,
  handoff: ReturnType<typeof createIntegrationCallbackHandoff>,
  github: Callback,
) {
  return async (request: Request) => {
    let response: Response;
    if (request.headers.get("origin") !== origin)
      response = new Response(null, { status: 403 });
    else if (
      !/^Bearer [A-Za-z0-9_.-]{1,16384}$/i.test(
        request.headers.get("authorization") ?? "",
      )
    )
      response = new Response(null, { status: 401 });
    else {
      const target = handoff.read(request.headers);
      if (!target) response = new Response(null, { status: 400 });
      else {
        const url = new URL(target, origin);
        const handler =
          url.pathname === "/api/github/callback"
            ? github
            : productCallbacks.get(origin)?.[url.pathname];
        response = handler
          ? await handler(new Request(url, { headers: request.headers }), {
              refreshed: true,
            })
          : new Response(null, { status: 400 });
      }
    }
    const headers = new Headers(response.headers);
    headers.append("set-cookie", handoff.clear());
    headers.set("cache-control", "no-store");
    headers.set("referrer-policy", "no-referrer");
    return new Response(response.body, { status: response.status, headers });
  };
}
