export const pluginTables = [
  "plugin_source_import",
  "plugin_source_chunk",
  "plugin_bootstrap",
  "plugin_installation",
  "plugin_operation",
  "plugin_release",
  "plugin_grant",
  "plugin_document",
  "plugin_configuration",
  "plugin_runtime",
  "plugin_participant",
  "plugin_job_execution",
  "plugin_invocation",
  "plugin_document_receipt",
] as const;
export function pluginPolicy(table: string) {
  const operator = "webdock_rls.operator()";
  const controller = "webdock_rls.worker(ARRAY['plugin-controller']::text[])";
  const projection =
    "webdock_rls.worker(ARRAY['plugin-public-view','plugin-admission']::text[])";
  if (table === "plugin_source_chunk") {
    const own = `${operator} AND EXISTS(SELECT 1 FROM webdock_auth.plugin_source_import i WHERE i.id="plugin_source_chunk".import_id AND i.actor_id=webdock_rls.subject())`;
    return {
      read: own,
      write: own,
      workerRead: controller,
      workerWrite: controller,
    };
  }
  if (table === "plugin_document" || table === "plugin_document_receipt") {
    const scope = (cap: string) =>
      `webdock_rls.plugin_allows("${table}".plugin_id,"${table}".customer_id,"${table}".project_id,'${cap}')`;
    const own =
      table === "plugin_document_receipt"
        ? ` AND "${table}".operation_id=webdock_rls.plugin_context('plugin.data.write')->>'operationID'`
        : "";
    // The trusted broker needs row visibility for CAS UPDATE; public data.read
    // still checks its own capability. Receipt replay needs only write authority.
    const read =
      table === "plugin_document_receipt"
        ? scope("plugin.data.write") + own
        : `(${scope("plugin.data.read")} OR ${scope("plugin.data.write")})`;
    return {
      read,
      write: scope("plugin.data.write") + own,
      workerRead: read,
      workerWrite: scope("plugin.data.write") + own,
    };
  }
  if (table === "plugin_participant") {
    const own = `webdock_rls.worker(ARRAY['plugin-participant']::text[]) AND "plugin_participant".id::text=webdock_rls.request_scope()->>'participantID'`;
    return {
      read: operator,
      write: "false",
      workerRead: `${controller} OR ${projection} OR (${own})`,
      workerWrite: own,
    };
  }
  if (table === "plugin_bootstrap")
    return {
      read: "webdock_rls.actor() IS NOT NULL",
      write: "false",
      workerRead: `${controller} OR ${projection}`,
      workerWrite: "false",
    };
  if (table === "plugin_installation")
    return {
      read: "webdock_rls.actor() IS NOT NULL",
      write: operator,
      workerRead: `${controller} OR ${projection}`,
      workerWrite: controller,
    };
  if (table === "plugin_release")
    return {
      read: operator,
      write: operator,
      workerRead: controller,
      workerWrite: "false",
    };
  return {
    read: operator,
    write: operator,
    workerRead: controller,
    workerWrite: [
      "plugin_source_import",
      "plugin_source_chunk",
      "plugin_operation",
      "plugin_runtime",
      "plugin_job_execution",
      "plugin_invocation",
    ].includes(table)
      ? controller
      : "false",
  };
}
