/** Temporary offline setup, bound to IPv4 loopback only. No application route imports this module. */
import { randomBytes, timingSafeEqual } from "node:crypto";
import {
  createServer,
  type IncomingMessage,
  type ServerResponse,
} from "node:http";
import {
  operatorConfig,
  operatorInput,
  OperatorSetupError,
  type InitialOperatorInput,
} from "./operator-config";
import {
  runOfflineOperator,
  OperatorRunError,
  type OperatorSummary,
} from "./operator-offline";
import { operatorSetupPage } from "./operator-setup-page";

export type SetupOutcome =
  | { status: "success"; summary: OperatorSummary }
  | { status: "expired" | "closed" | "uncertain" };
type SetupOptions = {
  env: Record<string, string | undefined>;
  target: string;
  destination: string;
  ttlMs?: number;
  provision?: (input: InitialOperatorInput) => Promise<OperatorSummary>;
  diagnostic?: (message: string) => void;
};
class RequestError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
  }
}
function headerOnce(req: IncomingMessage, name: string) {
  let count = 0;
  for (let index = 0; index < req.rawHeaders.length; index += 2)
    if (req.rawHeaders[index].toLowerCase() === name) count++;
  return count === 1 ? req.headers[name] : undefined;
}
function readBody(req: IncomingMessage): Promise<unknown> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    let size = 0;
    const timer = setTimeout(
      () => finish(new RequestError(408, "Setup request timed out")),
      5000,
    );
    function finish(error?: Error) {
      clearTimeout(timer);
      req.off("data", data);
      req.off("end", end);
      req.off("error", failed);
      if (error) {
        req.resume();
        reject(error);
        return;
      }
      try {
        resolve(JSON.parse(Buffer.concat(chunks).toString("utf8")));
      } catch {
        reject(new RequestError(400, "Enter valid setup details"));
      }
    }
    function data(chunk: Buffer) {
      size += chunk.length;
      if (size > 16384)
        finish(new RequestError(413, "Setup request is too large"));
      else chunks.push(chunk);
    }
    function end() {
      finish();
    }
    function failed() {
      finish(new RequestError(400, "Setup request was interrupted"));
    }
    req.on("data", data);
    req.on("end", end);
    req.on("error", failed);
  });
}

