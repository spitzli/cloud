import { Queue, Worker, type RedisOptions } from "bullmq";
/** Queue owners are registered by trusted composition, never supplied by requests. */
export type JobDomain = string;
export type CoreQueueConfiguration = {
  connection: RedisOptions;
  prefix: string;
};
export function readJobQueueConfiguration(
  env: Record<string, string | undefined> = process.env,
): CoreQueueConfiguration {
  const value = env.SPITZLI_JOBS_REDIS_URL;
  if (!value)
    throw Error("SPITZLI_JOBS_REDIS_URL is required for Core BullMQ workers");
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw Error("Invalid SPITZLI_JOBS_REDIS_URL");
  }
  const prefix = env.SPITZLI_JOBS_PREFIX ?? "webdock";
  if (
    !["redis:", "rediss:"].includes(url.protocol) ||
    url.search ||
    url.hash ||
    !/^\/(?:[0-9]+)?$/.test(url.pathname) ||
    !url.hostname ||
    !/^\w[\w-]{0,63}$/.test(prefix)
  )
    throw Error("Invalid Core Redis URL or prefix");
  return {
    prefix,
    connection: {
      host: url.hostname.replace(/^\[|\]$/g, ""),
      port: Number(url.port || 6379),
      db: Number(url.pathname.slice(1) || 0),
      ...(url.username ? { username: decodeURIComponent(url.username) } : {}),
      ...(url.password ? { password: decodeURIComponent(url.password) } : {}),
      ...(url.protocol === "rediss:" ? { tls: {} } : {}),
      connectTimeout: 5000,
    },
  };
}
/** One durable BullMQ queue per owning domain. Database leases remain the business-operation fence. */
export async function startCoreJobWorkers(
  options: CoreQueueConfiguration & {
    handlers: Partial<Record<JobDomain, () => Promise<void>>>;
    onError: (event: {
      domain: string;
      event: "queue_error" | "tick_failed";
    }) => void;
    intervalMs?: number;
  },
) {
  for (const [domain, handler] of Object.entries(options.handlers)) {
    if (!/^[a-z][a-z0-9-]{0,63}$/.test(domain) || typeof handler !== "function")
      throw Error("Invalid Core worker ownership");
  }
  const queues: Queue[] = [];
  const workers: Worker[] = [];
  let closed = false;
  async function close() {
    if (closed) return;
    closed = true;
    await Promise.allSettled(workers.map((w) => w.close()));
    await Promise.allSettled(queues.map((q) => q.close()));
  }
  try {
    for (const [domain, handler] of Object.entries(options.handlers)) {
      if (!handler) throw Error("Invalid Core worker ownership");
      const name = "core-" + domain;
      const queue = new Queue(name, {
        connection: { ...options.connection, maxRetriesPerRequest: 1 },
        prefix: options.prefix,
      });
      queues.push(queue);
      queue.on("error", () =>
        options.onError({ domain, event: "queue_error" }),
      );
      const worker = new Worker(name, async () => handler(), {
        connection: { ...options.connection, maxRetriesPerRequest: null },
        prefix: options.prefix,
        concurrency: 1,
      });
      workers.push(worker);
      worker.on("error", () =>
        options.onError({ domain, event: "queue_error" }),
      );
      worker.on("failed", () =>
        options.onError({ domain, event: "tick_failed" }),
      );
      let timer: ReturnType<typeof setTimeout>;
      try {
        await Promise.race([
          Promise.all([queue.waitUntilReady(), worker.waitUntilReady()]),
          new Promise((_, reject) => {
            timer = setTimeout(
              () => reject(Error("Core Redis readiness timeout")),
              10000,
            );
          }),
        ]);
      } finally {
        clearTimeout(timer!);
      }
      await queue.upsertJobScheduler(
        "dispatch",
        { every: options.intervalMs ?? 5000 },
        {
          name: "dispatch",
          data: {},
          opts: {
            attempts: 3,
            backoff: { type: "exponential", delay: 1000 },
            removeOnComplete: { count: 20 },
            removeOnFail: { count: 100 },
          },
        },
      );
    }
    return { close };
  } catch (error) {
    await Promise.allSettled(workers.map((w) => w.close(true)));
    await Promise.allSettled(queues.map((q) => q.backend.close(true)));
    throw error;
  }
}
