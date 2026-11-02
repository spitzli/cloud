import { inspectWebmail } from "./mail-sessions";
import { createNativeWrites } from "@webdock/module-mail/server/native-writes";
export {
  reserveWriteSchema,
  finishWriteSchema,
} from "@webdock/module-mail/server/native-writes";
export const { reserveWebmailWrite, finishWebmailWrite } = createNativeWrites({
  inspectWebmail,
});
