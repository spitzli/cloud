import { createMailCron } from "@webdock/module-mail/server/product-cron";
import { withDatabaseScope } from "@webdock/database-runtime";
import { mailJobs, runMailJob, type MailJob } from "./jobs";
export const { handleMailCron } = createMailCron({
  withDatabaseScope,
  runMailJob,
});
