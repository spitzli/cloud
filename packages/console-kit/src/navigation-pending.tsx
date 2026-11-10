"use client";
import * as React from "react";
import { useLinkStatus } from "next/link";
import { useI18n } from "@spitzli/i18n/react";

export function NavigationPending() {
  const { pending } = useLinkStatus();
  const { t } = useI18n();
  return (
    <span className="navigation-pending" role="status">
      {pending && (
        <>
          <span className="navigation-spinner" aria-hidden="true" />
          <span className="sr-only">{t("Loading your workspace…")}</span>
        </>
      )}
    </span>
  );
}
