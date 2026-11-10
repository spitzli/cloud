import type {
  HostingRuntimePorts,
  DatabaseReader,
} from "@spitzli/hosting-contracts/runtime-ports";
import type { HostingAllowances } from "@spitzli/hosting-contracts";
/** Host-provided runtime/credential ports. Database services never construct provider clients. */
export interface DatabaseRuntimePorts extends HostingRuntimePorts {
  requireDatabases(intent?: "access" | "provision"): void;
  encryptSecret(value: unknown, purpose: string): string;
  decryptSecret<T>(value: string, purpose: string): T;
  nativeClusterDemand(
    db: DatabaseReader,
    clusterID: string,
  ): Promise<{ [K in keyof HostingAllowances]: number }>;
  clusterCapacityDemand(
    db: DatabaseReader,
    clusterID: string,
    kind: "apps" | "native" | "all",
    excludedCustomer?: string,
  ): Promise<{ [K in keyof HostingAllowances]: number }>;
  databasePoolCapacity(
    db: DatabaseReader,
    clusterID: string,
  ): Promise<
    {
      node: string;
      pool_id: string;
      capacity_bytes: string;
      held_cpu: number;
      held_memory: number;
      held_volume: number;
      known_cpu: number;
      known_memory: number;
      allocated: number;
    }[]
  >;
}
