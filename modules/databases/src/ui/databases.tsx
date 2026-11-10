"use client";
import * as React from "react";
import type { ComponentProps } from "react";
import { resourceScope as hostingUIScope } from "@spitzli/console-kit/resource-scope";
import { useActionState, useState } from "react";
import Link from "next/link";
import { useI18n } from "@spitzli/i18n/react";
import type { DatabasePlan } from "@spitzli/hosting-contracts";
import type { DatabaseOrderState } from "../ui-types";
import { useDatabaseActions } from "./actions";
export type DatabaseCatalogEntry = {
  id: string;
  plan: DatabasePlan;
  availability: "disabled" | "pilot" | "public";
  revision: number;
};
function DatabaseOrderFormState({
  projectID,
  plans,
  requestKey,
  databaseID,
  databaseName,
}: {
  projectID: string;
  plans: DatabaseCatalogEntry[];
  requestKey: string;
  databaseID?: string;
  databaseName?: string;
}) {
  const { prepareDatabaseOffer, acceptDatabaseOffer, createDatabasePlan } =
    useDatabaseActions();

  const { t, error, locale } = useI18n();
  const [key, setKey] = useState(requestKey);
  const [review, prepare, preparing] = useActionState(prepareDatabaseOffer, {});
  const [order, accept, accepting] = useActionState(
    acceptDatabaseOffer,
    {} as DatabaseOrderState,
  );
  const money = (cents: number) =>
    new Intl.NumberFormat(locale, {
      style: "currency",
      currency: "EUR",
    }).format(cents / 100);
  if (order.databasePath)
    return (
      <p role="status">
        <Link className="button" href={order.databasePath}>
          {t("Open database")}
        </Link>
      </p>
    );
  return (
    <>
      <form action={prepare} className="hosting-form">
        <input type="hidden" name="projectID" value={projectID} />
        {databaseID && (
          <input type="hidden" name="databaseID" value={databaseID} />
        )}
        <input type="hidden" name="idempotencyKey" value={key} />
        <fieldset
          disabled={preparing || accepting || plans.length === 0}
          onChange={() => setKey(crypto.randomUUID())}
        >
          <label className="field">
            {t("Name")}
            <input
              name="name"
              required
              maxLength={160}
              defaultValue={databaseName}
              readOnly={!!databaseID}
            />
          </label>
          <label className="field">
            {t("Database plan")}
            <select name="planRevisionID" required>
              {plans.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.plan.name} · {money(p.plan.monthlyPriceCents)}{" "}
                  {t("net / month")}
                </option>
              ))}
            </select>
          </label>
          <button className="button">
            {preparing ? t("Loading…") : t("Review paid order")}
          </button>
        </fieldset>
        {review.error && <p role="alert">{error(review.error)}</p>}
        {!plans.length && <p>{t("No database plans are available yet.")}</p>}
      </form>
      {review.offer && (
        <form action={accept} className="hosting-form">
          <h3>{review.offer.name}</h3>
          <p>
            <strong>
              {money(review.offer.snapshot.monthlyPriceCents)}{" "}
              {t("net / month")}
            </strong>
          </p>
          <p>{review.offer.snapshot.terms}</p>
          <p>
            {review.offer.snapshot.kind === "shared"
              ? t("Shared CPU and memory; measured logical storage allowance.")
              : t(
                  "Dedicated PostgreSQL process and bounded database filesystem.",
                )}{" "}
            {new Intl.NumberFormat(locale, { maximumFractionDigits: 4 }).format(
              (review.offer.snapshot.logicalStorageAllowanceBytes ??
                review.offer.snapshot.volumeBytes ??
                0) / 1073741824,
            )}{" "}
            GiB
          </p>
          <p>
            {t("Backups are not configured. No recovery window is included.")}
          </p>
          <p>
            {t(
              "Billing starts after confirmed provisioning. Pausing does not reduce the monthly price.",
            )}
          </p>
          <input type="hidden" name="projectID" value={projectID} />
          <input type="hidden" name="offerID" value={review.offer.id} />
          <input
            type="hidden"
            name="idempotencyKey"
            value={review.offer.id + "-accept-database"}
          />
          <label className="field">
            <span>
              <input type="checkbox" name="confirmPrice" value="yes" required />{" "}
              {t("I accept this monthly price and the displayed terms.")}
            </span>
          </label>
          {review.offer.snapshot.requiresInterruption && (
            <label className="field">
              <span>
                <input
                  type="checkbox"
                  name="confirmInterruption"
                  value="yes"
                  required
                />{" "}
                {t(
                  "I agree to interrupt database connections for this upgrade now.",
                )}
              </span>
            </label>
          )}
          {order.error && <p role="alert">{error(order.error)}</p>}
          <button className="button" disabled={accepting}>
            {accepting
              ? t("Ordering…")
              : t("Order database with payment obligation")}
          </button>
        </form>
      )}
    </>
  );
}
function DatabasePlanFormState({ requestKey }: { requestKey: string }) {
  const { prepareDatabaseOffer, acceptDatabaseOffer, createDatabasePlan } =
    useDatabaseActions();

  const { t, error } = useI18n();
  const [state, action, pending] = useActionState(createDatabasePlan, {});
  return (
    <form action={action} className="hosting-form">
      <input type="hidden" name="idempotencyKey" value={requestKey} />
      <fieldset disabled={pending}>
        <label className="field">
          {t("Name")}
          <input name="name" required maxLength={160} />
        </label>
        <label className="field">
          {t("Database kind")}
          <select name="kind">
            <option value="shared">{t("Shared")}</option>
            <option value="dedicated">{t("Dedicated instance")}</option>
          </select>
        </label>
        <label className="field">
          {t("Monthly net price (EUR)")}
          <input
            name="monthlyPrice"
            inputMode="decimal"
            required
            placeholder="5.90"
          />
        </label>
        <label className="field">
          {t("Contract terms")}
          <textarea
            name="terms"
            minLength={40}
            maxLength={6000}
            required
            defaultValue="Always on. Fixed monthly net price, including while paused. Backups are not configured; no recovery window or high availability is included."
          />
        </label>
        <button className="button">{t("Create disabled plan")}</button>
      </fieldset>
      {state.error && <p role="alert">{error(state.error)}</p>}
      {state.message && (
        <p role="status">{t("Database plan saved as disabled.")}</p>
      )}
    </form>
  );
}

export function DatabaseOrderForm(
  props: ComponentProps<typeof DatabaseOrderFormState>,
) {
  return <DatabaseOrderFormState key={hostingUIScope(props)} {...props} />;
}
export function DatabasePlanForm(
  props: ComponentProps<typeof DatabasePlanFormState>,
) {
  return <DatabasePlanFormState key={hostingUIScope(props)} {...props} />;
}
