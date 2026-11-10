import type { Pool } from "pg";
import type { DatabaseReader, SessionReference } from "@spitzli/core/identity";
import type { ClusterObservation } from "@spitzli/hosting-contracts";
import type { MailCredentialMaster, MailSecrets } from "./provider-ports";
export type Connection = DatabaseReader;
export type Agent = {
  clusterID: string;
  generation: number;
  credentialHash: string;
};
export interface NativeCredentialIssuer {
  createConsumerKey(
    token: string,
    label: string,
    options: { permissions: ("SEND_API" | "SEND_SMTP")[]; ips: string[] },
  ): Promise<{ consumerKey: string; consumerSecret: string }>;
}
/** Composition supplies live authenticated identity, transaction ownership and provider adapters. */
export interface NativeMailPorts {
  clerkMailAccess(db: Connection, subject: string, customerID: string): Promise<{ operator: boolean; canManage: boolean; clerkGrant: string } | null | undefined>;
  clerkMailPeople(db: Connection, customerID: string, operator?: boolean): Promise<Array<{ id: string; name: string; email: string }> | undefined>;
  env: Record<string, string | undefined>;
  database: Pool;
  transaction<T>(run: (db: Connection) => Promise<T>): Promise<T>;
  withAuthenticatedDatabase<T>(
    headers: Headers,
    run: () => Promise<T>,
  ): Promise<T>;
  resolveSessionReference(headers: Headers): Promise<SessionReference | null>;
  requireMail(intent?: "access" | "provision"): void;
  getTenant(
    headers: Headers,
    customerID: string,
  ): Promise<{ operator: boolean; tenant: { status: string } }>;
  requireAccessOperator(headers: Headers): Promise<{ user: { id: string } }>;
  getPlatformSettings(): Promise<{
    mailEnabled: boolean;
    mailRegion: "eu" | "global";
  }>;
  verifyAgent(
    db: Connection,
    agent: Agent,
  ): Promise<{ observation?: ClusterObservation }>;
  readHostingJSON(request: Request, maximum?: number): Promise<unknown>;
  requireHostingEnvironment(): void;
  byokPolicy(
    db: Connection,
    customerID: string,
  ): Promise<{ turbosmtp: boolean }>;
  decryptMailSecret: MailSecrets["decryptMailSecret"];
  credentialMaster(customerID: string): Promise<MailCredentialMaster>;
  credentialIssuer(token: string): NativeCredentialIssuer;
}
