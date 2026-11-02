import Link from "next/link";
import { getRequestI18n } from "@webdock/i18n/next";
import { getConsoleContext } from "@/lib/console-context";
import { AreaNavigation } from "./area-navigation";
import { TenantNavigation } from "./navigation";
import { MailNavigation, TenantDatabaseNavigation } from "@distribution/sidebar";

export async function WorkspaceSidebar({
  segments = [],
}: {
  segments?: string[];
}) {
  const context = await getConsoleContext(segments.join("/"));
  if (
    !context ||
    context.session.preview?.status === "expired" ||
    (!context.tenant && !context.name)
  )
    return null;
  const { t } = await getRequestI18n();
  const { session, tenant, customerID, name, links, parent, mail, databases } =
    context;
  const resources = (
    <>
      {MailNavigation && mail && session.installation?.modules.mail && session.installation.modules.mail !== "disabled" && (
        <div className="sidebar-resource">
          <p className="sidebar-context-name">{t("Mail")}</p>
          <MailNavigation
            customerID={customerID!}
            canManage={tenant?.canManage ?? false}
          />
        </div>
      )}
      {TenantDatabaseNavigation && databases && (
        <div className="sidebar-resource">
          <TenantDatabaseNavigation customerID={customerID!} />
        </div>
      )}
      {name && (
        <div className="sidebar-resource">
          {parent && (
            <Link className="sidebar-back" href={parent.href}>
              ‹ {parent.label}
            </Link>
          )}
          <p className="sidebar-context-name">{name}</p>
          <AreaNavigation label={t("Workspace areas")} links={links} />
        </div>
      )}
    </>
  );
  if (tenant && name && !mail && !databases)
    return (
      <div className="workspace-sidebar">
        {resources}
        <details className="workspace-services">
          <summary className="navigation-label">
            {t("Workspace services")}
          </summary>
          <TenantNavigation
            customerID={customerID!}
            canManage={tenant.canManage}
            preview={!!session.preview}
          />
        </details>
      </div>
    );
  return (
    <div className="workspace-sidebar">
      {tenant ? (
        <>
          <p className="navigation-label">{t("Services")}</p>
          <TenantNavigation
            customerID={customerID!}
            canManage={tenant.canManage}
            preview={!!session.preview}
          >
            {resources}
          </TenantNavigation>
        </>
      ) : (
        resources
      )}
    </div>
  );
}
