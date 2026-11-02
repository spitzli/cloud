import { AsyncLocalStorage } from "node:async_hooks";
import nodemailer from "nodemailer";
import { publicPlatformSettings } from "../platform/platform-settings";
export const testOutbox: { to: string; subject: string; text: string }[] = [];
type AuthMail = { to: string; subject: string; text: string };
const deliveries = new AsyncLocalStorage<Promise<void>[]>();
export class AuthMailDeliveryError extends Error {
  constructor() {
    super(
      "The invitation email could not be sent. The invitation is saved; please resend it after the mail service is available again.",
    );
  }
}
// Better Auth swallows mail callback failures; administrative actions must await delivery themselves.
export async function withAuthMailDelivery<T>(
  operation: () => Promise<T>,
): Promise<T> {
  const pending: Promise<void>[] = [];
  const result = await deliveries.run(pending, operation);
  try {
    if (!pending.length) throw new AuthMailDeliveryError();
    await Promise.all(pending);
  } catch {
    throw new AuthMailDeliveryError();
  }
  return result;
}
export async function sendAuthMail(message: AuthMail) {
  const delivery = deliverAuthMail(message);
  deliveries.getStore()?.push(delivery);
  await delivery;
}
async function deliverAuthMail(message: AuthMail) {
  if (
    process.env.AUTH_TEST_MAIL === "true" &&
    process.env.NODE_ENV !== "production"
  ) {
    testOutbox.push(message);
    return;
  }
  if (
    !process.env.SMTP_HOST ||
    !process.env.SMTP_USER ||
    !process.env.SMTP_PASS
  )
    throw Error("Authentication email transport is not configured");
  const port = Number(process.env.SMTP_PORT || 587);
  const transport = nodemailer.createTransport({
    host: process.env.SMTP_HOST,
    port,
    secure: port === 465,
    requireTLS: port !== 465,
    auth: { user: process.env.SMTP_USER, pass: process.env.SMTP_PASS },
    disableFileAccess: true,
    disableUrlAccess: true,
    connectionTimeout: 10000,
    socketTimeout: 15000,
  });
  const platform = await publicPlatformSettings();
  await transport.sendMail({
    ...message,
    from: {
      name: platform.name,
      address: process.env.SMTP_FROM || "noreply@webdock.dev",
    },
    replyTo: platform.supportEmail,
  });
}
