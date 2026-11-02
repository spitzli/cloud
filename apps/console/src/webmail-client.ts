import type { z } from "zod";
import { exchangeSchema, gatewayCommand, httpsOrigin, launchSchema, uploadSchema, savedSchema, updatedSchema, submissionSchema, type GatewayCommand, type WebmailLaunch, type MailAttachment } from "./webmail-contracts";

export class WebmailError extends Error {
  constructor(message: string, readonly denied = false, readonly uncertain = false) { super(message); }
}
function live(signal: AbortSignal, expires = Infinity) {
  if (signal.aborted) throw new WebmailError("Mailbox connection closed.", true);
  if (Date.now() >= expires) throw new WebmailError("Mail access expired. Reconnect to verify access.", true);
}
async function parsed<T>(response: Response, schema: z.ZodType<T>, write: boolean): Promise<T> {
  if ([401, 403].includes(response.status)) throw new WebmailError("Mail access expired or was revoked.", true);
  const raw = await response.text();
  if (raw.length > 4 * 1024 * 1024) throw new WebmailError("Mail response exceeds the supported size.", false, write);
  let value: unknown;
  try { value = JSON.parse(raw); } catch { throw new WebmailError("Mail returned an unreadable response.", false, write); }
  if (!response.ok) {
    const uncertain = write && (response.status >= 500 || response.status === 409);
    throw new WebmailError(uncertain ? "The change outcome is unknown. Review the mailbox before another change." : "Mail request was not accepted.", false, uncertain);
  }
  const result = schema.safeParse(value);
  if (!result.success) throw new WebmailError("Mail returned an unsupported response.", false, write);
  return result.data;
}
function expiration(value: string, maximum: number) {
  const time = Date.parse(value);
  if (!Number.isFinite(time) || time <= Date.now() || time > Date.now() + maximum)
    throw new WebmailError("Invalid or expired Mail handoff.");
  return time;
}
export async function openWebmail(
  launch: (signal: AbortSignal) => Promise<WebmailLaunch>,
  configuredOrigin: string, customerID: string, mailboxID: string, signal: AbortSignal,
  fetcher: typeof fetch = fetch,
): Promise<WebmailConnection> {
  const origin = httpsOrigin.parse(configuredOrigin);
  const admitted = launchSchema.parse(await launch(signal)).data;
  live(signal);
  if (admitted.gatewayOrigin !== origin || admitted.customerID !== customerID || admitted.mailboxID !== mailboxID)
    throw new WebmailError("Mail launch does not match this mailbox.");
  expiration(admitted.expiresAt, 65_000);
  const response = await fetcher(`${origin}/session`, {
    method: "POST", credentials: "include", cache: "no-store", redirect: "error",
    headers: { "Content-Type": "application/json" }, body: JSON.stringify({ code: admitted.code }),
    signal: AbortSignal.any([signal, AbortSignal.timeout(30_000)]),
  });
  const exchanged = await parsed(response, exchangeSchema, false);
  if (exchanged.customerID !== customerID || exchanged.mailboxID !== mailboxID)
    throw new WebmailError("Mail session does not match this mailbox.");
  const connection = new WebmailConnection(origin, exchanged.scopeID, expiration(exchanged.expiresAt, 16 * 60_000), fetcher);
  if (signal.aborted) { connection.close(); live(signal); }
  return connection;
}
export class WebmailConnection {
  #controller = new AbortController();
  #fetcher: typeof fetch;
  #base: string;
  readonly expiresAt: number;
  #denied = new Set<() => void>();
  constructor(origin: string, scope: string, expiresAt: number, fetcher: typeof fetch) {
    this.#base = `${origin}/s/${scope}`;
    this.expiresAt = expiresAt;
    // Browser fetch throws "Illegal invocation" when called with this connection as receiver.
    this.#fetcher = (input, init) => fetcher(input, init);
  }
  // A method, not a getter: TypeScript must not narrow it across awaits.
  isClosed() { return this.#controller.signal.aborted; }
  onDenied(listener: () => void): () => void {
    this.#denied.add(listener);
    return () => { this.#denied.delete(listener); };
  }
  deny() {
    this.close();
    for (const listener of [...this.#denied]) listener();
  }
  close() {
    if (this.isClosed()) return;
    this.#controller.abort();
    void this.#fetcher(`${this.#base}/logout`, {
      method: "POST", credentials: "include", cache: "no-store", redirect: "error",
      headers: { "Content-Type": "application/json" }, body: "{}",
      signal: AbortSignal.timeout(5000),
    }).catch(() => undefined);
  }
  async #request(path: string, options: RequestInit, write: boolean, signal?: AbortSignal): Promise<Response> {
    live(this.#controller.signal, this.expiresAt);
    try {
      const signals = [this.#controller.signal, AbortSignal.timeout(30_000)];
      if (signal) signals.push(signal);
      const response = await this.#fetcher(`${this.#base}/${path}`, {
        ...options, method: "POST", credentials: "include", cache: "no-store", redirect: "error", signal: AbortSignal.any(signals),
      });
      live(this.#controller.signal, this.expiresAt);
      if (signal) live(signal);
      if ([401, 403].includes(response.status)) {
        this.deny();
        throw new WebmailError("Mail access expired or was revoked.", true);
      }
      return response;
    } catch (error) {
      if (error instanceof WebmailError) throw error;
      if (this.isClosed() || signal?.aborted) throw new WebmailError("Mailbox connection closed.", true);
      throw new WebmailError("Mail connection interrupted. Review the mailbox before repeating a change.", false, write);
    }
  }
  async call<T>(input: GatewayCommand, schema: z.ZodType<T>, signal?: AbortSignal): Promise<T> {
    const command = gatewayCommand.parse(input);
    const write = ["update", "draft", "submit"].includes(command.action) ||
      (command.action === "submission" && command.operation !== "status");
    const response = await this.#request("mail", { headers: { "Content-Type": "application/json" }, body: JSON.stringify(command) }, write, signal);
    const result = await parsed(response, schema, write);
    live(this.#controller.signal, this.expiresAt);
    if (signal) live(signal);
    return result;
  }
  async upload(file: File): Promise<MailAttachment> {
    if (file.size > 10485760 || !file.name || file.name.length > 200 || /[/\\\0\r\n]/.test(file.name))
      throw new WebmailError("Use an attachment of at most 10 MiB with a valid filename.");
    const response = await this.#request("upload", {
      headers: { "Content-Type": file.type || "application/octet-stream", "X-Webdock-Filename": encodeURIComponent(file.name) },
      body: file,
    }, false);
    const result = await parsed(response, uploadSchema, false);
    live(this.#controller.signal, this.expiresAt);
    return result.data;
  }
  async download(emailId: string, blobId: string): Promise<Blob> {
    const response = await this.#request("download", {
      headers: { "Content-Type": "application/json" }, body: JSON.stringify({ emailId, blobId }),
    }, false);
    if (!response.ok) throw new WebmailError("Attachment download failed.");
    if (Number(response.headers.get("Content-Length") ?? "0") > 10485760) throw new WebmailError("Attachment exceeds 10 MiB.");
    const blob = await response.blob();
    live(this.#controller.signal, this.expiresAt);
    if (blob.size > 10485760) throw new WebmailError("Attachment exceeds 10 MiB.");
    return blob;
  }
}
export type WriteCommand = Extract<GatewayCommand, { action: "draft" | "update" | "submit" | "submission" }>;
export async function writeMail(connection: WebmailConnection, command: WriteCommand) {
  switch (command.action) {
    case "draft": return { kind: "draft" as const, command, value: (await connection.call(command, savedSchema)).data };
    case "update": return { kind: "update" as const, command, value: (await connection.call(command, updatedSchema)).data };
    case "submit":
    case "submission": return { kind: "submission" as const, command, value: (await connection.call(command, submissionSchema)).data };
  }
}
export type WriteOutcome = Awaited<ReturnType<typeof writeMail>>;
