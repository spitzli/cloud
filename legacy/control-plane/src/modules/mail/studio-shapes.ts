import type { MailKeyInput } from "./mail-keys";
import { mailStudioShapes } from "@webdock/module-mail/server/product-studio-shapes";
const isObject = (value: unknown): value is Record<string, unknown> =>
  value !== null && typeof value === "object" && !Array.isArray(value);
const isString = (value: unknown): value is string =>
  typeof value === "string" && value.length <= 32768;
const isInput = (value: unknown): value is Record<string, string> =>
  isObject(value) &&
  Object.keys(value).length <= 100 &&
  Object.values(value).every(isString);
const isMailKeyInput = (value: unknown): value is MailKeyInput =>
  isObject(value) &&
  isString(value.action) &&
  Object.entries(value).every(([key, field]) =>
    ["permissions", "ips"].includes(key)
      ? Array.isArray(field) && field.length <= 100 && field.every(isString)
      : ["action", "label", "consumerKey", "confirm"].includes(key) &&
        isString(field),
  );
export const mailOperationShapes: Record<
  string,
  ((value: unknown) => boolean)[]
> = {
  ...mailStudioShapes,
  getTenantMail: [isString],
  manageTenantMail: [isString, isInput],
  getTenantMailboxes: [isString],
  manageTenantMailboxes: [isString, isInput],
  launchTenantWebmail: [isString, isString],
  getNativeSending: [isString],
  manageNativeSending: [isString, isInput],
  getTenantMailService: [isString],
  manageTenantMailService: [isString, isInput],
  getTenantSenderDomains: [isString],
  manageTenantSenderDomain: [
    isString,
    (value) =>
      isInput(value) &&
      ["register", "refresh", "unregister"].includes(value.action) &&
      isString(value.domainID),
  ],
  getTenantMailKeys: [isString],
  manageTenantMailKeys: [isString, isMailKeyInput],
  getTenantTracking: [isString],
  manageTenantTracking: [isString, isInput],
  getMailConnectionStatus: [],
  saveMailCredentials: [isString, isString],
  clearMailCredentials: [],
  testMailConnection: [],
};
