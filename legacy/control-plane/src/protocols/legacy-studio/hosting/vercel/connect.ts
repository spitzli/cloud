import { hostingCall as call } from "../../../../modules/portal/hosting";
const hostingCall = async <T>(command: unknown): Promise<T> =>
  (await call(command)) as T;
import { resourceID } from "@webdock/hosting-contracts";
export async function POST(request: Request) {
  const origin = process.env.WEBDOCK_STUDIO_ORIGIN;
  if (!origin || request.headers.get("origin") !== new URL(origin).origin)
    return new Response(null, { status: 403 });
  try {
    const form = await request.formData();
    const customerID = resourceID.parse(form.get("customerID"));
    const result = await hostingCall<{ url: string }>({
      action: "byok.vercel.begin",
      customerID,
    });
    return new Response(null, {
      status: 303,
      headers: {
        Location: result.url,
        "Cache-Control": "no-store",
        "Referrer-Policy": "no-referrer",
        "Set-Cookie": `${new URL(origin).protocol === "https:" ? "__Host-" : ""}webdock-byok-vercel=${new URL(result.url).searchParams.get("state")}; Path=/; HttpOnly; SameSite=Lax; Max-Age=600${new URL(origin).protocol === "https:" ? "; Secure" : ""}`,
      },
    });
  } catch {
    return new Response(null, {
      status: 303,
      headers: {
        Location: new URL("/hosting/connection-error", origin).href,
        "Cache-Control": "no-store",
      },
    });
  }
}
