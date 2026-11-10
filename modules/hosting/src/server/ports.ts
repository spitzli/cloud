import type { MailAgentPorts } from "@spitzli/mail-contracts/agent-ports";
import type { Pool } from "pg";
import type { IdentityDirectory } from "@spitzli/core/identity";
import type { HostingActor } from "@spitzli/hosting-contracts";
import type { DatabaseScope } from "@spitzli/database-runtime";
import type {
  MailSecrets,
  MailFetch,
  MailDomainProvider,
  MailProviderAuth,
} from "@spitzli/mail-contracts/provider-ports";
export type Fetch = MailFetch;
/** Runtime resources are supplied by the host; the module creates no pools or global provider sessions. */
export interface HostingPorts extends MailSecrets, MailAgentPorts {
  createGitHubProvider(
    config?: import("../github").GitHubConfig,
    fetcher?: Fetch,
  ): import("../github").GitHubProvider;
  env: Record<string, string | undefined>;
  database: Pick<Pool, "query" | "connect">;
  authDatabase: Pick<Pool, "query">;
  identity: IdentityDirectory;
  currentConsolePrincipal():
    | { issuer: string; userID: string; sessionID: string; operator: boolean }
    | undefined;
  organizationPermission(
    subject: string,
    organizationID: string,
    permission: string,
  ): Promise<boolean | undefined>;
  currentDatabaseScope(): DatabaseScope | undefined;
  withDatabaseScope<T>(scope: DatabaseScope, run: () => T): T;
  withHostingDatabase<T>(
    actor: HostingActor,
    run: () => Promise<T>,
  ): Promise<T>;
  setupProductAvailable(product: "mail" | "databases"): boolean;
  requireDatabases(intent?: "access" | "provision"): void;
  requireMail(intent?: "access" | "provision"): void;
  requireHostingCommand(action: string): void;
  auth: { handler(request: Request): Promise<Response> };
  mcpResource: string;
  currentHostingMCPClaims(
    subject: string | undefined,
  ): Promise<{ disabled: boolean }>;
  vercelSettings(): {
    clientID: string;
    clientSecret: string;
    slug: string;
    origin: string;
    teamID: string;
  } | null;
  createVercelJSONTransport: (
    fetcher?: Fetch,
  ) => import("../providers").VercelJSONTransport;
  domainProvider(auth: MailProviderAuth, fetcher?: Fetch): MailDomainProvider;
  fetch: Fetch;
  assertMailAccess: import("@spitzli/mail-contracts/provider-ports").MailProductStore["assertMailAccess"];
  createMailKeyInTransaction(
    db: import("@spitzli/core/identity").DatabaseReader,
    actor: import("@spitzli/mail-contracts").MailActor,
    input: {
      name: string;
      permission: import("@spitzli/mail-contracts").Permission;
      domainID?: string;
    },
  ): Promise<{ id: string; key: string; prefix: string }>;
  nativeClusterDemand(
    db: import("@spitzli/core/identity").DatabaseReader,
    clusterID: string,
  ): Promise<{
    [K in keyof import("@spitzli/hosting-contracts").HostingAllowances]: number;
  }>;
  clusterCapacityDemand(
    db: import("@spitzli/core/identity").DatabaseReader,
    clusterID: string,
    kind: "apps" | "native" | "all",
    excludedCustomer?: string,
  ): Promise<{
    [K in keyof import("@spitzli/hosting-contracts").HostingAllowances]: number;
  }>;
  databasePoolCapacity(
    db: import("@spitzli/core/identity").DatabaseReader,
    clusterID: string,
  ): Promise<
    {
      node: string;
      pool_id: string;
      capacity_bytes: string;
      held_cpu: number;
      held_memory: number;
      held_volume: number;
      known_cpu: number;
      known_memory: number;
      allocated: number;
    }[]
  >;
  fetchVercelProject(input: {
    token: string;
    teamID: string;
    projectID: string;
    fetcher?: Fetch;
  }): Promise<
    import("@spitzli/hosting-provider-contracts").VercelProjectSnapshot
  >;
  listVercelProjects(input: {
    token: string;
    teamID: string;
    cursor?: string;
    fetcher?: Fetch;
  }): Promise<{
    projects: { id: string; name: string }[];
    nextCursor: string | null;
  }>;
}
