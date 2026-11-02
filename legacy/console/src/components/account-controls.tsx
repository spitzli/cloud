"use client";
import { OrganizationSwitcher, UserButton } from "@clerk/nextjs";
import { useI18n } from "@webdock/i18n/react";
import { ConsoleIcon } from "./console-icon";

export function AccountControls({ preview = false, operator = false }: { preview?: boolean; operator?: boolean }) {
  const { t } = useI18n();
  return (
    <div className="console-clerk-account">
      {!preview && (operator ? (
        <span className="console-operator-context"><ConsoleIcon name="overview" />{t("Operator area")}</span>
      ) : (
        <div className="console-workspace-menu" role="group" aria-label={t("Workspaces")}>
          <ConsoleIcon name="apps" />
          <OrganizationSwitcher
            hidePersonal={true}
            afterSelectOrganizationUrl="/tenants"
            afterSelectPersonalUrl="/tenants"
            appearance={{ elements: {
              organizationSwitcherTrigger: { "& .cl-avatarBox": { display: "none" } },
            } }}
          />
        </div>
      ))}
      <div className="console-profile-menu" role="group" aria-label={t("Account & preferences")} title={t("Account & preferences")}>
        <UserButton userProfileMode="navigation" userProfileUrl="/account" />
      </div>
    </div>
  );
}
