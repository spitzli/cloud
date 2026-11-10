import { withDatabaseScope } from "@spitzli/database-runtime";
import { createHash, timingSafeEqual } from "node:crypto";
import { z } from "zod";
import { MailServiceError } from "./instances";
import { HostingError } from "@spitzli/hosting-contracts";
import { reserveWriteSchema, finishWriteSchema } from "./native-writes";
import { nativeKeyToken } from "./native-keys";
import {
  prepareWebmailSendSchema,
  stageWebmailSendSchema,
  submissionPolicySchema,
  admissionSchema,
  sendEnvelope,
} from "./native-submissions";
import type { createNativeSessions } from "./native-sessions";
import type { createNativeWrites } from "./native-writes";
import type { createNativeKeys } from "./native-keys";
import type { createNativeSubmissions } from "./native-submissions";
import type { NativeMailPorts } from "./native-ports";
const resource = z.string().regex(/^[1-9][0-9]{0,18}$/);

const sendLease = z
  .object({
    id: resource,
    customerID: resource,
    sourceID: z.string().max(200),
    payloadRef: z.string().uuid(),
    payloadBytes: z.number().int().positive().max(26214400),
    requestHash: z.string().regex(/^[a-f0-9]{64}$/),
    payloadFormat: z.enum(["request", "mime"]),
    sourceContext: z.record(z.string(), z.unknown()),
    route: z.record(z.string(), z.unknown()),
    envelope: z.record(z.string(), z.unknown()),
    leaseToken: z.string().uuid(),
  })
  .strict();

const payloadLease = z
  .object({
    id: resource,
    customerID: resource,
    payloadRef: z.string().uuid(),
    uploadToken: z.string().uuid(),
  })
  .strict();

const cleanupItem = z
  .object({ id: resource, customerID: resource, ref: z.string().uuid() })
  .strict();

const token = z.string().regex(/^[A-Za-z0-9_-]{43}$/);

