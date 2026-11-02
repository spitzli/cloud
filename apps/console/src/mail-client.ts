import { skipToken, useQuery } from "@tanstack/react-query";
import { tenantsSchema } from "./hosting-contracts";
import type { MailDirectory, MailService } from "./mail-contracts";
import { useCallback } from "react";
import type { z } from "zod";
import { useApi, useSession } from "./context";
import { commandSchema, type MailCommand } from "./mail-contracts";
export function useMailCommand() {
  const api = useApi();
  return useCallback(
    <T>(command: MailCommand, schema: z.ZodType<T>, signal?: AbortSignal) =>
      api.request("/api/console/mail", schema, {
        method: "POST",
        body: JSON.stringify(commandSchema.parse(command)),
        signal: signal ?? null,
      }),
    [api],
  );
}

export function useMailWorkspace(
  active: boolean,
  customerID: string | undefined,
) {
  const api = useApi();
  const session = useSession();
  const tenants = useQuery({
    queryKey: ["tenants"],
    queryFn: ({ signal }) =>
      api.get("/api/console/tenants", tenantsSchema, signal),
    enabled: active && api.config.enabledFeatures.includes("mail"),
    staleTime: 0,
    refetchOnMount: "always",
  });
  const service = useQuery<MailService>({
    queryKey: ["mail", session.session.id, customerID, "service"],
    queryFn: skipToken,
    enabled: false,
  });
  const directory = useQuery<MailDirectory>({
    queryKey: ["mail", session.session.id, customerID, "directory"],
    queryFn: skipToken,
    enabled: false,
  });
  const failed = tenants.isError || service.isError || directory.isError;
  const tenant =
    !failed && tenants.isFetchedAfterMount
      ? tenants.data?.find((value) => value.id === customerID)
      : undefined;
  return {
    active,
    customer: tenant,
    label: failed
      ? "Workspace unavailable"
      : (tenant?.name ?? "Mail workspaces"),
  };
}
