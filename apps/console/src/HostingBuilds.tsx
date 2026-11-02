import { useEffect, useRef, useState, type SyntheticEvent } from "react";
import { Link } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useApi, useSession } from "./context";
import { useHostingCommand, uncertainOutcome } from "./hosting-client";
import { projectSchema } from "./hosting-contracts";
import { HostingPagination } from "./Hosting";
import { ActionFailure } from "./HostingActionState";
import { Empty, Failure, Loading } from "./common";
import { buildLogEventsPath, readBuildLogs, useBuildCommand } from "./build-client";
import { buildOutcome, buildPolling, buildSchema, buildsPageSchema, canCancelBuild, emptyLogWindow, logPageSchema, mergeLogPage, releaseSchema, releasesPageSchema, sourceGetSchema, sourcesSchema, sourceSchema, sourceSettingsCommand, type Build, type BuildCommand, type BuildRelease, type BuildSource, type SourceSettings, webhookReceiptSchema, type WebhookReceipt, type LogPage, type LogWindow } from "./build-contracts";
import * as ui from "./ui";
import { ApiError } from "./api";
import {useI18n} from "@webdock/i18n/react";
import {msgid,type I18n} from "@webdock/i18n";
import {buildCommandSchema,gitConnectionsSchema,gitRepositoriesSchema,type BuildSourceInfo} from "./build-contracts";

type WriteCommand = Extract<BuildCommand, { action: "git.builds.request" | "git.builds.cancel" | "git.source.configure" | "git.source.webhook.configure" | "git.releases.approve" | "git.releases.rollback" }>;
type SaveSource=(command:WriteCommand)=>Promise<unknown>;
export function HostingBuilds({ projectID }: { projectID: string }) { return <ProjectBuilds key={projectID} projectID={projectID} />; }

