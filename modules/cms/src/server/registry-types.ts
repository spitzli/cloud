import type { Project } from "@spitzli/core/registry/types";
export interface CmsInstance {
  id: string;
  label: string;
  project: string | Project;
  adminURL: string;
  schemaName: string;
  provider: "vercel" | "other";
  providerProjectID: string;
  template:
    "webdock-landing" | "spitzli-portfolio" | "stall-business" | "custom";
  payloadVersion?: string | null;
  status: "active" | "suspended" | "retired";
  notes?: string | null;
  updatedAt: string;
  createdAt: string;
}
