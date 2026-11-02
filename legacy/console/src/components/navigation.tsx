"use client";
import { lazy, Suspense } from "react";
import { useModuleNavigation, useModuleView } from "@webdock/module-sdk/react";
import {hasProductModules} from "@/lib/module-data";
import { moduleForPath } from "@webdock/module-sdk/selection";
import { productManifests } from "@distribution/navigation";
import { msgid } from "@webdock/i18n";

import { useI18n } from "@webdock/i18n/react";

import { ConsoleIcon, type ConsoleIconName } from "./console-icon";
import { NavigationPending } from "./navigation-pending";
import Link from "next/link";
import { usePathname, useRouter } from "next/navigation";
import { mainArea } from "@/lib/workspace-navigation";
import { AreaNavigation } from "./area-navigation";
import { useState, useSyncExternalStore } from "react";
const SelectedAuthControls = lazy(() =>
  import("./selected-auth-controls").then(module => ({ default: module.SelectedAuthControls })),
);
export function Navigation({
  operator = true,
  preview = false,
  accountProvider = "better-auth",
}: {
  accountProvider?: "clerk" | "better-auth";
  operator?: boolean;
  preview?: boolean;
}) {
  const view = useModuleView();
  const i18n = useI18n();

  const path = usePathname();
  const resourceOpen =
    /^\/(?:hosting\/(?:projects|apps|databases)|projects|sites)\/[1-9]/.test(
      path,
    );
  const active =
    path === "/hosting/developer" ? "developer" : mainArea(path, operator);
  const primary = operator
    ? [
        ["projects", "/", msgid("Projects")],
        ["customers", "/customers", msgid("Customers")],
        ...(view.modules.hosting ? [["operations", "/operations", msgid("Operations")]] : []),
      ]
    : [
        ["workspace", "/tenants", msgid("Workspaces")],
      ];
  const management = [
    ...(accountProvider === "clerk" ? [["people", "/admin/registry-users", msgid("Studio accounts")]] : [["people", "/people", msgid("People & access")]]),
    ...(hasProductModules(view) ? [["plans", "/admin/plans", msgid("Plans")]] : []),
    ...(["enabled", "draining"].includes(view.modules.hosting ?? "") ? [["connections", "/integrations", msgid("Connections")]] : []),
    ["settings", "/admin", msgid("Settings")],
  ];
  const icons: Record<string, ConsoleIconName> = {
    projects: "apps",
    customers: "team",
    operations: "overview",
    workspace: "overview",
    sites: "website",
    people: "team",
    plans: "billing",
    connections: "apps",
    settings: "settings",
  };
  const renderLinks = (links: string[][]) =>
    links.filter(([,href])=>{const module=moduleForPath(href);return !module || ["enabled","draining"].includes(view.modules[module]);}).map(([id, href, label]) => (
      <Link
        key={id}
        href={href}
        aria-current={
          active === id &&
          (!["workspace", "sites"].includes(id) || path === href)
            ? "page"
            : undefined
        }
      >
        <ConsoleIcon name={icons[id] || "overview"} />
        <span>{i18n.t(label)}</span>
        <NavigationPending />
      </Link>
    ));
  return (
    <div className="navigation-groups">
      <nav aria-label={i18n.t("Main navigation")}>
        {renderLinks(primary)}
        {!preview && view.modules.hosting !== "disabled" && view.modules.hosting !== undefined && (
          <Link
            href="/hosting/developer"
            aria-current={path === "/hosting/developer" ? "page" : undefined}
          >
            <ConsoleIcon name="code" />
            <span>{i18n.t("Developer tools")}</span>
            <NavigationPending />
          </Link>
        )}
      </nav>
      {operator && (
        <details className="navigation-management" open={!resourceOpen}>
          <summary className="navigation-label">
            {i18n.t("Administration")}
          </summary>
          <nav aria-label={i18n.t("Administration")}>
            {renderLinks(management)}
          </nav>
        </details>
      )}
    </div>
  );
}
export function TenantNavigation({
  customerID,
  canManage = false,
  preview = false,
  overviewHref,
  children,
}: {
  customerID: string;
  canManage?: boolean;
  preview?: boolean;
  overviewHref?: string;
  children?: React.ReactNode;
}) {
  const view = useModuleView();
  const contributions = useModuleNavigation(productManifests, "tenant.services");
  const i18n = useI18n();

  const base = `/tenants/${encodeURIComponent(customerID)}`;
  return (
    <>
      <AreaNavigation
        label={i18n.t("Workspace navigation")}
        links={[
          {
            href: overviewHref || base,
            label: msgid("Overview"),
            icon: "overview",
            exact: true,
          },
          {
            href: base + "/hosting",
            label: msgid("Applications"),
            icon: "apps",
          },
          {
            href: base + (preview ? "/managed-databases" : "/databases"),
            label: msgid("Databases"),
            icon: "database",
            activePaths: [base + "/managed-databases", base + "/databases"],
          },
          ...(!preview ? contributions.map(item => ({
            href: base + item.path, label: item.label, icon: "mail" as const,
          })) : []),
        ]}
      />
      {children}
      <div className="navigation-management">
        <p className="navigation-label">{i18n.t("Workspace settings")}</p>
        <AreaNavigation
          label={i18n.t("Workspace settings")}
          links={[
            ...(canManage
              ? [
                  {
                    href: base + "/team",
                    label: msgid("Team"),
                    icon: "team" as const,
                  },
                ]
              : []),
            ...(hasProductModules(view) ? [{ href: base + "/usage", label: msgid("Plan & usage"), icon: "billing" as const }] : []),
            {
              href: base + "/profile",
              label: msgid("Profile"),
              icon: "settings",
            },
          ]}
        />
      </div>
    </>
  );
}

