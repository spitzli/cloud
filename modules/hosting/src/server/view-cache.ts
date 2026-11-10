type JSONValue =
  null | boolean | number | string | JSONValue[] | { [key: string]: JSONValue };
import type { HostingActor } from "@spitzli/hosting-contracts";
export type ReadCachedView = <T>(
  options: {
    kind:
      | "branding"
      | "catalog"
      | "registry"
      | "operations"
      | "navigation"
      | "projectsummary";
    scope: Record<string, string | number | boolean | null>;
    query: JSONValue;
    ttlSeconds?: number;
  },
  revision: () => Promise<string>,
  load: () => Promise<T>,
) => Promise<T>;
import type { Connection } from "./authorization";

type Options = Parameters<ReadCachedView>[0] & { tables: string[] };

export function createHostingViewCache(readCachedView: ReadCachedView) {
  /** Call only after live resource authorization; cache keys never grant access. */
  function readHostingView<T>(
    db: Connection,
    actor: HostingActor,
    { tables, ...key }: Options,
    load: () => Promise<T>,
  ): Promise<T> {
    return readCachedView(
      {
        ...key,
        scope: {
          ...key.scope,
          subject: actor.subject,
          sessionID: actor.sessionID,
          source: actor.source,
          scopes: [...actor.scopes].sort().join(" "),
          gitSourceID: actor.gitSourceID ?? null,
          gitSourceRevision: actor.gitSourceRevision ?? null,
        },
      },
      async () =>
        (
          await db.query(
            "SELECT webdock_cache.version($1::text[]) AS version",
            [tables],
          )
        ).rows[0].version,
      load,
    );
  }

  return { readHostingView };
}
