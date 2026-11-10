import Link from "next/link";
import { mailHref, type MailPageContext } from "./product-page";

export function MailGettingStarted({ context }: { context: MailPageContext }) {
  const { i18n, canManage } = context;
  return (
    <section className="auth-panel mail-getting-started">
      <h2>{i18n.t("Your first API email")}</h2>
      <p>
        {i18n.t(
          "Send transactional email from your application. Follow these steps once per sending connection.",
        )}
      </p>
      {!canManage && (
        <p className="notice">
          {i18n.t(
            "An owner or admin sets up the connection, domain and API key. You can view messages and delivery results.",
          )}
        </p>
      )}
      <ol>
        <li>
          <strong>
            {canManage ? (
              <Link href={mailHref(context, "/settings")}>
                {i18n.t("Connect a sending account")}
              </Link>
            ) : (
              i18n.t("Connect a sending account")
            )}
          </strong>
          <p>
            {i18n.t(
              "Managed Mail uses an account provided by Webdock. BYOK connects your own turboSMTP account using its Consumer key and secret.",
            )}
          </p>
        </li>
        <li>
          <strong>
            <Link href={mailHref(context, "/domains")}>
              {i18n.t("Verify your sender domain")}
            </Link>
          </strong>
          <p>
            {i18n.t(
              "Add the domain after @ in your sender address. Publish the displayed DNS records, then verify ownership and sending readiness.",
            )}
          </p>
        </li>
        <li>
          <strong>
            {canManage ? (
              <Link href={mailHref(context, "/api-keys")}>
                {i18n.t("Create a sending API key")}
              </Link>
            ) : (
              i18n.t("Create a sending API key")
            )}
          </strong>
          <p>
            {i18n.t(
              "Choose Sending only for your application. Save the key as SPITZLI_MAIL_API_KEY on your server; it is shown once. Never put it in browser code.",
            )}
          </p>
        </li>
        <li>
          <strong>
            <Link href={mailHref(context, "/quickstart") + "#first-request"}>
              {i18n.t("Run the simulator example")}
            </Link>
          </strong>
          <p>
            {i18n.t(
              "Use your verified sender with delivered@test.mail.webdock.dev. No real email leaves Webdock. Inspect the message before switching to your own recipient.",
            )}
          </p>
        </li>
      </ol>
      <p>
        <Link href={mailHref(context, "/emails")}>
          {i18n.t("Inspect messages and delivery results")}
        </Link>
      </p>
    </section>
  );
}