const subscribeTheme = (notify: () => void) => {
  window.addEventListener("webdock-theme", notify);
  window.addEventListener("storage", notify);
  return () => {
    window.removeEventListener("webdock-theme", notify);
    window.removeEventListener("storage", notify);
  };
};
export function Appearance() {
  const i18n = useI18n();

  const theme = useSyncExternalStore(
    subscribeTheme,
    () => document.documentElement.dataset.theme || "system",
    () => "system",
  );
  return (
    <label className="appearance">
      {i18n.t("Appearance")}
      <select
        aria-label={i18n.t("Appearance")}
        value={theme}
        onChange={(e) => {
          const value = e.target.value;
          if (value === "system") delete document.documentElement.dataset.theme;
          else document.documentElement.dataset.theme = value;
          try {
            localStorage.setItem("webdock-theme", value);
          } catch {}
          window.dispatchEvent(new Event("webdock-theme"));
        }}
      >
        <option value="system">{i18n.t("System")}</option>
        <option value="light">{i18n.t("Light")}</option>
        <option value="dark">{i18n.t("Dark")}</option>
      </select>
    </label>
  );
}
export function Logout({ ssoEnabled = false, clerk = false }: { ssoEnabled?: boolean; clerk?: boolean }) {
  const i18n = useI18n();

  const router = useRouter();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState(false);
  if (clerk) return <Suspense fallback={null}><SelectedAuthControls /></Suspense>;
  if (ssoEnabled)
    return (
      <form action="/api/sso/logout" method="post">
        <button className="text-button" type="submit">
          {i18n.t("Sign out")}
        </button>
      </form>
    );
  return (
    <>
      <button
        className="text-button"
        disabled={busy}
        onClick={async () => {
          setBusy(true);
          try {
            const r = await fetch("/api/users/logout", { method: "POST" });
            if (!r.ok) throw Error();
            router.replace("/login");
            router.refresh();
          } catch {
            setBusy(false);
            setError(true);
          }
        }}
      >
        {i18n.t("Sign out")}
      </button>
      {error && (
        <small role="alert">{i18n.t("Sign out failed. Please retry.")}</small>
      )}
    </>
  );
}
