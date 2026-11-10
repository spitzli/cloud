"use client";
import * as React from "react";

import Link from "next/link";
import { useI18n } from "@spitzli/i18n/react";
import type { ManagedDatabaseEntry } from "@spitzli/database-contracts";

export function ManagedDatabaseRows({
  databases,
}: {
  databases: ManagedDatabaseEntry[];
}) {
  const { t } = useI18n();
  return databases.map((database) => (
    <article key={database.id} className="database-row">
      <div className="database-row-heading">
        <div>
          <strong>{database.name}</strong>
          <p className="muted">PostgreSQL · {t(database.state)}</p>
        </div>
        <Link href={`/hosting/databases/${database.id}`}>
          {t("Manage database")}
        </Link>
        {database.canOpen && (
          <Link
            className="button secondary"
            href={`/hosting/databases/${database.id}/viewer`}
          >
            {t("Open in Tabularis")}
          </Link>
        )}
      </div>
    </article>
  ));
}
