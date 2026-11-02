import { formText } from "../form";
import { useId, useState, type SubmitEvent } from "react";
import { parsePanel, revealSchema, type Action, type Panel } from "./contracts";
import * as ui from "../ui";
export type InvokeAction = (
  action: string,
  input: Record<string, string | number | boolean | null>,
  signal?: AbortSignal,
) => Promise<unknown>;

function ActionForm({
  action,
  fields = [],
  invoke,
  active,
}: {
  action: Action;
  fields?: Extract<Panel["blocks"][number], { kind: "form" }>["fields"];
  invoke: InvokeAction;
  active: boolean;
}) {
  const prefix = useId();
  const [pending, setPending] = useState(false);
  const [confirmed, setConfirmed] = useState(false);
  const [message, setMessage] = useState("");
  const [failed, setFailed] = useState(false);
  const submit = async (event: SubmitEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!active || pending || (action.destructive && !confirmed)) return;
    const form = event.currentTarget;
    const input: Record<string, string | number | boolean> = {};
    const data = new FormData(form);
    for (const field of fields) {
      const value = formText(data, field.name);
      if (field.type === "checkbox") {
        input[field.name] = data.has(field.name);
        continue;
      }
      if (field.type !== "number" || !value) {
        input[field.name] = value;
        continue;
      }
      if (!Number.isFinite(Number(value))) return;
      input[field.name] = Number(value);
    }
    setPending(true);
    setMessage("");
    setFailed(false);
    try {
      await invoke(action.id, input);
      // Clear the form so write-only values such as secrets do not stay in the page.
      form.reset();
      setMessage("Action completed.");
      setConfirmed(false);
    } catch {
      setMessage("Action failed. Check your access and try again.");
      setFailed(true);
    } finally {
      setPending(false);
    }
  };
  return (
    <form
      className="grid gap-4 text-sm"
      onSubmit={(event) => {
        void submit(event);
      }}
    >
      {fields.map((field) => (
        <label key={field.name} htmlFor={`${prefix}-${field.name}`} className={ui.field}>
          {field.label}
          {field.type === "select" ? (
            <select
              className={ui.input}
              id={`${prefix}-${field.name}`}
              name={field.name}
              required={field.required}
              disabled={pending || !active}
              defaultValue=""
            >
              <option value="">Choose an option</option>
              {field.options?.map((option) => (
                <option key={option.value} value={option.value}>
                  {option.label}
                </option>
              ))}
            </select>
          ) : field.type === "textarea" ? (
            <textarea
              className={ui.cx(ui.input, "min-h-24 py-2")}
              id={`${prefix}-${field.name}`}
              name={field.name}
              required={field.required}
              maxLength={4000}
              disabled={pending || !active}
            />
          ) : field.type === "checkbox" ? (
            <input
              className="size-4 accent-accent"
              id={`${prefix}-${field.name}`}
              name={field.name}
              type="checkbox"
              disabled={pending || !active}
            />
          ) : (
            <input
              className={ui.input}
              id={`${prefix}-${field.name}`}
              name={field.name}
              type={field.type === "secret" ? "password" : field.type}
              autoComplete={field.type === "secret" ? "new-password" : undefined}
              required={field.required}
              maxLength={4000}
              disabled={pending || !active}
            />
          )}
        </label>
      ))}
      {action.destructive ? (
        <label className="flex items-center gap-2 text-[13px]">
          <input
            type="checkbox"
            className="size-4 accent-accent"
            checked={confirmed}
            onChange={(event) => {
              setConfirmed(event.target.checked);
            }}
            disabled={!active || pending}
          />
          Confirm {action.label.toLowerCase()}
        </label>
      ) : null}
      <div>
        <button
          className={ui.button()}
          disabled={
            !active || pending || Boolean(action.destructive && !confirmed)
          }
        >
          {pending ? "Working…" : action.label}
        </button>
      </div>
      {message ? (
        <p
          role={failed ? "alert" : "status"}
          className={failed ? "text-danger" : "text-success"}
        >
          {message}
        </p>
      ) : null}
    </form>
  );
}

