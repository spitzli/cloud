import { createHostingMailBinding } from "@webdock/module-hosting/server/mail-binding";
import { requireMail } from "../../installation";
import { currentDatabaseScope } from "@webdock/database-runtime";
import { assertMailAccess } from "../mail/product/access";
import { createMailKeyInTransaction } from "../mail/product/api-keys";
export const {
  applicationMailRoute,
  mailBindingMaterial,
  revokeBindingMail,
  bindingMailReady,
} = createHostingMailBinding({
  get requireMail() {
    return requireMail;
  },
  get currentDatabaseScope() {
    return currentDatabaseScope;
  },
  get assertMailAccess() {
    return assertMailAccess;
  },
  get createMailKeyInTransaction() {
    return createMailKeyInTransaction;
  },
  env: process.env,
});
