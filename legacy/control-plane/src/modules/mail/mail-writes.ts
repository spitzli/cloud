import { inspectWebmail } from "./mail-sessions";
import { createNativeWrites } from "@spitzli/module-mail/server/native-writes";
export {
  reserveWriteSchema,
  finishWriteSchema,
} from "@spitzli/module-mail/server/native-writes";
export const { reserveWebmailWrite, finishWebmailWrite } = createNativeWrites({
  inspectWebmail,
});
