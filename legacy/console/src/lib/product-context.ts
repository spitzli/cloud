import type { AreaLink } from "@/components/area-navigation";
export type ProductContextReader = (resource: string, id: string, t: (message: string) => string) => Promise<{
 customerID?: string; selectedProjectID?: string; selectedProjectName?: string; name?: string; links: AreaLink[]; parent?: {label: string; href: string};
}>;
