import type { HostingPorts } from "./ports";
import type { Connection } from "./authorization";
import type { HostingActor } from "@spitzli/hosting-contracts";
export type Dependencies = {
  database: HostingPorts["database"];
};

export function createHostingDb(dependencies: Dependencies) {
  async function transaction<T>(run: (db: Connection) => Promise<T>) {
    const c = await dependencies.database.connect();
    try {
      await c.query("BEGIN");
      await c.query("SET LOCAL lock_timeout='5s'");
      const result = await run(c);
      await c.query("COMMIT");
      return result;
    } catch (error) {
      await c.query("ROLLBACK");
      throw error;
    } finally {
      c.release();
    }
  }
  async function audit(
    db: Connection,
    actor: HostingActor,
    action: string,
    target: string,
    outcome = "succeeded",
  ) {
    await db.query(
      "INSERT INTO webdock_auth.hosting_audit(subject,action,target_id,outcome) VALUES($1,$2,$3,$4)",
      [actor.subject, action, target, outcome],
    );
  }
  async function planLock(db: Connection, customerID: string) {
    await db.query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))", [
      `plan:${customerID}`,
    ]);
  }
  async function clusterLock(db: Connection, clusterID: string) {
    await db.query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))", [
      `hosting-cluster:${clusterID}`,
    ]);
  }
  return { transaction, audit, planLock, clusterLock };
}
