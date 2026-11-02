import { createHash, createHmac, timingSafeEqual } from "node:crypto";
import { withDatabaseScope } from "@webdock/database-runtime";
import { workerDatabase } from "../../../db/runtime";
import { vercelSettings } from "../vercel-settings";
import { decodeVercelCredential } from "./vercel-store";
import type { Fetch } from "../../shared/http";
class EventError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
  }
}
export async function vercelRegionEvent(
  request: Request,
  fetcher: Fetch = fetch,
): Promise<Response> {
  try {
    const settings = vercelSettings();
    if (!settings || !/^team_[A-Za-z0-9]+$/.test(settings.teamID))
      throw new EventError(503, "Vercel integration is not configured.");
    const signature = request.headers.get("x-vercel-signature") || "";
    if (!/^[a-fA-F0-9]{40}$/.test(signature))
      throw new EventError(403, "Invalid webhook signature.");
    const reader = request.body?.getReader();
    if (!reader) throw new EventError(400, "Invalid body");
    let size = 0;
    const chunks: Uint8Array[] = [];
    try {
      for (;;) {
        const part = await reader.read();
        if (part.done) break;
        size += part.value.byteLength;
        if (size > 65536) {
          await reader.cancel();
          throw new EventError(413, "Webhook body too large.");
        }
        chunks.push(part.value);
      }
    } finally {
      reader.releaseLock();
    }
    const raw = Buffer.concat(chunks),
      expected = createHmac("sha1", settings.clientSecret).update(raw).digest();
    if (!timingSafeEqual(expected, Buffer.from(signature, "hex")))
      throw new EventError(403, "Invalid webhook signature.");
    let event;
    try {
      event = JSON.parse(raw.toString("utf8"));
    } catch {
      throw new EventError(400, "Invalid webhook JSON.");
    }
    if (event?.type !== "project.created")
      throw new EventError(400, "Only project.created is supported.");
    if (event.payload?.team?.id !== settings.teamID)
      throw new EventError(
        403,
        "Webhook team is not managed by this integration.",
      );
    const project = event.payload?.project?.id;
    if (
      typeof project !== "string" ||
      !/^prj_[A-Za-z0-9]{1,128}$/.test(project)
    )
      throw new EventError(400, "Invalid webhook project.");
    const hash = createHash("sha256").update(raw).digest("hex"),
      eventID =
        typeof event.id === "string" &&
        event.id.length > 0 &&
        event.id.length <= 200
          ? event.id
          : hash;
    return await withDatabaseScope(
      { kind: "system", purpose: `vercel-event:${settings.teamID}:${project}` },
      async () => {
        const access = (
          await workerDatabase.query(
            "SELECT webdock_rls.vercel_event_access($1,$2,$3,$4) AS value",
            [settings.teamID, project, eventID, hash],
          )
        ).rows[0].value;
        if (!access)
          throw new EventError(503, "Connect Vercel in Webdock first.");
        if (access.receipt)
          return Response.json(access.receipt, {
            headers: { "cache-control": "no-store" },
          });
        const credential = await decodeVercelCredential(
          access.encrypted,
          settings.teamID,
          settings.cookieSecret,
        );
        const url = new URL("https://api.vercel.com/v9/projects/" + project);
        url.searchParams.set("teamId", settings.teamID);
        const call = async (method: "GET" | "PATCH") => {
          const response = await fetcher(url, {
            method,
            redirect: "error",
            cache: "no-store",
            signal: AbortSignal.timeout(15_000),
            headers: {
              authorization: "Bearer " + credential.accessToken,
              "content-type": "application/json",
            },
            ...(method === "PATCH"
              ? {
                  body: JSON.stringify({
                    resourceConfig: {
                      functionDefaultRegions: ["fra1"],
                      functionZeroConfigFailover: false,
                    },
                  }),
                }
              : {}),
          });
          if (!response.ok)
            throw new EventError(
              502,
              "Vercel could not apply the project region policy.",
            );
          const body = await response.json();
          if (body?.id !== project || body?.accountId !== settings.teamID)
            throw new EventError(
              502,
              "Vercel returned a different project or team.",
            );
          return body;
        };
        const compliant = (body: Awaited<ReturnType<typeof call>>) =>
          Array.isArray(body.resourceConfig?.functionDefaultRegions) &&
          body.resourceConfig.functionDefaultRegions.length === 1 &&
          body.resourceConfig.functionDefaultRegions[0] === "fra1" &&
          body.resourceConfig.functionZeroConfigFailover === false;
        const changed = !compliant(await call("GET"));
        if (changed) {
          await call("PATCH");
          if (!compliant(await call("GET")))
            throw new EventError(
              502,
              "Vercel did not confirm the project region policy.",
            );
        }
        const result = (
          await workerDatabase.query(
            "SELECT webdock_rls.record_vercel_event($1,$2,$3,$4,$5,$6) AS value",
            [settings.teamID, project, access.owner, changed, eventID, hash],
          )
        ).rows[0].value;
        return Response.json(result, {
          headers: { "cache-control": "no-store" },
        });
      },
    );
  } catch (error) {
    return Response.json(
      {
        error:
          error instanceof EventError
            ? error.message
            : "Vercel region policy could not be completed.",
      },
      {
        status: error instanceof EventError ? error.status : 503,
        headers: { "cache-control": "no-store" },
      },
    );
  }
}
