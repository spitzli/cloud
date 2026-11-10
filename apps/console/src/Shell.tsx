import { useI18n } from "@webdock/i18n/react";
import { WorkspaceAvatar } from "./WorkspaceAvatar";
import { CustomerScopeBanner } from "./CustomerScope";
import { Fragment, useEffect, useRef, useState } from "react";
import {
  Link,
  Outlet,
  useNavigate,
  useRouterState,
} from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { z } from "zod";
import { EnvelopeIcon } from "@phosphor-icons/react/dist/csr/Envelope";
import { BuildingsIcon } from "@phosphor-icons/react/dist/csr/Buildings";
import { FolderSimpleIcon } from "@phosphor-icons/react/dist/csr/FolderSimple";
import { ClockCounterClockwiseIcon } from "@phosphor-icons/react/dist/csr/ClockCounterClockwise";
import { MagnifyingGlassIcon } from "@phosphor-icons/react/dist/csr/MagnifyingGlass";
import { ListIcon } from "@phosphor-icons/react/dist/csr/List";
import { ThemeButton } from "./theme";
import { UserMenu } from "./UserMenu";
import { UserCircleIcon } from "@phosphor-icons/react/dist/csr/UserCircle";
import { CommandIcon } from "@phosphor-icons/react/dist/csr/Command";
import { CaretUpDownIcon } from "@phosphor-icons/react/dist/csr/CaretUpDown";
import { GaugeIcon } from "@phosphor-icons/react/dist/csr/Gauge";
import { HardDrivesIcon } from "@phosphor-icons/react/dist/csr/HardDrives";
import { PlugsConnectedIcon } from "@phosphor-icons/react/dist/csr/PlugsConnected";
import { useApi, useSession } from "./context";
import { LocalePicker } from "./Locale";
import { parseConsoleSearch } from "./search";
import { Empty, Failure, Loading, Modal } from "./common";
import { isReadOnlyPreview, listSchema, recordSchema, type Session } from "./api";
import { useMailWorkspace } from "./mail-client";
import { useHostingContext } from "./hosting-context";
import * as ui from "./ui";
import { useWorkspaceProjects } from "./plugins/workspace";
import { WorkspaceNav, useTenants, useWorkspaceHeader } from "./WorkspaceNav";
const crumbSeparator = "select-none px-1.5 text-[22px] font-light text-outline-strong";
const paletteItem =
  "flex w-full items-center gap-3 rounded-md px-3 py-2.5 text-left text-foreground hover:bg-soft focus-visible:bg-soft";
const navItem = (selected: boolean) =>
  `my-px flex min-h-11 items-center gap-2.5 rounded-md px-2.5 text-sm [overflow-wrap:anywhere] hover:bg-soft hover:text-foreground hover:no-underline md:min-h-[34px] [&_svg]:shrink-0 ${selected ? "bg-soft font-medium text-foreground" : "text-muted"}`;
const destinations = [
  { collection: "projects", label: "Projects", icon: FolderSimpleIcon },
  { collection: "customers", label: "Customers", icon: BuildingsIcon },
  { collection: "access-requests", label: "Waitlist", icon: EnvelopeIcon },
  {
    collection: "audit-events",
    label: "Audit trail",
    icon: ClockCounterClockwiseIcon,
  },
];
const operatorDestination = (collection: string) => collection === "access-requests"
  ? { to: "/access-requests" as const }
  : { to: "/$collection" as const, params: { collection } };
