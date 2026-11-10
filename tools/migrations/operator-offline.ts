/** Shared offline-only transaction and durable secret output. Never import from a runtime service. */
import { constants } from "node:fs";
import { open, unlink } from "node:fs/promises";
import { Pool, type PoolClient } from "pg";
import { verifiedPostgresConfig } from "@spitzli/database-runtime";
import {
  OperatorSetupError,
  type InitialOperatorInput,
  type operatorConfig,
} from "./operator-config";
import {
  provisionInitialOperator,
  type InitialOperatorSecrets,
} from "./operator-provision";

export class OperatorRunError extends OperatorSetupError {
  constructor(
    message: string,
    readonly commitUncertain: boolean,
  ) {
    super(message);
  }
}
export type OperatorSummary = {
  created: true;
  operatorID: string;
  clientID: string;
  checksum: string;
  securitySetupRequired: true;
};

/** Owns and closes the pool. The output is durable before COMMIT, and retained if COMMIT is uncertain. */
export async function persistInitialOperator(
  pool: Pool,
  database: string,
  destination: string,
  provision: (db: PoolClient) => Promise<InitialOperatorSecrets>,
): Promise<OperatorSummary> {
  let output: Awaited<ReturnType<typeof open>> | undefined;
  let createdOutput = false,
    commitStarted = false;
  try {
    output = await open(
      destination,
      constants.O_WRONLY |
        constants.O_CREAT |
        constants.O_EXCL |
        constants.O_NOFOLLOW,
      0o600,
    );
    createdOutput = true;
    const db = await pool.connect();
    try {
      await db.query("BEGIN");
      if (
        (await db.query("SELECT current_database() AS name")).rows[0].name !==
        database
      )
        throw new OperatorSetupError(
          "Connected database differs from explicit target",
        );
      const result = await provision(db);
      await output.writeFile(
        JSON.stringify(
          { status: "verify-command-result", ...result },
          null,
          2,
        ) + "\n",
      );
      await output.sync();
      commitStarted = true;
      await db.query("COMMIT");
      return {
        created: true,
        operatorID: result.operatorID,
        clientID: result.clientID,
        checksum: result.checksum,
        securitySetupRequired: true,
      };
    } catch (error) {
      await db.query("ROLLBACK").catch(() => {});
      throw error;
    } finally {
      db.release();
    }
  } catch (error) {
    throw new OperatorRunError(
      commitStarted
        ? "Initial setup commit could not be confirmed; retain the private output and inspect the recorded IDs before retrying"
        : error instanceof OperatorSetupError
          ? error.message
          : "Initial operator setup failed; no changes committed",
      commitStarted,
    );
  } finally {
    // Cleanup errors must neither leak diagnostics nor undo a confirmed commit.
    await output?.close().catch(() => {});
    await pool.end().catch(() => {});
    if (createdOutput && !commitStarted)
      await unlink(destination).catch(() => {});
  }
}

export function runOfflineOperator(
  config: ReturnType<typeof operatorConfig>,
  input: InitialOperatorInput,
  destination: string,
) {
  const pool = new Pool({
    ...(config.local
      ? { connectionString: config.url.href }
      : verifiedPostgresConfig(config.url.href, config.ca)),
    max: 1,
    connectionTimeoutMillis: 10000,
  });
  return persistInitialOperator(
    pool,
    decodeURIComponent(config.url.pathname.slice(1)),
    destination,
    (db) => provisionInitialOperator(db, input),
  );
}
