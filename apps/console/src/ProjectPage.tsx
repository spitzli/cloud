import { Placement } from "./Placement";
import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { z } from "zod";
import { ArrowLeftIcon } from "@phosphor-icons/react/dist/csr/ArrowLeft";
import { PlusIcon } from "@phosphor-icons/react/dist/csr/Plus";
import { useApi, useSession } from "./context";
import { Empty, Failure, Loading } from "./common";
import { isReadOnlyPreview } from "./api";
import { PluginPanel } from "./plugins/Panel";
import { creatable, invokePlugin, usePluginViews, useProject, type JsonSchema, type PluginAction, type PluginView } from "./plugins/views";
import { useNativeUI } from "./plugins/native";
import * as ui from "./ui";

/** A project as its customer sees it: plugin resources by kind, and what the viewer may add. */
export function ProjectPage({ customer, id }: { customer: string; id: string }) {
  const session = useSession();
  const project = useProject(id);
  const views = usePluginViews(id, project.isSuccess && project.data.customer.id === customer);
  const [adding, setAdding] = useState<string>();
  if (project.isPending) return <Loading>Loading project…</Loading>;
  if (project.error) return <Failure error={project.error} retry={() => void project.refetch()} />;
  if (project.data.customer.id !== customer) return <Empty title="Project unavailable"><span>This project is not available in this workspace.</span> <Link to="/workspaces/$id" params={{ id: customer }}>Open workspace</Link></Empty>;
  if (views.isPending) return <Loading>Loading project…</Loading>;
  const plugins = views.error ? [] : views.data;
  const operator = session.user.role === "operator" && !session.user.preview;
  const options = isReadOnlyPreview(session) ? [] : plugins.flatMap((view) => creatable(view).map((option) => ({ view, ...option, key: `${view.manifest.name}:${option.action.name}` })));
  const chosen = options.find((option) => option.key === adding);
  const hasResources = plugins.some((view) => view.resources.length > 0);
  return (
    <section>
      <div className={ui.pageHeading}>
        <div>
          <Link className={ui.backLink} to="/workspaces/$id" params={{ id: customer }}>
            <ArrowLeftIcon />
            {project.data.customer.name ?? "Workspace"}
          </Link>
          <h1 className={ui.pageTitle}>{project.data.name}</h1>
          <p className={ui.pageLead}>Everything this project uses, in one place.</p>
        </div>
        {options.length ? (
          <details className="relative">
            <summary className={ui.cx(ui.button("primary"), "list-none [&::-webkit-details-marker]:hidden")}>
              <PlusIcon />
              {operator ? "Assign to this project" : "Add"}
            </summary>
            <ul className={ui.actionMenu} aria-label="Add a resource">
              {options.map((option) => (
                <li key={option.key}>
                  <button
                    type="button"
                    className="w-full rounded-md px-3 py-2 text-left text-sm hover:bg-subtle"
                    onClick={(event) => {
                      event.currentTarget.closest("details")?.removeAttribute("open");
                      setAdding(option.key);
                    }}
                  >
                    <span className="block font-medium">{option.feature.label}</span>
                    {option.offering.description ? <span className="mt-0.5 line-clamp-2 text-[13px] text-muted">{option.offering.description}</span> : null}
                  </button>
                </li>
              ))}
            </ul>
          </details>
        ) : null}
      </div>
      {chosen ? (
        <ChosenCreate
          key={chosen.key}
          view={chosen.view}
          action={chosen.action}
          title={chosen.feature.label}
          customer={customer}
          projectID={id}
          onCancel={() => { setAdding(undefined); }}
        />
      ) : null}
      {views.error ? <Failure error={views.error} retry={() => void views.refetch()} /> : hasResources ? (
        <div className="grid gap-6">
          {plugins.flatMap((view) => view.manifest.offerings.map((offering) => {
            const resources = view.resources.filter((r) => r.offering === offering.id);
            if (!resources.length) return null;
            const page = Boolean(view.manifest.areas.selfService?.pages.length);
            return (
              <section key={`${view.manifest.name}:${offering.id}`} className={ui.card} aria-label={offering.label}>
                <h2 className={ui.cx(ui.sectionTitle, "px-6 py-4 max-sm:px-[18px]")}>{offering.label}</h2>
                <ul>
                  {resources.map((resource) => (
                    <li key={resource.id} className={ui.cx(ui.cardRow, page ? ui.clickableRow : "")}>
                      {page ? (
                        <Link
                          className={ui.cx(ui.rowLink, "font-medium")}
                          to="/workspaces/$customer/projects/$id/resources/$plugin/$resource"
                          params={{ customer, id, plugin: view.manifest.name, resource: resource.id }}
                        >
                          {resource.name}
                        </Link>
                      ) : (
                        <span className="font-medium">{resource.name}</span>
                      )}
                      {resource.createdVia === "operator" ? <span className="text-[13px] text-muted">Added by your provider</span> : null}
                    </li>
                  ))}
                </ul>
              </section>
            );
          }))}
        </div>
      ) : !chosen ? (
        <Empty title="Nothing here yet">
          {options.length
            ? `Use ${operator ? "“Assign to this project”" : "“Add”"} to create the first resource.`
            : "Your provider adds resources to this project for you."}
        </Empty>
      ) : null}
      {operator ? plugins.flatMap((view) => view.manifest.areas.operator.pages.map((page) => (
        <OperatorPage key={`${view.manifest.name}:${page.id}`} view={view} projectID={id} action={page.action} label={page.label} />
      ))) : null}
      {!views.error ? <Placement projectID={id} role={project.data.role} /> : null}
    </section>
  );
}

