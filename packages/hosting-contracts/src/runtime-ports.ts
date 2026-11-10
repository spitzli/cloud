import type { PoolClient } from "pg";
import type { DatabaseScope } from "@spitzli/database-runtime";
import type { HostingActor, ClusterObservation } from "./index";
/** Shared control-plane infrastructure, independent of product service implementations. */
export type DatabaseReader = Pick<PoolClient, "query">;
export type Connection = DatabaseReader;
export type Agent = {
  clusterID: string;
  generation: number;
  credentialHash: string;
};
export type HostingAuthorizationTarget = {
  customerID?: string;
  projectID?: string;
  write?: boolean;
  operator?: boolean;
  live?: boolean;
  tenantAdmin?: boolean;
  permission?: string;
  legacyRole?: boolean;
};
export type HostingAuthorization = {
  operator: boolean;
  liveReads: boolean;
  customerID?: string;
  project?: {
    id: string;
    customer_id: string;
    status: string;
    mode?: string | null;
    provider?: string | null;
    cluster_id?: string | null;
    revision?: number | null;
    own_images?: boolean | null;
  };
  canWrite: boolean;
};
export type VerifiedAgent = {
  generation: number;
  credential_hash: string;
  revoked: boolean;
  last_seen: Date | null;
  observation: ClusterObservation | null;
};
export interface HostingRuntimePorts {
  env: Record<string, string | undefined>;
  withDatabaseScope<T>(scope: DatabaseScope, run: () => T): T;
  withHostingDatabase<T>(
    actor: HostingActor,
    run: () => Promise<T>,
  ): Promise<T>;
  transaction<T>(run: (db: DatabaseReader) => Promise<T>): Promise<T>;
  authorizeHosting(
    actor: HostingActor,
    target: HostingAuthorizationTarget,
    db: DatabaseReader,
  ): Promise<HostingAuthorization>;
  audit(
    db: DatabaseReader,
    actor: HostingActor,
    action: string,
    target: string,
    outcome?: string,
  ): Promise<void>;
  clusterLock(db: DatabaseReader, clusterID: string): Promise<void>;
  planLock(db: DatabaseReader, customerID: string): Promise<void>;
  verifyAgent(db: DatabaseReader, agent: Agent): Promise<VerifiedAgent>;
  replay<Result = unknown>(
    db: DatabaseReader,
    actor: HostingActor,
    key: string,
    payload: unknown,
  ): Promise<{
    hash: string;
    existing:
      | {
          id: string;
          status: string;
          payload_hash: string;
          result: Result;
          app_id?: string | null;
          target_revision?: number | null;
        }
      | undefined;
  }>;
  recordResult(
    db: DatabaseReader,
    actor: HostingActor,
    key: string,
    hash: string,
    action: string,
    result: unknown,
    target?: { customerID?: string; projectID?: string; clusterID?: string },
  ): Promise<void>;
}
