import {requireInstalledOperation} from "../../installation";
import { withDatabaseScope } from "@webdock/database-runtime";
import { withHostingDatabase } from "../../db/database-context";
import { createHash, timingSafeEqual } from "node:crypto";
import { z } from "zod";
import { HostingError } from "@webdock/hosting-contracts";
import { hostingIdentity } from "../hosting/bridge";
import { readHostingJSON, requireHostingEnvironment } from "../hosting/http";
import { transaction } from "../hosting/db";
import { executeDatabase, exchangeLaunch, inspectSession } from "./service";

function responseError(error: unknown) {
  const status =
    error instanceof HostingError
      ? error.status
      : error instanceof z.ZodError
        ? 400
        : 503;
  return Response.json(
    {
      error:
        status === 503
          ? "Database service is unavailable."
          : error instanceof HostingError
            ? error.message
            : "Invalid database request.",
    },
    { status, headers: { "Cache-Control": "no-store" } },
  );
}

export async function databaseBridge(request: Request) {
  try {
    requireHostingEnvironment();
    const body = z
      .object({
        accessToken: z.string().min(1).max(16384),
        command: z.unknown(),
      })
      .strict()
      .parse(await readHostingJSON(request));
    const actor = await hostingIdentity(request, body.accessToken);
    requireInstalledOperation("database.command",[body.command]);
    const data = await withHostingDatabase(actor, () =>
      transaction((db) => executeDatabase(db, actor, body.command)),
    );
    return Response.json(
      { data },
      { headers: { "Cache-Control": "no-store" } },
    );
  } catch (error) {
    return responseError(error);
  }
}

export async function databaseGateway(request: Request) {
  try {
    requireHostingEnvironment();
    const expected = process.env.WEBDOCK_DATABASE_GATEWAY_SECRET;
    const supplied = request.headers
      .get("authorization")
      ?.match(/^Bearer (\S+)$/)?.[1];
    if (
      !expected ||
      expected.length < 32 ||
      !supplied ||
      supplied.length > 512 ||
      !timingSafeEqual(
        createHash("sha256").update(expected).digest(),
        createHash("sha256").update(supplied).digest(),
      )
    )
      throw new HostingError(401, "Gateway authentication required.");
    const body = z
      .discriminatedUnion("action", [
        z
          .object({
            action: z.literal("exchange"),
            code: z.string().length(43),
          })
          .strict(),
        z
          .object({
            action: z.literal("inspect"),
            token: z.string().length(43),
          })
          .strict(),
        z
          .object({ action: z.literal("logout"), token: z.string().length(43) })
          .strict(),
      ])
      .parse(await readHostingJSON(request, 2048));
    const data = await withDatabaseScope(
      { kind: "system", purpose: "database-gateway" },
      () =>
        transaction(async (db) => {
          if (body.action === "exchange") return exchangeLaunch(db, body.code);
          if (body.action === "inspect") return inspectSession(db, body.token);
          await db.query(
            "DELETE FROM webdock_auth.database_session WHERE token_hash=$1",
            [createHash("sha256").update(body.token).digest("hex")],
          );
          await db.query(
            "DELETE FROM webdock_auth.managed_database_session WHERE token_hash=$1",
            [createHash("sha256").update(body.token).digest("hex")],
          );
          return { ok: true };
        }),
    );
    return Response.json(
      { data },
      { headers: { "Cache-Control": "no-store" } },
    );
  } catch (error) {
    return responseError(error);
  }
}
