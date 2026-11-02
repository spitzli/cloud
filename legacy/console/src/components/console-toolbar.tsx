import { AccountControls } from "./account-controls";
import { browserAuthConfiguration } from "@/lib/browser-auth-config";
import { getRequestI18n } from "@webdock/i18n/next";
import { LanguagePicker } from "@webdock/i18n/picker";
import { getConsoleContext } from "@/lib/console-context";
import { ConsoleControls } from "./console-controls";
import { Navigation, Appearance, Logout } from "./navigation";
import { WorkspaceSidebar } from "./workspace-sidebar";

export async function ConsoleToolbar({
  segments = [],
}: {
  segments?: string[];
}) {
  const context = await getConsoleContext(segments.join("/"));
  if (!context) return null;
  const { session, tenant, customerID, projectID, projectName, name, tenants } =
    context;
  const { t } = await getRequestI18n();
  const accountURL =
    browserAuthConfiguration.provider === "clerk"
      ? "/account"
      : new URL(
          "/account",
          process.env.WEBDOCK_AUTH_ISSUER ||
            "https://auth.webdock.dev/api/auth",
        ).href;
  return (
    <ConsoleControls
      key={`${customerID || ""}:${projectID || ""}:${name || ""}:${session.preview?.status || ""}`}
      scope={{
        customerID,
        projectID,
        customerName: tenant?.tenant.name,
        resourceName: projectName || name,
      }}
      workspaces={tenants.map((item) => ({ id: item.id, name: item.name }))}
      operator={session.operator}
      preview={!!session.preview}
      navigation={
        <>
          <Navigation accountProvider={browserAuthConfiguration.provider} operator={session.operator} preview={!!session.preview} />
          <WorkspaceSidebar segments={segments} />
        </>
      }
      account={browserAuthConfiguration.provider === "clerk" ? <AccountControls preview={!!session.preview} operator={session.operator} /> :
        <details className="console-account-menu">
          <summary aria-label={t("Account & preferences")}>
            <span className="console-avatar" aria-hidden="true">
              {session.user.name.slice(0, 1).toLocaleUpperCase()}
            </span>
            <span>{session.user.name}</span>
          </summary>
          <div>
            <strong>{session.user.name}</strong>
            <small>
              {session.operator ? t("Operator studio") : t("Customer studio")}
            </small>
            {!session.preview && (
              <a href={accountURL}>{t("Account & security ↗")}</a>
            )}
            <Appearance />
            <LanguagePicker className="studio-language-picker" />
            <Logout
              ssoEnabled
            />
          </div>
        </details>
      }
    />
  );
}
