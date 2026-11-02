import type { MailOperation } from "@webdock/mail-core";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { isIP } from "node:net";
import { setTimeout } from "node:timers/promises";
import { StalwartClient } from "../stalwart/client.ts";
export type InstanceCredentials = { bootstrapPassword?: string; username?: string; password?: string };
export type InstanceAddress = { internalURL: string; publicURL: string };
const execute = promisify(execFile);
const image = "stalwartlabs/stalwart:v0.16.25@sha256:74e5a7d55303ba525d939c6bf97ed4e010df7521f52d80afc22a815b66bd53f3";
type DockerObject = {
  Labels?: Record<string, string>; Config?: { Labels?: Record<string, string>; Env?: string[]; Image?: string };
  State?: { Running: boolean }; Internal?: boolean;
  NetworkSettings?: { Networks: Record<string, { IPAddress: string }> };
  Mounts?: { Name: string; Destination: string; Type: string }[];
};

/** Runs only on a trusted local host worker. The HTTP applications never receive Docker access. */
export class DockerMailRuntime {
  private readonly socket: string;
  private readonly namespace: string;
  private readonly hostnameSuffix: string;
  constructor(options: { socket: string; namespace: string; hostnameSuffix: string }) {
    if (!/^unix:\/\/\/[^\s]+$/.test(options.socket) || !/^[a-z][a-z0-9-]{1,45}$/.test(options.namespace) ||
      !/^(?:[a-z0-9](?:[a-z0-9-]*[a-z0-9])?\.)+[a-z]{2,}$/.test(options.hostnameSuffix))
      throw new Error("Invalid Mail worker host configuration");
    this.socket = options.socket; this.namespace = options.namespace; this.hostnameSuffix = options.hostnameSuffix;
  }
  private name(operation: MailOperation) {
    if (!/^[1-9][0-9]{0,18}$/.test(operation.customerID) || operation.instanceKey !== `mail-${operation.customerID}`)
      throw new Error("Mail instance binding is invalid");
    return `${this.namespace}-${operation.instanceKey}`;
  }
  private async docker(args: string[], missingAllowed = false, extraEnv: Record<string, string> = {}): Promise<string | null> {
    const env = { ...process.env, ...extraEnv }; delete env.DOCKER_CONTEXT;
    try {
      const result = await execute("docker", ["--host", this.socket, ...args], { env, timeout: 20_000, maxBuffer: 1_048_576 });
      return result.stdout.trim();
    } catch (error) {
      const details = error as { code?: number; stderr?: string };
      if (missingAllowed && details.code === 1 && /No such (object|container|volume|network)|not found/i.test(details.stderr || "")) return null;
      throw new Error(`Mail host ${args[0]} operation failed; inspect before retrying`);
    }
  }
  private async inspect(kind: "container" | "network" | "volume", name: string): Promise<DockerObject | null> {
    const result = await this.docker([kind, "inspect", name], true);
    if (result === null) return null;
    const parsed = JSON.parse(result);
    if (!Array.isArray(parsed) || parsed.length !== 1) throw new Error("Invalid Mail host inspection result");
    return parsed[0];
  }
  private ownership(resource: DockerObject, operation: MailOperation) {
    const labels = resource.Config?.Labels || resource.Labels;
    if (labels?.["webdock.mail.namespace"] !== this.namespace || labels?.["webdock.mail.customer"] !== operation.customerID)
      throw new Error("Existing Mail host resource belongs to another instance");
  }
  private async container(operation: MailOperation) {
    const name = this.name(operation), resource = await this.inspect("container", name);
    if (resource) {
      this.ownership(resource, operation);
      if (resource.Config?.Image !== image || Object.keys(resource.NetworkSettings?.Networks || {}).some(key => key !== name) ||
        !["config", "data"].every(part => resource.Mounts?.some(mount => mount.Name === `${name}-${part}` && mount.Type === "volume" && mount.Destination === (part === "config" ? "/etc/stalwart" : "/var/lib/stalwart"))))
        throw new Error("Existing Mail instance configuration differs; operator review required");
    }
    return resource;
  }
  private async create(operation: MailOperation, bootstrapPassword?: string) {
    const name = this.name(operation);
    const labels = ["--label", `webdock.mail.namespace=${this.namespace}`, "--label", `webdock.mail.customer=${operation.customerID}`];
    const extra: Record<string, string> = { STALWART_PUBLIC_URL: `https://${operation.instanceKey}.${this.hostnameSuffix}` };
    if (bootstrapPassword) extra.STALWART_RECOVERY_ADMIN = `admin:${bootstrapPassword}`;
    await this.docker(["create", "--name", name, ...labels, "--network", name, "--restart", "unless-stopped",
      "--memory", "1g", "--cpus", "1", "--log-opt", "max-size=10m", "--log-opt", "max-file=3",
      "--mount", `type=volume,src=${name}-config,dst=/etc/stalwart`, "--mount", `type=volume,src=${name}-data,dst=/var/lib/stalwart`,
      "--env", "STALWART_PUBLIC_URL", ...(bootstrapPassword ? ["--env", "STALWART_RECOVERY_ADMIN"] : []), image], false, extra);
  }
  private async address(operation: MailOperation) {
    const resource = await this.container(operation);
    const ip = resource?.NetworkSettings?.Networks[this.name(operation)]?.IPAddress;
    if (!ip || isIP(ip) !== 4) throw new Error("Mail instance has no private network address");
    return `http://${ip}:8080`;
  }
  private client(url: string, username: string, password: string) {
    return new StalwartClient({ url, authorization: `Basic ${Buffer.from(`${username}:${password}`).toString("base64")}`, allowInsecureHttp: true, timeoutMs: 3000 });
  }
  private async ready<T>(read: () => Promise<T>): Promise<T> {
    const deadline = Date.now() + 15_000;
    for (;;) {
      try { return await read(); }
      catch (error) { if (Date.now() >= deadline) throw error; await setTimeout(250); }
    }
  }
  async provision(operation: MailOperation, credentials: InstanceCredentials, save: (credentials: InstanceCredentials) => Promise<void>): Promise<InstanceAddress> {
    if (!operation.enabled) throw new Error("Cannot provision an instance without email activation");
    if (!(credentials.username && credentials.password) && !credentials.bootstrapPassword) throw new Error("Persist instance credentials before provisioning");
    const name = this.name(operation);
    for (const part of ["config", "data"]) {
      const existing = await this.inspect("volume", `${name}-${part}`);
      if (existing) this.ownership(existing, operation);
      else {
        if (credentials.username) throw new Error("Mail volume is missing; restore the existing instance before resuming");
        await this.docker(["volume", "create", "--label", `webdock.mail.namespace=${this.namespace}`, "--label", `webdock.mail.customer=${operation.customerID}`, `${name}-${part}`]);
      }
    }
    const network = await this.inspect("network", name);
    if (network) {
      this.ownership(network, operation);
      if (!network.Internal) throw new Error("Mail instance network permits unreviewed egress");
    } else await this.docker(["network", "create", "--internal", "--label", `webdock.mail.namespace=${this.namespace}`, "--label", `webdock.mail.customer=${operation.customerID}`, name]);
    let resource = await this.container(operation);
    if (!resource) { await this.create(operation, credentials.username ? undefined : credentials.bootstrapPassword); resource = await this.container(operation); }
    if (!resource?.State?.Running) await this.docker(["start", name]);
    if (!credentials.username || !credentials.password) {
      const client = this.client(await this.address(operation), "admin", credentials.bootstrapPassword!);
      await this.ready(() => client.call("x:Bootstrap/get", { ids: ["singleton"] }));
      const result = await client.call("x:Bootstrap/set", { update: { singleton: {
        serverHostname: `${operation.instanceKey}.${this.hostnameSuffix}`, defaultDomain: `${operation.instanceKey}.${this.hostnameSuffix}`,
        requestTlsCertificate: false, generateDkimKeys: false, tracer: { "@type": "Stdout", level: "warn" },
      } } });
      const admin = (result.updated as { singleton?: { username?: string; secret?: string } } | undefined)?.singleton;
      if (!admin?.username || !admin.secret) throw new Error("Bootstrap outcome unknown; recover the existing instance");
      credentials = { ...credentials, username: admin.username, password: admin.secret };
      await save(credentials);
    }
    resource = await this.container(operation);
    if (resource?.Config?.Env?.some(value => value.startsWith("STALWART_RECOVERY_ADMIN="))) {
      await this.docker(["stop", "--time", "5", name]);
      await this.docker(["rm", name]); // Configuration and mail volumes are deliberately retained.
      await this.create(operation);
      await this.docker(["start", name]);
    }
    const internalURL = await this.address(operation);
    const client = this.client(internalURL, credentials.username!, credentials.password!);
    await this.ready(() => client.call("x:Domain/get", { ids: null }));
    await save({ username: credentials.username, password: credentials.password });
    return { internalURL, publicURL: `https://${operation.instanceKey}.${this.hostnameSuffix}` };
  }
  async suspend(operation: MailOperation): Promise<void> {
    const resource = await this.container(operation);
    if (resource?.State?.Running) await this.docker(["stop", "--time", "5", this.name(operation)]);
  }
}