function ProjectBuilds({ projectID }: { projectID: string }) {
  const i18n=useI18n();const {t,date}=i18n;
  const api = useApi(), session = useSession(), call = useBuildCommand(), hosting = useHostingCommand(), client = useQueryClient();
  const enabled = api.config.enabledFeatures.includes("hosting");
  const preview = session.user.preview === true && session.user.acting !== true;
  const [target, setTarget] = useState(""), [page, setPage] = useState(1), [selected, setSelected] = useState<string>();
  const project = useQuery({ queryKey: ["hosting", "project", projectID], queryFn: ({ signal }) => hosting({ action: "projects.get", projectID }, projectSchema, signal), enabled });
  const configured=enabled && project.data?.configured === true;
  const sources = useQuery({ queryKey: ["git-builds", projectID, "sources"], queryFn: ({ signal }) => call({ action: "git.sources.list", projectID }, sourcesSchema, signal), enabled:configured });
  const selectedTarget = target || sources.data?.docs[0]?.targetID;
  const source = useQuery({ queryKey: ["git-builds", projectID, "source", selectedTarget], queryFn: ({ signal }) => call({ action: "git.source.get", projectID, ...(selectedTarget ? { targetID: selectedTarget } : {}) }, sourceGetSchema, signal), enabled: configured && sources.data !== undefined });
  const history = useQuery({ queryKey: ["git-builds", projectID, "history", page], queryFn: ({ signal }) => call({ action: "git.builds.list", projectID, page, limit: 20 }, buildsPageSchema, signal), enabled:configured, refetchInterval: (query) => !query.state.error && query.state.data?.docs.some((build) => buildPolling(build.status, false)) ? 1000 : false });

  const mutation = useMutation({
    mutationFn: (command: WriteCommand): Promise<Build | BuildSource | BuildRelease | WebhookReceipt> => {
      if (command.action === "git.source.configure") return call(command, sourceSchema);
      if(command.action==="git.source.webhook.configure") return call(command,webhookReceiptSchema);
      if (command.action === "git.releases.approve" || command.action === "git.releases.rollback") return call(command, releaseSchema);
      return call(command, buildSchema);
    }, retry: false, gcTime: 0,
    onSuccess: (result,command) => { if ("sourceSHA" in result) setSelected(result.id); if(command.action==="git.builds.request") setPage(1); void client.invalidateQueries({ queryKey: ["git-builds", projectID] }); },
  });
  const uncertain = uncertainOutcome(mutation.error), locked = mutation.isPending || uncertain;
  const canBuild = !preview && source.data?.canBuild === true, canConfigure = !preview && source.data?.canConfigure === true, canApprove = !preview && source.data?.canApprove === true;
  const selectedID = selected ?? history.data?.docs[0]?.id;
  if (!enabled) return <Empty title={t("Builds are unavailable")}>{t("Hosting is not enabled for this installation.")}</Empty>;
  if (project.error) return <Failure error={project.error} retry={() => void project.refetch()} />;
  if (!project.data) return <Loading>{t("Loading project…")}</Loading>;
  if (!project.data.configured) return <Empty title={t("Hosting is not configured")}>{t("Configure hosting for this project before connecting a repository source.")}</Empty>;
  const requiredError = sources.error ?? source.error;
  if (requiredError) return <Failure error={requiredError} retry={() => { void sources.refetch(); void source.refetch(); }} />;
  if (!sources.data) return <Loading>{t("Loading sources…")}</Loading>;
  return <section>
    <div className={ui.pageHeading}><div>
      <Link className={ui.backLink} to="/hosting/projects/$projectID" params={{ projectID }}>{t("Applications")}</Link>
      <h1 className={ui.pageTitle}>{t("{project} builds",{project:project.data.name})}</h1>
      <p className={ui.pageLead}>{t("Build repository commits, review their output, and follow publication separately.")}</p>
    </div><button className={ui.button()} disabled={history.isFetching} onClick={() => { void history.refetch(); void client.invalidateQueries({ queryKey: ["git-builds", projectID, "releases"] }); void source.refetch(); }}>{t("Refresh")}</button></div>
    {preview ? <p className={`${ui.notice} mb-6`}>{t("Customer preview is read-only. Build history and retained logs remain available.")}</p> : null}
    <ActionFailure error={mutation.error} uncertain={uncertain} pending={mutation.isPending} retry={() => { if (mutation.variables) mutation.mutate(mutation.variables); }} />
    {mutation.isSuccess ? <p role="status" className="mb-4 text-sm text-success">{t("Request recorded.")}</p> : null}
    <div className="mb-8 grid gap-4">
      {sources.data.docs.length ? <div className="flex flex-wrap items-end gap-3"><label className={`${ui.field} min-w-0 flex-1 sm:max-w-[420px]`}>{t("Repository source")}<select className={ui.input} value={selectedTarget ?? ""} disabled={locked} onChange={(event) => { setTarget(event.target.value); }}>{sources.data.docs.map((entry) => <option key={entry.id} value={entry.targetID}>{sourceLabel(entry,source.data?.targets,t("Repository"))}</option>)}</select></label>
        {canBuild ? <button className={ui.button("primary")} disabled={requestDisabled(locked,source.data)} onClick={() => { if (selectedTarget) mutation.mutate({ action: "git.builds.request", projectID, targetID: selectedTarget, idempotencyKey: crypto.randomUUID() }); }}>{mutation.isPending ? t("Recording request…") : t("Build latest commit")}</button> : null}
      </div> : <Empty title={t("No repository source")}>{t("Project managers can connect an authorized repository below. Your provider must enable new GitHub account access.")}</Empty>}
      {source.data?.blockers.map((blocker) => <p key={blocker} className={ui.notice}>{blockerMessage(blocker,i18n)}</p>)}
      <SourceConfiguration info={source.data} canConfigure={canConfigure} provider={project.data.provider} projectID={projectID} customerID={project.data.customerID} recorded={recordedSource(mutation.data)} locked={locked} save={command=>mutation.mutateAsync(command)} />
    </div>
    <div className="grid items-start gap-7 xl:grid-cols-[minmax(0,1fr)_minmax(0,1.1fr)]">
      <section aria-label={t("Build history")}><h2 className={`${ui.sectionTitle} mb-3`}>{t("Build history")}</h2>
        {history.isPending ? <Loading>{t("Loading build history…")}</Loading> : history.error ? <Failure error={history.error} retry={() => void history.refetch()} /> : history.data.docs.length ? <>
          <div className={ui.tableFrame}><table className={ui.table}><thead><tr><th className={ui.th} scope="col">{t("Commit")}</th><th className={ui.th} scope="col">{t("Build status")}</th></tr></thead><tbody>{history.data.docs.map((build) => <tr className={ui.tr} key={build.id}><td className={ui.td}><button type="button" className="text-left font-medium underline-offset-4 hover:underline" aria-pressed={selectedID === build.id} onClick={() => { setSelected(build.id); }}>{build.sourceSHA.slice(0, 8)}</button><span className="mt-1 block text-xs text-muted">{when(build.createdAt,date,t("Not recorded"))}</span></td><td className={ui.td}><BuildBadge status={build.status} /><span className="mt-1 block text-xs text-muted">{t(phaseLabel(build.phase,build.status))}</span></td></tr>)}</tbody></table></div>
          <HostingPagination page={history.data.page} totalPages={history.data.totalPages} onPage={(next) => { setPage(next); setSelected(undefined); }} />
        </> : <Empty title={t("No builds yet")}>{t("A requested build will appear here once it has been recorded.")}</Empty>}
      </section>
      {selectedID && !history.error ? <BuildDetail key={selectedID} projectID={projectID} id={selectedID} canWrite={canBuild} preview={preview} locked={locked} cancel={(id) => { mutation.mutate({ action: "git.builds.cancel", projectID, buildID: id }); }} /> : null}
    </div>
    <PublicationHistory projectID={projectID} canApprove={canApprove} locked={locked} mutate={(command) => { mutation.mutate(command); }} />
  </section>;
}

