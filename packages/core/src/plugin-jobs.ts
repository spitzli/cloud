import { createHash } from "node:crypto";
import type { RedisOptions } from "bullmq";
import { Queue, Worker, UnrecoverableError } from "bullmq";
import {
  enqueueJobSchema,
  jobIDSchema,
  PluginError,
  type Capability,
  type JobStatus,
  type EnqueueJob,
} from "@spitzli/plugin-sdk/contracts";
export type JobScope = {
  pluginID: string;
  customerID: string;
  projectID: string;
  runtimeGeneration: string;
  grantRevision: string;
};
export type JobGrant = { scope: JobScope; jobs: readonly string[] };
export type JobAuthority = {
  /** Authenticate the invocation, then recheck current membership, scope, capability and write intent. */
  authorize(request: Request, capability: Capability): Promise<JobGrant>;
  /** Recheck enabled installation, generation, current grant and allowed job before dispatch and while running. */
  canRun(scope: JobScope, name: string): Promise<boolean>;
};
type Envelope = {
  scope: JobScope;
  name: string;
  payload: unknown;
  fingerprint: string;
};
const stable = (v: unknown): string =>
  JSON.stringify(
    v && typeof v === "object"
      ? Array.isArray(v)
        ? v.map((x) => JSON.parse(stable(x)))
        : Object.fromEntries(
            Object.entries(v)
              .sort(([a], [b]) => a.localeCompare(b))
              .map(([k, x]) => [k, JSON.parse(stable(x))]),
          )
      : v,
  );
const hash = (v: unknown) =>
  createHash("sha256").update(stable(v)).digest("hex");
const partition = (s: JobScope) => [s.pluginID, s.customerID, s.projectID];
export const jobIdentity = (scope: JobScope, key: string) =>
  "job-" + hash([partition(scope), key]);
const denied = () =>
  new PluginError("permission_denied", "Plugin job access denied");
