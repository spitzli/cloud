import { HostingError, type HostingCommand } from "@webdock/hosting-contracts";
import { hostingBridge } from "../hosting/bridge";
export async function hostingTokenCall(token: string, command: HostingCommand) {
  const id = process.env.WEBDOCK_SSO_CLIENT_ID,
    secret = process.env.WEBDOCK_SSO_CLIENT_SECRET;
  if (!id || !secret)
    throw new HostingError(503, "Hosting authentication is unavailable.");
  const response = await hostingBridge(
    new Request(
      new URL(
        "/api/hosting/bridge",
        process.env.BETTER_AUTH_URL || "http://localhost:3125",
      ),
      {
        method: "POST",
        headers: {
          "content-type": "application/json",
          authorization:
            "Basic " + Buffer.from(id + ":" + secret).toString("base64"),
        },
        body: JSON.stringify({ accessToken: token, command }),
      },
    ),
  );
  const result = (await response.json()) as { data?: unknown; error?: string };
  if (!response.ok)
    throw new HostingError(
      response.status,
      result.error || "Hosting is unavailable.",
    );
  return result.data;
}
