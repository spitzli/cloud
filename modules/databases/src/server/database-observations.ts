import type { DatabaseRuntimePorts } from "./ports";
import { z } from "zod";
import { resourceID, HostingError } from "@spitzli/hosting-contracts";
import type { Agent } from "@spitzli/hosting-contracts/runtime-ports";
export type Dependencies = {
  verifyAgent: DatabaseRuntimePorts["verifyAgent"];
  transaction: DatabaseRuntimePorts["transaction"];
};

export function createHostingDatabaseObservations(dependencies: Dependencies) {
  const bytes = z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER);
  const observation = z
    .object({
      observedAt: z.iso.datetime(),
      installations: z
        .array(
          z
            .object({
              installationID: resourceID,
              cnpgUID: z.string().min(1).max(128),
              ready: z.boolean(),
              filesystemBytes: bytes.nullable(),
              databases: z
                .array(
                  z.object({ id: resourceID, logicalBytes: bytes }).strict(),
                )
                .max(100),
            })
            .strict(),
        )
        .max(100),
    })
    .strict();
  async function databaseObservationConfig(agent: Agent) {
    return dependencies.transaction(async (db) => {
      const a = await dependencies.verifyAgent(db, agent);
      if (a.observation?.capabilities?.databaseVersion !== 1)
        return { installations: [] };
      const rows = (
        await db.query(
          "SELECT id,kind,cnpg_uid,host,direct_port,pooled_port,state FROM webdock_auth.hosting_database_backend WHERE cluster_id=$1 AND state<>'purged' AND cnpg_uid IS NOT NULL ORDER BY id LIMIT 100",
          [agent.clusterID],
        )
      ).rows;
      const installations = [];
      for (const b of rows) {
        const members = (
          await db.query(
            "SELECT id FROM webdock_auth.hosting_database WHERE backend_id=$1 AND state<>'purged' ORDER BY id LIMIT 100",
            [b.id],
          )
        ).rows.map((d) => d.id);
        installations.push({
          id: b.id,
          namespace: "wd-db-" + b.id,
          cnpgUID: b.cnpg_uid,
          host: b.host,
          directPort: b.direct_port,
          pooledPort: b.pooled_port,
          state: b.state,
          members,
        });
      }
      return { installations };
    });
  }
  async function reportDatabaseObservations(agent: Agent, input: unknown) {
    const data = observation.parse(input),
      age = Date.now() - Date.parse(data.observedAt);
    if (age > 30000 || age < -30000)
      throw new HostingError(409, "Database observations must be fresh.");
    return dependencies.transaction(async (db) => {
      const a = await dependencies.verifyAgent(db, agent);
      if (a.observation?.capabilities?.databaseVersion !== 1)
        throw new HostingError(
          409,
          "A qualified database observer is required.",
        );
      for (const item of data.installations) {
        const b = (
          await db.query(
            "SELECT id,kind FROM webdock_auth.hosting_database_backend WHERE id=$1 AND cluster_id=$2 AND cnpg_uid=$3",
            [item.installationID, agent.clusterID, item.cnpgUID],
          )
        ).rows[0];
        if (!b)
          throw new HostingError(
            409,
            "Database observation ownership changed.",
          );
        await db.query(
          `UPDATE webdock_auth.hosting_database_backend SET observed_at=$2,filesystem_used_bytes=$3,
    state=CASE WHEN state IN ('ready','degraded') THEN CASE WHEN $4 THEN 'ready' ELSE 'degraded' END ELSE state END
    WHERE id=$1 AND (observed_at IS NULL OR observed_at<=$2)`,
          [b.id, data.observedAt, item.filesystemBytes, item.ready],
        );
        for (const d of item.databases) {
          await db.query(
            `UPDATE webdock_auth.hosting_database SET usage=$3,observed_at=$4,
     state=CASE WHEN state IN ('ready','degraded') THEN CASE WHEN $5 THEN 'ready' ELSE 'degraded' END ELSE state END
     WHERE id=$1 AND backend_id=$2 AND state<>'purged' AND (observed_at IS NULL OR observed_at<=$4)`,
            [
              d.id,
              b.id,
              JSON.stringify({
                logicalBytes: d.logicalBytes,
                filesystemBytes:
                  b.kind === "dedicated" ? item.filesystemBytes : null,
                observedAt: data.observedAt,
              }),
              data.observedAt,
              item.ready,
            ],
          );
        }
        await db.query(
          "UPDATE webdock_auth.hosting_database SET state='degraded' WHERE backend_id=$1 AND state='ready' AND NOT(id=ANY($2::varchar[])) AND (observed_at IS NULL OR observed_at<=$3)",
          [b.id, item.databases.map((d) => d.id), data.observedAt],
        );
        if (!item.ready)
          await db.query(
            "UPDATE webdock_auth.hosting_database SET state='degraded' WHERE backend_id=$1 AND state='ready' AND (observed_at IS NULL OR observed_at<=$2)",
            [b.id, data.observedAt],
          );
      }
      return { accepted: true };
    });
  }
  return { databaseObservationConfig, reportDatabaseObservations };
}
