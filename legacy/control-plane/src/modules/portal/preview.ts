import {
  msgid,
  preferenceFromCookie,
  resolveLocale,
  translator,
} from "@webdock/i18n";
export async function tenantPreview(
  request: Request,
  action: string,
  origin: string,
  studioCall: <T>(operation: string, args?: unknown[]) => Promise<T>,
) {
  const i18n = translator(
    resolveLocale(
      request.headers.get("accept-language"),
      preferenceFromCookie(request.headers.get("cookie")),
    ),
  );
  const failure = (message: string, status: number) =>
    new Response(i18n.t(message), {
      status,
      headers: { "Cache-Control": "no-store", "Content-Language": i18n.locale },
    });
  if (request.headers.get("origin") !== origin)
    return failure(msgid("Invalid origin."), 403);
  if (!["start", "exit"].includes(action))
    return failure(msgid("Page not found."), 404);
  try {
    if (Number(request.headers.get("content-length")) > 4096)
      return failure(msgid("The request is too large."), 413);
    const reader = request.body?.getReader();
    const chunks: Uint8Array[] = [];
    let size = 0;
    if (reader)
      try {
        for (;;) {
          const part = await reader.read();
          if (part.done) break;
          size += part.value.byteLength;
          if (size > 4096) {
            await reader.cancel();
            return failure(msgid("The request is too large."), 413);
          }
          chunks.push(part.value);
        }
      } finally {
        reader.releaseLock();
      }
    const body = Buffer.concat(chunks).toString("utf8");
    const form = new URLSearchParams(body);
    if (action === "start") {
      const customerID = form.get("customerID") || "";
      if (
        form.getAll("customerID").length !== 1 ||
        !/^[1-9]\d{0,18}$/.test(customerID)
      )
        return failure(msgid("Invalid customer."), 400);
      await studioCall("startTenantPreview", [customerID]);
      return new Response(null, {
        status: 303,
        headers: {
          Location: `${origin}/tenants/${customerID}`,
          "Cache-Control": "no-store",
        },
      });
    }
    const result = await studioCall<{ customerID?: string }>(
      "exitTenantPreview",
    );
    const target =
      result.customerID && /^[1-9]\d{0,18}$/.test(result.customerID)
        ? `/tenants/${result.customerID}`
        : "/tenants";
    return new Response(null, {
      status: 303,
      headers: { Location: origin + target, "Cache-Control": "no-store" },
    });
  } catch {
    return failure(
      msgid("Could not switch customer view. Go back and try again."),
      403,
    );
  }
}