function SourceConfiguration({info,canConfigure,provider,projectID,customerID,recorded,locked,save}:{info:BuildSourceInfo|undefined;canConfigure:boolean;provider:string|null;projectID:string;customerID:string;recorded:BuildSource|undefined;locked:boolean;save:SaveSource}){
  const {t}=useI18n();
  if(!canConfigure || !info)return null;
  if(info.source)return <SourceSettings key={info.source.id} source={info.source} recorded={recorded} locked={locked} save={command=>{void save(command).catch(()=>undefined);}} />;
  if(provider!=="k3s" && provider!=="docker")return <Empty title={t("Repository setup is unavailable")}>{t("This hosting provider does not support repository source setup here.")}</Empty>;
  return <ConnectSource projectID={projectID} customerID={customerID} targets={info.targets} locked={locked} save={save} />;
}

function ConnectSource({projectID,customerID,targets,locked,save}:{projectID:string;customerID:string;targets:{id:string;name:string}[];locked:boolean;save:SaveSource}){
  const {t}=useI18n(),call=useBuildCommand();
  const saving=useRef(false);
  const [connection,setConnection]=useState(""),[repository,setRepository]=useState(""),[target,setTarget]=useState(""),[branch,setBranch]=useState<string|undefined>(),[rootDirectory,setRootDirectory]=useState("."),[strategy,setStrategy]=useState<"auto"|"dockerfile"|"railpack">("auto"),[error,setError]=useState(false);
  const connections=useQuery({queryKey:["git-connections",customerID],queryFn:({signal})=>call({action:"git.connections.list",customerID},gitConnectionsSchema,signal)});
  const active=connections.data?.docs.filter(value=>value.state==="active") ?? [];
  const selectedConnection=active.find(value=>value.id===(connection || active[0]?.id));
  const repositories=useQuery({queryKey:["git-repositories",customerID,selectedConnection?.id],queryFn:({signal})=>call({action:"git.repositories.list",customerID,connectionID:selectedConnection?.id},gitRepositoriesSchema,signal),enabled:selectedConnection!==undefined});
  const availableRepositories=repositories.data?.docs ?? [];
  const selectedRepository=availableRepositories.find(value=>value.repositoryID===(repository || availableRepositories[0]?.repositoryID));
  const selectedTarget=targets.find(value=>value.id===(target || targets[0]?.id));
  if(connections.isPending)return <Loading>{t("Loading GitHub connections…")}</Loading>;
  if(connections.error)return <Failure error={connections.error} retry={()=>void connections.refetch()} />;
  if(!active.length)return <Empty title={t("No active GitHub connection")}>{t("Ask your provider to connect your GitHub account and authorize repositories for this workspace.")}<button className={ui.button("quiet")} onClick={()=>void connections.refetch()}>{t("Refresh connections")}</button></Empty>;
  if(!targets.length)return <Empty title={t("No application target")}>{t("Create an application in this project before connecting a repository.")}</Empty>;
  const submit=(event:SyntheticEvent<HTMLFormElement>)=>{
    event.preventDefault();if(saving.current || locked || !selectedConnection || !selectedRepository || !selectedTarget)return;
    const parsed=sourceSettingsCommandForNewSource({projectID,targetID:selectedTarget.id,connectionID:selectedConnection.id,repositoryID:selectedRepository.repositoryID,branch:branch ?? selectedRepository.defaultBranch ?? "main",rootDirectory,strategy});
    if(!parsed){setError(true);return;}setError(false);saving.current=true;
    void save(parsed).catch(()=>undefined).finally(()=>{saving.current=false;});
  };
  return <form aria-label={t("Connect a repository")} className="grid gap-4 rounded-lg border border-outline p-4 sm:grid-cols-2" onSubmit={submit}>
    <h2 className={`${ui.sectionTitle} sm:col-span-2`}>{t("Connect a repository")}</h2>
    <p className="text-sm text-muted sm:col-span-2">{t("Select a repository already authorized for this workspace. Build access and publication are approved separately.")}</p>
    <label className={ui.field}>{t("Application")}<select className={ui.input} value={selectedTarget?.id ?? ""} disabled={locked} onChange={event=>{setTarget(event.target.value);}}><option value="" disabled>{t("Select an application")}</option>{targets.map(value=><option key={value.id} value={value.id}>{value.name}</option>)}</select></label>
    <label className={ui.field}>{t("GitHub account")}<select className={ui.input} value={selectedConnection?.id ?? ""} disabled={locked} onChange={event=>{setConnection(event.target.value);setRepository("");setBranch(undefined);}}><option value="" disabled>{t("Select a GitHub account")}</option>{active.map(value=><option key={value.id} value={value.id}>{value.accountLogin}</option>)}</select></label>
    <label className={ui.field}>{t("Repository")}<select className={ui.input} value={selectedRepository?.repositoryID ?? ""} disabled={locked || repositories.isPending || !!repositories.error || !availableRepositories.length} onChange={event=>{setRepository(event.target.value);setBranch(undefined);}}><option value="" disabled>{t("Select a repository")}</option>{availableRepositories.map(value=><option key={value.repositoryID} value={value.repositoryID}>{value.fullName ?? t("Authorized repository")}</option>)}</select></label>
    <label className={ui.field}>{t("Branch")}<input className={ui.input} value={branch ?? selectedRepository?.defaultBranch ?? "main"} disabled={locked} required maxLength={200} onChange={event=>{setBranch(event.target.value);}} /></label>
    <label className={ui.field}>{t("Root directory")}<input className={ui.input} value={rootDirectory} disabled={locked} required maxLength={240} onChange={event=>{setRootDirectory(event.target.value);}} /></label>
    <label className={ui.field}>{t("Build strategy")}<select className={ui.input} value={strategy} disabled={locked} onChange={event=>{setStrategy(event.target.value==="railpack"?"railpack":event.target.value==="dockerfile"?"dockerfile":"auto");}}><option value="auto">{t("Automatic")}</option><option value="dockerfile">{t("Dockerfile")}</option><option value="railpack">{t("Railpack")}</option></select></label>
    {repositories.error ? <div className="sm:col-span-2"><Failure error={repositories.error} retry={()=>void repositories.refetch()} /></div> : repositories.isPending ? <p className="text-sm text-muted sm:col-span-2">{t("Loading repositories…")}</p> : !availableRepositories.length ? <p className={`${ui.notice} sm:col-span-2`}>{t("No repositories are authorized for this account. Ask your provider to update its repository access.")}<button type="button" className={ui.button("quiet")} disabled={repositories.isFetching} onClick={()=>{void repositories.refetch();}}>{t("Refresh repositories")}</button></p> : null}
    {error ? <p role="alert" className="text-sm text-danger sm:col-span-2">{t("Check the branch and relative root directory.")}</p> : null}
    <div className="sm:col-span-2"><button className={ui.button("primary")} disabled={locked || !selectedRepository || !selectedConnection || !selectedTarget || repositories.isFetching || !!repositories.error}>{t("Connect repository")}</button></div>
  </form>;
}

