import { signedDatabaseScope } from "./signed-scope";
import { createDatabases, createWorkerDatabases } from "./client";
import { currentDatabaseScope } from "@webdock/database-runtime";
const local = process.env.NODE_ENV !== "production";
const businessURL = process.env.DATABASE_URL || "";
export const databasePools =
  process.env.WEBDOCK_PROCESS_ROLE === "worker"
    ? createWorkerDatabases({
        environment: local ? "development" : "production",
        workerURL: process.env.DATABASE_WORKER_URL || "",
        signingSecret: process.env.DATABASE_RLS_SECRET,
        scope: signedDatabaseScope,
        ca: process.env.DATABASE_CA_CERT,
        max: Number(process.env.DATABASE_POOL_MAX || (local ? 4 : 0)),
      })
    : createDatabases({
        environment: local ? "development" : "production",
        businessURL,
        authURL: process.env.AUTH_DATABASE_URL || (local ? businessURL : ""),
        workerURL:
          process.env.DATABASE_WORKER_URL || (local ? businessURL : ""),
        signingSecret: process.env.DATABASE_RLS_SECRET,
        scope: signedDatabaseScope,
        ca: process.env.DATABASE_CA_CERT,
        max: Number(process.env.DATABASE_POOL_MAX || (local ? 4 : 0)),
      });
export const authDatabase = process.env.DATABASE_RLS_SECRET
  ? databasePools.authentication
  : databasePools.business;
export const workerDatabase = process.env.DATABASE_RLS_SECRET
  ? databasePools.worker
  : databasePools.business;
export const database = new Proxy(databasePools.business, {
  get(target, key, receiver) {
    if (key === "query" || key === "connect")
      return (...args: unknown[]) => {
        const pool =
          process.env.DATABASE_RLS_SECRET &&
          currentDatabaseScope()?.kind === "system"
            ? databasePools.worker
            : databasePools.business;
        return Reflect.apply(Reflect.get(pool, key), pool, args);
      };
    const value = Reflect.get(target, key, receiver);
    return typeof value === "function" ? value.bind(target) : value;
  },
});
export const closeDatabases = databasePools.close;
