"use client";
import * as React from "react";
import { useI18n } from "@spitzli/i18n/react";
import type {
  SetupConfiguration,
  SetupOptions,
} from "@spitzli/hosting-contracts";
export function SetupSourceFields({
  configuration,
  options,
  onChange,
}: {
  configuration: SetupConfiguration;
  options: SetupOptions;
  onChange: (value: SetupConfiguration) => void;
}) {
  const { t } = useI18n(),
    source = configuration.source;
  const accounts = options.connections.filter((c) => c.state === "active");
  const account =
    source.kind === "git"
      ? accounts.find((c) => c.id === source.connectionID)
      : undefined;
  const setSource = (next: SetupConfiguration["source"]) =>
    onChange({ ...configuration, source: next });
  return (
    <>
      <h2>{t("Choose your source")}</h2>
      <fieldset className="setup-source-choices">
        <legend className="sr-only">{t("Application source")}</legend>
        {(["git", "image", "template"] as const).map((kind, index) => (
          <label
            key={kind}
            className={
              "setup-source-choice" + (source.kind === kind ? " selected" : "")
            }
          >
            <input
              type="radio"
              name="sourceKind"
              checked={source.kind === kind}
              onChange={() => {
                if (kind === "git")
                  setSource({
                    kind,
                    branch:
                      accounts[0]?.repositories[0]?.defaultBranch ?? "main",
                    rootDirectory: ".",
                    dockerfile: "Dockerfile",
                    buildProvider: options.builds.isolated
                      ? "isolated"
                      : options.builds.githubActions
                        ? "github-actions"
                        : "isolated",
                    workflowPath: ".github/workflows/webdock.yml",
                    artifactPrefix: "webdock",
                    connectionID: accounts[0]?.id,
                    repositoryID: accounts[0]?.repositories[0]?.repositoryID,
                  });
                else if (kind === "image") setSource({ kind, reference: "" });
                else
                  onChange({
                    ...configuration,
                    source: { kind, template: "healthcheck" },
                    spec: {
                      ...configuration.spec,
                      port: 8080,
                      healthPath: "/ping",
                    },
                  });
              }}
            />
            <span>
              <strong>
                {
                  [t("Git repository"), t("Container image"), t("Template")][
                    index
                  ]
                }
              </strong>
              <small>
                {
                  [
                    t("Build and deploy from your repository."),
                    t("Use an existing immutable image."),
                    t("Start with the HTTP healthcheck application."),
                  ][index]
                }
              </small>
            </span>
          </label>
        ))}
      </fieldset>
      {source.kind === "git" && (
        <>
          <div className="setup-fields">
            <label className="field">
              {t("GitHub account")}
              <select
                value={source.connectionID ?? ""}
                onChange={(e) => {
                  const c = accounts.find((c) => c.id === e.target.value);
                  setSource({
                    ...source,
                    connectionID: c?.id,
                    repositoryID: c?.repositories[0]?.repositoryID,
                    branch: c?.repositories[0]?.defaultBranch ?? "main",
                  });
                }}
              >
                <option value="">{t("Choose an account")}</option>
                {accounts.map((c) => (
                  <option key={c.id} value={c.id}>
                    {c.accountLogin}
                  </option>
                ))}
              </select>
            </label>
            <label className="field">
              {t("Repository")}
              <select
                value={source.repositoryID ?? ""}
                onChange={(e) => {
                  const r = account?.repositories.find(
                    (r) => r.repositoryID === e.target.value,
                  );
                  setSource({
                    ...source,
                    repositoryID: r?.repositoryID,
                    branch: r?.defaultBranch ?? source.branch,
                  });
                }}
              >
                <option value="">{t("Choose a repository")}</option>
                {account?.repositories.map((r) => (
                  <option key={r.repositoryID} value={r.repositoryID}>
                    {r.fullName}
                  </option>
                ))}
              </select>
            </label>
            <label className="field">
              {t("Branch")}
              <input
                required
                value={source.branch}
                maxLength={200}
                onChange={(e) =>
                  setSource({ ...source, branch: e.target.value })
                }
              />
            </label>
            <label className="field">
              {t("Root directory")}
              <input
                required
                value={source.rootDirectory}
                maxLength={240}
                onChange={(e) =>
                  setSource({ ...source, rootDirectory: e.target.value })
                }
              />
              <small>
                {t(
                  "For monorepos, choose the directory containing this application’s Dockerfile.",
                )}
              </small>
            </label>
          </div>
          <div className="workspace-actions">
            <button className="button secondary" name="intent" value="connect">
              {t("Connect GitHub")}
            </button>
            <button
              className="button secondary"
              name="intent"
              value="inspect"
              disabled={!source.connectionID || !source.repositoryID}
            >
              {t("Inspect repository")}
            </button>
          </div>
          <details>
            <summary>{t("Build settings")}</summary>
            <div className="setup-fields">
              <label className="field">
                {t("Build provider")}
                <select
                  value={source.buildProvider}
                  onChange={(e) =>
                    setSource({
                      ...source,
                      buildProvider: e.target.value as
                        "isolated" | "github-actions",
                    })
                  }
                >
                  <option value="isolated">
                    {t("Isolated Webdock worker")}
                  </option>
                  <option value="github-actions">GitHub Actions</option>
                </select>
              </label>
              {source.buildProvider === "github-actions" && (
                <>
                  <label className="field">
                    {t("GitHub Actions workflow")}
                    <input
                      required
                      value={source.workflowPath}
                      onChange={(e) =>
                        setSource({ ...source, workflowPath: e.target.value })
                      }
                    />
                  </label>
                  <label className="field">
                    {t("Artifact name prefix")}
                    <input
                      required
                      value={source.artifactPrefix}
                      onChange={(e) =>
                        setSource({ ...source, artifactPrefix: e.target.value })
                      }
                    />
                  </label>
                  <p>
                    {t(
                      "The workflow must already produce the Webdock artifact for the exact commit.",
                    )}
                  </p>
                </>
              )}
              <label className="check">
                <input
                  type="checkbox"
                  checked={configuration.autoPublish}
                  onChange={(e) =>
                    onChange({
                      ...configuration,
                      autoPublish: e.target.checked,
                    })
                  }
                />
                {t("Automatically publish successful builds")}
              </label>
            </div>
          </details>
        </>
      )}
      {source.kind === "image" && (
        <label className="field">
          {t("Image digest")}
          <input
            required
            value={source.reference}
            maxLength={512}
            placeholder="registry.example.com/project/app@sha256:…"
            onChange={(e) =>
              setSource({ ...source, reference: e.target.value })
            }
          />
          <small>
            {t(
              "Use an image already available on the cluster or in its authorized project registry.",
            )}
          </small>
        </label>
      )}
      {source.kind === "template" && (
        <p>
          {t(
            "The healthcheck template uses port 8080 and /ping. It helps verify the deployment before you add your own application.",
          )}
        </p>
      )}
    </>
  );
}
