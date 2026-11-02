import {msgid} from '@webdock/i18n';

import { getRequestI18n } from '@webdock/i18n/next';
import Link from "next/link";
import {requireRegistryOperator as requireOperator} from "../lib/registry-page";
import {studioCall} from "../lib/studio-client";
import type {VercelView} from "@webdock/api-client/integrations";
import { VercelLinkForm } from "./vercel-link-form";
export async function VercelIntegration({
  status,
  cursor,
}: {
  status?: string;
  cursor?: string;
}) {
  const i18n = await getRequestI18n();

  const actor=await requireOperator(),view=await studioCall<VercelView>("integrations.vercel",[cursor||""]),settings=view.configured;
  const message =
    status === "connected"
      ? msgid("Vercel connected. Choose a project below.")
      : status === "disconnected"
        ? msgid("Vercel disconnected from Studio.")
        : status === "failed"
          ? msgid("The Vercel connection could not be verified. Start from Studio and select the configured team with the required project permissions.")
          : null;
  const credential=view.connected,available=view.available,problem=view.problem?msgid("Vercel could not be reached. Reconnect or review its selected project permissions."):"";
  const projects = credential
    ? await actor.payload.find({
        collection: "projects",
        where: { status: { equals: "active" } },
        limit: 100,
        depth: 0,
        sort: "name",
        user: actor.user,
        overrideAccess: false,
      })
    : null;
  return (
    <section className="panel" id="vercel">
      <div className="section-heading">
        <h2>{i18n.t("Vercel")}</h2>
        <span
          className={"badge " + (credential && !problem ? "connected" : "")}
        >
          {credential
            ? i18n.t("Connected")
            : settings
              ? i18n.t("Not connected")
              : i18n.t("Setup required")}
        </span>
      </div>
      <p>{i18n.t("View hosting status and domains. Deleting a project additionally requires project write permission in Vercel.")}</p>
      {message && (
        <p className="status-notice" role="status">
          {i18n.t(message)}
        </p>
      )}
      {problem && (
        <p className="error" role="alert">
          {i18n.error(problem)}
        </p>
      )}
      {!settings ? (
        <p>{i18n.t("Register the private Webdock Studio integration in Vercel and configure its client credentials to enable connection.")}</p>
      ) : (
        <>
          <form action="/api/vercel/connect" method="post">
            <button className="button">
              {credential ? i18n.t("Reconnect Vercel") : i18n.t("Connect Vercel")}
            </button>
          </form>
          {credential && (
            <>
              <VercelLinkForm
                projects={projects?.docs || []}
                vercelProjects={available.projects}
              />
              <p className="muted">{i18n.t("Existing CMS hosting references are used automatically. You can also link projects that have no CMS.")}</p>
              <nav className="pagination" aria-label={i18n.t("Vercel project pages")}>
                {cursor && (
                  <Link href="/integrations#vercel">{i18n.t("First Vercel projects")}</Link>
                )}
                {available.nextCursor && (
                  <Link
                    href={
                      "/integrations?vercelCursor=" +
                      encodeURIComponent(available.nextCursor) +
                      "#vercel"
                    }
                  >{i18n.t("More Vercel projects")}</Link>
                )}
              </nav>
              <details>
                <summary>{i18n.t("Disconnect")}</summary>
                <form action="/api/vercel/disconnect" method="post">
                  <label className="field">
                    <span>{i18n.t("Disconnect the team connection from Studio")}</span>
                    <input type="checkbox" required />
                  </label>
                  <button className="button secondary">{i18n.t("Disconnect from Studio")}</button>
                </form>
                <p className="muted">{i18n.t("Project mappings are preserved. To revoke Vercel’s permission completely, remove this integration in Vercel.")}</p>
                <a
                  href="https://vercel.com/dashboard/integrations"
                  target="_blank"
                  rel="noreferrer"
                >{i18n.t("Manage Vercel permissions ↗")}</a>
              </details>
            </>
          )}
        </>
      )}
    </section>
  );
}
