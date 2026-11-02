import { createContext, useContext } from "react";
import type { ApiClient, Session } from "./api";
export const ApiContext = createContext<ApiClient | null>(null);
export const SessionContext = createContext<Session | null>(null);
export function useApi() {
  const api = useContext(ApiContext);
  if (!api) throw new Error("API is not configured");
  return api;
}
export function useSession() {
  const session = useContext(SessionContext);
  if (!session) throw new Error("Session is unavailable");
  return session;
}
