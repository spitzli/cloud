import { getStudioSession } from "@/lib/studio-client";
import { ProjectApplicationsPanel, ProjectVercelPanel } from "@distribution/project-panels";
import {auditSummary} from "@/lib/ui-labels";
import {uiLabel} from "@/lib/ui-labels";

import { msgid } from '@webdock/i18n';

import { getRequestI18n } from '@webdock/i18n/next';

import Link from "next/link";
import { notFound } from "next/navigation";
import { Editor, ArchiveButton } from "../../../../../components/editor";
import {
  saveProject,
  archiveRecord,
} from "../../../../../lib/actions";
import { requireRegistryOperator } from "../../../../../lib/registry-page";
import {
  relatedID,
  date,
  hostname,
  validRecordID,
} from "../../../../../lib/presentation";
export default async function Project({
  params,
}: {
  params: Promise<{ id: string }>;
}) {
  const i18n = await getRequestI18n();

  const { id } = await params;
  if (!validRecordID(id)) notFound();
  const modules = (await getStudioSession())?.installation?.modules;
  const available = (id: string) => ["enabled", "draining"].includes(modules?.[id] ?? "");
  const { payload, user } = await requireRegistryOperator();
  const project = await payload.findByID({
    collection: "projects",
    id,
    user,
    overrideAccess: false,
    depth: 1,
    disableErrors: true,
  });
  if (!project) notFound();
  const [events, customers] = await Promise.all([
    payload.find({
      collection: "audit-events",
      where: {
        and: [
          { targetID: { equals: id } },
          { targetCollection: { equals: "projects" } },
        ],
      },
      limit: 5,
      sort: "-createdAt",
      user,
      overrideAccess: false,
      depth: 0,
    }),
    payload.find({
      collection: "customers",
      where: {
        or: [
          { status: { equals: "active" } },
          { id: { equals: relatedID(project.customer) } },
        ],
      },
      pagination: false,
      sort: "name",
      depth: 0,
      user,
      overrideAccess: false,
    }),
  ]);
  return (
    <>
      <Link className="back" href="/">{i18n.t("← Projects")}</Link>
      <div className="page-heading">
        <div>
          <h1>{project.name}</h1>
          <p>
            <Link href={"/customers/" + relatedID(project.customer)}>
              {typeof project.customer === "object"
                ? project.customer.name
                : i18n.t("Customer")}
            </Link>{" "}
            <span className="badge">{i18n.t("Record")}: {i18n.t(uiLabel(project.status))}</span>
          </p>
        </div>
        {project.url && (
          <a
            href={project.url}
            className="button secondary"
            target="_blank"
            rel="noreferrer"
          >{i18n.t("Visit website ↗")}</a>
        )}
      </div>
      {ProjectApplicationsPanel && available("hosting") && <ProjectApplicationsPanel projectID={id} customerID={relatedID(project.customer)} />}
      <Link className="button secondary" href={`/plugins/${id}`}>{i18n.t("Project extensions")}</Link>
      <div className="project-overview">
        <div>
          <span>{i18n.t("Website")}</span>
          {project.url ? (
            <a href={project.url} target="_blank" rel="noreferrer">
              {hostname(project.url,i18n.t("No domain yet"))} ↗
            </a>
          ) : (
            <strong>{i18n.t("No website linked")}</strong>
          )}
        </div>
        <div>
          <span>{i18n.t("Repository")}</span>
          {project.repositoryURL ? (
            <a href={project.repositoryURL} target="_blank" rel="noreferrer">{i18n.t("Open repository ↗")}</a>
          ) : (
            <strong>{i18n.t("No repository linked")}</strong>
          )}
        </div>
        <div>
          <span>{i18n.t("Last updated")}</span>
          <strong>{date(project.updatedAt,i18n.locale)}</strong>
        </div>
      </div>
      {ProjectVercelPanel && available("hosting") && <ProjectVercelPanel projectID={id} />}
      <div className="detail-grid">
        <div>
          <details className="workspace-disclosure">
            <summary>{i18n.t("Edit project details")}</summary>
            <Editor
              action={saveProject.bind(null, id)}
              fields={[
                {
                  name: "name",
                  label: msgid("Project name"),
                  required: true,
                  value: project.name,
                },
                {
                  name: "customer",
                  label: msgid("Customer"),
                  type: "select",
                  required: true,
                  value: relatedID(project.customer),
                  options: customers.docs.map((customer) => ({
                    value: customer.id,
                    translate: false,
                    label:
                      customer.name +
                      (customer.status === "archived" ? i18n.t(" (archived)") : ""),
                  })),
                },
                {
                  name: "url",
                  label: msgid("Website URL"),
                  type: "url",
                  value: project.url || "",
                },
                {
                  name: "repositoryURL",
                  label: msgid("Repository URL"),
                  type: "url",
                  value: project.repositoryURL || "",
                },
                {
                  name: "notes",
                  label: msgid("Notes"),
                  type: "textarea",
                  value: project.notes || "",
                },
              ]}
            />
          </details>
          <section className="panel">
            <div className="section-heading">
              <h2>{i18n.t("Recent changes")}</h2>
              <Link
                className="small-link"
                href={"/activity?collection=projects&target=" + id}
              >{i18n.t("View full history")}</Link>
            </div>
            {events.docs.map((e) => (
              <div className="activity-row" key={e.id}>
                <span>{auditSummary(e.summary,i18n.t)}</span>
                <small>{date(e.createdAt,i18n.locale)}</small>
              </div>
            ))}
            {!events.totalDocs && (
              <p className="muted">{i18n.t("No changes recorded yet.")}</p>
            )}
          </section>
        </div>
        <aside>
          <section className="record-meta">
            <p>{i18n.t("Project ID")}<code>{id}</code>
              <small>{i18n.t("Created ")}{date(project.createdAt,i18n.locale)}</small>
            </p>
            <ArchiveButton
              action={archiveRecord.bind(null, "projects", id)}
              archived={project.status === "archived"}
            />
          </section>
        </aside>
      </div>
    </>
  );
}
