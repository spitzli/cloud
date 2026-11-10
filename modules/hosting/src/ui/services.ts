import type { ComponentType } from "react";
import type {
  HostingCommand,
  HostingProjectContext,
} from "@spitzli/hosting-contracts";
export interface HostingUIServices {
  hostingCall<T>(command: HostingCommand): Promise<T>;
  hostingPageCall<T>(command: HostingCommand, returnTo: string): Promise<T>;
  applicationState(status: string): string;
  getAvailableProjects(
    customerID: string,
  ): Promise<{ id: string; name: string }[]>;
  getHostingProject(projectID: string): Promise<HostingProjectContext>;
  getStudioSession(): Promise<{ preview?: unknown } | null>;
  mailCall<T>(operation: string, customerID: string): Promise<T>;
  OwnInfrastructure: ComponentType<{ customerID: string; path: string }>;
  AreaNavigation: ComponentType<{
    label?: string;
    links: { href: string; label: string; exact?: boolean }[];
  }>;
  MailProductForm: ComponentType<{
    customer: string;
    connection?: string;
    operation: string;
    label: string;
    hidden?: Record<string, string>;
    fields?: {
      name: string;
      label: string;
      type?: "text" | "password";
      required?: boolean;
    }[];
  }>;
}
