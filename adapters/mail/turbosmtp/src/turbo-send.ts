import type { Fetch } from "./http";
import type { DispatchOutcome, EmailRequest } from "@spitzli/mail-contracts";
import { parseEmail } from "@spitzli/mail-contracts/validation";
type Credential = {
  consumerKey: string;
  consumerSecret: string;
  region: "eu" | "global";
};
/** One HTTP attempt only. The durable caller owns idempotency and dispatch fencing. */
export async function sendTurboMail(
  credential: Credential,
  body: EmailRequest,
  reference: string,
  signal: AbortSignal,
  send: Fetch = fetch,
): Promise<DispatchOutcome> {
  let parsed;
  try {
    if (
      !["eu", "global"].includes(credential.region) ||
      ![credential.consumerKey, credential.consumerSecret].every(
        (v) => typeof v === "string" && /^[\x21-\x7e]{1,4096}$/.test(v),
      ) ||
      !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(
        reference,
      )
    )
      throw Error();
    parsed = parseEmail(body, new Date(), { ignoreScheduleTime: true });
  } catch {
    return { kind: "rejected", code: "invalid_send_input" };
  }
  const request = parsed.request;
  const list = (value: string | string[] | undefined) =>
    value === undefined ? "" : Array.isArray(value) ? value.join(",") : value;
  const payload = {
    from: request.from,
    to: list(request.to),
    cc: list(request.cc),
    bcc: list(request.bcc),
    subject: request.subject,
    content: request.text,
    html_content: request.html,
    reference_id: reference,
    custom_headers: {
      ...request.headers,
      ...(list(request.reply_to) ? { "Reply-To": list(request.reply_to) } : {}),
    },
    ...(request.attachments?.length
      ? {
          attachments: request.attachments.map((a) => ({
            content: a.content,
            name: a.filename,
            type: a.content_type,
            content_id: a.content_id,
          })),
        }
      : {}),
  };
  const controller = new AbortController(),
    combined = AbortSignal.any([
      signal,
      controller.signal,
      AbortSignal.timeout(15000),
    ]);
  let reader: ReadableStreamDefaultReader<Uint8Array> | undefined;
  let stop: () => void = () => {};
  const aborted = new Promise<DispatchOutcome>((resolve) => {
    stop = () => resolve({ kind: "unknown", code: "provider_timeout" });
    combined.addEventListener("abort", stop, { once: true });
    if (combined.aborted) stop();
  });
  try {
    if (combined.aborted)
      return { kind: "rejected", code: "dispatch_deadline" };
    return await Promise.race([
      aborted,
      (async (): Promise<DispatchOutcome> => {
        const response = await send(
          `https://${credential.region === "eu" ? "api.eu.turbo-smtp.com" : "api.turbo-smtp.com"}/api/v2/mail/send`,
          {
            method: "POST",
            redirect: "error",
            cache: "no-store",
            signal: combined,
            headers: {
              "Content-Type": "application/json",
              consumerKey: credential.consumerKey,
              consumerSecret: credential.consumerSecret,
            },
            body: JSON.stringify(payload),
          },
        );
        if (!response.ok) {
          void response.body?.cancel().catch(() => {});
          return [400, 401, 403, 422].includes(response.status)
            ? { kind: "rejected", code: "provider_rejected" }
            : { kind: "unknown", code: "provider_response_uncertain" };
        }
        if (Number(response.headers.get("content-length")) > 65536)
          return { kind: "unknown", code: "provider_response_invalid" };
        reader = response.body?.getReader();
        if (!reader)
          return { kind: "unknown", code: "provider_response_invalid" };
        const chunks: Uint8Array[] = [];
        let size = 0;
        for (;;) {
          const part = await reader.read();
          if (part.done) break;
          size += part.value.byteLength;
          if (size > 65536)
            return { kind: "unknown", code: "provider_response_invalid" };
          chunks.push(part.value);
        }
        const text = new TextDecoder("utf-8", { fatal: true }).decode(
          Buffer.concat(chunks),
        );
        const data = JSON.parse(
          text,
          (key: string, value: unknown, context?: { source?: string }) =>
            key === "mid" && typeof value === "number"
              ? context?.source
              : value,
        ) as { mid?: unknown };
        if (
          !data ||
          typeof data.mid !== "string" ||
          !/^[1-9][0-9]{0,19}$/.test(data.mid)
        )
          return { kind: "unknown", code: "provider_response_invalid" };
        return { kind: "accepted", mid: data.mid };
      })(),
    ]);
  } catch {
    return { kind: "unknown", code: "provider_response_uncertain" };
  } finally {
    combined.removeEventListener("abort", stop);
    controller.abort();
    void reader?.cancel().catch(() => {});
  }
}
