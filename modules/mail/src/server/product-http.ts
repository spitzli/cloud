import type { MailProductStore } from "./provider-ports";
import { MailError } from "./product-access";
import { randomUUID } from "node:crypto";
import type { MailActor, ApiResult, Permission } from "@spitzli/mail-contracts";
import { MailValidationError } from "@spitzli/mail-contracts/validation";
import { redactMailLog } from "./product-request-logs";
type Dependencies = {
  withMailDatabase: <T>(
    actor: import("@spitzli/mail-contracts").MailActor,
    run: () => Promise<T>,
  ) => Promise<T>;
  authorizeMailKey: ReturnType<
    typeof import("./product-api-keys").createMailApiKeys
  >["authorizeMailKey"];
  consumeMailRate: ReturnType<
    typeof import("./product-rate-limit").createMailRateLimit
  >["consumeMailRate"];
  database: MailProductStore["database"];
};
export type MailRouteContext = { params?: Promise<Record<string, string>> };
export function createMailHttp(dependencies: Dependencies) {
  const { withMailDatabase, authorizeMailKey, consumeMailRate, database } =
    dependencies;
  async function readBoundedText(
    request: Request,
    maxBytes = 4_000_000,
  ): Promise<string> {
    if (!Number.isSafeInteger(maxBytes) || maxBytes < 1 || maxBytes > 4_000_000)
      throw Error("Invalid JSON body limit");
    if (
      !/^application\/json(?:\s*;\s*charset=utf-8)?$/i.test(
        request.headers.get("content-type") ?? "",
      )
    )
      throw new MailError(
        415,
        "unsupported_media_type",
        "Use application/json",
      );
    if (
      request.headers.has("content-encoding") &&
      request.headers.get("content-encoding") !== "identity"
    )
      throw new MailError(
        415,
        "unsupported_encoding",
        "Compressed request bodies are not supported",
      );
    if (Number(request.headers.get("content-length")) > maxBytes)
      throw new MailError(
        413,
        "payload_too_large",
        "Request body is too large",
      );
    const reader = request.body?.getReader();
    if (!reader)
      throw new MailError(422, "invalid_json", "JSON body is required");
    const chunks: Uint8Array[] = [];
    let size = 0;
    try {
      for (;;) {
        const part = await reader.read();
        if (part.done) break;
        size += part.value.byteLength;
        if (size > maxBytes)
          throw new MailError(
            413,
            "payload_too_large",
            "Request body is too large",
          );
        chunks.push(part.value);
      }
      try {
        return new TextDecoder("utf-8", { fatal: true }).decode(
          Buffer.concat(chunks),
        );
      } catch {
        throw new MailError(422, "invalid_json", "Invalid JSON body");
      }
    } finally {
      await reader.cancel().catch(() => {});
      reader.releaseLock();
    }
  }
  async function readBoundedJson(
    request: Request,
    maxBytes = 4_000_000,
  ): Promise<unknown> {
    const raw = await readBoundedText(request, maxBytes);
    try {
      return JSON.parse(raw);
    } catch {
      throw new MailError(422, "invalid_json", "Invalid JSON body");
    }
  }
  function mailEndpoint(
    permission: Permission,
    handler: (
      request: Request,
      actor: MailActor,
      context: { params: Record<string, string>; body: unknown },
    ) => Promise<ApiResult>,
    route: string,
  ) {
    return async (
      request: Request,
      context: MailRouteContext = {},
    ): Promise<Response> => {
      const started = Date.now();
      let requestID: string = randomUUID(),
        logID: string | undefined,
        body: unknown = null,
        result: ApiResult;
      try {
        const actor = await authorizeMailKey(request, permission);
        return await withMailDatabase(actor, async () => {
          try {
            await consumeMailRate(actor, {
              requests: 1,
              recipients: 0,
              keys: 0,
            });
            logID = (
              await database
                .query(
                  "INSERT INTO webdock_auth.mail_request_log(customer_id,connection_id,key_id,route,method,status,duration_ms,request_id) VALUES($1,$2,$3,$4,$5,0,0,$6) RETURNING id",
                  [
                    actor.customerID,
                    actor.connectionID,
                    actor.principal.kind === "api_key"
                      ? actor.principal.keyID
                      : null,
                    route,
                    request.method,
                    requestID,
                  ],
                )
                .catch(() => ({ rows: [] }))
            ).rows[0]?.id;
            const accept = request.headers.get("accept");
            if (
              accept &&
              !accept.includes("application/json") &&
              !accept.includes("*/*")
            )
              throw new MailError(
                406,
                "not_acceptable",
                "Request application/json",
              );
            if (["POST", "PATCH"].includes(request.method))
              body = request.body ? await readBoundedJson(request) : {};
            result = await handler(request, actor, {
              params: await (context.params ?? Promise.resolve({})),
              body,
            });
            if (
              request.headers.has("idempotency-key") &&
              ["/emails", "/emails/batch"].includes(route) &&
              request.method === "POST"
            ) {
              const saved = (
                await database.query(
                  "SELECT operation_token FROM webdock_auth.mail_idempotency WHERE customer_id=$1 AND connection_id=$2 AND key=$3",
                  [
                    actor.customerID,
                    actor.connectionID,
                    request.headers.get("idempotency-key"),
                  ],
                )
              ).rows[0];
              if (saved) requestID = saved.operation_token;
            }
          } catch (error) {
            const status =
              error instanceof MailError
                ? error.status
                : error instanceof MailValidationError
                  ? 422
                  : 503;
            result = {
              status,
              body: {
                statusCode: status,
                name:
                  error instanceof MailError
                    ? error.code
                    : error instanceof MailValidationError
                      ? "validation_error"
                      : "service_unavailable",
                message:
                  error instanceof MailError ||
                  error instanceof MailValidationError
                    ? error.message
                    : "Mail service is temporarily unavailable",
                request_id: requestID,
              },
            };
          }
          if (
            result.status >= 400 &&
            result.body &&
            typeof result.body === "object" &&
            !("request_id" in result.body)
          )
            result = {
              ...result,
              body: { ...result.body, request_id: requestID },
            };
          if (
            result.body &&
            typeof result.body === "object" &&
            "request_id" in result.body &&
            typeof result.body.request_id === "string"
          )
            requestID = result.body.request_id;
          if (logID)
            await database
              .query(
                "UPDATE webdock_auth.mail_request_log SET status=$2,duration_ms=$3,request_id=$4,request_body=$5,response_body=$6 WHERE id=$1",
                [
                  logID,
                  result.status,
                  Date.now() - started,
                  requestID,
                  JSON.stringify(redactMailLog(body)),
                  JSON.stringify(redactMailLog(result.body)),
                ],
              )
              .catch(() => {});
          return Response.json(result.body, {
            status: result.status,
            headers: {
              "Cache-Control": "no-store",
              "X-Request-ID": requestID,
              ...(result.status === 429 ? { "Retry-After": "60" } : {}),
            },
          });
        });
      } catch (error) {
        const status = error instanceof MailError ? error.status : 503;
        return Response.json(
          {
            statusCode: status,
            name:
              error instanceof MailError ? error.code : "service_unavailable",
            message:
              error instanceof MailError
                ? error.message
                : "Mail service is temporarily unavailable",
            request_id: requestID,
          },
          {
            status,
            headers: { "Cache-Control": "no-store", "X-Request-ID": requestID },
          },
        );
      }
    };
  }
  return { readBoundedText, readBoundedJson, mailEndpoint };
}
