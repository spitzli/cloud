import { z } from "zod";

// Every reader of the shared ["tenants"] query must retain the same branding data.
export const tenantsSchema = z.array(z.object({
  id: z.string(),
  name: z.string(),
  icon: z.string().nullish(),
  status: z.string().optional(),
  role: z.string().nullish(),
}));