export async function startOperatorSetup(options: SetupOptions) {
  const env = Object.freeze({ ...options.env });
  const config = operatorConfig(env, options.target);
  if (!options.destination)
    throw new OperatorSetupError(
      "An explicit new private output destination is required",
    );
  if (env.WEBDOCK_INITIAL_OPERATOR_PASSWORD !== undefined)
    throw new OperatorSetupError(
      "Unset WEBDOCK_INITIAL_OPERATOR_PASSWORD before using browser setup",
    );
  // Validate immutable canonical origins before opening any listening socket.
  const configured = operatorInput(
    {
      name: "Initial operator",
      email: "setup@example.test",
      password: "origin-validation-only",
      authOrigin: env.BETTER_AUTH_URL,
      studioOrigin: env.WEBDOCK_STUDIO_ORIGIN,
      consoleOrigin: env.WEBDOCK_CONSOLE_ORIGIN,
    },
    env,
    config.local,
  );
  const origins = Object.freeze({
    authOrigin: configured.authOrigin,
    studioOrigin: configured.studioOrigin,
    consoleOrigin: configured.consoleOrigin,
  });
  const ttlMs = options.ttlMs ?? 600000;
  if (!Number.isSafeInteger(ttlMs) || ttlMs < 1 || ttlMs > 900000)
    throw new OperatorSetupError("Setup expiry must be at most 15 minutes");
  const capability = randomBytes(32).toString("base64url");
  const expectedAuthorization = Buffer.from("Bearer " + capability);
  const expiresAt = Date.now() + ttlMs;
  const provision =
    options.provision ??
    ((input: InitialOperatorInput) =>
      runOfflineOperator(config, input, options.destination));
  let url = "",
    busy = false,
    consumed = false,
    stopping = false,
    listenerClosed = false;
  let outcome: SetupOutcome = { status: "closed" };
  let resolveClosed!: (value: SetupOutcome) => void;
  const closed = new Promise<SetupOutcome>((resolve) => {
    resolveClosed = resolve;
  });
  let expiry: ReturnType<typeof setTimeout>;
  const server = createServer(
    { maxHeaderSize: 8192, requestTimeout: 10000, headersTimeout: 5000 },
    (req, res) => {
      void handle(req, res).catch(() => {
        // Never let request errors escape to the process or emit raw diagnostics.
        if (!res.headersSent)
          reply(res, 500, {
            error: "Initial operator setup failed; check the terminal",
          });
        else res.destroy();
      });
    },
  );
  server.maxConnections = 8;
  server.setTimeout(10000, (socket) => socket.destroy());
  function headers(res: ServerResponse, nonce?: string) {
    res.setHeader("Cache-Control", "no-store");
    res.setHeader("Referrer-Policy", "no-referrer");
    res.setHeader("X-Content-Type-Options", "nosniff");
    res.setHeader("X-Frame-Options", "DENY");
    res.setHeader("Connection", "close");
    res.setHeader(
      "Content-Security-Policy",
      `default-src 'none'; script-src ${nonce ? `'nonce-${nonce}'` : "'none'"}; style-src ${nonce ? `'nonce-${nonce}'` : "'none'"}; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'`,
    );
  }
  function reply(res: ServerResponse, status: number, body: object) {
    headers(res);
    res.writeHead(status, {
      "Content-Type": "application/json; charset=utf-8",
    });
    res.end(JSON.stringify(body));
  }
  async function close() {
    if (!stopping) {
      stopping = true;
      clearTimeout(expiry);
      server.close(() => {
        listenerClosed = true;
        if (!busy) resolveClosed(outcome);
      });
      server.closeIdleConnections();
    }
    return closed;
  }
  async function handle(req: IncomingMessage, res: ServerResponse) {
    if (
      req.socket.remoteAddress !== "127.0.0.1" ||
      headerOnce(req, "host") !== new URL(url).host ||
      (req.headers.origin !== undefined && headerOnce(req, "origin") !== url)
    ) {
      reply(res, 403, { error: "Setup access denied" });
      return;
    }
    if (Date.now() >= expiresAt || stopping) {
      reply(res, 410, { error: "Setup session has expired" });
      return;
    }
    if (consumed) {
      reply(res, 410, { error: "Setup session has already been used" });
      return;
    }
    if (req.method === "GET" && req.url === "/") {
      const nonce = randomBytes(18).toString("base64url");
      headers(res, nonce);
      res.writeHead(200, { "Content-Type": "text/html; charset=utf-8" });
      res.end(operatorSetupPage(nonce, origins));
      return;
    }
    if (req.method !== "POST" || req.url !== "/setup") {
      reply(res, 404, { error: "Not found" });
      return;
    }
    const authorization = headerOnce(req, "authorization");
    const candidate = Buffer.from(
      typeof authorization === "string" ? authorization : "",
    );
    if (
      headerOnce(req, "origin") !== url ||
      (req.headers["sec-fetch-site"] !== undefined &&
        req.headers["sec-fetch-site"] !== "same-origin") ||
      candidate.length !== expectedAuthorization.length ||
      !timingSafeEqual(candidate, expectedAuthorization)
    ) {
      reply(res, 403, { error: "Setup access denied" });
      return;
    }
    if (
      headerOnce(req, "content-type") !== "application/json" ||
      req.headers["content-encoding"] !== undefined
    ) {
      reply(res, 415, {
        error: "Setup requires application/json without content encoding",
      });
      return;
    }
    if (Number(req.headers["content-length"]) > 16384) {
      reply(res, 413, { error: "Setup request is too large" });
      return;
    }
    if (busy) {
      reply(res, 409, { error: "Setup is already running" });
      return;
    }
    busy = true;
    try {
      const value = await readBody(req);
      if (Date.now() >= expiresAt || stopping)
        throw new RequestError(410, "Setup session has expired");
      if (!value || typeof value !== "object" || Array.isArray(value))
        throw new RequestError(400, "Enter valid setup details");
      const data = value as Record<string, unknown>;
      if (
        Object.keys(data).some(
          (key) =>
            !["email", "name", "password", "ownershipConfirmed"].includes(key),
        )
      )
        throw new RequestError(
          400,
          "Only operator account details may be submitted",
        );
      if (data.ownershipConfirmed !== true)
        throw new RequestError(
          400,
          "Confirm that you have verified ownership of this email address",
        );
      const input = operatorInput(
        {
          email: data.email,
          name: data.name,
          password: data.password,
          ...origins,
        },
        env,
        config.local,
      );
      const summary = await provision(input);
      consumed = true;
      outcome = { status: "success", summary };
      reply(res, 200, { created: true, securitySetupRequired: true });
    } catch (error) {
      if (error instanceof RequestError) {
        // An unread body leaves the connection out of sync; never reuse it for the next request.
        if (error.status === 408 || error.status === 413) res.setHeader("Connection", "close");
        reply(res, error.status, { error: error.message });
        if (error.status === 408 || error.status === 413) res.on("finish", () => req.socket.destroy());
      }
      else if (error instanceof OperatorSetupError) {
        options.diagnostic?.(error.message);
        if (error instanceof OperatorRunError && error.commitUncertain) {
          consumed = true;
          outcome = { status: "uncertain" };
        }
        reply(res, error instanceof OperatorRunError ? 500 : 400, {
          error: error.message,
        });
      } else {
        options.diagnostic?.(
          "Initial operator setup failed; no changes committed",
        );
        reply(res, 500, {
          error: "Initial operator setup failed; no changes committed",
        });
      }
    } finally {
      busy = false;
      // close() stops accepting new requests while allowing this response to finish.
      // Do not rely on response close/finish events: the browser may already be gone.
      if (consumed) void close();
      if (listenerClosed) resolveClosed(outcome);
    }
  }
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      server.off("error", reject);
      resolve();
    });
  });
  url = `http://127.0.0.1:${(server.address() as { port: number }).port}`;
  expiry = setTimeout(() => {
    if (!consumed) outcome = { status: "expired" };
    void close();
  }, ttlMs);
  return { url, capability, expiresAt, closed, close };
}
