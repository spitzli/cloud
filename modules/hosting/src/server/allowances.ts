import { allocationTotals as totals } from "@spitzli/hosting-contracts/capacity-totals";
export { allocationTotals as totals } from "@spitzli/hosting-contracts/capacity-totals";
import {
  effectiveHostingAllowances,
  hostingDimensions,
} from "@spitzli/hosting-contracts";
import type { Connection } from "./authorization";
export async function limits(
  db: Connection,
  customerID: string,
  projectID?: string,
  provider?: string,
) {
  const sub = (
    await db.query(
      "SELECT base,extras,revision FROM webdock_auth.tenant_subscription WHERE customer_id=$1",
      [customerID],
    )
  ).rows[0];
  const effective = effectiveHostingAllowances(
    sub?.base?.hosting,
    sub?.extras?.hosting,
  );
  const rows = (
    await db.query(
      "SELECT scope,values,revision FROM webdock_auth.hosting_limit WHERE customer_id=$1 ORDER BY scope",
      [customerID],
    )
  ).rows;
  const applicable = [
    "customer",
    ...(provider ? [`provider:${provider}`] : []),
    ...(projectID ? [`project:${projectID}`] : []),
  ];
  for (const row of rows.filter((r) => applicable.includes(r.scope)))
    for (const k of hostingDimensions) {
      const v = row.values[k];
      if (v !== undefined && v !== null)
        effective[k] = effective[k] === null ? v : Math.min(effective[k], v);
    }
  return { effective, subscriptionRevision: sub?.revision ?? 0, limits: rows };
}
export async function usage(db: Connection, customerID: string) {
  const cap = await limits(db, customerID);
  const rows = (
    await db.query(
      "SELECT demand,status FROM webdock_auth.hosting_reservation WHERE customer_id=$1 AND status<>'released'",
      [customerID],
    )
  ).rows;
  const reserved = totals(rows.filter((r) => r.status === "reserved")),
    allocated = totals(rows.filter((r) => r.status === "active"));
  const total = totals(rows);
  return {
    ...cap,
    reserved,
    allocated,
    measured: null,
    checkedAt: null,
    enforcement: "webdock-enforced",
    overallocated: hostingDimensions.some(
      (k) => cap.effective[k] !== null && total[k] > cap.effective[k],
    ),
  };
}
