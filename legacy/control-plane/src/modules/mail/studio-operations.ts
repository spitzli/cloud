import { dispatchMailStudio } from "./product/studio";
import { MailError } from "./product/access";
import { MailValidationError } from "@webdock/mail-contracts/validation";
import { getNativeSending, manageNativeSending } from "./mail-sending";
import { getTenantMailboxes, manageTenantMailboxes } from "./mailboxes";
import { launchTenantWebmail } from "./mail-sessions";
import {
  TenantMailError,
  getTenantMail,
  manageTenantMail,
} from "./tenant-mail";
import {
  MailServiceError,
  getTenantMailService,
  manageTenantMailService,
} from "./mail-service";
import {
  MailDomainsError,
  getTenantSenderDomains,
  manageTenantSenderDomain,
} from "./mail-domains";
import {
  MailKeysError,
  getTenantMailKeys,
  manageTenantMailKeys,
  type MailKeyInput,
} from "./mail-keys";
import {
  TrackingError,
  getTenantTracking,
  manageTenantTracking,
} from "./mail-tracking";
import { mailStudioShapes } from "@webdock/module-mail/server/product-studio-shapes";
import { requireAccessOperator } from "../access/access-management";
import {
  getMailConnectionStatus,
  saveMailCredentials,
  clearMailCredentials,
  testMailConnection,
} from "../platform/platform";
import type { PublicOperationError } from "../portal/product-operation";
export async function dispatch(
  operation: string,
  args: unknown[],
  headers: Headers,
  userID: string,
): Promise<unknown> {
  const text = args[0] as string,
    tenantInput = args[1] as Record<string, string>;
  if (Object.hasOwn(mailStudioShapes, operation))
    return dispatchMailStudio(operation, args, headers);
  switch (operation) {
    case "getTenantMail":
      return getTenantMail(headers, text);
    case "getTenantMailboxes":
      return getTenantMailboxes(headers, text);
    case "launchTenantWebmail":
      return launchTenantWebmail(headers, text, args[1] as string);
    case "manageTenantMailboxes":
      return manageTenantMailboxes(headers, text, tenantInput);
    case "getNativeSending":
      return getNativeSending(headers, text);
    case "manageNativeSending":
      return manageNativeSending(headers, text, tenantInput);
    case "getTenantMailService":
      return getTenantMailService(headers, text);
    case "manageTenantMailService":
      return manageTenantMailService(headers, text, tenantInput);
    case "manageTenantMail":
      return manageTenantMail(headers, text, tenantInput);
    case "getTenantSenderDomains":
      return getTenantSenderDomains(headers, text);
    case "manageTenantSenderDomain":
      return manageTenantSenderDomain(
        headers,
        text,
        args[1] as Parameters<typeof manageTenantSenderDomain>[2],
      );
    case "getTenantMailKeys":
      return getTenantMailKeys(headers, text);
    case "manageTenantMailKeys":
      return manageTenantMailKeys(headers, text, args[1] as MailKeyInput);
    case "getTenantTracking":
      return getTenantTracking(headers, text);
    case "manageTenantTracking":
      return manageTenantTracking(headers, text, tenantInput);
  }
  await requireAccessOperator(headers);
  switch (operation) {
    case "getMailConnectionStatus":
      return getMailConnectionStatus();
    case "saveMailCredentials":
      return saveMailCredentials(userID, text, args[1] as string);
    case "clearMailCredentials":
      return clearMailCredentials(userID);
    case "testMailConnection":
      return testMailConnection();
  }
  throw Error("Unknown Mail operation");
}
export function publicError(error: unknown): PublicOperationError | undefined {
  if (error instanceof MailError || error instanceof MailValidationError)
    return {
      kind: "MailProductError",
      message: error.message,
      status: error instanceof MailError ? error.status : 422,
    };
  const known = (
    [
      ["TenantMailError", TenantMailError],
      ["MailServiceError", MailServiceError],
      ["MailDomainsError", MailDomainsError],
      ["MailKeysError", MailKeysError],
      ["TrackingError", TrackingError],
    ] as const
  ).find(([, type]) => error instanceof type);
  if (known && error instanceof Error)
    return { kind: known[0], message: error.message, status: 400 };
}
