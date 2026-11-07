import { WorkspaceDatabases } from "./WorkspaceDatabases";
import { PublicWaitlist,WaitlistReview } from "./Waitlist";
import { useState, lazy, Suspense, type ReactNode } from "react";
import {
  QueryClient,
  QueryClientProvider,
  useQuery,
} from "@tanstack/react-query";
import {
  createRootRoute,
  createRoute,
  createRouter,
  Navigate,
  RouterProvider,
} from "@tanstack/react-router";
import { ApiClient, ApiError, loadRuntimeConfig } from "./api";
import { ApiContext, useSession } from "./context";
import { Account, ResetPassword } from "./Auth";
import { PageFailure, PageProgress } from "./AuthFrame";
const AccountApp = lazy(() =>
  import("./AccountUI").then((module) => ({ default: module.AccountApp })),
);
import { WorkspaceMembers, Workspaces } from "./Workspaces";
import { LocaleProvider } from "./Locale";
import { parseConsoleSearch } from "./search";
const DatabaseDirectory = lazy(() =>
  import("./Databases").then((module) => ({
    default: module.DatabaseDirectory,
  })),
);
const DatabaseViewer = lazy(() =>
  import("./Databases").then((module) => ({ default: module.DatabaseViewer })),
);
import { Shell } from "./Shell";
const Mail = lazy(() =>
  import("./Mail").then((module) => ({ default: module.Mail })),
);
const Webmail = lazy(() =>
  import("./Webmail").then((module) => ({ default: module.Webmail })),
);
const HostingDirectory = lazy(() =>
  import("./Hosting").then((module) => ({ default: module.HostingDirectory })),
);
const HostingBuilds = lazy(() => import("./HostingBuilds").then((module) => ({default:module.HostingBuilds})));
const HostingProject = lazy(() =>
  import("./Hosting").then((module) => ({ default: module.HostingProject })),
);
const HostingApplication = lazy(() =>
  import("./HostingApplication").then((module) => ({
    default: module.HostingApplication,
  })),
);
const HostingOperation = lazy(() =>
  import("./HostingOperation").then((module) => ({
    default: module.HostingOperation,
  })),
);
const InstalledExtensions = lazy(() =>
  import("./plugins/InstalledExtensions").then((module) => ({
    default: module.InstalledExtensions,
  })),
);
const InstallExtension = lazy(() => import("./plugins/InstallExtension").then((module) => ({ default: module.InstallExtension })));
const ExtensionVersions = lazy(() => import("./plugins/ExtensionVersions").then((module) => ({ default: module.ExtensionVersions })));
const PluginSettings = lazy(() =>
  import("./plugins/SelfServicePolicy").then((module) => ({
    default: module.PluginSettings,
  })),
);
const ResourcePage = lazy(() =>
  import("./plugins/ResourcePage").then((module) => ({
    default: module.ResourcePage,
  })),
);
import { WorkspaceProjects } from "./WorkspaceProjects";
import { useTenants } from "./WorkspaceNav";
import { ProductPage } from "./plugins/ProductPage";
import { Plans, WorkspaceUsage } from "./Usage";
import { WorkspaceDomains } from "./Domains";
import { WorkspaceApiTokens } from "./ApiTokens";
const WorkspaceConnections = lazy(() => import("./Connections").then(module => ({ default: module.WorkspaceConnections })));
import { Clusters } from "./Placement";
import { ProjectPage } from "./ProjectPage";
const ExtensionOperation = lazy(() =>
  import("./plugins/ExtensionOperation").then((module) => ({
    default: module.ExtensionOperation,
  })),
);
import {
  RegistryList,
  RegistryDetail,
  RegistryNew,
  isCollection,
} from "./Registry";
import { Empty, Failure, Loading } from "./common";
import { ConsoleSessionGate } from "./ConsoleSessionGate";
import { DatabaseBackupIntentProvider } from "./database-backup-intents";
const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 15000,
      retry: (count, error) =>
        !(error instanceof ApiError && error.status < 500) && count < 1,
      refetchOnWindowFocus: true,
    },
    mutations: { retry: false },
  },
});
function SessionGate() { return <ConsoleSessionGate><Shell /></ConsoleSessionGate>; }
/** Route-level guidance complements the server's authorization; it grants no access. */
function OperatorOnly({ children }: { children: ReactNode }) {
  const session = useSession();
  if (session.user.role === "operator" && !session.user.preview) return children;
  return <Empty title="Operator access required">This area manages the platform. Open your workspace to manage your own services.</Empty>;
}
function RegistryRoute({ kind }: { kind: "list" | "detail" | "new" }) {
  const session = useSession();
  const params: { collection: string; id?: string } = (
    kind === "list" ? listRoute : kind === "detail" ? detailRoute : newRoute
  ).useParams();
  const { customer } = (
    kind === "list" ? listRoute : kind === "detail" ? detailRoute : newRoute
  ).useSearch();
  if (session.user.mustChangePassword)
    return <Navigate to="/account" replace />;
  if (params.collection === "tenants" && kind === "detail" && params.id && !(session.user.preview && !session.user.acting))
    return <WorkspaceMembers id={params.id} />;
  if (
    session.user.role !== "operator" ||
    session.user.preview ||
    params.collection === "tenants"
  )
    return <Workspaces />;
  if (!isCollection(params.collection))
    return (
      <Empty title="Page not found">
        Choose a destination from the navigation.
      </Empty>
    );
  if (kind === "detail" && params.id)
    return <RegistryDetail collection={params.collection} id={params.id} />;
  if (kind === "new")
    return params.collection === "audit-events" ? (
      <Empty title="Audit records are read-only">
        Changes are recorded automatically.
      </Empty>
    ) : (
      <RegistryNew collection={params.collection} customer={customer} />
    );
  return (
    <RegistryList
      key={`${params.collection}:${customer ?? ""}`}
      collection={params.collection}
      customer={customer}
    />
  );
}
const rootRoute = createRootRoute({
  component: SessionGate,
  notFoundComponent: () => (
    <Empty title="Page not found">
      Choose a destination from the navigation.
    </Empty>
  ),
  errorComponent: ({ error, reset }) => (
    <Failure
      error={
        error instanceof Error ? error : new Error("This view is unavailable.")
      }
      retry={reset}
    />
  ),
});
const waitlistReviewRoute=createRoute({getParentRoute:()=>rootRoute,path:"/access-requests",component:()=> <OperatorOnly><WaitlistReview /></OperatorOnly>});
const indexRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/",
  component: Home,
});
/** Customers with a single workspace start inside it; everyone else starts at the list. */
function Home() {
  const session = useSession();
  const tenants = useTenants();
  const customer = session.user.role !== "operator";
  if (customer && tenants.isPending) return <Loading>Opening your workspace…</Loading>;
  const [only] = tenants.data ?? [];
  if (customer && tenants.data?.length === 1 && only) return <Navigate to="/workspaces/$id" params={{ id: only.id }} replace />;
  return <Navigate to="/$collection" params={{ collection: "projects" }} replace />;
}
const accountRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/account",
  component: Account,
});
const extensionsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/extensions",
  component: () => (
    <OperatorOnly>
    <Suspense fallback={<Loading>Loading extensions…</Loading>}>
      <InstalledExtensions />
    </Suspense>
    </OperatorOnly>
  ),
});
const installExtensionRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/extensions/install/$catalogID/$version",
  component: () => {
    const { catalogID, version } = installExtensionRoute.useParams();
    return <OperatorOnly><Suspense fallback={<Loading>Loading release…</Loading>}><InstallExtension catalogID={catalogID} version={version} /></Suspense></OperatorOnly>;
  },
});
const extensionVersionsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/extensions/$plugin/releases",
  component: () => {
    const { plugin } = extensionVersionsRoute.useParams();
    return <OperatorOnly><Suspense fallback={<Loading>Loading release history…</Loading>}><ExtensionVersions key={plugin} plugin={plugin} /></Suspense></OperatorOnly>;
  },
});
const resetRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/reset-password",
  component: ResetPassword,
});
const validateSearch = (
  search: Record<string, unknown>,
): { customer?: string | undefined } => ({
  customer:
    typeof search.customer === "string" &&
    /^[1-9][0-9]{0,18}$/.test(search.customer)
      ? search.customer
      : undefined,
});
const mailRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/mail",
  validateSearch,
  component: () => {
    const { customer } = mailRoute.useSearch();
    return (
      <Suspense fallback={<Loading>Loading Mail…</Loading>}>
        <Mail customerID={customer} />
      </Suspense>
    );
  },
});
const inboxRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/mail/inbox/$mailboxID",
  validateSearch,
  component: () => {
    const { customer } = inboxRoute.useSearch();
    const { mailboxID } = inboxRoute.useParams();
    return <Suspense fallback={<Loading>Opening mailbox…</Loading>}><Webmail customerID={customer} mailboxID={mailboxID} /></Suspense>;
  },
});
const hostingRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/hosting",
  validateSearch,
  component: () => {
    const { customer } = hostingRoute.useSearch();
    return (
      <Suspense fallback={<Loading>Loading applications…</Loading>}>
        <HostingDirectory customerID={customer} />
      </Suspense>
    );
  },
});
const hostingProjectRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/hosting/projects/$projectID",
  component: () => {
    const { projectID } = hostingProjectRoute.useParams();
    return (
      <Suspense fallback={<Loading>Loading project…</Loading>}>
        <HostingProject key={projectID} projectID={projectID} />
      </Suspense>
    );
  },
});
const hostingBuildsRoute=createRoute({
  getParentRoute:()=>rootRoute,
  path:"/hosting/projects/$projectID/builds",
  component:()=>{const {projectID}=hostingBuildsRoute.useParams();return <Suspense fallback={<Loading>Loading builds…</Loading>}><HostingBuilds key={projectID} projectID={projectID}/></Suspense>;},
});
const hostingAppRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/hosting/apps/$appID",
  component: () => {
    const { appID } = hostingAppRoute.useParams();
    return (
      <Suspense fallback={<Loading>Loading application…</Loading>}>
        <HostingApplication key={appID} appID={appID} />
      </Suspense>
    );
  },
});
const hostingSettingsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/hosting/apps/$appID/settings",
  component: () => {
    const { appID } = hostingSettingsRoute.useParams();
    return (
      <Suspense fallback={<Loading>Loading settings…</Loading>}>
        <HostingApplication key={appID} appID={appID} view="settings" />
      </Suspense>
    );
  },
});
const hostingLogsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/hosting/apps/$appID/logs",
  component: () => {
    const { appID } = hostingLogsRoute.useParams();
    return (
      <Suspense fallback={<Loading>Loading logs…</Loading>}>
        <HostingApplication key={appID} appID={appID} view="logs" />
      </Suspense>
    );
  },
});
const hostingOperationRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/hosting/operations/$operationID",
  component: () => {
    const { operationID } = hostingOperationRoute.useParams();
    return (
      <Suspense fallback={<Loading>Loading operation…</Loading>}>
        <HostingOperation key={operationID} id={operationID} />
      </Suspense>
    );
  },
});
const extensionRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/extensions/$plugin",
  component: () => {
    const { plugin } = extensionRoute.useParams();
    return (
      <OperatorOnly>
      <Suspense fallback={<Loading>Loading extension…</Loading>}>
        <PluginSettings key={plugin} plugin={plugin} />
      </Suspense>
      </OperatorOnly>
    );
  },
});
const clustersRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/clusters",
  component: () => <OperatorOnly><Clusters /></OperatorOnly>,
});
const plansRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/plans",
  component: () => <OperatorOnly><Plans /></OperatorOnly>,
});
const usageRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/workspaces/$id/usage",
  component: () => {
    const { id } = usageRoute.useParams();
    return <WorkspaceUsage key={id} id={id} />;
  },
});
const domainsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/workspaces/$id/domains",
  component: () => {
    const { id } = domainsRoute.useParams();
    return <WorkspaceDomains key={id} id={id} />;
  },
});
const apiTokensRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/workspaces/$id/api-tokens",
  component: () => {
    const { id } = apiTokensRoute.useParams();
    return <WorkspaceApiTokens key={id} id={id} />;
  },
});
const connectionsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/workspaces/$id/connections",
  component: () => {
    const { id } = connectionsRoute.useParams();
    return <Suspense fallback={<Loading>Loading connections…</Loading>}><WorkspaceConnections key={id} id={id} /></Suspense>;
  },
});
const workspaceRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/workspaces/$id",
  component: () => {
    const { id } = workspaceRoute.useParams();
    return <WorkspaceProjects key={id} id={id} />;
  },
});
const projectRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/workspaces/$customer/projects/$id",
  component: () => {
    const { customer, id } = projectRoute.useParams();
    return <ProjectPage key={`${customer}:${id}`} customer={customer} id={id} />;
  },
});
const workspaceDatabasesRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/workspaces/$id/databases",
  component: () => { const { id } = workspaceDatabasesRoute.useParams(); return <WorkspaceDatabases key={id} customer={id} />; },
});
const productRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/workspaces/$customer/products/$plugin",
  component: () => {
    const { customer, plugin } = productRoute.useParams();
    return <ProductPage key={`${customer}:${plugin}`} customer={customer} plugin={plugin} />;
  },
});
const resourceRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/workspaces/$customer/projects/$id/resources/$plugin/$resource",
  component: () => {
    const { customer, id, plugin, resource } = resourceRoute.useParams();
    return (
      <Suspense fallback={<Loading>Loading…</Loading>}>
        <ResourcePage key={`${customer}:${id}:${plugin}:${resource}`} customer={customer} projectID={id} plugin={plugin} resource={resource} />
      </Suspense>
    );
  },
});
const extensionOperationRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/extensions/operations/$operationID",
  component: () => {
    const { operationID } = extensionOperationRoute.useParams();
    return (
      <OperatorOnly>
      <Suspense fallback={<Loading>Loading operation…</Loading>}>
        <ExtensionOperation key={operationID} id={operationID} />
      </Suspense>
      </OperatorOnly>
    );
  },
});
const databasesRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/databases",
  validateSearch,
  component: () => {
    const { customer } = databasesRoute.useSearch();
    return (
      <Suspense fallback={<Loading>Loading databases…</Loading>}>
        <DatabaseDirectory customerID={customer} />
      </Suspense>
    );
  },
});
const databaseViewerRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/databases/$id",
  validateSearch: (search: Record<string, unknown>) => ({
    ...validateSearch(search),
    managed: search.managed === true || search.managed === "true",
  }),
  component: () => {
    const { id } = databaseViewerRoute.useParams();
    const { customer, managed } = databaseViewerRoute.useSearch();
    return (
      <Suspense fallback={<Loading>Opening database…</Loading>}>
        <DatabaseViewer id={id} managed={managed} customerID={customer} />
      </Suspense>
    );
  },
});
const listRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/$collection",
  validateSearch,
  component: () => <RegistryRoute kind="list" />,
});
const newRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/$collection/new",
  validateSearch,
  component: () => <RegistryRoute kind="new" />,
});
const detailRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/$collection/$id",
  validateSearch,
  component: () => <RegistryRoute kind="detail" />,
});
const router = createRouter({
  parseSearch: parseConsoleSearch,
  routeTree: rootRoute.addChildren([
    indexRoute,
    waitlistReviewRoute,
    accountRoute,
    extensionsRoute,
    installExtensionRoute,
    extensionVersionsRoute,
    extensionRoute,
    extensionOperationRoute,
    plansRoute,
    clustersRoute,
    usageRoute,
    domainsRoute,
    apiTokensRoute,
    connectionsRoute,
    workspaceRoute,
    projectRoute,
    productRoute,
    workspaceDatabasesRoute,
    resourceRoute,
    mailRoute,
    inboxRoute,
    hostingRoute,
    hostingProjectRoute,
    hostingBuildsRoute,
    hostingAppRoute,
    hostingSettingsRoute,
    hostingLogsRoute,
    hostingOperationRoute,
    resetRoute,
    databasesRoute,
    databaseViewerRoute,
    listRoute,
    newRoute,
    detailRoute,
  ]),
  defaultPreload: "intent",
});
declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}
function Bootstrap() {
  const [attempt, setAttempt] = useState(0);
  const config = useQuery({
    queryKey: ["runtime-config", attempt],
    queryFn: loadRuntimeConfig,
    staleTime: Infinity,
    retry: false,
  });
  if (config.isPending) return <PageProgress>Connecting to Spitzli Cloud…</PageProgress>;
  if (config.isError)
    return (
      <PageFailure
        title="Spitzli Cloud unavailable"
        error={config.error}
        retry={() => {
          setAttempt(attempt + 1);
        }}
      />
    );
  return (
    <ApiContext.Provider value={new ApiClient(config.data)}>
      <LocaleProvider preference={config.data.localePreference ?? "system"}>
        {window.location.pathname === "/waitlist" || window.location.pathname === "/waitlist/confirmed" ? (
          <PublicWaitlist confirmed={window.location.pathname === "/waitlist/confirmed"}/>
        ) : config.data.surface === "account" ? (
          <Suspense fallback={<PageProgress product="Account">Opening your account…</PageProgress>}>
            <AccountApp />
          </Suspense>
        ) : (
          <RouterProvider router={router} />
        )}
      </LocaleProvider>
    </ApiContext.Provider>
  );
}
export default function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <DatabaseBackupIntentProvider><Bootstrap /></DatabaseBackupIntentProvider>
    </QueryClientProvider>
  );
}