function sourceSettingsCommandForNewSource(input:{projectID:string;targetID:string;connectionID:string;repositoryID:string;branch:string;rootDirectory:string;strategy:"auto"|"dockerfile"|"railpack"}):Extract<BuildCommand,{action:"git.source.configure"}>|undefined{
  const parsed=buildCommandSchema.safeParse({action:"git.source.configure",projectID:input.projectID,targetID:input.targetID,connectionID:input.connectionID,repositoryID:input.repositoryID,branch:input.branch,rootDirectory:input.rootDirectory,recipe:"dockerfile",revision:0,enabled:true,autoPublish:false,healthPath:"/",workflowPath:".github/workflows/webdock.yml",artifactPrefix:"webdock",executionBackend:"kubernetes",containerBuild:{strategy:input.strategy,contextDirectory:".",dockerfile:"Dockerfile",buildArgs:{}}});
  return parsed.success && parsed.data.action==="git.source.configure" ? parsed.data : undefined;
}

function PublicationHistory({ projectID,canApprove,locked,mutate }:{projectID:string;canApprove:boolean;locked:boolean;mutate:(command:WriteCommand)=>void}) {
  const i18n=useI18n();const {t,date}=i18n;
  const call=useBuildCommand();
  const [releasePage,setReleasePage]=useState(1);
  const releases = useQuery({ queryKey: ["git-builds", projectID, "releases", releasePage], queryFn: ({ signal }) => call({ action: "git.releases.list", projectID, page: releasePage, limit: 20 }, releasesPageSchema, signal), refetchInterval: (query) => !query.state.error && query.state.data?.docs.some((release) => ["queued", "deploying"].includes(release.status)) ? 1000 : false });
  return <section className="mt-9" aria-label={t("Publications")}><h2 className={`${ui.sectionTitle} mb-2`}>{t("Publications")}</h2><p className="mb-4 text-sm text-muted">{t("Build success confirms the build output. Deployment has its own status and approval.")}</p>
      {releases.isPending ? <Loading>{t("Loading publications…")}</Loading> : releases.error ? <Failure error={releases.error} retry={() => void releases.refetch()} /> : releases.data.docs.length ? <>
        <div className={ui.tableFrame}><table className={ui.table}><thead><tr><th className={ui.th} scope="col">{t("Requested")}</th><th className={ui.th} scope="col">{t("Deployment status")}</th><th className={ui.th} scope="col">{t("Actions")}</th></tr></thead><tbody>{releases.data.docs.map((release) => <tr key={release.id} className={ui.tr}><td className={ui.td}>{when(release.createdAt,date,t("Not recorded"))}</td><td className={ui.td}><span className={ui.badge(ui.statusTone(release.status))}>{t(releaseLabel(release.status))}</span></td><td className={ui.td}><div className="flex flex-wrap gap-2">
          {release.operationID ? <Link className={ui.button("quiet")} to="/hosting/operations/$operationID" params={{ operationID: release.operationID }}>{t("Rollout operation")}</Link> : null}
          {canApprove && release.status === "awaiting-approval" ? <button className={ui.button()} disabled={locked} onClick={() => { mutate({ action: "git.releases.approve", projectID, releaseID: release.id, revision: release.revision, idempotencyKey: crypto.randomUUID() }); }}>{t("Approve publication")}</button> : null}
          {canApprove && release.status === "ready" ? <button className={ui.button()} disabled={locked} onClick={() => { mutate({ action: "git.releases.rollback", projectID, releaseID: release.id, revision: release.revision, idempotencyKey: crypto.randomUUID() }); }}>{t("Redeploy this version")}</button> : null}
        </div></td></tr>)}</tbody></table></div><HostingPagination page={releases.data.page} totalPages={releases.data.totalPages} onPage={setReleasePage} />
      </> : <p className="text-sm text-muted">{t("No retained build output has been submitted for publication.")}</p>}
    </section>;
}