function shellLayout(viewer: boolean) {
  return viewer ? {
    frame: "grid h-dvh grid-cols-[minmax(0,1fr)] grid-rows-[56px_minmax(0,1fr)] overflow-hidden",
    main: "min-h-0 w-full flex-1 outline-none",
    sidebar: "hidden",
  } : {
    frame: "grid min-h-dvh grid-cols-[minmax(0,1fr)] grid-rows-[56px_1fr] md:grid-cols-[216px_minmax(0,1fr)] lg:grid-cols-[224px_minmax(0,1fr)]",
    main: "mx-auto w-full max-w-[1200px] flex-1 px-4 pt-5 pb-10 outline-none md:px-6 md:pt-6 md:pb-12 lg:px-8 lg:pt-8 lg:pb-14",
    sidebar: "flex",
  };
}
export function Shell() {
  const { t } = useI18n();
  const session = useSession();
  const navigationCopy = session.user.role === "operator"
    ? { section: "Workspace", switch: "Switch customer or project", search: "Find a project or customer" }
    : { section: "Switch", switch: "Switch workspace", search: t("Find a workspace or project") };
  const api = useApi();
  const queryClient = useQueryClient();
  const pathname = useRouterState({
    select: (state) => state.location.pathname,
  });
  const layout = shellLayout(/^\/databases\/[^/]+\/?$/.test(pathname));
  const searchString = useRouterState({
    select: (state) => state.location.searchStr,
  });
  const hosting = useHostingContext(
    pathname,
    parseConsoleSearch(searchString).customer,
  );
  const mail = useMailWorkspace(
    pathname === "/mail" || pathname.startsWith("/mail/"),
    parseConsoleSearch(searchString).customer,
  );
  const projectId = /^\/projects\/([1-9][0-9]{0,18})$/.exec(pathname)?.[1];
  const project = useQuery({
    queryKey: ["registry", "projects", projectId],
    queryFn: ({ signal }) =>
      api.get(
        `/v1/operator/registry/projects/${projectId ?? ""}`,
        recordSchema,
        signal,
      ),
    enabled:
      !hosting.active &&
      !mail.active &&
      Boolean(projectId) &&
      session.user.role === "operator",
  });
  const customerId = currentCustomer(
    hosting,
    mail,
    pathname,
    project.data?.customer,
    searchString,
  );
  const customer = useQuery({
    queryKey: ["registry", "customers", customerId],
    queryFn: ({ signal }) =>
      api.get(
        `/v1/operator/registry/customers/${customerId ?? ""}`,
        recordSchema,
        signal,
      ),
    enabled:
      !hosting.active &&
      !mail.active &&
      Boolean(customerId) &&
      session.user.role === "operator",
  });
  const tenants = useTenants();
  const workspace = workspaceIdentity(
    hosting,
    customerId,
    workspaceName(customer.data, tenants.data, customerId),
    customer.isError,
    session,
    mail,
  );
  const header = useWorkspaceHeader(pathname);
  const identity = header ?? { ...workspace, icon: workspaceIcon(tenants.data, customerId) };
  const [palette, setPalette] = useState(false);
  const [mobile, setMobile] = useState(false);
  useEffect(() => {
    const listener = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        setPalette((value) => !value);
      }
    };
    window.addEventListener("keydown", listener);
    return () => {
      window.removeEventListener("keydown", listener);
    };
  }, []);
  const [previousPath, setPreviousPath] = useState(pathname);
  if (previousPath !== pathname) {
    setPreviousPath(pathname);
    setMobile(false);
    setPalette(false);
  }
  const logout = useMutation({
    mutationFn: () =>
      api.config.sso
        ? api.write(
            "/api/sso/logout",
            "POST",
            {},
            z.object({ redirect: z.url() }),
          )
        : api.write(
            "/api/auth/sign-out",
            "POST",
            {},
            z.object({ redirect: z.string().optional() }),
            true,
          ),
    onSuccess: (value) => {
      queryClient.clear();
      window.location.assign(value.redirect?.trim() ? value.redirect : "/");
    },
  });
  const workspaceNav = header?.customer;
  const navigation = (
    <>
      {hosting.project ? (
        <div className="mb-[18px]">
          <p className="px-2.5 py-1.5 text-xs text-muted">Current project</p>
          <Link
            to="/hosting/projects/$projectID"
            params={{ projectID: hosting.project.projectID }}
            className={navItem(false)}
          >
            <FolderSimpleIcon size={18} />
            {hosting.project.name}
          </Link>
          {hosting.application ? (
            <Link
              to="/hosting/apps/$appID"
              params={{ appID: hosting.application.id }}
              className={navItem(true)}
            >
              {hosting.application.name}
            </Link>
          ) : null}
        </div>
      ) : null}
      {!hosting.active && projectId && project.data ? (
        <div className="mb-[18px]">
          <p className="px-2.5 py-1.5 text-xs text-muted">Current project</p>
          <Link
            to="/$collection/$id"
            params={{ collection: "projects", id: projectId }}
            className={navItem(true)}
          >
            <FolderSimpleIcon size={18} />
            {project.data.name}
          </Link>
        </div>
      ) : null}
      {workspaceNav ? <WorkspaceNav key={workspaceNav} customer={workspaceNav} item={navItem} /> : null}
      <div className="mb-[18px]">
        <p className="px-2.5 py-1.5 text-xs text-muted">{navigationCopy.section}</p>
        {session.user.role === "operator" ? (
          destinations.map(({ collection, label, icon: Icon }) => (
            <Link
              key={collection}
              {...operatorDestination(collection)}
              search={collection === "projects" ? { customer: customerId } : {}}
              className={navItem(pathname.startsWith(`/${collection}`))}
              aria-current={
                pathname.startsWith(`/${collection}`) ? "page" : undefined
              }
            >
              <Icon size={18} />
              {label}
            </Link>
          ))
        ) : (
          <Link
            to="/$collection"
            params={{ collection: "tenants" }}
            className={navItem(pathname === "/tenants" || pathname === "/workspaces")}
          >
            <BuildingsIcon size={18} />
            All workspaces
          </Link>
        )}
      </div>
      <div className="mb-[18px]">
        <p className="px-2.5 py-1.5 text-xs text-muted">Manage</p>
        {api.config.enabledFeatures.includes("hosting") ? (
          <Link
            to="/hosting"
            search={{ customer: customerId }}
            className={navItem(pathname.startsWith("/hosting"))}
          >
            <FolderSimpleIcon size={18} />
            Applications
          </Link>
        ) : null}
        {api.config.enabledFeatures.includes("mail") ? (
          <Link
            to="/mail"
            search={{ customer: customerId }}
            className={navItem(pathname.startsWith("/mail"))}
          >
            <EnvelopeIcon size={18} />
            Mail
          </Link>
        ) : null}
        {api.config.enabledFeatures.includes("databases") &&
        !isReadOnlyPreview(session) ? (
          <Link
            to="/databases"
            search={{ customer: customerId }}
            className={navItem(pathname.startsWith("/databases"))}
          >
            <FolderSimpleIcon size={18} />
            Databases
          </Link>
        ) : null}
        {session.user.role === "operator" ? (
          <Link
            to="/extensions"
            className={navItem(pathname.startsWith("/extensions"))}
          >
            <PlugsConnectedIcon size={18} />
            Extensions
          </Link>
        ) : null}
        {session.user.role === "operator" ? (
          <Link to="/plans" className={navItem(pathname.startsWith("/plans"))}>
            <GaugeIcon size={18} />
            Plans
          </Link>
        ) : null}
        {session.user.role === "operator" ? (
          <Link to="/clusters" className={navItem(pathname.startsWith("/clusters"))}>
            <HardDrivesIcon size={18} />
            Clusters
          </Link>
        ) : null}
        <Link
          to="/account"
          className={navItem(pathname === "/account")}
        >
          <UserCircleIcon size={18} />
          Account
        </Link>
      </div>
    </>
  );
  return (
    <div className={layout.frame}>
      <a className="fixed -top-24 left-4 z-50 border border-focus bg-surface px-4 py-2.5 focus:top-2.5" href="#main">
        Skip to content
      </a>
      <header className="sticky top-0 z-10 col-span-full flex items-center gap-1 border-b border-outline bg-page px-3 md:gap-3 md:px-4">
        <Link to="/" className="shrink-0 hover:no-underline" aria-label="Spitzli Cloud Console home">
          <span className={ui.brandMark} aria-hidden>
            <img src="/spitzli-logo.png" alt="" width={24} height={24} />
          </span>
        </Link>
        <nav className="flex min-w-0 items-center gap-0.5 text-sm text-muted [&>span:last-child]:font-medium [&>span:last-child]:text-foreground" aria-label="Breadcrumb">
          <span className={crumbSeparator} aria-hidden>
            /
          </span>
          <button
            className="flex h-8 min-w-0 items-center gap-2 rounded-md pr-1.5 pl-1 font-medium text-foreground hover:bg-soft"
            onClick={() => {
              setPalette(true);
            }}
            aria-label={navigationCopy.switch}
          >
            <WorkspaceAvatar name={identity.label} icon={identity.icon} size="size-[22px] text-[11px]" />
            <span className="min-w-0 truncate">{identity.label}</span>
            <span className="rounded-full border border-outline px-1.5 text-xs leading-[18px] font-normal text-muted max-md:hidden">{identity.detail}</span>
            <CaretUpDownIcon size={14} className="shrink-0 text-muted" />
          </button>
          {(hosting.active
            ? hosting.breadcrumbs
            : header?.crumbs ?? [customer.data?.name ?? "Workspace", pathname === "/access-requests" ? t("Waitlist") : sectionTitle(pathname)]
          )
            .slice(1)
            .map((label, index) => (
              <Fragment key={`${String(index)}:${label}`}>
                <span className={`${crumbSeparator} max-md:hidden`} aria-hidden>
                  /
                </span>
                <BreadcrumbLabel header={header} index={index} pathname={pathname} label={label} />
              </Fragment>
            ))}
        </nav>
        <div className="ml-auto flex items-center gap-0 md:gap-1">
          <button
            className="flex size-9 items-center justify-center gap-2 rounded-md border border-outline-strong bg-surface text-[13px] text-muted hover:border-muted lg:mr-2 lg:h-8 lg:w-[220px] lg:justify-start lg:px-2"
            aria-label={navigationCopy.search}
            onClick={() => {
              setPalette(true);
            }}
          >
            <MagnifyingGlassIcon size={16} />
            <span className="max-lg:hidden">Find…</span>
            <kbd className="ml-auto inline-flex items-center gap-px rounded border border-outline px-1 font-sans text-[11px] leading-[18px] max-lg:hidden [&_svg]:w-[11px]">
              <CommandIcon />K
            </kbd>
          </button>
          <LocalePicker />
          <ThemeButton />
          <UserMenu
            name={session.user.name}
            email={session.user.email}
            accountHref={
              api.config.sso
                ? new URL("/account", api.config.authOrigin).href
                : "/account"
            }
            signOut={() => {
              logout.mutate();
            }}
          />
          <button
            className={ui.cx(`${ui.button("quiet", "icon")} md:hidden`)}
            aria-label="Open navigation"
            onClick={() => {
              setMobile(true);
            }}
          >
            <ListIcon size={20} />
          </button>
        </div>
      </header>
      <aside className={ui.cx("sticky top-14 h-[calc(100dvh-56px)] flex-col self-start overflow-y-auto border-r border-outline bg-subtle px-2 py-3 max-md:hidden", layout.sidebar)}>
        <nav aria-label="Primary navigation">{navigation}</nav>
        {logout.isError ? (
          <p role="alert" className="mt-auto px-2.5 text-[13px] text-danger">
            {logout.error.message}
          </p>
        ) : null}
      </aside>
      <div className="flex min-h-0 min-w-0 flex-col">
        <CustomerScopeBanner />
        <main id="main" tabIndex={-1} className={layout.main}>
          <Outlet />
        </main>
      </div>
      {mobile ? (
        <Modal
          title="Navigation"
          close={() => {
            setMobile(false);
          }}
        >
          <nav
            aria-label="Mobile navigation"
            onClick={() => {
              setMobile(false);
            }}
          >
            {navigation}
          </nav>
          <button
            className={ui.cx(`${ui.button()} mt-3 w-full`)}
            onClick={() => {
              logout.mutate();
            }}
            disabled={logout.isPending}
          >
            Sign out
          </button>
          {logout.isError ? (
            <p role="alert" className="text-danger">
              {logout.error.message}
            </p>
          ) : null}
        </Modal>
      ) : null}
      {palette ? (
        <CommandPalette customer={workspaceNav}
          close={() => {
            setPalette(false);
          }}
        />
      ) : null}
    </div>
  );
}
function BreadcrumbLabel({ header, index, pathname, label }: { header: ReturnType<typeof useWorkspaceHeader>; index: number; pathname: string; label: string }) {
  const style = "min-w-0 truncate max-md:hidden";
  if (header?.project && index === 0 && pathname.includes("/resources/")) return <Link
    to="/workspaces/$customer/projects/$id" params={{ customer: header.customer, id: header.project.id }}
    className={style} title={label}
  >{label}</Link>;
  return <span className={style} title={label}>{label}</span>;
}

