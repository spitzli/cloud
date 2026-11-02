import { withDatabaseScope } from "@webdock/database-runtime";
import { timingSafeEqual } from "node:crypto";
import { refreshStorageBatch } from "../../../../../modules/platform/storage-usage";

export const dynamic = "force-dynamic";
export const maxDuration = 300;

export async function GET(request: Request) {
  const secret = process.env.CRON_SECRET;
  const actual = Buffer.from(request.headers.get("authorization") || "");
  const expected = Buffer.from(`Bearer ${secret || ""}`);
  if (
    !secret ||
    actual.length !== expected.length ||
    !timingSafeEqual(actual, expected)
  ) {
    return Response.json({ error: "Unauthorized" }, { status: 401 });
  }
  return Response.json(
    await withDatabaseScope({ kind: "system", purpose: "storage-cron" }, () =>
      refreshStorageBatch(),
    ),
    { headers: { "Cache-Control": "no-store" } },
  );
}
