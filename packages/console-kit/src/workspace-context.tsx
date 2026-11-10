import * as React from "react";
import Link from "next/link";
import { getRequestI18n } from "@spitzli/i18n/next";

export async function WorkspaceContext({
  items,
}: {
  items: { label: string; href?: string }[];
}) {
  const { t } = await getRequestI18n();
  return (
    <nav className="workspace-context" aria-label={t("Location")}>
      <ol>
        {items.map((item, index) => (
          <li key={index}>
            {item.href ? (
              <Link href={item.href}>{item.label}</Link>
            ) : (
              <span aria-current="page">{item.label}</span>
            )}
          </li>
        ))}
      </ol>
    </nav>
  );
}