function CommandPalette({ close, customer }: { close: () => void; customer: string | undefined }) {
  const { t } = useI18n();
  const api = useApi();
  const session = useSession();
  const navigate = useNavigate();
  const searchInput = useRef<HTMLInputElement>(null);
  useEffect(() => { searchInput.current?.focus(); }, []);
  const [text, setText] = useState("");
  const [search, setSearch] = useState("");
  useEffect(() => {
    const timer = setTimeout(() => {
      setSearch(text.trim());
    }, 250);
    return () => {
      clearTimeout(timer);
    };
  }, [text]);
  const results = useQuery({
    queryKey: ["registry", "command", search],
    queryFn: async ({ signal }) => {
      const lists = await Promise.all(
        (["projects", "customers"] as const).map(async (collection) => ({
          collection,
          result: await api.get(
            `/v1/operator/registry/${collection}?limit=8&search=${encodeURIComponent(search)}`,
            listSchema,
            signal,
          ),
        })),
      );
      return lists.flatMap(({ collection, result }) =>
        result.docs.map((record) => ({ ...record, collection })),
      );
    },
    enabled: session.user.role === "operator" && search.length >= 2,
    gcTime: 0,
    staleTime: 0,
  });
  return (
    <Modal title="Go to workspace" close={close}>
      <label className="relative block">
        <MagnifyingGlassIcon size={16} className="pointer-events-none absolute top-1/2 left-3 -translate-y-1/2 text-muted" />
        <input
          className={ui.cx(`${ui.input} pl-9`)}
          ref={searchInput}
          aria-label={session.user.role === "operator" ? "Search projects and customers" : t("Search workspaces and projects")}
          placeholder={session.user.role === "operator" ? "Search by name…" : t("Search workspaces and projects…")}
          value={text}
          maxLength={160}
          onChange={(event) => {
            setText(event.target.value);
          }}
        />
      </label>
      <div className="mt-3 max-h-[55dvh] overflow-y-auto">
        {session.user.role !== "operator" ? (
          <WorkspaceChoices customer={customer} search={text} close={close} />
        ) : search.length >= 2 ? (
          results.isPending ? (
            <Loading>Searching…</Loading>
          ) : results.isError ? (
            <Failure
              error={results.error}
              retry={() => {
                void results.refetch();
              }}
            />
          ) : results.data.length ? (
            results.data.map((record) => (
              <button
                key={`${record.collection}:${record.id}`}
                className={paletteItem}
                onClick={() => {
                  void navigate({
                    to: "/$collection/$id",
                    params: { collection: record.collection, id: record.id },
                  });
                  close();
                }}
              >
                <span>{record.name}</span>
                <span className="ml-auto text-xs text-muted">
                  {record.collection === "projects" ? "Project" : "Customer"}
                </span>
              </button>
            ))
          ) : (
            <Empty title="No results">Try a different name.</Empty>
          )
        ) : (
          <>
            {destinations.map((item) => (
                  <button
                    key={item.collection}
                    className={paletteItem}
                    onClick={() => {
                      void navigate(operatorDestination(item.collection));
                      close();
                    }}
                  >
                    <item.icon size={20} />
                    {item.label}
                  </button>
                ))}
            {api.config.enabledFeatures.includes("hosting") ? (
              <button
                className={paletteItem}
                onClick={() => {
                  void navigate({ to: "/hosting" });
                  close();
                }}
              >
                <FolderSimpleIcon size={20} />
                Applications
              </button>
            ) : null}
            <button
              className={paletteItem}
              onClick={() => {
                void navigate({ to: "/account" });
                close();
              }}
            >
              <UserCircleIcon size={20} />
              Account
            </button>
          </>
        )}
      </div>
    </Modal>
  );
}

