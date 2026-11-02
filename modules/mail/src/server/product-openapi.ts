import { mailOperations } from "./product-api-contract";
const string = { type: "string" };
const email = {
  type: "object",
  required: ["from", "to", "subject"],
  additionalProperties: false,
  properties: {
    from: {
      type: "string",
    },
    to: {
      oneOf: [
        {
          type: "string",
        },
        {
          type: "array",
          items: {
            type: "string",
          },
          maxItems: 50,
        },
      ],
    },
    cc: {
      oneOf: [
        {
          type: "string",
        },
        {
          type: "array",
          items: {
            type: "string",
          },
          maxItems: 50,
        },
      ],
    },
    bcc: {
      oneOf: [
        {
          type: "string",
        },
        {
          type: "array",
          items: {
            type: "string",
          },
          maxItems: 50,
        },
      ],
    },
    reply_to: {
      oneOf: [
        {
          type: "string",
        },
        {
          type: "array",
          items: {
            type: "string",
          },
          maxItems: 50,
        },
      ],
    },
    subject: {
      type: "string",
      description: "At most 998 UTF-8 bytes, no line breaks",
    },
    text: {
      type: "string",
    },
    html: {
      type: "string",
    },
    headers: {
      type: "object",
      additionalProperties: {
        type: "string",
      },
      maxProperties: 50,
    },
    tags: {
      type: "array",
      maxItems: 50,
      items: {
        type: "object",
        required: ["name", "value"],
        properties: {
          name: {
            type: "string",
            pattern: "^[A-Za-z0-9_-]{1,256}$",
          },
          value: {
            type: "string",
            pattern: "^[A-Za-z0-9_-]{1,256}$",
          },
        },
      },
    },
    attachments: {
      type: "array",
      maxItems: 10,
      description: "At most 2 MiB decoded; no URL/path attachments",
      items: {
        type: "object",
        required: ["content"],
        additionalProperties: false,
        properties: {
          content: {
            type: "string",
            contentEncoding: "base64",
          },
          filename: {
            type: "string",
          },
          content_type: {
            type: "string",
          },
          content_id: {
            type: "string",
            description:
              "CID local part. The verified Turbo transport appends @sender-domain; reference cid:<content_id>@<sender-domain> in HTML.",
          },
        },
      },
    },
    scheduled_at: {
      type: "string",
      format: "date-time",
    },
  },
};
const object = (
  properties: Record<string, unknown>,
  required: string[] = [],
) => ({ type: "object", properties, required, additionalProperties: false });
function requestSchema(id: string): unknown {
  if (id === "emails.send") return { $ref: "#/components/schemas/Email" };
  if (id === "emails.batch") {
    const properties = { ...email.properties } as Record<string, unknown>;
    delete properties.attachments;
    delete properties.scheduled_at;
    return {
      type: "array",
      minItems: 1,
      maxItems: 100,
      items: { ...email, properties },
    };
  }
  if (id === "emails.reschedule")
    return object({ scheduled_at: { type: "string", format: "date-time" } }, [
      "scheduled_at",
    ]);
  if (id === "domains.create") return object({ name: string }, ["name"]);
  if (id === "keys.create")
    return object(
      {
        name: { type: "string", maxLength: 80 },
        permission: { enum: ["sending_access", "full_access"] },
        domain_id: { $ref: "#/components/schemas/ID" },
      },
      ["name", "permission"],
    );
  if (id === "webhooks.create" || id === "webhooks.update")
    return object(
      {
        url: { type: "string", format: "uri" },
        events: {
          type: "array",
          items: { $ref: "#/components/schemas/EventType" },
        },
        ...(id === "webhooks.update" ? { enabled: { type: "boolean" } } : {}),
      },
      id === "webhooks.create" ? ["url", "events"] : [],
    );
  if (id === "suppressions.create")
    return object(
      {
        email: string,
        reason: { enum: ["manual", "bounced", "complained", "unsubscribed"] },
      },
      ["email"],
    );
  return object({});
}
const response = (schema: unknown, description: string) => ({
  description,
  content: { "application/json": { schema } },
});
export function mailOpenAPI() {
  const paths: Record<string, Record<string, unknown>> = {};
  for (const op of mailOperations) {
    const parameters: unknown[] = [...op.path.matchAll(/\{([^}]+)\}/g)].map(
      ([, name]) => ({
        name,
        in: "path",
        required: true,
        schema: {
          $ref:
            name === "email"
              ? "#/components/schemas/Address"
              : "#/components/schemas/ID",
        },
      }),
    );
    if (op.id === "emails.send" || op.id === "emails.batch")
      parameters.push({
        name: "Idempotency-Key",
        in: "header",
        schema: { type: "string", minLength: 1, maxLength: 256 },
        description:
          "Completed responses replay for 24 hours. Unknown handoffs remain reserved until reconciliation; never retry with a new key.",
      });
    if (["emails.reschedule", "emails.cancel"].includes(op.id))
      parameters.push({
        name: "If-Match",
        in: "header",
        required: true,
        schema: { type: "string" },
        description: "Current message revision; absent: 428, stale: 409",
      });
    if (op.id.endsWith(".list") || op.id === "webhooks.deliveries")
      parameters.push(
        {
          name: "limit",
          in: "query",
          schema: { type: "integer", minimum: 1, maximum: 100, default: 20 },
        },
        {
          name: "after",
          in: "query",
          schema: string,
          description: "Opaque cursor returned as nextCursor",
        },
      );
    if (
      [
        "domains.list",
        "keys.list",
        "webhooks.list",
        "suppressions.list",
        "logs.list",
      ].includes(op.id)
    )
      parameters.push({
        name: "search",
        in: "query",
        schema: { type: "string", maxLength: 200 },
      });
    if (op.id === "logs.list")
      parameters.push(
        {
          name: "method",
          in: "query",
          schema: {
            type: "string",
            enum: ["GET", "POST", "PATCH", "PUT", "DELETE", "HEAD", "OPTIONS"],
          },
        },
        {
          name: "status",
          in: "query",
          schema: { type: "integer" },
          description: "HTTP status, or 0 for an unfinished request",
        },
      );
    if (["emails.list", "analytics.get"].includes(op.id))
      parameters.push(
        {
          name: "domain_id",
          in: "query",
          schema: { $ref: "#/components/schemas/ID" },
        },
        {
          name: "include_simulated",
          in: "query",
          schema: { type: "boolean", default: false },
        },
      );
    if (op.id === "emails.list")
      parameters.push(
        {
          name: "search",
          in: "query",
          schema: { type: "string", maxLength: 200 },
        },
        { name: "status", in: "query", schema: string },
      );
    if (op.id === "analytics.get")
      parameters.push({
        name: "days",
        in: "query",
        schema: { type: "integer", enum: [7, 30, 90], default: 30 },
      });
    const status = op.id.endsWith(".create") ? "201" : "200";
    const responses: Record<string, unknown> = {
      [status]: response(
        op.id === "emails.send"
          ? { $ref: "#/components/schemas/SendResult" }
          : op.id === "emails.batch" || op.id === "batches.get"
            ? { $ref: "#/components/schemas/Batch" }
            : op.id.endsWith(".list") || op.id === "webhooks.deliveries"
              ? { $ref: "#/components/schemas/Page" }
              : { type: "object" },
        "Successful operation",
      ),
    };
    if (op.id === "emails.batch")
      responses["202"] = response(
        { $ref: "#/components/schemas/Batch" },
        "Durable batch still processing; poll with a Full key",
      );
    for (const code of [
      "401",
      "403",
      "404",
      "409",
      "413",
      "415",
      "422",
      "428",
      "429",
      "502",
      "503",
    ])
      responses[code] = response(
        { $ref: "#/components/schemas/Error" },
        code === "502"
          ? "Failed or uncertain handoff. Inspect email_id; never automatically resend."
          : "Request rejected",
      );
    paths[op.path] ??= {};
    paths[op.path][op.method.toLowerCase()] = {
      operationId: op.id.replaceAll(".", "_"),
      summary: op.id,
      description: `Requires ${op.permission}. Account and connection are derived from the key.`,
      security: [{ MailApiKey: [] }],
      parameters,
      ...(["POST", "PATCH"].includes(op.method)
        ? {
            requestBody: {
              required: ![
                "emails.cancel",
                "domains.verify",
                "webhooks.rotate",
                "webhooks.replay",
              ].includes(op.id),
              content: { "application/json": { schema: requestSchema(op.id) } },
            },
          }
        : {}),
      responses,
    };
  }
  return {
    openapi: "3.1.0",
    info: {
      title: "Spitzli Cloud Mail API",
      version: "1.0.0",
      description:
        "Transactional Mail. Accepted means provider acceptance, not delivery. JSON limit 4,000,000 bytes; text+HTML 512 KiB. Requests 120/minute/tenant. Unknown handoffs must never be resent with a new key. Webhooks: HMAC-SHA256 over id.timestamp.rawBody, at-least-once.",
    },
    servers: [{ url: "/api/mail/v1" }],
    paths,
    components: {
      ...components,
      schemas: { ...components.schemas, Email: email },
    },
  };
}
const components = {
  securitySchemes: {
    MailApiKey: {
      type: "http",
      scheme: "bearer",
      bearerFormat: "wdm_\u2026",
    },
  },
  schemas: {
    ID: {
      type: "string",
      pattern: "^[1-9][0-9]{0,18}$",
    },
    Address: {
      type: "string",
    },
    EventType: {
      enum: [
        "email.scheduled",
        "email.canceled",
        "email.sent",
        "email.delivered",
        "email.delivery_delayed",
        "email.opened",
        "email.clicked",
        "email.bounced",
        "email.complained",
        "email.unsubscribed",
        "email.failed",
        "email.suppressed",
        "domain.created",
        "domain.updated",
        "domain.deleted",
      ],
    },
    SendResult: {
      type: "object",
      required: ["id"],
      properties: {
        id: {
          $ref: "#/components/schemas/ID",
        },
      },
    },
    Batch: {
      type: "object",
      required: ["id", "status", "data"],
      properties: {
        id: {
          type: "string",
        },
        status: {
          enum: ["processing", "succeeded", "failed"],
        },
        data: {
          type: "array",
          items: {
            $ref: "#/components/schemas/SendResult",
          },
        },
        failed_index: {
          type: "integer",
        },
        failed_email_id: {
          type: ["string", "null"],
        },
      },
    },
    Page: {
      type: "object",
      required: ["data"],
      properties: {
        data: {
          type: "array",
          items: {
            type: "object",
          },
        },
        nextCursor: {
          type: ["string", "null"],
        },
      },
    },
    Error: {
      type: "object",
      required: ["statusCode", "name", "message", "request_id"],
      properties: {
        statusCode: {
          type: "integer",
        },
        name: {
          type: "string",
        },
        message: {
          type: "string",
        },
        request_id: {
          type: "string",
        },
        email_id: {
          type: "string",
        },
        id: {
          type: "string",
        },
        data: {
          type: "array",
          items: {
            $ref: "#/components/schemas/SendResult",
          },
        },
        failed_index: {
          type: "integer",
        },
        failed_email_id: {
          type: ["string", "null"],
        },
      },
    },
  },
};
