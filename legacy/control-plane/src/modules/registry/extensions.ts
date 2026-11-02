import { sql } from "drizzle-orm";
import { RegistryError } from "@webdock/core/registry/errors";
import type { RegistryTransaction } from "@webdock/core/registry/ports";

/** Historical inventory still protects project ownership after CMS removal. */
export async function retainedRegistryInventory(
  tx: RegistryTransaction,
  collection: string,
  key: string,
) {
  if (collection !== "projects") return;
  const present = await tx.execute<{ present: boolean }>(
    sql`SELECT to_regclass('webdock_admin.cms_instances') IS NOT NULL AS present`,
  );
  if (!present.rows[0]?.present) return;
  const linked = await tx.execute<{ id: string }>(
    sql`SELECT id FROM webdock_admin.cms_instances WHERE project_id = ${key} AND status <> 'retired' LIMIT 1`,
  );
  if (linked.rows.length)
    throw new RegistryError(
      "Retire the linked CMS before archiving this project.",
    );
}