function sourceLabel(source:BuildSource,targets:{id:string;name:string}[]|undefined,fallback:string):string {const target=targets?.find((entry)=>entry.id===source.targetID);return `${target?.name ?? source.repositoryName ?? fallback}: ${source.branch}`;}
function phaseLabel(value:string|undefined,status:Build["status"]):string {return ({queued:msgid("Queued"),provisioning:msgid("Preparing build environment"),cloning:msgid("Cloning"),preparing:msgid("Preparing"),building:msgid("Building"),pushing:msgid("Pushing"),"awaiting-approval":msgid("Awaiting approval"),succeeded:msgid("Build succeeded"),failed:msgid("Build failed"),cancelled:msgid("Cancelled")} as Record<string,string>)[value ?? ""] ?? buildOutcome(status);}
function blockerMessage(message:string,i18n:I18n):string {return message==="Repository provider adapter is unavailable." ? i18n.t(msgid("Repository provider adapter is unavailable.")) : i18n.error(message,msgid("Builds are unavailable"));}
function failureMessage(code:string):string {return ({SOURCE_CHANGED:msgid("The source changed before this build completed."),SUPERSEDED:msgid("A newer commit replaced this queued build."),KUBERNETES_JOB_RETIRED:msgid("Build execution ended before output was retained."),BUILD_DEADLINE_EXCEEDED:msgid("The build exceeded its time limit."),BUILD_MEMORY_LIMIT:msgid("The build exceeded its memory limit."),BUILD_RESOURCE_EVICTED:msgid("The build environment ran out of resources."),KUBERNETES_NODE_UNAVAILABLE:msgid("The build node is temporarily unavailable."),KUBERNETES_API_UNAVAILABLE:msgid("The build service is temporarily unavailable."),BUILD_FAILED:msgid("The build failed. Review the retained logs.")} as Record<string,string>)[code] ?? msgid("Build execution did not complete. Review the retained logs.");}
function requestDisabled(locked:boolean,data:{source:BuildSource|null;blockers:string[]}|undefined) {return locked || !data?.source?.enabled || data.blockers.length>0 || data.source.buildProvider!=="isolated" || data.source.recipe==="vercel";}

