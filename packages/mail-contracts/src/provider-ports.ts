import type { DatabaseReader } from "@spitzli/core/identity";
import type { MailActor, MailCapability } from "./index";

/** Existing provider identities are opaque durable bindings, never email-derived IDs. */
export interface MailProviderAccount {
  id: string;
  email?: string;
  active: boolean;
  limit?: number;
  sent?: number;
  interval?: "Daily" | "Monthly" | "Yearly";
}
export interface MailAccountProvider {
  createSubaccount(input: {
    email: string;
    first_name: string;
    last_name: string;
    ip: string;
    policy_agree: true;
    password: string;
    confirm_password: string;
  }): Promise<MailProviderAccount>;
  getSubaccount(id: string): Promise<MailProviderAccount>;
  getSubaccountPlan(id: string): Promise<MailProviderAccount>;
  listSubaccounts(options: {
    filterByEmail: string;
    limit: number;
  }): Promise<{ results: MailProviderAccount[] }>;
  setSubaccountActive(
    id: string,
    active: boolean,
  ): Promise<MailProviderAccount>;
  setSubaccountLimit(id: string, limit: number): Promise<MailProviderAccount>;
}
export interface MailCredentialMaster {
  getSubaccount(id: string): Promise<MailProviderAccount>;
  authorizeSubaccount(email: string): Promise<string>;
}
export interface MailCredentialIssuer {
  createConsumerKey(
    token: string,
    label: string,
    options: { permissions: "SEND_API"[]; ips: string[] },
  ): Promise<{ consumerKey: string; consumerSecret: string }>;
  deleteConsumerKey?(key: string): Promise<void>;
}
export interface MailSecrets {
  encryptMailSecret(value: unknown, context: string): string;
  decryptMailSecret<T>(encrypted: string, context: string): T;
}
export interface MailProductStore {
  database: DatabaseReader;
  withMailTransaction<T>(run: (db: DatabaseReader) => Promise<T>): Promise<T>;
  assertMailAccess(
    actor: MailActor,
    capability: MailCapability,
    db?: DatabaseReader,
    admission?: boolean,
  ): Promise<void>;
}

export type MailProviderAuth =
  { consumerKey: string; consumerSecret: string } | { apiKey: string };
export type MailSendingCredential = {
  consumerKey: string;
  consumerSecret: string;
  region: "eu" | "global";
};
export type SenderDomain = {
  id: string;
  domain: string;
  spf_verified: boolean;
  dkim_verified: boolean;
  dmarc_verified: boolean;
};
export interface MailDomainProvider {
  listSenderDomains(): Promise<SenderDomain[]>;
  registerSenderDomain(domain: string): Promise<SenderDomain>;
  deleteSenderDomain(id: string): Promise<void>;
}
export type MailSend = (
  credential: MailSendingCredential,
  body: import("@spitzli/mail-contracts").EmailRequest,
  reference: string,
  signal: AbortSignal,
  fetcher?: MailFetch,
) => Promise<import("@spitzli/mail-contracts").DispatchOutcome>;
export type MailAccountVerification = (
  pair: MailSendingCredential,
  fetcher?: MailFetch,
) => Promise<{ accountID: string }>;

export type MailFetch = (
  input: RequestInfo | URL,
  init?: RequestInit,
) => Promise<Response>;

export type MailConsumerPermission = "SEND_SMTP" | "SEND_API" | "APIS";
export type MailConsumerKey = {
  consumerKey: string;
  label: string;
  creation_time: string;
  ips: string[];
  is_legacy: boolean;
  permissions: string[];
};
export interface MailConsumerKeyProvider {
  listConsumerKeys(): Promise<{ count: number; results: MailConsumerKey[] }>;
  createConsumerKey(
    token: string,
    label: string,
    options: { permissions: MailConsumerPermission[]; ips?: string[] },
  ): Promise<{ consumerKey: string; consumerSecret: string }>;
  deleteConsumerKey(key: string): Promise<void>;
}
