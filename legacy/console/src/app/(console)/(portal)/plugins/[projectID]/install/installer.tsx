"use client";

import Link from "next/link";
import { useEffect, useRef, useState, type FormEvent } from "react";
import type { GitHubView } from "@webdock/api-client/integrations";
import type {
  PluginImport,
  PluginSource,
  PluginSourceCommand,
  PluginSourceView,
} from "@webdock/api-client/plugin-sources";
import {
  encodePluginChunk,
  gitPluginSource,
  parsePluginConfiguration,
  shouldPollPluginImports,
  shouldRetryPluginActivation,
  validatePluginArchive,
} from "@/lib/plugin-source-ui";
import { pluginSourceAction } from "./actions";

const notices: Record<string, string> = {
  connected: "GitHub connected. Choose a repository below.",
  disconnected: "GitHub disconnected from this browser.",
  failed: "GitHub authorization did not complete. Connect again.",
  expired: "Your GitHub connection expired. Connect again.",
  setup: "Complete the GitHub App setup before connecting.",
};

export function PluginInstaller({
  projectID,
  initialSources,
  github,
  githubNotice,
}: {
  projectID: string;
  initialSources: PluginSourceView;
  github: GitHubView;
  githubNotice?: string;
}) {
  const [sources, setSources] = useState(initialSources);
  const [sourceKind, setSourceKind] = useState<PluginSource["kind"]>("github");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [pollPaused, setPollPaused] = useState(false);
  const pollAttempts = useRef(0);
  const [activationAttempts, setActivationAttempts] = useState(0);
  const path = `/plugins/${encodeURIComponent(projectID)}/install`;
  const pending = shouldPollPluginImports(sources.imports, 0);

  useEffect(() => {
    if (
      !sources.configured ||
      busy ||
      pollPaused ||
      !shouldPollPluginImports(sources.imports, pollAttempts.current)
    )
      return;
    let cancelled = false;
    const timer = setTimeout(async () => {
      pollAttempts.current += 1;
      try {
        const ready = sources.imports.find((job) => job.status === "ready");
        const result = await pluginSourceAction(
          ready
            ? { action: "activate", id: ready.id }
            : { action: "list", projectID },
        );
        if (cancelled) return;
        if (!result.ok) {
          if (
            ready &&
            pollAttempts.current < 200 &&
            shouldRetryPluginActivation(result.status, activationAttempts + 1)
          ) {
            setActivationAttempts((count) => count + 1);
            return;
          }
          setError(result.error);
          setPollPaused(true);
          return;
        }
        setActivationAttempts(0);
        if ("imports" in result.data) setSources(result.data);
        else {
          const updated = result.data;
          setSources((current) => ({
            ...current,
            imports: current.imports.map((job) =>
              job.id === updated.id ? updated : job,
            ),
          }));
        }
        if (pollAttempts.current >= 200) setPollPaused(true);
      } catch {
        if (!cancelled) {
          setError(
            "Import status could not be refreshed. Use Refresh status to try again.",
          );
          setPollPaused(true);
        }
      }
    }, 3000);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [
    sources.imports,
    sources.configured,
    projectID,
    busy,
    pollPaused,
    activationAttempts,
  ]);

  async function run(command: PluginSourceCommand) {
    const result = await pluginSourceAction(command);
    if (!result.ok) throw new Error(result.error);
    if ("imports" in result.data) setSources(result.data);
    else {
      const job = result.data;
      setSources((current) => ({
        ...current,
        imports: [job, ...current.imports.filter((item) => item.id !== job.id)],
      }));
    }
    return result.data;
  }

  async function refresh() {
    setBusy(true);
    setError("");
    try {
      await run({ action: "list", projectID });
      pollAttempts.current = 0;
      setActivationAttempts(0);
      setPollPaused(false);
    } catch {
      setError("Import status could not be refreshed. Try again.");
    } finally {
      setBusy(false);
    }
  }

  async function create(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = event.currentTarget;
    const data = new FormData(form);
    let source: PluginSource;
    let archive: File | undefined;
    try {
      const ref = String(data.get("ref") || "").trim();
      if (sourceKind === "github") {
        source = {
          kind: "github",
          installation: github.installation,
          installationPage: github.installationPage,
          page: github.page,
          repository: Number(data.get("repository")),
          ...(ref ? { ref } : {}),
        };
      } else if (sourceKind === "git") {
        source = gitPluginSource(
          String(data.get("url") || ""),
          ref,
          String(data.get("username") || ""),
          String(data.get("token") || ""),
        );
      } else {
        const file = data.get("archive");
        if (!(file instanceof File)) throw new Error("Choose a ZIP archive.");
        validatePluginArchive(file);
        archive = file;
        source = { kind: "zip", name: file.name, size: file.size };
      }
    } catch (problem) {
      setError(
        problem instanceof Error ? problem.message : "Check the source fields.",
      );
      return;
    }
    // Clear credentials immediately; they are never stored in component state or browser storage.
    for (const name of ["username", "token"]) {
      const input = form.elements.namedItem(name);
      if (input instanceof HTMLInputElement) input.value = "";
      data.delete(name);
    }
    setBusy(true);
    setError("");
    try {
      const created = await run({
        action: "create",
        projectID,
        source,
        mode: "build",
        idempotencyKey: crypto.randomUUID(),
      });
      if ("imports" in created)
        throw new Error("Unexpected installer response.");
      if (archive) {
        for (let offset = 0; offset < archive.size; offset += 64 * 1024) {
          const bytes = new Uint8Array(
            await archive.slice(offset, offset + 64 * 1024).arrayBuffer(),
          );
          await run({
            action: "chunk",
            id: created.id,
            offset,
            data: encodePluginChunk(bytes),
          });
        }
        await run({ action: "submit", id: created.id });
      }
      pollAttempts.current = 0;
      setActivationAttempts(0);
      setPollPaused(false);
    } catch {
      setError(
        "The import could not be completed. Refresh status before starting another import.",
      );
    } finally {
      setBusy(false);
    }
  }

  async function mutate(command: PluginSourceCommand) {
    setBusy(true);
    setError("");
    try {
      await run(command);
      pollAttempts.current = 0;
      setActivationAttempts(0);
      setPollPaused(false);
    } catch {
      setError(
        "The plugin change could not be completed. Refresh status before retrying.",
      );
    } finally {
      setBusy(false);
    }
  }

  const browseURL = (input: Record<string, string>) =>
    `${path}?${new URLSearchParams(input)}`;
  const githubAction = (action: string) =>
    `/api/github/${action}?${new URLSearchParams({ returnTo: path })}`;
  return (
    <>
      {!sources.configured && (
        <section className="panel">
          <h2>Installer setup required</h2>
          <p>
            The plugin installer is not configured. Configure source storage,
            the build worker, image registry and deployment runner before
            importing plugins.
          </p>
        </section>
      )}
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      {githubNotice && notices[githubNotice] && (
        <p role="status">{notices[githubNotice]}</p>
      )}
      <section className="panel" aria-labelledby="plugin-source-heading">
        <h2 id="plugin-source-heading">Plugin source</h2>
        <div
          className="workspace-actions"
          role="group"
          aria-label="Source type"
        >
          {(["github", "git", "zip"] as const).map((kind) => (
            <button
              key={kind}
              className={`button ${sourceKind === kind ? "" : "secondary"}`}
              type="button"
              aria-pressed={sourceKind === kind}
              disabled={busy}
              onClick={() => setSourceKind(kind)}
            >
              {kind === "github"
                ? "GitHub"
                : kind === "git"
                  ? "Git URL"
                  : "ZIP upload"}
            </button>
          ))}
        </div>
        {sourceKind === "github" && (
          <>
            {!github.configured ? (
              <p>
                GitHub App setup is required to browse repositories. Grant
                read-only Contents access to import source from selected
                repositories.
              </p>
            ) : (
              <>
                {github.session ? (
                  <>
                    <p>
                      Connected as <strong>{github.session.login}</strong>.
                      Private repositories are available when granted to the
                      GitHub App.
                    </p>
                    <div className="workspace-actions">
                      {github.installURL && (
                        <a href={github.installURL}>
                          Manage selected repositories
                        </a>
                      )}
                      <form action={githubAction("disconnect")} method="post">
                        <button className="text-button" disabled={busy}>
                          Disconnect this browser
                        </button>
                      </form>
                    </div>
                  </>
                ) : (
                  <form action={githubAction("connect")} method="post">
                    <button className="button" disabled={busy}>
                      Connect GitHub
                    </button>
                  </form>
                )}
                {github.problem && (
                  <>
                    <p className="error" role="alert">
                      GitHub access could not be checked. Reconnect or review
                      the app’s repository access.
                    </p>
                    <form action={githubAction("connect")} method="post">
                      <button className="button secondary" disabled={busy}>
                        Reconnect GitHub
                      </button>
                    </form>
                  </>
                )}
                {github.session && !github.problem && (
                  <>
                    {github.installations.length ? (
                      <form className="editor" action={path} method="get">
                        <input
                          type="hidden"
                          name="installationPage"
                          value={github.installationPage}
                        />
                        <div className="field wide">
                          <label htmlFor="plugin-installation">
                            GitHub account or organisation
                          </label>
                          <select
                            id="plugin-installation"
                            name="installation"
                            defaultValue={github.installation}
                            disabled={busy}
                          >
                            {github.installations.map((item) => (
                              <option key={item.id} value={item.id}>
                                {item.account}
                              </option>
                            ))}
                          </select>
                        </div>
                        <div className="form-footer">
                          <button className="button secondary" disabled={busy}>
                            Browse repositories
                          </button>
                        </div>
                      </form>
                    ) : (
                      <p>
                        No accessible installations. Install the GitHub App and
                        grant access to your plugin repository.
                      </p>
                    )}
                    {github.installationPages > 1 && (
                      <nav
                        className="pagination"
                        aria-label="GitHub account pages"
                      >
                        {github.installationPage > 1 && (
                          <Link
                            href={browseURL({
                              installationPage: String(
                                github.installationPage - 1,
                              ),
                            })}
                          >
                            Previous accounts
                          </Link>
                        )}
                        <span>
                          Accounts page {github.installationPage} of{" "}
                          {github.installationPages}
                        </span>
                        {github.installationPage < github.installationPages && (
                          <Link
                            href={browseURL({
                              installationPage: String(
                                github.installationPage + 1,
                              ),
                            })}
                          >
                            Next accounts
                          </Link>
                        )}
                      </nav>
                    )}
                  </>
                )}
              </>
            )}
          </>
        )}
        <form className="editor" onSubmit={create}>
          <fieldset
            disabled={busy || !sources.configured}
            className="field wide"
          >
            <legend>Import settings</legend>
            {sourceKind === "github" && (
              <div className="field wide">
                <label htmlFor="plugin-repository">GitHub repository</label>
                <select
                  id="plugin-repository"
                  name="repository"
                  defaultValue=""
                  required
                  disabled={
                    !github.session ||
                    github.problem ||
                    !github.repositories.length
                  }
                >
                  <option value="" disabled>
                    Choose a repository
                  </option>
                  {github.repositories.map((repo) => (
                    <option key={repo.id} value={repo.id}>
                      {repo.name} · {repo.private ? "Private" : "Public"}
                    </option>
                  ))}
                </select>
                {github.session &&
                  !github.problem &&
                  !github.repositories.length && (
                    <small>
                      No repositories on this page. Check the GitHub App’s
                      selected repositories.
                    </small>
                  )}
              </div>
            )}
            {sourceKind === "git" && (
              <>
                <div className="field wide">
                  <label htmlFor="plugin-git-url">HTTPS Git URL</label>
                  <input
                    id="plugin-git-url"
                    name="url"
                    type="url"
                    placeholder="https://git.example.com/team/plugin.git"
                    required
                    autoComplete="off"
                  />
                </div>
                <div className="field wide">
                  <label htmlFor="plugin-git-user">
                    Username (private repository)
                  </label>
                  <input
                    id="plugin-git-user"
                    name="username"
                    autoComplete="off"
                  />
                </div>
                <div className="field wide">
                  <label htmlFor="plugin-git-token">
                    Access token (private repository)
                  </label>
                  <input
                    id="plugin-git-token"
                    name="token"
                    type="password"
                    autoComplete="new-password"
                    aria-describedby="plugin-token-hint"
                  />
                  <small id="plugin-token-hint">
                    Credentials are used only to fetch this import and are
                    cleared from this form on submission. Do not include
                    credentials in the URL.
                  </small>
                </div>
              </>
            )}
            {sourceKind === "zip" ? (
              <div className="field wide">
                <label htmlFor="plugin-archive">Plugin ZIP archive</label>
                <input
                  id="plugin-archive"
                  name="archive"
                  type="file"
                  accept=".zip,application/zip"
                  required
                  aria-describedby="plugin-archive-hint"
                />
                <small id="plugin-archive-hint">
                  Maximum 32 MiB. Include tabularium.json, a Dockerfile and the
                  plugin source.
                </small>
              </div>
            ) : (
              <div className="field wide">
                <label htmlFor="plugin-ref">
                  Branch, tag or commit (optional)
                </label>
                <input id="plugin-ref" name="ref" autoComplete="off" />
                <small>
                  Leave empty to use the repository’s default branch.
                </small>
              </div>
            )}
            <p>
              The installer builds an image automatically. After you review and
              approve its permissions, it deploys and activates the plugin.
            </p>
            <div className="form-footer">
              <button
                className="button"
                type="submit"
                disabled={
                  sourceKind === "github" &&
                  (!github.session ||
                    github.problem ||
                    !github.repositories.length)
                }
              >
                {busy ? "Working…" : "Import plugin"}
              </button>
            </div>
          </fieldset>
        </form>
        {sourceKind === "github" &&
          github.session &&
          !github.problem &&
          github.repositoryPages > 1 && (
            <nav className="pagination" aria-label="GitHub repository pages">
              {github.page > 1 && (
                <Link
                  href={browseURL({
                    installation: String(github.installation),
                    installationPage: String(github.installationPage),
                    page: String(github.page - 1),
                  })}
                >
                  Previous repositories
                </Link>
              )}
              <span>
                Repositories page {github.page} of {github.repositoryPages}
              </span>
              {github.page < github.repositoryPages && (
                <Link
                  href={browseURL({
                    installation: String(github.installation),
                    installationPage: String(github.installationPage),
                    page: String(github.page + 1),
                  })}
                >
                  Next repositories
                </Link>
              )}
            </nav>
          )}
      </section>
      <section aria-labelledby="plugin-imports-heading">
        <div className="section-heading">
          <h2 id="plugin-imports-heading">Imports</h2>
          <button
            className="button secondary"
            type="button"
            disabled={busy}
            onClick={refresh}
          >
            Refresh status
          </button>
        </div>
        {pending && (
          <p role="status">
            {pollPaused
              ? "Automatic refresh paused. Refresh status to continue."
              : "Checking import status every 3 seconds, for up to 10 minutes."}
          </p>
        )}
        {activationAttempts > 0 && !pollPaused && (
          <p role="status">
            Waiting for the runtime configuration to reach the API. Activation
            retries every 3 seconds for up to one minute.
          </p>
        )}
        {!sources.imports.length && <p>No plugin imports yet.</p>}
        {sources.imports.map((job) => (
          <PluginImportCard
            key={`${job.id}:${job.manifestSha256 || ""}`}
            job={job}
            busy={busy}
            configured={sources.configured}
            retryActivation={pollPaused}
            mutate={mutate}
          />
        ))}
      </section>
    </>
  );
}

function PluginImportCard({
  job,
  busy,
  configured,
  retryActivation,
  mutate,
}: {
  job: PluginImport;
  busy: boolean;
  configured: boolean;
  retryActivation: boolean;
  mutate: (command: PluginSourceCommand) => Promise<void>;
}) {
  const [approved, setApproved] = useState(false);
  const [configuration, setConfiguration] = useState("{}");
  const [error, setError] = useState("");
  async function approve(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!approved || !job.manifest || !job.manifestSha256) return;
    let parsed: Record<string, unknown>;
    try {
      parsed = parsePluginConfiguration(configuration);
    } catch {
      setError("Configuration must be a JSON object.");
      return;
    }
    setError("");
    await mutate({
      action: "approve",
      id: job.id,
      manifestSha256: job.manifestSha256,
      capabilities: job.manifest.capabilities,
      configuration: parsed,
    });
    setApproved(false);
  }
  return (
    <article className="panel">
      <div className="section-heading">
        <h3>{job.manifest?.name || job.source.label}</h3>
        <span className="badge" role="status">
          {job.status}
        </span>
      </div>
      <p>{job.source.label} · Build from source</p>
      {job.source.commit && (
        <p>
          Commit: <code>{job.source.commit}</code>
        </p>
      )}
      {job.source.sha256 && (
        <p style={{ overflowWrap: "anywhere" }}>
          Archive SHA-256: <code>{job.source.sha256}</code>
        </p>
      )}
      {job.status === "uploading" && (
        <>
          <label htmlFor={`upload-${job.id}`}>
            Uploaded {job.uploadedBytes.toLocaleString()} of{" "}
            {job.expectedBytes.toLocaleString()} bytes
          </label>
          <progress
            id={`upload-${job.id}`}
            max={job.expectedBytes || 1}
            value={job.uploadedBytes}
          />
          <p>Keep this page open until the upload finishes.</p>
        </>
      )}
      {job.status === "failed" && (
        <p className="error" role="alert">
          {job.error ||
            "The import failed. Check the installer configuration and source before trying again."}
        </p>
      )}
      {job.manifest && (
        <details open={job.status === "review"}>
          <summary>Manifest and permissions</summary>
          <p>
            {job.manifest.name} · {job.manifest.version}
          </p>
          <p>{job.manifest.description}</p>
          <dl>
            <dt>Exact image</dt>
            <dd style={{ overflowWrap: "anywhere" }}>
              <code>{job.manifest.image}</code>
            </dd>
            <dt>Manifest SHA-256</dt>
            <dd style={{ overflowWrap: "anywhere" }}>
              <code>{job.manifestSha256}</code>
            </dd>
          </dl>
          <h4>Requested capabilities</h4>
          {job.manifest.capabilities.length ? (
            <ul>
              {job.manifest.capabilities.map((capability) => (
                <li key={capability}>
                  <code>{capability}</code>
                </li>
              ))}
            </ul>
          ) : (
            <p>No capabilities requested.</p>
          )}
          <details>
            <summary>Configuration schema</summary>
            <pre style={{ whiteSpace: "pre-wrap", overflowWrap: "anywhere" }}>
              {JSON.stringify(job.manifest.configuration, null, 2)}
            </pre>
          </details>
        </details>
      )}
      {job.status === "review" && job.manifest && job.manifestSha256 && (
        <form className="editor" onSubmit={approve}>
          <div className="field wide">
            <label htmlFor={`config-${job.id}`}>
              Plugin configuration (JSON object)
            </label>
            <textarea
              id={`config-${job.id}`}
              rows={8}
              value={configuration}
              onChange={(event) => {
                setConfiguration(event.target.value);
                setApproved(false);
              }}
              disabled={busy}
              spellCheck={false}
            />
          </div>
          <div className="field wide">
            <label htmlFor={`approve-${job.id}`}>
              I approve this exact image, manifest, configuration and all listed
              capabilities.
            </label>
            <input
              id={`approve-${job.id}`}
              type="checkbox"
              checked={approved}
              onChange={(event) => setApproved(event.target.checked)}
              disabled={busy || !configured}
              required
            />
          </div>
          {error && (
            <p className="error" role="alert">
              {error}
            </p>
          )}
          <div className="form-footer">
            <button
              className="button"
              disabled={busy || !configured || !approved}
            >
              Approve and deploy
            </button>
          </div>
        </form>
      )}
      {job.status === "ready" && (
        <>
          <p role="status">
            Deployment is ready.{" "}
            {retryActivation
              ? "Automatic activation paused. Retry when ready."
              : "Activating the approved plugin automatically…"}
          </p>
          {retryActivation && (
            <button
              className="button"
              disabled={busy || !configured}
              onClick={() => mutate({ action: "activate", id: job.id })}
            >
              Retry activation
            </button>
          )}
        </>
      )}
      {["uploading", "queued", "review"].includes(job.status) && (
        <button
          className="text-button"
          disabled={busy}
          onClick={() => mutate({ action: "discard", id: job.id })}
        >
          Discard import
        </button>
      )}
      {job.status === "enabled" && (
        <p className="success" role="status">
          Plugin enabled.{" "}
          <Link href={`/plugins/${encodeURIComponent(job.projectID)}`}>
            Open project extensions
          </Link>
        </p>
      )}
    </article>
  );
}
