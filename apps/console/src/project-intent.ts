import { useEffect, useRef } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useApi, useSession } from "./context";
import { prefetchProjectSummary } from "./project-query";
import { sessionCacheScope } from "./session-cache";

export function useProjectIntent() {
  const api = useApi(), session = useSession(), client = useQueryClient();
  const scope = sessionCacheScope(session);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const cancel = () => { clearTimeout(timer.current); timer.current = undefined; };
  useEffect(() => () => { clearTimeout(timer.current); }, [scope]);
  const warm = (projectID: string) => { void prefetchProjectSummary(client, api, session, projectID); };
  return {
    hover(projectID: string) { cancel(); timer.current = setTimeout(() => { timer.current = undefined; warm(projectID); }, 125); },
    focus(projectID: string) { cancel(); warm(projectID); },
    cancel,
  };
}
