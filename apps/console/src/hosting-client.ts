import { useCallback } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { commandSchema, type HostingCommand } from "@webdock/hosting-contracts";
import type { z } from "zod";
import { useApi } from "./context";
import { ApiError } from "./api";
import { receiptSchema } from "./hosting-contracts";
export function uncertainOutcome(error: Error | null): boolean {
  return (
    error !== null && (!(error instanceof ApiError) || error.status >= 500)
  );
}
export function useHostingCommand() {
  const api = useApi();
  return useCallback(
    <T>(input: unknown, schema: z.ZodType<T>, signal?: AbortSignal) =>
      api.request("/api/console/hosting", schema, {
        method: "POST",
        body: JSON.stringify(commandSchema.parse(input)),
        signal: signal ?? null,
      }),
    [api],
  );
}
export function useHostingAction(onRecorded?: () => void) {
  const call = useHostingCommand();
  const client = useQueryClient();
  const mutation = useMutation({
    mutationFn: (command: HostingCommand) => call(command, receiptSchema),
    retry: false,
    gcTime: 0,
    onSuccess: () => {
      onRecorded?.();
      void client.invalidateQueries({ queryKey: ["hosting"] });
    },
  });
  const retry = () => {
    if (mutation.variables) mutation.mutate(mutation.variables);
  };
  return { mutation, retry, uncertain: uncertainOutcome(mutation.error) };
}