export function createPluginJobs(options: {
  connection: RedisOptions;
  prefix: string;
  authority: JobAuthority;
  execute: (
    scope: JobScope,
    name: string,
    payload: unknown,
    context: { jobID: string; signal: AbortSignal },
  ) => Promise<unknown>;
  concurrency?: number;
  onError?: (error: Error) => void;
}) {
  if (!/^[a-z][a-z0-9-]{0,63}$/.test(options.prefix))
    throw new Error("Explicit safe Redis prefix required");
  const queue = new Queue<Envelope>("plugin-jobs", {
    connection: {
      ...options.connection,
      maxRetriesPerRequest: 1,
      enableOfflineQueue: false,
      connectTimeout: 3000,
      commandTimeout: 5000,
    },
    prefix: options.prefix,
  });
  async function ready() {
    let timer: ReturnType<typeof setTimeout>;
    try {
      await Promise.race([
        queue.waitUntilReady(),
        new Promise((_, reject) => {
          timer = setTimeout(
            () =>
              reject(
                new PluginError("unavailable", "Job service unavailable", true),
              ),
            3000,
          );
        }),
      ]);
    } finally {
      clearTimeout(timer!);
    }
  }
  queue.on("error", options.onError ?? (() => {}));
  const cancelledKey = (id: string) =>
    `${options.prefix}:plugin-job-cancel:${id}`;
  async function cancelled(id: string) {
    return !!(await (await queue.backend.client).get(cancelledKey(id)));
  }
  async function owned(grant: JobGrant, id: string) {
    if (!jobIDSchema.safeParse(id).success)
      throw new PluginError("invalid_input", "Invalid job ID");
    await ready();
    const job = await queue.getJob(id);
    if (
      !job ||
      stable(partition(job.data.scope)) !== stable(partition(grant.scope)) ||
      !grant.jobs.includes(job.data.name)
    )
      throw new PluginError("not_found", "Job not found");
    return job;
  }
  async function enqueue(request: Request, raw: EnqueueJob) {
    const parsed = enqueueJobSchema.safeParse(raw);
    if (!parsed.success)
      throw new PluginError("invalid_input", "Invalid job request");
    const input = parsed.data;
    const grant = await options.authority.authorize(
      request,
      "plugin.jobs.enqueue",
    );
    if (
      !grant.jobs.includes(input.name) ||
      !(await options.authority.canRun(grant.scope, input.name))
    )
      throw denied();
    await ready();
    const id = jobIdentity(grant.scope, input.idempotencyKey);
    const fingerprint = hash(input);
    await queue.add(
      input.name,
      {
        scope: grant.scope,
        name: input.name,
        payload: input.payload,
        fingerprint,
      },
      {
        jobId: id,
        delay: input.runAt
          ? Math.max(0, Date.parse(input.runAt) - Date.now())
          : input.delaySeconds * 1000,
        attempts: input.attempts,
        backoff: { type: "exponential", delay: 1000 },
        removeOnComplete: false,
        removeOnFail: false,
      },
    );
    // Read the persisted winner: add() can return the losing caller's in-memory data on a duplicate.
    const stored = await queue.getJob(id);
    if (!stored || stored.data.fingerprint !== fingerprint)
      throw new PluginError(
        "conflict",
        "Idempotency key already used with different input",
      );
    return { id };
  }
  async function get(request: Request, id: string): Promise<JobStatus> {
    const grant = await options.authority.authorize(
      request,
      "plugin.jobs.read",
    );
    const job = await owned(grant, id);
    const state = await job.getState();
    const done = state === "completed" || state === "failed";
    const stopped = await cancelled(id);
    const publicState: JobStatus["state"] =
      state === "completed"
        ? "completed"
        : stopped
          ? "cancelled"
          : state === "failed"
            ? "failed"
            : state === "active"
              ? "active"
              : state === "delayed"
                ? "delayed"
                : "waiting";
    return {
      id,
      name: job.data.name,
      state: publicState,
      attempts: job.attemptsMade,
      ...(done && state === "completed" ? { result: job.returnvalue } : {}),
    };
  }
  async function cancel(request: Request, id: string) {
    const grant = await options.authority.authorize(
      request,
      "plugin.jobs.cancel",
    );
    const job = await owned(grant, id);
    if (["completed", "failed"].includes(await job.getState()))
      throw new PluginError("conflict", "Job already finished");
    // Retain both record and tombstone: deleting a job would allow an idempotent retry to execute again.
    await (await queue.backend.client).set(cancelledKey(id), "1");
    return { id, cancelRequested: true as const };
  }
  async function schedule(
    scope: JobScope,
    name: string,
    intervalSeconds: number,
  ) {
    if (
      !Number.isSafeInteger(intervalSeconds) ||
      intervalSeconds < 60 ||
      intervalSeconds > 604800 ||
      !(await options.authority.canRun(scope, name))
    )
      throw new PluginError(
        "permission_denied",
        "Invalid or unauthorized schedule",
      );
    await ready();
    return queue.upsertJobScheduler(
      "schedule-" + hash([partition(scope), name]),
      { every: intervalSeconds * 1000 },
      {
        name,
        data: {
          scope,
          name,
          payload: {},
          fingerprint: hash([scope, name, intervalSeconds]),
        },
        opts: {
          attempts: 3,
          backoff: { type: "exponential", delay: 1000 },
          // Count-based cleanup also evicts other jobs in this queue, breaking their idempotency.
          removeOnComplete: false,
          removeOnFail: false,
        },
      },
    );
  }
  function startWorker() {
    const worker = new Worker<Envelope>(
      "plugin-jobs",
      async (job) => {
        const { scope, name, payload } = job.data;
        if (
          (await cancelled(job.id!)) ||
          !(await options.authority.canRun(scope, name))
        )
          throw new UnrecoverableError(
            "Job cancelled or authorization revoked",
          );
        const controller = new AbortController();
        let checking = false;
        const check = setInterval(() => {
          if (checking) return;
          checking = true;
          void Promise.all([
            cancelled(job.id!),
            options.authority.canRun(scope, name),
          ])
            .then(
              ([stop, allowed]) => {
                if (stop || !allowed) controller.abort();
              },
              () => controller.abort(),
            )
            .finally(() => {
              checking = false;
            });
        }, 250);
        const deadline = setTimeout(() => controller.abort(), 60_000);
        try {
          const result = await Promise.race([
            options.execute(scope, name, payload, {
              jobID: job.id!,
              signal: controller.signal,
            }),
            new Promise<never>((_, reject) =>
              controller.signal.addEventListener(
                "abort",
                () => reject(new UnrecoverableError("Job aborted")),
                { once: true },
              ),
            ),
          ]);
          if (
            controller.signal.aborted ||
            (await cancelled(job.id!)) ||
            !(await options.authority.canRun(scope, name))
          )
            throw new UnrecoverableError(
              "Job cancelled or authorization revoked",
            );
          const serialized = JSON.stringify(result ?? null);
          if (Buffer.byteLength(serialized) > 32768)
            throw new UnrecoverableError("Job result exceeds 32 KiB");
          return JSON.parse(serialized);
        } catch (error) {
          if (controller.signal.aborted)
            throw new UnrecoverableError(
              "Job cancelled or authorization revoked",
            );
          if (error instanceof UnrecoverableError) throw error;
          // Never put plugin exception messages (which may contain secrets) into public status.
          throw new Error("Plugin job failed");
        } finally {
          clearInterval(check);
          clearTimeout(deadline);
        }
      },
      {
        connection: {
          ...options.connection,
          maxRetriesPerRequest: null,
          enableOfflineQueue: true,
        },
        prefix: options.prefix,
        concurrency: options.concurrency ?? 4,
      },
    );
    worker.on("error", options.onError ?? (() => {}));
    return worker;
  }
  async function handle(request: Request): Promise<Response> {
    try {
      const path = new URL(request.url).pathname;
      if (path === "/v1/plugin/jobs" && request.method === "POST") {
        // Stream-cap the body; Content-Length is not authoritative.
        const reader = request.body?.getReader();
        let bytes = 0;
        const chunks: Uint8Array[] = [];
        if (reader)
          try {
            for (;;) {
              const { done, value } = await reader.read();
              if (done) break;
              bytes += value.length;
              if (bytes > 36 * 1024)
                throw new PluginError("invalid_input", "Request too large");
              chunks.push(value);
            }
          } finally {
            await reader.cancel();
          }
        let body;
        try {
          body = JSON.parse(Buffer.concat(chunks).toString("utf8"));
        } catch {
          throw new PluginError("invalid_input", "Invalid JSON");
        }
        return Response.json(await enqueue(request, body), { status: 202 });
      }
      const match = /^\/v1\/plugin\/jobs\/(job-[a-f0-9]{64})$/.exec(path);
      if (match && request.method === "GET")
        return Response.json(await get(request, match[1]));
      if (match && request.method === "DELETE")
        return Response.json(await cancel(request, match[1]));
      return Response.json(
        { error: { code: "not_found", message: "Route not found" } },
        { status: 404 },
      );
    } catch (error) {
      const safe =
        error instanceof PluginError
          ? error
          : new PluginError("unavailable", "Job service unavailable", true);
      const status = {
        invalid_input: 400,
        permission_denied: 403,
        not_found: 404,
        conflict: 409,
        unavailable: 503,
        capability_unavailable: 503,
        not_supported: 400,
        deadline_exceeded: 504,
      }[safe.code];
      return Response.json({ error: safe.toJSON() }, { status });
    }
  }
  return {
    enqueue,
    get,
    cancel,
    handle,
    startWorker,
    schedule,
    close: () => queue.close(),
  };
}

export { readJobQueueConfiguration } from "@spitzli/core/jobs";