function BuildBadge({ status }: { status: Build["status"] }) { const {t}=useI18n();return <span className={ui.badge(status === "artifact-ready" ? "warning" : ui.statusTone(status))}>{t(buildOutcome(status))}</span>; }
function when(value:string|null,format:I18n["date"],empty:string) {return value ? format(value,{dateStyle:"medium",timeStyle:"short"}) : empty;}
function requestToCompletion(build:Build):number|undefined {
  if(!build.createdAt || !build.finishedAt)return undefined;
  const seconds=(Date.parse(build.finishedAt)-Date.parse(build.createdAt))/1000;
  return Number.isFinite(seconds) && seconds>=0 ? Math.round(seconds) : undefined;
}
function releaseLabel(status: BuildRelease["status"]): string { return ({ "awaiting-approval": msgid("Awaiting approval"), queued: msgid("Publication queued"), deploying: msgid("Deploying"), ready: msgid("Deployment ready"), failed: msgid("Rollout failed"), superseded: msgid("Superseded"), "needs-reconciliation": msgid("Needs review") })[status]; }

function BuildDetail({ projectID, id, canWrite, preview, locked, cancel }: { projectID: string; id: string; canWrite: boolean; preview: boolean; locked: boolean; cancel: (id: string) => void }) {
  const {t,date,number}=useI18n();
  const call = useBuildCommand();
  const build = useQuery({ queryKey: ["git-builds", projectID, "detail", id], queryFn: ({ signal }) => call({ action: "git.builds.get", projectID, buildID: id }, buildSchema, signal), refetchInterval: (query) => buildPolling(query.state.data?.status, query.state.status === "error") });
  if (build.isPending) return <Loading>{t("Loading build…")}</Loading>;
  if (build.error) return <Failure error={build.error} retry={() => void build.refetch()} />;
  const elapsed=requestToCompletion(build.data);
  return <section aria-label={t("Selected build")} className="min-w-0"><div className="mb-3 flex flex-wrap items-center justify-between gap-3"><h2 className={ui.sectionTitle}>{t("Build {commit}",{commit:build.data.sourceSHA.slice(0,8)})}</h2><BuildBadge status={build.data.status} /></div>
    <div className="mb-4 flex flex-wrap items-center gap-3 text-sm"><span className="text-muted">{t("Phase: {phase}",{phase:t(phaseLabel(build.data.phase,build.data.status))})}</span>{canCancelBuild(build.data.status, canWrite, preview) ? <button className={ui.button()} disabled={locked} onClick={() => { cancel(id); }}>{t("Cancel build")}</button> : null}</div>
    <dl className="mb-4 grid gap-x-6 gap-y-2 text-sm sm:grid-cols-2">
      <div><dt className="text-muted">{t("Attempt")}</dt><dd>{build.data.generation>0 ? number(build.data.generation) : t("Not started")}</dd></div>
      {build.data.executionBackend==="kubernetes" ? <div><dt className="text-muted">{t("Retries")}</dt><dd>{t("{count} of 2",{count:number(build.data.retryCount)})}</dd></div> : null}
      <div><dt className="text-muted">{t("Requested")}</dt><dd>{when(build.data.createdAt,date,t("Not recorded"))}</dd></div>
      {build.data.finishedAt ? <div><dt className="text-muted">{t("Finished")}</dt><dd>{when(build.data.finishedAt,date,t("Not recorded"))}</dd></div> : null}
      {elapsed!==undefined ? <div><dt className="text-muted">{t("Request to completion")}</dt><dd>{t("{seconds} s",{seconds:number(elapsed)})}</dd></div> : null}
    </dl>
    {build.data.status==="queued" && build.data.retryCount>0 && build.data.nextEligibleAt ? <p className={`${ui.notice} mb-4`} role="status">{t("Eligible for retry after {date}",{date:when(build.data.nextEligibleAt,date,t("Not recorded"))})}</p> : null}
    {build.data.lastInfrastructureFailure ? <p className="mb-4 text-sm text-muted">{t("Previous build environment issue: {reason}",{reason:t(failureMessage(build.data.lastInfrastructureFailure))})}</p> : null}
    {build.data.status === "artifact-ready" ? <p className={`${ui.notice} mb-4`}>{t("Build output is retained and awaiting publication. Registry publication has not been confirmed.")}</p> : null}
    {build.data.failureCode ? <p className="mb-4 text-sm text-danger" role="alert">{t(failureMessage(build.data.failureCode))}</p> : null}
    <BuildLogs key={id} projectID={projectID} build={build.data} />
  </section>;
}