const command = z.discriminatedUnion("action", [
  z
    .object({
      action: z.literal("send.webmail.lookup"),
      token,
      emailID: z.string().regex(/^[A-Za-z0-9_-]{1,255}$/),
    })
    .strict(),
  z
    .object({
      action: z.literal("send.webmail.prepare"),
      submission: prepareWebmailSendSchema,
    })
    .strict(),
  z
    .object({
      action: z.literal("send.webmail.stage"),
      submission: stageWebmailSendSchema,
    })
    .strict(),
  z
    .object({
      action: z.literal("send.webmail.abort"),
      operationID: resource,
      leaseToken: z.string().uuid(),
    })
    .strict(),
  z
    .object({
      action: z.literal("send.webmail.status"),
      token,
      id: resource,
      cancel: z.boolean().optional(),
    })
    .strict(),
  z
    .object({
      action: z.literal("send.webmail.filingAccess"),
      id: resource,
      token: token.optional(),
      customerID: resource.optional(),
      leaseToken: z.string().uuid().optional(),
    })
    .strict(),
  z
    .object({
      action: z.literal("send.webmail.filed"),
      id: resource,
      customerID: resource,
      leaseToken: z.string().uuid(),
      filed: z.boolean(),
    })
    .strict(),
  z
    .object({ action: z.literal("send.authenticate"), token: nativeKeyToken })
    .strict(),
  z
    .object({ action: z.literal("send.commitPayload"), lease: payloadLease })
    .strict(),
  z
    .object({ action: z.literal("send.abortPayload"), lease: payloadLease })
    .strict(),
  z.object({ action: z.literal("send.cleanup") }).strict(),
  z
    .object({
      action: z.literal("send.cleanupDone"),
      items: z.array(cleanupItem).max(50),
    })
    .strict(),
  z
    .object({
      action: z.literal("send.policy.auth"),
      customerID: resource,
      token: submissionPolicySchema.shape.token,
    })
    .strict(),
  z
    .object({
      action: z.literal("send.preflight"),
      token: nativeKeyToken,
      envelope: sendEnvelope,
    })
    .strict(),
  z
    .object({ action: z.literal("send.admit"), submission: admissionSchema })
    .strict(),
  z
    .object({
      action: z.literal("send.status"),
      token: nativeKeyToken,
      id: resource,
      cancel: z.boolean().optional(),
    })
    .strict(),
  z.object({ action: z.literal("send.claim") }).strict(),
  z
    .object({
      action: z.literal("send.policy"),
      submission: submissionPolicySchema,
    })
    .strict(),
  z
    .object({
      action: z.literal("send.begin"),
      lease: sendLease,
      digest: z
        .string()
        .regex(/^[a-f0-9]{64}$/)
        .optional(),
    })
    .strict(),
  z
    .object({ action: z.literal("send.reviewPreparation"), lease: sendLease })
    .strict(),
  z
    .object({
      action: z.literal("send.finish"),
      lease: sendLease,
      result: z
        .object({
          state: z.enum(["accepted", "rejected", "unknown"]),
          providerID: z
            .string()
            .regex(/^[A-Za-z0-9_-]{1,255}$/)
            .optional(),
        })
        .strict(),
    })
    .strict(),
  z.object({ action: z.literal("exchange"), code: token }).strict(),
  z.object({ action: z.literal("inspect"), token }).strict(),
  z.object({ action: z.literal("logout"), token }).strict(),
  z
    .object({ action: z.literal("reserve"), write: reserveWriteSchema })
    .strict(),
  z.object({ action: z.literal("finish"), write: finishWriteSchema }).strict(),
]);
export interface NativeGatewayDependencies {
  readHostingJSON: NativeMailPorts["readHostingJSON"];
  requireHostingEnvironment: NativeMailPorts["requireHostingEnvironment"];
  transaction: NativeMailPorts["transaction"];
  exchangeWebmail: ReturnType<typeof createNativeSessions>["exchangeWebmail"];
  inspectWebmail: ReturnType<typeof createNativeSessions>["inspectWebmail"];
  revokeWebmail: ReturnType<typeof createNativeSessions>["revokeWebmail"];
  reserveWebmailWrite: ReturnType<
    typeof createNativeWrites
  >["reserveWebmailWrite"];
  finishWebmailWrite: ReturnType<
    typeof createNativeWrites
  >["finishWebmailWrite"];
  authorizeMailKey: ReturnType<typeof createNativeKeys>["authorizeMailKey"];
  requireNativeSending: ReturnType<
    typeof createNativeKeys
  >["requireNativeSending"];
  lookupWebmailSend: ReturnType<
    typeof createNativeSubmissions
  >["lookupWebmailSend"];
  prepareWebmailSend: ReturnType<
    typeof createNativeSubmissions
  >["prepareWebmailSend"];
  stageWebmailSend: ReturnType<
    typeof createNativeSubmissions
  >["stageWebmailSend"];
  abortWebmailPreparation: ReturnType<
    typeof createNativeSubmissions
  >["abortWebmailPreparation"];
  webmailSendStatus: ReturnType<
    typeof createNativeSubmissions
  >["webmailSendStatus"];
  webmailFilingAccess: ReturnType<
    typeof createNativeSubmissions
  >["webmailFilingAccess"];
  finishWebmailFiling: ReturnType<
    typeof createNativeSubmissions
  >["finishWebmailFiling"];
  authenticateSubmissionPolicy: ReturnType<
    typeof createNativeSubmissions
  >["authenticateSubmissionPolicy"];
  authorizeSubmissionPolicy: ReturnType<
    typeof createNativeSubmissions
  >["authorizeSubmissionPolicy"];
  commitSubmissionPayload: ReturnType<
    typeof createNativeSubmissions
  >["commitSubmissionPayload"];
  abortSubmissionPayload: ReturnType<
    typeof createNativeSubmissions
  >["abortSubmissionPayload"];
  claimSubmission: ReturnType<
    typeof createNativeSubmissions
  >["claimSubmission"];
  submissionCleanup: ReturnType<
    typeof createNativeSubmissions
  >["submissionCleanup"];
  finishSubmissionCleanup: ReturnType<
    typeof createNativeSubmissions
  >["finishSubmissionCleanup"];
  reviewSubmissionPreparation: ReturnType<
    typeof createNativeSubmissions
  >["reviewSubmissionPreparation"];
  preflightSubmission: ReturnType<
    typeof createNativeSubmissions
  >["preflightSubmission"];
  admitSubmission: ReturnType<
    typeof createNativeSubmissions
  >["admitSubmission"];
  beginSubmission: ReturnType<
    typeof createNativeSubmissions
  >["beginSubmission"];
  finishSubmission: ReturnType<
    typeof createNativeSubmissions
  >["finishSubmission"];
  submissionStatus: ReturnType<
    typeof createNativeSubmissions
  >["submissionStatus"];
  env: NativeMailPorts["env"];
}
export function createNativeGateway(dependencies: NativeGatewayDependencies) {
  const {
    readHostingJSON,
    requireHostingEnvironment,
    transaction,
    exchangeWebmail,
    inspectWebmail,
    revokeWebmail,
    reserveWebmailWrite,
    finishWebmailWrite,
    authorizeMailKey,
    requireNativeSending,
    lookupWebmailSend,
    prepareWebmailSend,
    stageWebmailSend,
    abortWebmailPreparation,
    webmailSendStatus,
    webmailFilingAccess,
    finishWebmailFiling,
    authenticateSubmissionPolicy,
    authorizeSubmissionPolicy,
    commitSubmissionPayload,
    abortSubmissionPayload,
    claimSubmission,
    submissionCleanup,
    finishSubmissionCleanup,
    reviewSubmissionPreparation,
    preflightSubmission,
    admitSubmission,
    beginSubmission,
    finishSubmission,
    submissionStatus,
    env,
  } = dependencies;
  async function mailGateway(request: Request) {
    const headers = { "Cache-Control": "no-store" };
    try {
      requireHostingEnvironment();
      const supplied = request.headers
        .get("authorization")
        ?.match(/^Bearer (\S+)$/)?.[1];
      const matches = (expected: string | undefined) =>
        Boolean(
          expected &&
          expected.length >= 32 &&
          supplied &&
          supplied.length <= 512 &&
          timingSafeEqual(
            createHash("sha256").update(expected).digest(),
            createHash("sha256").update(supplied).digest(),
          ),
        );
      const browser = matches(env.MAIL_GATEWAY_SECRET);
      const worker =
        env.MAIL_SEND_WORKER_SECRET !== env.MAIL_GATEWAY_SECRET &&
        matches(env.MAIL_SEND_WORKER_SECRET);
      if (!browser && !worker)
        throw new HostingError(401, "Mail gateway authentication required.");
      const body = command.parse(await readHostingJSON(request, 65536));
      if (
        body.action.startsWith("send.")
          ? !worker
          : !browser && body.action !== "inspect"
      )
        throw new HostingError(403, "Mail control scope is unavailable.");
      const data = await withDatabaseScope(
        {
          kind: "system",
          purpose: worker ? "mail-send-gateway" : "mail-webmail-gateway",
        },
        () =>
          transaction<unknown>((db) => {
            if (body.action === "send.webmail.lookup")
              return lookupWebmailSend(db, body.token, body.emailID);
            if (body.action === "send.webmail.prepare")
              return prepareWebmailSend(db, body.submission);
            if (body.action === "send.webmail.stage")
              return stageWebmailSend(db, body.submission);
            if (body.action === "send.webmail.abort")
              return abortWebmailPreparation(db, body);
            if (body.action === "send.webmail.status")
              return webmailSendStatus(db, body.token, body.id, body.cancel);
            if (body.action === "send.webmail.filingAccess")
              return webmailFilingAccess(db, body);
            if (body.action === "send.webmail.filed")
              return finishWebmailFiling(db, body);
            if (body.action === "send.authenticate")
              return authorizeMailKey(db, body.token);
            if (body.action === "send.commitPayload")
              return commitSubmissionPayload(db, body.lease);
            if (body.action === "send.abortPayload")
              return abortSubmissionPayload(db, body.lease);
            if (body.action === "send.cleanup") return submissionCleanup(db);
            if (body.action === "send.cleanupDone")
              return finishSubmissionCleanup(db, body.items);
            if (body.action === "send.policy.auth")
              return authenticateSubmissionPolicy(
                db,
                body.customerID,
                body.token,
              );
            if (body.action === "send.policy")
              return authorizeSubmissionPolicy(db, body.submission);
            if (body.action === "send.preflight")
              return preflightSubmission(db, body.token, body.envelope);
            if (body.action === "send.admit")
              return admitSubmission(db, body.submission);
            if (body.action === "send.status")
              return submissionStatus(db, body.token, body.id, body.cancel);
            if (body.action === "send.claim") {
              requireNativeSending();
              return claimSubmission(db);
            }
            if (body.action === "send.reviewPreparation")
              return reviewSubmissionPreparation(db, body.lease);
            if (body.action === "send.begin")
              return beginSubmission(db, body.lease, body.digest);
            if (body.action === "send.finish")
              return finishSubmission(db, body.lease, body.result);
            if (body.action === "exchange")
              return exchangeWebmail(db, body.code);
            if (body.action === "inspect")
              return inspectWebmail(db, body.token);
            if (body.action === "reserve")
              return reserveWebmailWrite(db, body.write);
            if (body.action === "finish")
              return finishWebmailWrite(db, body.write);
            return revokeWebmail(db, body.token);
          }),
      );
      return Response.json({ data }, { headers });
    } catch (error) {
      const status =
        error instanceof HostingError
          ? error.status
          : error instanceof MailServiceError
            ? 403
            : error instanceof z.ZodError
              ? 400
              : 503;
      return Response.json(
        { error: "Mail access is unavailable." },
        { status, headers },
      );
    }
  }
  return { mailGateway };
}
