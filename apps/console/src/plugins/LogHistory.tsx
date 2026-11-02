import { useInfiniteQuery } from "@tanstack/react-query";
import { useState } from "react";
import { z } from "zod";
import { useApi } from "../context";
import * as ui from "../ui";

const lineSchema = z.object({ id: z.string(), at: z.string(), unit: z.string(), line: z.string() });
/** The last 3 days of a resource's logs as Core stored them, newest first, searchable. Logs often
 *  carry configuration values, so only owners, admins and operators see them. */
export function LogHistory({ viewer, ...props }: { projectID: string; plugin: string; resource: string; viewer: { role?: string | null } | undefined }) {
  return ["owner", "admin", "operator"].includes(viewer?.role ?? "") ? <History {...props} /> : null;
}
function History({ projectID, plugin, resource }: { projectID: string; plugin: string; resource: string }) {
  const api = useApi();
  const [search, setSearch] = useState("");
  const [query, setQuery] = useState("");
  const logs = useInfiniteQuery({
    queryKey: ["log-history", projectID, plugin, resource, query],
    initialPageParam: "",
    queryFn: ({ pageParam, signal }) => {
      const params = new URLSearchParams({ search: query, ...(pageParam ? { before: pageParam } : {}) });
      return api.get(`/api/console/projects/${projectID}/plugins/${plugin}/resources/${encodeURIComponent(resource)}/logs?${params.toString()}`, z.array(lineSchema), signal);
    },
    getNextPageParam: (last) => (last.length === 500 ? last.at(-1)?.id : undefined),
  });
  const lines = logs.data?.pages.flat() ?? [];
  if (!query && logs.isSuccess && !lines.length) return null;
  return (
    <section className={ui.cx(ui.card, "mt-6")} aria-label="Log history">
      <form
        className="flex flex-wrap items-end justify-between gap-3 px-6 py-4 max-sm:px-[18px]"
        onSubmit={(event) => {
          event.preventDefault();
          setQuery(search.trim());
        }}
      >
        <div>
          <h2 className={ui.sectionTitle}>Log history</h2>
          <p className={ui.hint}>The last 3 days, newest first.</p>
        </div>
        <div className="flex gap-2">
          <input className={ui.cx(ui.input, "w-56")} aria-label="Search logs" placeholder="Search" maxLength={200} value={search} onChange={(e) => { setSearch(e.target.value); }} />
          <button className={ui.button()}>Search</button>
        </div>
      </form>
      {logs.isError ? <p role="alert" className="px-6 pb-4 text-sm text-danger">{logs.error.message}</p> : null}
      <ol className="max-h-[32rem] overflow-auto border-t border-outline bg-subtle px-6 py-3 font-mono text-xs leading-5 max-sm:px-[18px]">
        {lines.map((l) => (
          <li key={l.id} className="flex gap-3 [overflow-wrap:anywhere]">
            <time className="shrink-0 text-muted" dateTime={l.at}>{new Date(l.at).toLocaleString()}</time>
            <span>{l.line}</span>
          </li>
        ))}
        {!lines.length && logs.isSuccess ? <li className="text-muted">No lines match.</li> : null}
        {logs.isPending ? <li className="text-muted">Loading…</li> : null}
      </ol>
      {logs.hasNextPage ? (
        <div className={ui.cardFooter}>
          <span />
          <button type="button" className={ui.button()} disabled={logs.isFetchingNextPage} onClick={() => void logs.fetchNextPage()}>Load older</button>
        </div>
      ) : null}
    </section>
  );
}