interface LogState {page:LogPage;window:LogWindow;origin:"history"|"stream"}
function authorityFailure(error:unknown):boolean {return error instanceof ApiError && [401,403].includes(error.status);}
function clearLogs(state:LogState|undefined):LogState|undefined {return state ? {...state,page:{...state.page,entries:[],cursor:"0"},window:emptyLogWindow()} : undefined;}
function isAborted(signal:AbortSignal):boolean {return signal.aborted;}
function reconnectPause(signal:AbortSignal):Promise<void> {return new Promise((resolve)=>{const done=()=>{clearTimeout(timer);signal.removeEventListener("abort",done);resolve();};const timer=setTimeout(done,1000);signal.addEventListener("abort",done,{once:true});if(signal.aborted)done();});}
function BuildLogs({ projectID, build }: { projectID: string; build: Build }) {
  const i18n=useI18n();const {t}=i18n;
  const api=useApi(),client=useQueryClient(),retained=useRef(emptyLogWindow());
  const [streamError,setStreamError]=useState<Error|null>(null);
  const queryKey=["git-builds",projectID,"logs",build.id];
  const query=useQuery<LogState>({queryKey,gcTime:0,retry:false,
    queryFn:async({signal})=>{
      try {const page=await readBuildLogs(api,projectID,build.id,retained.current.cursor,signal);const window=mergeLogPage(retained.current,page);retained.current=window;return {page,window,origin:"history"};}
      catch(error){if(authorityFailure(error)){retained.current=emptyLogWindow();client.setQueryData<LogState>(["git-builds",projectID,"logs",build.id],clearLogs);}throw error;}
    },
    refetchInterval:(value)=>!value.state.error && value.state.data?.origin==="history" && value.state.data.page.entries.length===25 ? 50 : false,
  });
  const ready=query.isSuccess && !query.isFetching && (query.data.origin==="stream" || query.data.page.entries.length<25);
  const terminal=query.data?.page.terminal===true && query.data.page.entries.length<25;
  const active=buildPolling(build.status,false)!==false;
  const wasActive=useRef(active);
  const refreshLogs=query.refetch;
  useEffect(()=>{
    if(wasActive.current && !active)void refreshLogs();
    wasActive.current=active;
  },[active,refreshLogs]);
  useEffect(()=>{
    if(!ready || !active || terminal || streamError)return;
    const controller=new AbortController();
    const run=async()=>{
      while(!isAborted(controller.signal)){
        try {await api.stream(buildLogEventsPath(projectID,build.id),logPageSchema,(page,cursor)=>{
          if(cursor!==page.cursor)throw new Error(t("The log event cursor is invalid."));
          const window=mergeLogPage(retained.current,page);retained.current=window;
          client.setQueryData<LogState>(["git-builds",projectID,"logs",build.id],{page,window,origin:"stream"});
          if(page.terminal && page.entries.length<25)controller.abort();
        },{signal:controller.signal,lastEventID:retained.current.cursor});}
        catch(error){if(isAborted(controller.signal))return;if(authorityFailure(error)){retained.current=emptyLogWindow();client.setQueryData<LogState>(["git-builds",projectID,"logs",build.id],clearLogs);}setStreamError(error instanceof Error ? error : new Error(t("Build logs are unavailable.")));return;}
        await reconnectPause(controller.signal);
      }
    };
    void run();return ()=>{controller.abort();};
  },[api,client,projectID,build.id,ready,active,terminal,streamError,t]);
  const error=streamError ?? query.error;
  if(error)return <Failure error={new Error(i18n.error(error.message,msgid("Build logs are unavailable.")))} retry={()=>{setStreamError(null);void query.refetch();}} />;
  const entries=query.data?.window.entries ?? [];
  return <div><div className="mb-2 flex items-center justify-between gap-3"><h3 className="text-sm font-medium">{t("Retained logs")}</h3><button className={ui.button("quiet")} disabled={query.isFetching} onClick={()=>{setStreamError(null);void query.refetch();}}>{t("Refresh logs")}</button></div>
    {query.isPending ? <Loading>{t("Loading logs…")}</Loading> : <pre aria-label={t("Build logs")} tabIndex={0} className="max-h-[60dvh] min-h-48 overflow-auto whitespace-pre-wrap break-all rounded-lg border border-outline bg-subtle p-4 font-mono text-xs leading-relaxed">{entries.length ? entries.map((entry)=>entry.text).join("\n") : build.logs.slice(-65536) || t("No log output yet.")}</pre>}
    {query.data?.window.truncated ? <p className={`${ui.hint} mt-2`}>{t("Only the most recent log output is displayed.")}</p> : null}
  </div>;
}

