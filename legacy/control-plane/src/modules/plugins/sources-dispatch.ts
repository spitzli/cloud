import { z } from "zod";
import type { SourceSession } from "./sources-identity";
import { sourceFail } from "./sources-policy";
export type ReadySource = {
  id: string;
  actor_scope: SourceSession;
  project_id: string;
  customer_id: string;
};

/** Selection commits before crossing into fresh operator authority or probing a runtime. */
export function createPluginInstallerDispatch(dependencies: {
  nextReady(): Promise<ReadySource | undefined>;
  withActor<T>(job: ReadySource, work: () => Promise<T>): Promise<T>;
  activate(id: string): Promise<unknown>;
  worker(action: string, input: unknown): Promise<unknown>;
}) {
  return async (action: string, input: unknown) => {
    if (action === "claim") {
      if (!z.strictObject({}).safeParse(input).success)
        sourceFail("invalid_input", "Invalid plugin installer request.", 400);
      const ready = await dependencies.nextReady();
      if (ready) {
        try {
          await dependencies.withActor(ready, () =>
            dependencies.activate(ready.id),
          );
        } catch {
          // Keep ready imports for fresh owner authority or a later runtime-health observation.
        }
      }
    }
    return dependencies.worker(action, input);
  };
}
