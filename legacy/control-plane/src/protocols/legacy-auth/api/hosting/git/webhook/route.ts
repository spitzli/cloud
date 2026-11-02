import { withDatabaseScope } from "@webdock/database-runtime";
import { HostingError, hostingError } from "@webdock/hosting-contracts";
import { parseGitHubWebhook } from "../../../../../../modules/hosting/git-github";
import { receiveGitEvent } from "../../../../../../modules/hosting/git-deployments";
import { requireHostingEnvironment } from "../../../../../../modules/hosting/http";
export async function POST(request: Request) {
  try {
    requireHostingEnvironment();
    const secret = process.env.WEBDOCK_GITHUB_WEBHOOK_SECRET;
    if (!secret)
      throw new HostingError(503, "GitHub webhook setup is incomplete.");
    const reader = request.body?.getReader();
    if (!reader) throw new HostingError(400, "Invalid GitHub webhook.");
    let size = 0;
    const chunks: Uint8Array[] = [];
    try {
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        size += value.length;
        if (size > 2_000_000) {
          await reader.cancel();
          throw new HostingError(413, "GitHub webhook is too large.");
        }
        chunks.push(value);
      }
    } finally {
      reader.releaseLock();
    }
    const event = parseGitHubWebhook(
      Buffer.concat(chunks),
      request.headers,
      secret,
    );
    const receipt = await withDatabaseScope(
      { kind: "system", purpose: "git-webhook" },
      () => receiveGitEvent(event),
    );
    return Response.json(receipt, {
      status: 202,
      headers: { "Cache-Control": "no-store" },
    });
  } catch (error) {
    const safe = hostingError(error);
    return Response.json(
      { error: safe.message },
      { status: safe.status, headers: { "Cache-Control": "no-store" } },
    );
  }
}
