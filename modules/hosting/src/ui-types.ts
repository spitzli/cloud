import type { SetupView, SetupInspection } from "@spitzli/hosting-contracts";
export type HostingFormState = {
  environmentReset?: boolean;
  error?: string;
  message?: string;
  setupPath?: string;
  token?: string;
  expiresAt?: string;
  appPath?: string;
  operationPath?: string;
  clusterPath?: string;
  revision?: number;
};
export type SetupActionState = {
  view?: SetupView;
  inspection?: SetupInspection;
  error?: string;
};

import type { z } from "zod";
import type {
  byokPolicySchema,
  HostingClusterView,
  InventoryResource,
} from "@spitzli/hosting-contracts";

export type OwnInfrastructurePolicy = z.infer<typeof byokPolicySchema> & {
  revision: number;
};
export type OwnInfrastructureView = {
  policy: OwnInfrastructurePolicy;
  operator: boolean;
  canWrite: boolean;
  clusters: HostingClusterView[];
  vercel: {
    team_id: string;
    revision: number;
    projects: string[];
    connected_at: string;
  } | null;
};
export type OwnClusterView = HostingClusterView & {
  policy: OwnInfrastructurePolicy;
  canWrite: boolean;
};
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
