import type { RegistryActor } from "../registry/compat";
export let projectHostingStatus: ((actor: RegistryActor, projectID: string) => Promise<unknown>) | undefined;
export function configureMCPProducts(status: NonNullable<typeof projectHostingStatus>) { projectHostingStatus = status; }
