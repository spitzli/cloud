"use client";
import { useI18n } from '@webdock/i18n/react';

import Link from "next/link";
export default function PortalError({ reset }: { reset: () => void }) {
  const i18n = useI18n();

  return (
    <section className="auth-panel" role="alert">
      <h1>{i18n.t("Studio couldn’t load this page")}</h1>
      <p>{i18n.t("Your workspace is temporarily unavailable. Try again, or return to your tenants to continue.")}</p>
      <div className="record-actions">
        {/* SSO sets cookies and redirects externally, so it needs a full browser navigation. */}
        {/* eslint-disable-next-line @next/next/no-html-link-for-pages */}
        <a className="button secondary" href="/api/sso/login">{i18n.t("Sign in")}</a>
        <button className="button" onClick={reset}>{i18n.t("Try again")}</button>
        <Link href="/tenants">{i18n.t("Go to tenants")}</Link>
      </div>
    </section>
  );
}
