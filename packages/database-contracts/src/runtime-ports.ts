import type {
  HostingActor,
  DatabaseCommand as ManagedCommand,
  DatabasePlan,
  DatabaseView,
} from "@spitzli/hosting-contracts";
import type {
  Agent,
  DatabaseReader,
} from "@spitzli/hosting-contracts/runtime-ports";
export type ManagedDatabaseOffer = {
  id: string;
  name: string;
  snapshot: DatabasePlan & {
    backupStatus: "not-configured";
    sourceDatabaseID?: string;
    requiresInterruption?: boolean;
  };
  expiresAt: string;
};
export type ManagedDatabasePlanEntry = {
  id: string;
  plan: DatabasePlan;
  availability: "disabled" | "pilot" | "public";
  revision: number;
};
export type ManagedDatabaseCredentials = {
  databaseID: string;
  databaseName: string;
  host: string;
  directPort: number;
  pooledPort: number;
  credentialRevision: number;
  application: { username: string; password: string };
  migration: { username: string; password: string };
  sslmode: string;
  caCertificate: string;
};
export type DatabaseAgentJob = {
  version: number;
  operationID: string;
  generation: number;
  leaseToken: string;
  leaseUntil: string;
  clusterID: string;
  action: string;
  phase: string;
  revision: number;
  previousPodUID: string | null;
  backend: {
    id: string;
    kind: string;
    namespace: string;
    node: string;
    poolID: string;
    revision: number;
    cnpgUID: string | null;
    resources: {
      cpuMillicores: number;
      memoryBytes: number;
      volumeBytes: number;
    };
    host: string;
    directPort: number;
    pooledPort: number;
  };
  database: {
    id: string;
    kind: string;
    name: string;
    revision: number;
    credentialRevision: number;
    connectionLimit: number;
    credentials: { applicationPassword: string; migrationPassword: string };
  } | null;
  members: {
    id: string;
    projectID: string;
    databaseName: string;
    enabled: boolean;
  }[];
  migration?: {
    id: string;
    source: {
      id: string;
      namespace: string;
      cnpgUID: string;
      host: string;
      kind: string;
    };
    transferPassword: string;
    jobUID: string | null;
    copySHA256: string | null;
  };
};
/** Public product services injected by API composition. No server implementation imports. */
export interface DatabaseServices {
  executeDatabaseCommand(
    actor: HostingActor,
    command: Extract<ManagedCommand, { action: "databases.offers.create" }>,
  ): Promise<ManagedDatabaseOffer>;
  executeDatabaseCommand(
    actor: HostingActor,
    command: Extract<ManagedCommand, { action: "databases.list" }>,
  ): Promise<DatabaseView[]>;
  executeDatabaseCommand(
    actor: HostingActor,
    command: Extract<ManagedCommand, { action: "databases.plans.list" }>,
  ): Promise<ManagedDatabasePlanEntry[]>;
  executeDatabaseCommand(
    actor: HostingActor,
    command: Extract<ManagedCommand, { action: "databases.get" }>,
  ): Promise<DatabaseView>;
  executeDatabaseCommand(
    actor: HostingActor,
    command: Extract<ManagedCommand, { action: "databases.acceptOrder" }>,
  ): Promise<{ orderID: string; databaseID: string; operationID: string }>;
  /** Raw transport boundary: the owning service parses and dispatches its command schema. */
  executeDatabaseCommand(
    actor: HostingActor,
    command: unknown,
  ): Promise<unknown>;
  displayDatabaseCredentials(
    actor: HostingActor,
    databaseID: string,
  ): Promise<ManagedDatabaseCredentials>;
  databaseBindingMaterial(
    db: DatabaseReader,
    input: {
      databaseID: string;
      projectID: string;
      customerID: string;
      clusterID: string;
      bindingID: string;
      variable: string;
    },
  ): Promise<{
    credentialRevision: number;
    environment: Record<string, string>;
    runtime: {
      id: string;
      revision: number;
      backendID: string;
      caCertificate: string;
    };
  }>;
  databaseObservationConfig(agent: Agent): Promise<{
    installations: {
      id: string;
      namespace: string;
      cnpgUID: string;
      host: string;
      directPort: number;
      pooledPort: number;
      state: string;
      members: string[];
    }[];
  }>;
  reportDatabaseObservations(
    agent: Agent,
    input: unknown,
  ): Promise<{ accepted: boolean }>;
  claimDatabaseOperation(agent: Agent): Promise<DatabaseAgentJob | null>;
  checkpointDatabaseOperation(
    agent: Agent,
    input: unknown,
  ): Promise<{ accepted: boolean }>;
  completeDatabaseOperation(
    agent: Agent,
    input: unknown,
  ): Promise<{ accepted: boolean; pending?: boolean }>;
  guardDatabaseOperation(
    agent: Agent,
    input: unknown,
  ): Promise<{ valid: boolean }>;
}