function recordedSource(value:Build|BuildSource|BuildRelease|WebhookReceipt|undefined):BuildSource|undefined {return value && "connectionID" in value ? value : undefined;}
function settings(source: BuildSource): SourceSettings { return { executionBackend: source.executionBackend, strategy: source.containerBuild.strategy, contextDirectory: source.containerBuild.contextDirectory, dockerfile: source.containerBuild.dockerfile }; }
function SourceSettings({ source, recorded, locked, save }: { source: BuildSource; recorded:BuildSource|undefined; locked: boolean; save: (command: WriteCommand) => void }) {
  const {t}=useI18n();
  const [snapshot, setSnapshot] = useState(source), [draft, setDraft] = useState(() => settings(source)), [error, setError] = useState<Error | null>(null);
  const acknowledged=recorded?.id===source.id && recorded.revision>=source.revision ? recorded : undefined;
  const baseline=acknowledged ?? snapshot;
  const changed=source.revision>baseline.revision;
  const blocked=locked || changed || source.revision<baseline.revision;
  const update = (value: Partial<SourceSettings>) => { setDraft((current) => ({ ...current, ...value })); setError(null); };
  const submit = (event: SyntheticEvent<HTMLFormElement>) => { event.preventDefault(); if (blocked) return; try { save(sourceSettingsCommand(baseline, draft)); setError(null); } catch (failure) { setError(failure instanceof Error ? failure : new Error("Check the build settings.")); } };
  if(source.buildProvider!=="isolated" || source.recipe==="vercel") return <p className={ui.notice}>{t("This source uses an external build provider.")}</p>;
  return <details className="rounded-lg border border-outline p-4"><summary className="cursor-pointer text-sm font-medium">{t("Build settings")}</summary><form className="mt-4 grid gap-4 sm:grid-cols-2" onSubmit={submit}>
    {changed ? <div className={`${ui.notice} sm:col-span-2`}>{t("The source changed. Reload its settings before saving.")}<button type="button" className={`${ui.button()} ml-3`} onClick={() => { setSnapshot(source); setDraft(settings(source)); setError(null); }}>{t("Reload settings")}</button></div> : null}
    <label className="flex items-start gap-2 text-sm sm:col-span-2"><input type="checkbox" checked={source.webhookEnabled} disabled={blocked || !source.enabled} onChange={(event)=>{save({action:"git.source.webhook.configure",projectID:source.projectID,sourceID:source.id,revision:source.revision,enabled:event.target.checked});}} /><span>{source.webhookTrigger==="workflow_run" ? t("Build after GitHub workflow succeeds") : t("Build on GitHub pushes")}<span className="mt-1 block text-xs text-muted">{source.webhookTrigger==="workflow_run" ? t("Only successful push runs of {workflow} may start automatic builds. Publication approval remains separate.",{workflow:source.workflowPath}) : t("Allow signed repository pushes to start builds. Publication approval remains separate.")}</span></span></label>
    <label className={ui.field}>{t("Build execution")}<select className={ui.input} value={draft.executionBackend} disabled={blocked} onChange={(event) => { update({ executionBackend: event.target.value === "kubernetes" ? "kubernetes" : "microvm" }); }}><option value="microvm" disabled={source.webhookTrigger==="workflow_run"}>{t("Existing isolated worker")}</option><option value="kubernetes">{t("Managed build cluster")}</option></select><span className={ui.hint}>{t("Build cluster access must be enabled for this project.")}</span></label>
    {draft.executionBackend === "kubernetes" && source.recipe === "dockerfile" ? <><label className={ui.field}>{t("Build strategy")}<select className={ui.input} value={draft.strategy} disabled={blocked} onChange={(event) => { update({ strategy: event.target.value === "railpack" ? "railpack" : event.target.value === "dockerfile" ? "dockerfile" : "auto" }); }}><option value="auto">{t("Automatic")}</option><option value="dockerfile">{t("Dockerfile")}</option><option value="railpack">{t("Railpack")}</option></select></label><label className={ui.field}>{t("Build context")}<input className={ui.input} value={draft.contextDirectory} disabled={blocked} maxLength={240} required onChange={(event) => { update({ contextDirectory: event.target.value }); }} /></label>{draft.strategy !== "railpack" ? <label className={ui.field}>{t("Dockerfile path")}<input className={ui.input} value={draft.dockerfile} disabled={blocked} maxLength={240} required onChange={(event) => { update({ dockerfile: event.target.value }); }} /></label> : null}</> : null}
    {error ? <p role="alert" className="text-sm text-danger sm:col-span-2">{t("Check the relative context and Dockerfile paths.")}</p> : null}<div className="sm:col-span-2"><button className={ui.button()} disabled={blocked}>{t("Save build settings")}</button></div>
  </form></details>;
}
