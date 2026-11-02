// Retained tables still require RLS; billing has no active application surface.
export const billingTables = [
  "billing_manual_release",
  "billing_change",
  "billing_offer",
  "billing_connection",
  "billing_product",
  "billing_product_version",
  "billing_product_mapping",
  "billing_customer",
  "billing_contract",
  "billing_checkout",
  "billing_subscription",
  "billing_order",
  "billing_webhook_event",
  "billing_job",
] as const;
export function billingPolicy(name: string) {
  if (!(billingTables as readonly string[]).includes(name))
    throw Error("Unknown billing table");
  const col = (field: string) => '"' + name + '"."' + field + '"';
  const operator = "webdock_rls.operator()",
    customer = `webdock_rls.billing_admin(${col("customer_id")}::text)`,
    actor =
      "webdock_rls.actor() IS NOT NULL AND webdock_rls.actor()->'preview'='null'::jsonb";
  const scoped = `webdock_rls.worker(ARRAY['polar:'||${col("environment")}||':customer']) AND webdock_rls.request_scope()->>'customerID'=${col("customer_id")}::text`;
  const catalog = `webdock_rls.worker(ARRAY['polar:'||${col("environment")}||':catalog'])`,
    inbox = `webdock_rls.worker(ARRAY['polar:'||${col("environment")}||':inbox'])`,
    dispatcher = `webdock_rls.worker(ARRAY['polar:'||${col("environment")}||':dispatcher'])`;
  let read = operator,
    write = operator,
    workerRead = "false",
    workerWrite = "false";
  if (name === "billing_product")
    read += ` OR ((${actor}) AND ${col("visibility")}='public' AND ${col("operationally_ready")} AND NOT ${col("archived")})`;
  if (name === "billing_product_version")
    read += ` OR ((${actor}) AND EXISTS(SELECT 1 FROM webdock_auth.billing_product p WHERE p.id=${col("product_id")} AND p.visibility='public' AND p.operationally_ready AND NOT p.archived))`;
  if (name === "billing_product_mapping") {
    read += ` OR ((${actor}) AND ${col("status")}='ready' AND ${col("environment")}='live' AND EXISTS(SELECT 1 FROM webdock_auth.billing_product_version v JOIN webdock_auth.billing_product p ON p.id=v.product_id WHERE v.id=${col("version_id")} AND p.visibility='public' AND p.operationally_ready AND NOT p.archived))`;
    workerRead = `${catalog} OR webdock_rls.worker(ARRAY['polar:'||${col("environment")}||':customer'])`;
    workerWrite = catalog;
  }
  if (
    [
      "billing_customer",
      "billing_contract",
      "billing_checkout",
      "billing_subscription",
      "billing_order",
    ].includes(name)
  ) {
    read = `${operator} OR ((${actor}) AND (webdock_rls.customer_allowed(${col("customer_id")}::text,true) OR ${customer}))`;
    write = [
      "billing_customer",
      "billing_contract",
      "billing_checkout",
    ].includes(name)
      ? `${operator} OR ((${actor}) AND ${customer})`
      : "false";
    workerRead = workerWrite = scoped;
  }
  if (name === "billing_manual_release") {
    read = write = operator;
    workerRead =
      "webdock_rls.worker(ARRAY['polar:live:customer']) AND webdock_rls.request_scope()->>'customerID'=" +
      col("customer_id") +
      "::text";
    workerWrite = "false";
  }
  if (name === "billing_change") {
    read = `${operator} OR ((${actor}) AND (webdock_rls.customer_allowed(${col("customer_id")}::text,true) OR ${customer}))`;
    write = `webdock_rls.billing_admin(${col("customer_id")}::text)`;
    workerRead = workerWrite = scoped;
  }
  if (name === "billing_offer") {
    read = `${operator} OR ((${actor}) AND (webdock_rls.customer_allowed(${col("customer_id")}::text,true) OR ${customer}))`;
    write = operator;
  }
  if (name === "billing_webhook_event") {
    write = "false";
    workerRead = workerWrite = `${inbox} OR ${dispatcher}`;
  }
  if (name === "billing_job") {
    workerRead =
      workerWrite = `${inbox} OR ${dispatcher} OR ${catalog} OR (${scoped})`;
  }
  return {
    read,
    write,
    workerRead,
    workerWrite,
    updateWrite: ["billing_change", "billing_manual_release"].includes(name)
      ? "false"
      : name === "billing_offer"
        ? `${operator} OR ((${actor}) AND ${customer})`
        : write,
  };
}