/** Fetches a credential only when asked and keeps it out of any cache; hidden again on unmount. */
function Reveal({ label, action, invoke, active }: { label: string; action: string; invoke: InvokeAction; active: boolean }) {
  const [value, setValue] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  const [pending, setPending] = useState(false);
  const show = async () => {
    setPending(true);
    setFailed(false);
    try {
      setValue(revealSchema.parse(await invoke(action, {})).value);
    } catch {
      setFailed(true);
    } finally {
      setPending(false);
    }
  };
  return (
    <div className="grid gap-2 text-sm">
      <div className="flex flex-wrap items-center gap-3">
        <span className="font-medium">{label}</span>
        {value === null ? (
          <button type="button" className={ui.button("secondary")} disabled={!active || pending} onClick={() => void show()}>
            {pending ? "Loading…" : `Show ${label.toLowerCase()}`}
          </button>
        ) : (
          <>
            <button type="button" className={ui.button("secondary")} onClick={() => void navigator.clipboard.writeText(value)}>
              Copy
            </button>
            <button type="button" className={ui.button("secondary")} onClick={() => { setValue(null); }}>
              Hide
            </button>
          </>
        )}
      </div>
      {value !== null ? <code className={ui.cx(ui.mono, "rounded-md border border-border bg-subtle px-3 py-2 break-all")}>{value}</code> : null}
      {failed ? <p role="alert" className="text-danger">Could not load {label.toLowerCase()}. Check your access and try again.</p> : null}
    </div>
  );
}

export function PluginPanel({
  value,
  invoke: invokeAction,
  active = true,
  context = {},
  onOpenResource,
}: {
  value: unknown;
  invoke: InvokeAction;
  active?: boolean;
  /** Merged into every action input, e.g. the resource a page shows. */
  context?: Record<string, string>;
  onOpenResource?: (id: string) => void;
}) {
  const invoke: InvokeAction = (action, input, signal) =>
    signal ? invokeAction(action, { ...input, ...context }, signal) : invokeAction(action, { ...input, ...context });
  let panel: Panel;
  try {
    panel = parsePanel(value);
  } catch {
    return (
      <p role="alert" className="text-danger">
        This extension returned an unsupported panel.
      </p>
    );
  }
  return (
    <section aria-label={panel.title} className={ui.cx(`${ui.card} ${ui.cardBody} gap-5`)}>
      <h2 className={ui.sectionTitle}>{panel.title}</h2>
      {!active ? (
        <p role="status" className={ui.notice}>
          Extension access is unavailable. Actions are disabled.
        </p>
      ) : null}
      {panel.blocks.map((block, index) => {
        switch (block.kind) {
          case "text":
            return (
              <p key={index} className="text-sm whitespace-pre-wrap">
                {block.text}
              </p>
            );
          case "status":
            return (
              <dl key={index} className="flex flex-wrap items-center gap-x-5 gap-y-1 text-sm">
                <dt className="text-muted">{block.label}</dt>
                <dd>
                  <span className={ui.cx(`${ui.badge(ui.statusTone(block.state))} normal-case`)}>{block.state}</span>{" "}
                  {block.text}
                </dd>
              </dl>
            );
          case "table":
            return (
              <div key={index} className={ui.tableFrame}>
                <table className={ui.cx(`${ui.table} [&>caption]:px-5 [&>caption]:py-3 [&>caption]:text-left [&>caption]:font-semibold`)}>
                  <caption>{block.title}</caption>
                  <thead>
                    <tr>
                      {block.columns.map((column) => (
                        <th scope="col" key={column.key} className={ui.th}>
                          {column.label}
                        </th>
                      ))}
                    </tr>
                  </thead>
                  <tbody>
                    {block.rows.length ? (
                      block.rows.map((row, rowIndex) => {
                        const resource = block.rowLink === "resource" && typeof row.resource_id === "string" ? row.resource_id : null;
                        return (
                          <tr key={rowIndex} className={resource ? ui.cx(ui.tr, "cursor-pointer") : ui.tr}>
                            {block.columns.map((column, columnIndex) => (
                              <td key={column.key} className={ui.td}>
                                {resource && columnIndex === 0 ? (
                                  <a
                                    href="#"
                                    className={ui.cx(ui.rowLink, "font-medium")}
                                    onClick={(event) => {
                                      event.preventDefault();
                                      onOpenResource?.(resource);
                                    }}
                                  >
                                    {String(row[column.key] ?? "—")}
                                  </a>
                                ) : (
                                  String(row[column.key] ?? "—")
                                )}
                              </td>
                            ))}
                          </tr>
                        );
                      })
                    ) : (
                      <tr className={ui.tr}>
                        <td colSpan={block.columns.length} className={ui.cx(`${ui.td} text-muted`)}>No records.</td>
                      </tr>
                    )}
                  </tbody>
                </table>
              </div>
            );
          case "form":
            return (
              <section key={index} className="grid gap-3">
                <h3 className="text-sm font-semibold">{block.title}</h3>
                <ActionForm
                  action={block.action}
                  fields={block.fields}
                  invoke={invoke}
                  active={active}
                />
              </section>
            );
          case "reveal":
            return <Reveal key={index} label={block.label} action={block.action} invoke={invoke} active={active} />;
          case "actions":
            return (
              <div key={index} className="flex flex-wrap gap-3">
                {block.actions.map((action) => (
                  <ActionForm
                    key={action.id}
                    action={action}
                    invoke={invoke}
                    active={active}
                  />
                ))}
              </div>
            );
        }
      })}
    </section>
  );
}
