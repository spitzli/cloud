import { createHostingByokMail } from "@webdock/module-hosting/server/byok-mail";
import { requireMail } from "../../installation";
import { domainProvider } from "./provider-composition";
import { encryptMailSecret, decryptMailSecret } from "../platform/platform";
import { byokPolicy, byokAccess } from "./byok";
import { authorizeHosting } from "./authorization";
import { transaction, audit, planLock } from "./db";
import { database } from "../../db/runtime";
export const { executeByokMail } = createHostingByokMail({
  get requireMail() {
    return requireMail;
  },
  get domainProvider() {
    return domainProvider;
  },
  get encryptMailSecret() {
    return encryptMailSecret;
  },
  get decryptMailSecret() {
    return decryptMailSecret;
  },
  get byokPolicy() {
    return byokPolicy;
  },
  get byokAccess() {
    return byokAccess;
  },
  get authorizeHosting() {
    return authorizeHosting;
  },
  get transaction() {
    return transaction;
  },
  get audit() {
    return audit;
  },
  get planLock() {
    return planLock;
  },
  get database() {
    return database;
  },
  fetch: (input, init) => fetch(input, init),
});
