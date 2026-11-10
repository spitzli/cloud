import type { Pool } from "pg";
import type { IdentityDirectory, SessionReference } from "@spitzli/core/identity";
export type StorageInventoryOptions = {
  token: string;
  prefix: string;
  cursor?: string;
  limit: number;
  mode: "expanded";
  abortSignal: AbortSignal;
};
export type StorageInventoryPage = {
  blobs: { url: string; pathname: string; size: number }[];
  hasMore: boolean;
  cursor?: string;
};
export type StorageDependencies = {
  list?: (options: StorageInventoryOptions) => Promise<StorageInventoryPage>;
};
export type StoragePorts = {
  identity: IdentityDirectory;
  database: Pick<Pool, "query" | "connect">;
  withAuthenticatedDatabase<T>(
    headers: Headers,
    run: () => Promise<T>,
  ): Promise<T>;
  getTenant(
    headers: Headers,
    customerID: string,
  ): Promise<{ operator: boolean; tenant: { status: string } }>;
  resolveSessionReference(headers: Headers): Promise<SessionReference | null>;
  requireStorage(intent?: "access" | "provision"): void;
  encryptSecret(value: string, context: string): string;
  decryptSecret(value: string, context: string): string;
  validateStoreToken(storeID: string, token: string): string;
  measureStorage(
    storeID: string,
    token: string,
    prefix: string,
    dependencies?: StorageDependencies,
  ): Promise<{ bytes: number; objects: number }>;
};
