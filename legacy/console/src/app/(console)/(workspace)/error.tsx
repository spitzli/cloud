"use client";
import { useI18n } from '@webdock/i18n/react';

import Link from "next/link";
export default function WorkspaceError({ reset }: { reset: () => void }) {
  const i18n = useI18n();

  return (
    <section className="panel empty" role="alert">
      <h1>{i18n.t("This view could not load")}</h1>
      <p>{i18n.t("Your workspace data is still saved. Retry this view, or return to your projects.")}</p>
      <div className="error-actions">
        {/* SSO sets cookies and redirects externally, so it needs a full browser navigation. */}
        {/* eslint-disable-next-line @next/next/no-html-link-for-pages */}
        <a className="button secondary" href="/api/sso/login">{i18n.t("Sign in")}</a>
        <button className="button" onClick={reset}>{i18n.t("Try again")}</button>
        <Link className="button secondary" href="/">{i18n.t("Back to projects")}</Link>
      </div>
    </section>
  );
}