function WorkspaceChoices({ search, close, customer }: { search: string; close: () => void; customer: string | undefined }) {
  const { t } = useI18n();
  const projects = useWorkspaceProjects(customer);
  const tenants = useTenants();
  const navigate = useNavigate();
  if (tenants.isPending) return <Loading>Loading workspaces…</Loading>;
  if (tenants.isError) return <Failure error={tenants.error} retry={() => void tenants.refetch()} />;
  const term = search.trim().toLocaleLowerCase();
  const matches = tenants.data.filter(tenant => tenant.name.toLocaleLowerCase().includes(term));
  const matchingProjects = (projects.data ?? []).filter(project => project.name.toLocaleLowerCase().includes(term));
  const workspace = tenants.data.find(tenant => tenant.id === customer);
  return <>
    {matchingProjects.length ? <div role="group" aria-label={t("Projects")}>
      <p className="px-3 py-2 text-xs text-muted">{t("Projects")} · {workspace?.name}</p>
      {matchingProjects.map(project => <button key={project.id} className={paletteItem} onClick={() => {
        void navigate({ to: "/workspaces/$customer/projects/$id", params: { customer: customer ?? "", id: project.id } });
        close();
      }}><FolderSimpleIcon size={20} className="shrink-0" aria-hidden /><span className="min-w-0 [overflow-wrap:anywhere]">{project.name}</span></button>)}
    </div> : null}
    {customer && projects.isPending ? <Loading>{t("Loading projects…")}</Loading> : null}
    {projects.isError ? <Failure error={projects.error} retry={() => void projects.refetch()} /> : null}
    {matches.length ? <p className="px-3 py-2 text-xs text-muted">{t("Workspaces")}</p> : null}
    {matches.map((tenant) => (
      <button key={tenant.id} className={paletteItem} onClick={() => {
        void navigate({ to: "/workspaces/$id", params: { id: tenant.id } });
        close();
      }}>
        <WorkspaceAvatar name={tenant.name} icon={tenant.icon} size="size-6 text-xs" />
        <span className="min-w-0 [overflow-wrap:anywhere]">{tenant.name}</span>
      </button>
    ))}
    {!matches.length && !matchingProjects.length && !projects.isError && !projects.isFetching ? <Empty title={t("No matching workspaces or projects")}>{t("Try another name or open all workspaces.")}</Empty> : null}
    <button className={paletteItem} onClick={() => {
      void navigate({ to: "/$collection", params: { collection: "tenants" } });
      close();
    }}>All workspaces</button>
  </>;
}