/** The plugin's own create form when it ships one, otherwise one generated from the input schema. */
export function ChosenCreate(props: { view: PluginView; action: PluginAction; title: string; customer: string; projectID: string; onCancel: () => void }) {
  const native = useNativeUI(props.view.manifest.name);
  const client = useQueryClient();
  const navigate = useNavigate();
  if (native.isLoading) return <Loading>Loading…</Loading>;
  const Native = native.data?.Create?.[props.action.name];
  if (!Native) return <CreateForm {...props} />;
  return (
    <Native
      plugin={props.view.manifest.name}
      projectID={props.projectID}
      customer={props.customer}
      action={props.action.name}
      title={props.title}
      resources={props.view.resources}
      onCancel={props.onCancel}
      onCreated={(resource) => {
        void client.invalidateQueries({ queryKey: ["project-plugins"] }).then(() => navigate({ to: "/workspaces/$customer/projects/$id/resources/$plugin/$resource", params: { customer: props.customer, id: props.projectID, plugin: props.view.manifest.name, resource } }));
      }}
    />
  );
}
function inputs(schema: JsonSchema | undefined) {
  return Object.entries(schema?.properties ?? {}).filter(([name]) => name !== "resource_id");
}
function secretInput(name: string) {
  return /(^|_)(secret|password)$/.test(name) || ["api_key", "access_token", "refresh_token", "consumer_key"].includes(name);
}
/** A form generated from the create action's input schema. */
function CreateForm({ view, action, title, customer, projectID, onCancel }: { view: PluginView; action: PluginAction; title: string; customer: string; projectID: string; onCancel: () => void }) {
  const api = useApi();
  const client = useQueryClient();
  const navigate = useNavigate();
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string>();
  const required = new Set(action.input?.required ?? []);
  return (
    <form
      className={ui.cx(ui.card, ui.cardBody, "mb-6 gap-4")}
      aria-label={title}
      onSubmit={(event) => {
        event.preventDefault();
        const data = new FormData(event.currentTarget);
        const input: Record<string, unknown> = {};
        for (const [name, field] of inputs(action.input)) {
          const raw = data.get(name);
          if (field.type === "boolean") {
            input[name] = raw !== null;
            continue;
          }
          if (typeof raw !== "string" || !raw) continue;
          input[name] = field.type === "integer" || field.type === "number" ? Number(raw) : raw;
        }
        // Keep credentials only in the request, including when submission fails.
        for (const [name] of inputs(action.input)) {
          const field = event.currentTarget.elements.namedItem(name);
          if (secretInput(name) && field instanceof HTMLInputElement) field.value = "";
        }
        setPending(true);
        setError(undefined);
        invokePlugin(api, view.manifest.name, projectID, action.name, input)
          .then(async (result) => {
            await client.invalidateQueries({ queryKey: ["project-plugins"] });
            const created = z.object({ resource: z.object({ id: z.string() }) }).safeParse(result);
            onCancel();
            if (created.success && view.manifest.areas.selfService?.pages.length)
              await navigate({ to: "/workspaces/$customer/projects/$id/resources/$plugin/$resource", params: { customer, id: projectID, plugin: view.manifest.name, resource: created.data.resource.id } });
          })
          .catch((failure: unknown) => {
            setError(failure instanceof Error ? failure.message : "Could not create it. Try again.");
          })
          .finally(() => {
            setPending(false);
          });
      }}
    >
      <h2 className={ui.sectionTitle}>{title}</h2>
      {inputs(action.input).map(([name, field]) => {
        const label = field.description ?? name.charAt(0).toUpperCase() + name.slice(1).replaceAll("_", " ");
        if (field.enum)
          return (
            <label key={name} className={ui.field}>
              {label}
              <select className={ui.input} name={name} required={required.has(name)} disabled={pending} defaultValue="">
                <option value="">Choose an option</option>
                {field.enum.map((value) => <option key={String(value)} value={String(value)}>{String(value)}</option>)}
              </select>
            </label>
          );
        if (field.type === "boolean")
          return (
            <label key={name} className="flex items-center gap-2 text-sm">
              <input type="checkbox" className="size-4 accent-accent" name={name} disabled={pending} />
              {label}
            </label>
          );
        return (
          <label key={name} className={ui.field}>
            {label}
            <input
              className={ui.input}
              name={name}
              // Inputs named like a secret are write-only fields: masked and never autofilled.
              type={secretInput(name) ? "password" : field.type === "integer" || field.type === "number" ? "number" : "text"}
              autoComplete={secretInput(name) ? "new-password" : undefined}
              required={required.has(name)}
              minLength={field.minLength}
              maxLength={field.maxLength}
              min={field.minimum}
              max={field.maximum}
              disabled={pending}
            />
          </label>
        );
      })}
      {error ? <p role="alert" className="text-sm text-danger">{error}</p> : null}
      <div className="flex gap-3">
        <button className={ui.button("primary")} disabled={pending}>{pending ? "Creating…" : "Create"}</button>
        <button type="button" className={ui.button()} onClick={onCancel} disabled={pending}>Cancel</button>
      </div>
    </form>
  );
}

function OperatorPage({ view, projectID, action, label }: { view: PluginView; projectID: string; action: string; label: string }) {
  const api = useApi();
  const active = view.state === "enabled" && view.runtime === "ready";
  const [value, setValue] = useState<unknown>();
  const initial = useQuery({
    queryKey: ["plugin-page", view.manifest.name, projectID, action],
    queryFn: ({ signal }) => invokePlugin(api, view.manifest.name, projectID, action, {}, signal),
    enabled: active,
    retry: false,
  });
  if (!active) return <p className="mt-6 text-sm text-muted">{label}: extension is unavailable.</p>;
  if (initial.isPending) return <Loading>Loading {label}…</Loading>;
  if (initial.error) return <Failure error={initial.error} retry={() => void initial.refetch()} />;
  return (
    <div className="mt-6">
      <PluginPanel
        value={value ?? initial.data}
        invoke={async (name, input, signal) => {
          const result = await invokePlugin(api, view.manifest.name, projectID, name, input, signal);
          if (name === action) setValue(result);
          return result;
        }}
      />
    </div>
  );
}
