import { hostingCall as call } from "../../../../modules/portal/hosting";
const hostingCall = async <T>(command: unknown): Promise<T> =>
  (await call(command)) as T;
import { commandSchema } from "@webdock/hosting-contracts";
import { vercelCompletionURL } from "../../../../modules/integrations/vercel/vercel-oauth";
export async function GET(request: Request) {
  const origin = process.env.WEBDOCK_STUDIO_ORIGIN!;
  const cookieName =
    (new URL(origin).protocol === "https:" ? "__Host-" : "") +
    "webdock-byok-vercel";
  const clearCookie = `${cookieName}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0${new URL(origin).protocol === "https:" ? "; Secure" : ""}`;
  try {
    const q = new URL(request.url).searchParams;
    if ([...q.keys()].some((k) => q.getAll(k).length !== 1) || q.has("error"))
      throw Error();
    const cookie = (request.headers.get("cookie") ?? "")
      .split(";")
      .map((v) => v.trim())
      .filter((v) => v.startsWith(cookieName + "="));
    if (
      cookie.length !== 1 ||
      cookie[0].slice(cookieName.length + 1) !== q.get("state")
    )
      throw Error();
    const completion = q.has("next")
      ? vercelCompletionURL(q.get("next"))
      : null;
    const command = commandSchema.parse({
      action: "byok.vercel.finish",
      state: q.get("state"),
      code: q.get("code"),
      configurationID: q.get("configurationId"),
      teamID: q.get("teamId"),
    });
    const result = await hostingCall<{ customerID: string }>(command);
    return new Response(null, {
      status: 303,
      headers: {
        Location:
          completion ??
          new URL(`/hosting/vercel/${result.customerID}`, origin).href,
        "Set-Cookie": clearCookie,
        "Cache-Control": "no-store",
        "Referrer-Policy": "no-referrer",
        "Cross-Origin-Opener-Policy": "unsafe-none",
      },
    });
  } catch {
    return new Response(null, {
      status: 303,
      headers: {
        Location: new URL("/hosting/connection-error", origin).href,
        "Set-Cookie": clearCookie,
        "Cache-Control": "no-store",
        "Referrer-Policy": "no-referrer",
      },
    });
  }
}
