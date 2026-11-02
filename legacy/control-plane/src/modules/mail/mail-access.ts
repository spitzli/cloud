import { clerkMailAccess } from "./mail-clerk-access";
import { selectedIdentityDirectory as betterAuthIdentity } from "../auth/principal";
import { createNativeMailAuthorization } from "@webdock/module-mail/server/native-access";
export { mailUnavailable, type MailActor } from "@webdock/module-mail/server/native-access";
export const { authorizeMail, authorizeMailbox, authorizeQueuedMailbox } = createNativeMailAuthorization(betterAuthIdentity, clerkMailAccess);
