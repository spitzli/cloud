import "server-only";
import { cache } from "react";
import type { ProjectOverview } from "@webdock/hosting-contracts";
import { hostingPageCall } from "./hosting-client";

export const getProjectOverview = cache((projectID: string) =>
  hostingPageCall<ProjectOverview>(
    { action: "projects.overview", projectID },
    `/hosting/projects/${encodeURIComponent(projectID)}/overview`,
  ),
);
