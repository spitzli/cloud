import type { Fetch } from "./http";
import { MailError } from "@spitzli/mail-contracts/errors";
export type TurboSendingCredential = {
  consumerKey: string;
  consumerSecret: string;
  region: "eu" | "global";
};
/** Live-verified 2026-10-12 with an isolated account, SEND_API/APIS, APIS-only and invalid controls. */
export async function verifyTurboSendingAccount(
  pair: TurboSendingCredential,
  fetcher: Fetch = fetch,
): Promise<{ accountID: string }> {
  if (
    !["eu", "global"].includes(pair.region) ||
    ![pair.consumerKey, pair.consumerSecret].every(
      (v) => typeof v === "string" && /^[\x21-\x7e]{1,4096}$/.test(v),
    )
  )
    throw new MailError(
      422,
      "validation_error",
      "Invalid provider credentials",
    );
  const signal = AbortSignal.timeout(15000);
  const response = await fetcher(
    "https://pro.api.serversmtp.com/api/v2/user/info",
    {
      headers: {
        consumerKey: pair.consumerKey,
        consumerSecret: pair.consumerSecret,
      },
      redirect: "error",
      cache: "no-store",
      signal,
    },
  );
  if (!response.ok)
    throw new MailError(
      422,
      "account_verification_required",
      "Provider account identity could not be verified",
    );
  const reader = response.body?.getReader();
  if (!reader)
    throw new MailError(
      503,
      "provider_unavailable",
      "Provider response unavailable",
    );
  let raw = "",
    bytes = 0;
  const decoder = new TextDecoder("utf8", { fatal: true });
  try {
    for (;;) {
      const part = await reader.read();
      if (part.done) break;
      bytes += part.value.byteLength;
      if (bytes > 65536) throw Error("Response limit");
      raw += decoder.decode(part.value, { stream: true });
    }
    raw += decoder.decode();
  } finally {
    await reader.cancel().catch(() => {});
  }
  const data = JSON.parse(
    raw,
    (key: string, value: unknown, context?: { source?: string }) =>
      key === "id" && typeof value === "number" ? context?.source : value,
  ) as { id?: unknown; active?: unknown };
  if (
    typeof data.id !== "string" ||
    !/^[1-9][0-9]{0,19}$/.test(data.id) ||
    ![true, 1, "1"].includes(data.active as boolean | number | string)
  )
    throw new MailError(
      422,
      "account_verification_required",
      "An active provider account is required",
    );
  // Missing mandatory recipient/sender fields guarantee this request cannot create a message.
  const probe = await fetcher(
    `https://${pair.region === "eu" ? "api.eu.turbo-smtp.com" : "api.turbo-smtp.com"}/api/v2/mail/send`,
    {
      method: "POST",
      headers: {
        consumerKey: pair.consumerKey,
        consumerSecret: pair.consumerSecret,
        "content-type": "application/json",
      },
      body: "{}",
      redirect: "error",
      cache: "no-store",
      signal,
    },
  );
  void probe.body?.cancel().catch(() => {});
  if (probe.status !== 400)
    throw new MailError(
      422,
      "sending_permission_required",
      "Consumer credentials need both management and sending access",
    );
  return { accountID: data.id };
}
