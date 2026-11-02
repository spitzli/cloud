import { configureFullInstallation } from "./full-installation";
configureFullInstallation();
import { validateClerkWorkerConfiguration } from "./modules/auth/clerk-configuration";
import { validateWorkerModules } from "./installation";
import { withDatabaseScope } from "@webdock/database-runtime";
process.env.WEBDOCK_PROCESS_ROLE = "worker";
import {
  readJobQueueConfiguration,
  startCoreJobWorkers,
  type JobDomain,
} from "@webdock/core/jobs";
const jobQueueConfiguration = readJobQueueConfiguration();

import { claim, complete, fail, renew, schedule, type JobName } from "./jobs";
const selected = (process.env.WEBDOCK_WORKER_DOMAINS || "")
  .split(",")
  .filter(Boolean);
if (
  selected.some(
    (name) => !["mail", "storage", "hosting"].includes(name),
  ) ||
  !selected.length
)
  throw Error("Explicit worker domain ownership required");
validateWorkerModules(selected);
validateClerkWorkerConfiguration(selected);
const { databasePools, workerDatabase } = await import("./db/runtime");
await (
  await import("./modules/plugins/readiness")
).verifyPluginStartup(workerDatabase);
await (await import("./full-module-preflight")).verifyModuleShutdown();
if (process.env.NODE_ENV === "production" || process.env.DATABASE_RLS_SECRET)
  await (
    await import("./db/readiness")
  ).verifyWorkerReadiness(workerDatabase, selected);
const handlers = new Map<JobName, () => Promise<void>>();
if (selected.includes("mail")) {
  const { runMailJob, mailJobs } = await import("./modules/mail/product/jobs");
  for (const name of mailJobs)
    handlers.set(`mail.${name}`, () =>
      withDatabaseScope({ kind: "system", purpose: "mail-cron" }, async () => {
        const result = await runMailJob(name, new Date(Date.now() + 45_000));
        if (result.failed) throw Error("Mail job failed");
      }),
    );
}
if (selected.includes("hosting")) {
  const { configureOperatorAppTemplates } = await import("./modules/hosting/approved-templates");
  configureOperatorAppTemplates();
  const { runSetupBatch } = await import("./modules/hosting/setup-worker");
  handlers.set("hosting.setup", async () => {
    await runSetupBatch();
  });
}
if (selected.includes("storage")) {
  const { refreshStorageBatch } =
    await import("./modules/platform/storage-usage");
  handlers.set("storage.refresh", () =>
    withDatabaseScope({ kind: "system", purpose: "storage-cron" }, async () => {
      const result = await refreshStorageBatch();
      if (result.failed) throw Error("Storage measurement incomplete");
    }),
  );
}
const scheduler = <T>(run: () => T) =>
  withDatabaseScope({ kind: "system", purpose: "api-scheduler" }, run);
let stopping = false;
let stop!: () => void;
const stopped = new Promise<void>((resolve) => {
  stop = resolve;
});
for (const signal of ["SIGTERM", "SIGINT"] as const)
  process.once(signal, () => {
    stopping = true;
    stop();
  });
async function runDomain(domain: JobDomain) {
  if (stopping) return;
  const owned = [...handlers.keys()].filter((name) =>
    name.startsWith(domain + "."),
  );
  const now = Date.now();
  for (const name of owned) {
    const interval =
      name === "hosting.setup"
        ? 10_000
        : name === "storage.refresh" || name === "mail.maintenance"
          ? 3_600_000
          : 60_000;
    const offset = name === "mail.maintenance" ? 17 * 60_000 : 0;
    await scheduler(() =>
      schedule(
        workerDatabase,
        name,
        new Date(Math.floor((now - offset) / interval) * interval + offset),
      ),
    );
  }
  for (let n = 0; n < 25 && !stopping; n++) {
    const job = await scheduler(() => claim(workerDatabase, owned));
    if (!job) break;
    let leaseLost = false;
    const heartbeat = setInterval(() => {
      void scheduler(() => renew(workerDatabase, job)).then(
        (ok) => {
          if (!ok) leaseLost = true;
        },
        () => {
          leaseLost = true;
        },
      );
    }, 20_000);
    try {
      await handlers.get(job.name)!();
      if (!leaseLost) await scheduler(() => complete(workerDatabase, job));
    } catch {
      if (!leaseLost) await scheduler(() => fail(workerDatabase, job));
      console.error(
        JSON.stringify({ event: "job_failed", id: job.id, name: job.name }),
      );
    } finally {
      clearInterval(heartbeat);
    }
  }
}
let queues: Awaited<ReturnType<typeof startCoreJobWorkers>> | undefined;
try {
  queues = await startCoreJobWorkers({
    ...jobQueueConfiguration,
    handlers: Object.fromEntries(
      selected.map((domain) => [domain, () => runDomain(domain as JobDomain)]),
    ),
    onError: (event) => console.error(JSON.stringify(event)),
  });
  await stopped;
} finally {
  stopping = true;
  await queues?.close();
  await databasePools.close();
}