function sectionTitle(pathname: string): string {
  if (pathname.startsWith("/customers")) return "Customers";
  if (pathname.startsWith("/projects")) return "Projects";
  if (pathname.startsWith("/hosting")) return "Applications";
  if (pathname.startsWith("/mail")) return "Mail";
  if (pathname.startsWith("/databases")) return "Databases";
  if (pathname === "/audit-events") return "Audit trail";
  if (pathname === "/account") return "Account";
  if (pathname === "/plans") return "Plans";
  if (pathname === "/clusters") return "Clusters";
  if (pathname.startsWith("/tenants") || pathname.startsWith("/workspaces")) return "Workspaces";
  return "Extensions";
}

function workspaceIdentity(
  hosting: ReturnType<typeof useHostingContext>,
  customerId: string | undefined,
  customerName: string | undefined,
  failed: boolean,
  session: Session,
  mail: ReturnType<typeof useMailWorkspace>,
): { label: string; detail: string; icon?: string | null | undefined } {
  if (mail.active) return { label: mail.label, detail: "Customer workspace" };
  if (hosting.active)
    return {
      label: hosting.label,
      detail: hosting.customer ? "Customer workspace" : "Applications",
    };
  if (!customerId)
    return {
      label:
        session.user.role === "operator" ? "All workspaces" : "Your workspace",
      detail:
        session.user.role === "operator"
          ? "Operator console"
          : session.user.name,
    };
  return {
    label:
      customerName ?? (failed ? "Workspace unavailable" : "Loading workspace…"),
    detail: "Customer workspace",
  };
}

function currentCustomer(
  hosting: ReturnType<typeof useHostingContext>,
  mail: ReturnType<typeof useMailWorkspace>,
  pathname: string,
  projectCustomer: string | undefined,
  search: string,
): string | undefined {
  if (mail.active) return mail.customer?.id;
  if (hosting.active) return hosting.customer?.id;
  return (
    /^\/customers\/([1-9][0-9]{0,18})$/.exec(pathname)?.[1] ??
    projectCustomer ??
    parseConsoleSearch(search).customer ??
    undefined
  );
}

function workspaceIcon(tenants: { id: string; icon?: string | null | undefined }[] | undefined, customer: string | undefined) {
  return tenants?.find(tenant => tenant.id === customer)?.icon;
}

function workspaceName(record: { name?: string | undefined } | undefined, tenants: { id: string; name: string }[] | undefined, customer: string | undefined) {
  return record?.name ?? tenants?.find(tenant => tenant.id === customer)?.name;
}
