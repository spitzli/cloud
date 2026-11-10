import { createHash, timingSafeEqual } from "node:crypto";
import { intersectRoles } from "@spitzli/payload-sso";
import type { Payload } from "payload";
import type { BridgeScope, BridgeRole } from "./types.ts";
export class BridgeError extends Error {
  constructor(
    message: string,
    public status = 400,
  ) {
    super(message);
  }
}
const token = (value: string | null) =>
  Boolean(value && /^[\x21-\x7e]{1,256}$/.test(value));
export function authenticateBridge(
  headers: Headers,
  config: { secret: string; instance: string; project: string },
): BridgeScope {
  const supplied = Buffer.from(headers.get("authorization") ?? "");
  const expected = Buffer.from(`Bearer ${config.secret}`);
  if (
    config.secret.length < 32 ||
    supplied.length !== expected.length ||
    !timingSafeEqual(supplied, expected)
  )
    throw new BridgeError("Unauthorized bridge.", 401);
  const instance = headers.get("x-webdock-instance"),
    project = headers.get("x-webdock-project"),
    subject = headers.get("x-webdock-subject"),
    role = headers.get("x-webdock-role"),
    grantEpoch = headers.get("x-webdock-grant-epoch");
  if (
    instance !== config.instance ||
    project !== config.project ||
    !token(subject) ||
    !token(grantEpoch) ||
    !["reader", "editor", "admin", "operator"].includes(role ?? "")
  )
    throw new BridgeError("Invalid bridge authority.", 403);
  return {
    instance,
    project,
    subject: subject!,
    role: role as BridgeRole,
    grantEpoch: grantEpoch!,
  };
}
/** The subject is the only lookup key. Existing local ceilings are never raised. */
export async function projectBridgeUser(payload: Payload, scope: BridgeScope) {
  const lookup = () =>
    payload.find({
      collection: "users",
      where: { authSubject: { equals: scope.subject } },
      limit: 2,
      depth: 0,
      overrideAccess: true,
    });
  let matches = await lookup();
  if (!matches.docs.length) {
    try {
      await payload.create({
        collection: "users",
        overrideAccess: true,
        context: { bootstrap: true },
        data: {
          authSubject: scope.subject,
          email: `${createHash("sha256").update(scope.subject).digest("hex")}@subject.invalid`,
          name: scope.subject,
          role: scope.role,
          disabled: false,
        } as never,
      });
    } catch (error) {
      // A simultaneous first request may win the unique subject insertion.
      matches = await lookup();
      if (matches.docs.length !== 1) throw error;
    }
    matches = await lookup();
  }
  const local = (matches.docs.length === 1 ? matches.docs[0] : null) as
    | (Record<string, unknown> & {
        id: string | number;
        role: string;
        disabled?: boolean;
        name?: string;
        email?: string;
        createdAt?: string;
        updatedAt?: string;
      })
    | null;
  const role = intersectRoles(local?.role, scope.role);
  if (!local || local.disabled || !role)
    throw new BridgeError("Local account access is disabled.", 403);
  return { ...local, collection: "users" as const, role };
}
