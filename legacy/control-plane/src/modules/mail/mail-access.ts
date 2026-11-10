import { clerkMailAccess } from "./mail-clerk-access";
import { selectedIdentityDirectory as betterAuthIdentity } from "../auth/principal";
import { createNativeMailAuthorization } from "@spitzli/module-mail/server/native-access";
export { mailUnavailable, type MailActor } from "@spitzli/module-mail/server/native-access";
export const { authorizeMail, authorizeMailbox, authorizeQueuedMailbox } = createNativeMailAuthorization(betterAuthIdentity, clerkMailAccess);
