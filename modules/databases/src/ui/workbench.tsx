"use client";
import * as React from "react";
import { resourceScope } from "@webdock/console-kit/resource-scope";

import { useEffect, useState, useSyncExternalStore } from "react";
import { HttpTransport, TabularisClient, TabularisWorkspace as Workspace, preloadTabularisWorkspace } from "@tabularis/web-ui";
import "@tabularis/web-ui/workspace.css";
import { useI18n } from "@webdock/i18n/react";
import { useDatabaseActions, type DatabaseUIActions } from "./actions";
import { rememberDatabaseSchemas } from "./schema-preferences";

const theme = () =>
  document.documentElement.dataset.theme === "light"
    ? "light"
    : document.documentElement.dataset.theme === "dark"
      ? "dark"
      : matchMedia("(prefers-color-scheme: dark)").matches
        ? "dark"
        : "light";
const subscribeTheme = (notify: () => void) => {
  const observer = new MutationObserver(notify);
  observer.observe(document.documentElement, {
    attributes: true,
    attributeFilter: ["data-theme"],
  });
  const media = matchMedia("(prefers-color-scheme: dark)");
  media.addEventListener("change", notify);
  return () => {
    observer.disconnect();
    media.removeEventListener("change", notify);
  };
};

function DatabaseWorkbenchState({
  bindingID,
  userID,
  managed = false,
  actions,
}: {
  bindingID: string;
  userID: string;
  managed?: boolean;
  actions: Pick<DatabaseUIActions, "launchDatabase" | "launchManagedDatabase">;
}) {
  const { launchDatabase, launchManagedDatabase } = actions;
  const i18n = useI18n();
  const appearance = useSyncExternalStore(subscribeTheme, theme, () => "light");
  const [workspace, setWorkspace] = useState<{
    scopeID: string;
    connectionID: string;
    client: TabularisClient;
  }>();
  const [error, setError] = useState("");
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    void preloadTabularisWorkspace().catch(() => undefined);
    let active = true;
    let close: (() => void) | undefined;
    const controller = new AbortController();
    const deadline = setTimeout(() => { controller.abort(); }, 30_000);
    // Server actions cannot be cancelled, so also fence their late results.
    const cancelled = new Promise<never>((_, reject) => {
      controller.signal.addEventListener("abort", () => { reject(new Error("Database access is unavailable.")); }, { once: true });
    });
    void Promise.race([(async () => {
      const result = await (managed
        ? launchManagedDatabase(bindingID)
        : launchDatabase(bindingID));
      controller.signal.throwIfAborted();
      if (!result.launch)
        throw Error("Database access is unavailable.");
      const response = await fetch(result.launch.gatewayOrigin + "/session", {
        method: "POST",
        credentials: "include",
        redirect: "error",
        signal: controller.signal,
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ code: result.launch.code }),
      });
      if (!response.ok) throw Error("Database access is unavailable.");
      const session: unknown = await response.json();
      controller.signal.throwIfAborted();
      if (
        !session || typeof session !== "object" ||
        !("scopeID" in session) || !("connectionID" in session) || !("baseUrl" in session) ||
        typeof session.scopeID !== "string" ||
        typeof session.connectionID !== "string" ||
        typeof session.baseUrl !== "string" ||
        session.baseUrl !==
          `${result.launch.gatewayOrigin}/s/${session.scopeID}`
      )
        throw Error("Invalid database response.");
      const baseUrl = session.baseUrl;
      const transport = new HttpTransport({
        baseUrl,
        fetch: (input, init) =>
          fetch(input, {
            ...init,
            credentials: "include",
            signal: init?.signal
              ? AbortSignal.any([controller.signal, init.signal])
              : controller.signal,
          }),
      });
      const negotiation = await transport.initialize();
      close = () => {
        close = undefined;
        void fetch(baseUrl + "/api/v1/logout", {
          method: "POST",
          credentials: "include",
          keepalive: true,
          headers: { "X-Tabularis-Csrf": negotiation.csrfToken },
        }).catch(() => undefined);
      };
      if (controller.signal.aborted) {
        close();
        return;
      }
      const client = new TabularisClient(transport);
      rememberDatabaseSchemas(client, {
        userID,
        bindingID,
        kind: managed ? "managed" : "registered",
        connectionID: session.connectionID,
      });
      setWorkspace({
        scopeID: session.scopeID,
        connectionID: session.connectionID,
        client,
      });
    })(), cancelled]).catch(() => {
      controller.abort();
      close?.();
      if (active) setError("Database access is unavailable.");
    }).finally(() => { clearTimeout(deadline); });
    return () => {
      active = false;
      clearTimeout(deadline);
      controller.abort();
      close?.();
    };
  }, [bindingID, userID, managed, launchDatabase, launchManagedDatabase, attempt]);
  if (error)
    return (
      <div className="notice error" role="alert">
        <p>{i18n.error(error)}</p>
        <button className="button secondary" onClick={() => {
          setError("");
          setWorkspace(undefined);
          setAttempt(value => value + 1);
        }}>
          {i18n.t("Try again")}
        </button>
      </div>
    );
  if (!workspace)
    return <p role="status">{i18n.t("Connecting to database…")}</p>;
  return (
    <div className="database-workbench">
      <Workspace
        className="spitzli-database-workspace"
        client={workspace.client}
        scopeId={workspace.scopeID}
        connectionId={workspace.connectionID}
        locale={i18n.locale}
        theme={appearance as "light" | "dark"}
      />
    </div>
  );
}

/** Explicit launch boundary for native browser facades; no unrelated provisioning stubs. */
export function DatabaseWorkbenchWithActions(props: React.ComponentProps<typeof DatabaseWorkbenchState>) {
  return <DatabaseWorkbenchState key={resourceScope(props)} {...props} />;
}
export function DatabaseWorkbench(props: Omit<React.ComponentProps<typeof DatabaseWorkbenchState>, "actions">) {
  const actions = useDatabaseActions();
  return <DatabaseWorkbenchWithActions {...props} actions={actions} />;
}
