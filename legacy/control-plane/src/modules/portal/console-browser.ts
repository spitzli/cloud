/** Cookie-only browser entrypoint. Verify CSRF before invoking the identity provider. */
export async function consoleBrowserRequest(
  request: Request,
  origin: string,
  dispatch: () => Promise<Response>,
): Promise<Response> {
  if (request.method !== "POST") return new Response(null, { status: 405 });
  if (request.headers.get("origin") !== origin)
    return new Response(null, { status: 403 });
  if (
    request.headers.has("authorization") ||
    request.headers.has("x-webdock-service-key")
  )
    return new Response(null, { status: 401 });
  if (
    request.headers.get("content-type")?.split(";")[0].trim() !==
    "application/json"
  )
    return new Response(null, { status: 415 });
  return dispatch();
}
