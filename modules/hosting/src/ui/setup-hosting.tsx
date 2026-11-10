import * as React from "react";
import { randomUUID } from "node:crypto";
import Link from "next/link";
import { getRequestI18n } from "@spitzli/i18n/next";
import type { SetupOptions } from "@spitzli/hosting-contracts";
import { HostingForm } from "./form";
export async function SetupHosting({ options }: { options: SetupOptions }) {
  const { t } = await getRequestI18n();
  if (options.project.provider === "vercel")
    return (
      <section className="panel">
        <h2>{t("Vercel application")}</h2>
        <p>
          {t(
            "Continue with this project’s existing Vercel deployment configuration.",
          )}
        </p>
        <Link
          className="button"
          href={`/hosting/projects/${options.project.projectID}/git/setup`}
        >
          {t("Configure deployments")}
        </Link>
      </section>
    );
  return (
    <section className="panel">
      <h2>{t("Choose hosting for this project")}</h2>
      {!options.project.canAssignHosting ? (
        <p>
          {t(
            "Ask your operator to assign hosting before deploying applications.",
          )}
        </p>
      ) : !options.clusters.length ? (
        <p>{t("No verified hosting cluster is available yet.")}</p>
      ) : (
        <HostingForm
          command={{
            action: "projects.create",
            projectID: options.project.projectID,
            provider: "k3s",
            clusterID: options.clusters[0].id,
            mode: "selfservice",
            ownImages: false,
            confirmSharedImages: false,
            idempotencyKey: randomUUID(),
          }}
          label={t("Assign hosting")}
        >
          <label className="field">
            {t("Cluster")}
            <select name="clusterID">
              {options.clusters.map((c) => (
                <option key={c.id} value={c.id}>
                  {c.name} ({c.country ?? "—"} · {c.region ?? "—"})
                </option>
              ))}
            </select>
          </label>
          <input type="hidden" name="ownImagesPresent" value="yes" />
          <label className="check">
            <input type="checkbox" name="ownImages" value="yes" />
            {t("Allow repository builds and custom container images")}
          </label>
          <label className="check">
            <input type="checkbox" name="confirmSharedImages" value="yes" />
            {t(
              "Also allow this project's images on a shared cluster (applications share the host kernel)",
            )}
          </label>
        </HostingForm>
      )}
    </section>
  );
}
