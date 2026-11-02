import { createMailSchedules } from "@webdock/module-mail/server/product-schedules";
import { withMailTransaction } from "./store";
import { assertMailAccess } from "./access";
import { recordMailEvent } from "./events";
export const { rescheduleMail, cancelScheduledMail } = createMailSchedules({
  withMailTransaction,
  assertMailAccess,
  recordMailEvent,
});
