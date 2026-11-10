import type { HostingPorts } from "./ports";
import { HostingError } from "@spitzli/hosting-contracts";
import type { SetupRow } from "./project-setup";
export type Dependencies = {
  executeAuthorizedSetup: ReturnType<
    typeof import("./setup-execution").createHostingSetupExecution
  >["executeSetupStep"];
  withDatabaseScope: HostingPorts["withDatabaseScope"];
  transaction: ReturnType<typeof import("./db").createHostingDb>["transaction"];
  executeSetupStep: ReturnType<
    typeof import("./setup-execution").createHostingSetupExecution
  >["executeSetupStep"];
  env: HostingPorts["env"];
};

export function createHostingSetupWorker(dependencies: Dependencies) {
  const dispatch = <T>(run: () => T) =>
    dependencies.withDatabaseScope(
      { kind: "system", purpose: "hosting-setup-dispatch" },
      run,
    );
  async function runSetupBatch({
    limit = 25,
    deadline = Date.now() + 40000,
  }: {
    limit?: number;
    deadline?: number;
  } = {}) {
    let processed = 0,
      failed = 0;
    if (dependencies.env.SPITZLI_SETUP_ENABLED !== "true")
      return { processed, failed };
    while (processed < limit && Date.now() < deadline) {
      const row = await dispatch(() =>
        dependencies.transaction(async (db) => {
          const row = (
            await db.query(`WITH next AS (SELECT id FROM webdock_auth.hosting_setup WHERE state='running' AND next_attempt_at<=now() AND (lease_until IS NULL OR lease_until<now()) ORDER BY next_attempt_at,id FOR UPDATE SKIP LOCKED LIMIT 1)
        UPDATE webdock_auth.hosting_setup s SET generation=generation+1,lease_until=now()+interval '60 seconds' FROM next WHERE s.id=next.id RETURNING s.*`)
          ).rows[0] as SetupRow | undefined;
          return row;
        }),
      );
      if (!row) break;
      processed++;
      try {
        const result = await dependencies.executeAuthorizedSetup(row);
        const advanced =
          JSON.stringify(result.steps) !== JSON.stringify(row.steps) ||
          JSON.stringify(result.resources) !== JSON.stringify(row.resources);
        await dispatch(() =>
          dependencies.transaction((db) =>
            db.query(
              "UPDATE webdock_auth.hosting_setup SET resources=$3,steps=$4,state=$5,lease_until=NULL,next_attempt_at=now()+make_interval(secs=>$6),reason=NULL,updated_at=now() WHERE id=$1 AND generation=$2 AND lease_until>now() AND state='running'",
              [
                row.id,
                row.generation,
                JSON.stringify(result.resources),
                JSON.stringify(result.steps),
                result.ready ? "ready" : "running",
                advanced ? 0 : 10,
              ],
            ),
          ),
        );
      } catch (error) {
        failed++;
        const reason =
          error instanceof HostingError
            ? error.message
            : "Setup could not continue. Review its services and retry.";
        const steps = row.steps.map((s) => ({ ...s }));
        const current = steps.find(
          (s) => !["succeeded", "skipped"].includes(s.state),
        );
        if (current) {
          current.state = "blocked";
          current.reason = reason;
        }
        await dispatch(() =>
          dependencies.transaction((db) =>
            db.query(
              "UPDATE webdock_auth.hosting_setup SET state='blocked',reason=$3,steps=$4,lease_until=NULL,updated_at=now() WHERE id=$1 AND generation=$2 AND lease_until>now() AND state='running'",
              [row.id, row.generation, reason, JSON.stringify(steps)],
            ),
          ),
        );
      }
    }
    return { processed, failed };
  }
  return { runSetupBatch };
}
