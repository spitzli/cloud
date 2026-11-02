import type { z } from "zod";
import type { byokPolicySchema, HostingClusterView, InventoryResource } from "@webdock/hosting-contracts";

export type OwnInfrastructurePolicy = z.infer<typeof byokPolicySchema> & { revision: number };
export type OwnInfrastructureView = {
  policy: OwnInfrastructurePolicy;
  operator: boolean;
  canWrite: boolean;
  clusters: HostingClusterView[];
  vercel: { team_id: string; revision: number; projects: string[]; connected_at: string } | null;
};
export type OwnClusterView = HostingClusterView & { policy: OwnInfrastructurePolicy; canWrite: boolean };
export type OwnInventoryView = {
  observedAt: string | null;
  complete: boolean;
  unavailable: string[];
  stale: boolean;
  resources: InventoryResource[];
  total: number;
  page: number;
  canWrite: boolean;
};
