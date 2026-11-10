"use client";
import * as React from "react";
import { useI18n } from "@spitzli/i18n/react";

import Link from "next/link";
import { LiveSearchForm } from "@spitzli/search/form";
import { listURL } from "./list-query";
export function ListControls({
  path,
  q,
  status,
  sort,
  placeholder,
  extra,
  activity = false,
  sortOptions,
}: {
  path: string;
  q: string;
  status?: string;
  sort: string;
  placeholder: string;
  extra?: React.ReactNode;
  activity?: boolean;
  sortOptions?: { value: string; label: string }[];
}) {
  const i18n = useI18n();

  return (
    <LiveSearchForm path={path} className="list-controls" role="search">
      <div className="search-field">
        <label htmlFor="q">{i18n.t("Search")}</label>
        <input
          id="q"
          name="q"
          type="search"
          defaultValue={q}
          data-search-default=""
          autoComplete="off"
          placeholder={placeholder}
          maxLength={160}
        />
      </div>
      {status && (
        <div>
          <label htmlFor="status">{i18n.t("Status")}</label>
          <select
            id="status"
            name="status"
            defaultValue={status}
            data-search-default="active"
          >
            <option value="active">{i18n.t("Active")}</option>
            <option value="archived">{i18n.t("Archived")}</option>
            <option value="all">{i18n.t("All statuses")}</option>
          </select>
        </div>
      )}
      {extra}
      <div>
        <label htmlFor="sort">{i18n.t("Sort by")}</label>
        <select
          id="sort"
          name="sort"
          defaultValue={sort}
          data-search-default={
            sortOptions?.[0]?.value ??
            (activity
              ? "-createdAt"
              : path === "/customers"
                ? "name"
                : "-updatedAt")
          }
        >
          {sortOptions ? (
            sortOptions.map((option) => (
              <option key={option.value} value={option.value}>
                {i18n.t(option.label)}
              </option>
            ))
          ) : activity ? (
            <>
              <option value="-createdAt">{i18n.t("Newest first")}</option>
              <option value="createdAt">{i18n.t("Oldest first")}</option>
            </>
          ) : (
            <>
              <option value="-updatedAt">{i18n.t("Recently updated")}</option>
              <option value="name">{i18n.t("Name A–Z")}</option>
              <option value="-name">{i18n.t("Name Z–A")}</option>
            </>
          )}
        </select>
      </div>
      <noscript>
        <button className="button secondary" type="submit">
          {i18n.t("Apply")}
        </button>
      </noscript>
      <Link href={path} className="filter-reset">
        {i18n.t("Reset")}
      </Link>
    </LiveSearchForm>
  );
}
export function Pagination({
  path,
  query,
  page,
  totalPages,
  totalDocs,
  limit,
}: {
  path: string;
  query: Record<string, string | number | undefined>;
  page: number;
  totalPages: number;
  totalDocs: number;
  limit: number;
}) {
  const i18n = useI18n();

  return (
    <nav className="pagination" aria-label={i18n.t("Results pagination")}>
      <span className="result-count">
        {totalDocs
          ? i18n.t("{start}–{end} of {total}", {
              start: i18n.number((page - 1) * limit + 1),
              end: i18n.number(Math.min(page * limit, totalDocs)),
              total: i18n.number(totalDocs),
            })
          : i18n.t("0 results")}
      </span>
      {page > 1 ? (
        <Link href={listURL(path, { ...query, page: page - 1 })}>
          {i18n.t("Previous")}
        </Link>
      ) : (
        <span aria-disabled="true">{i18n.t("Previous")}</span>
      )}
      <span>
        {i18n.t("Page ")}
        {page}
        {i18n.t(" of ")}
        {Math.max(1, totalPages)}
      </span>
      {page < totalPages ? (
        <Link href={listURL(path, { ...query, page: page + 1 })}>
          {i18n.t("Next")}
        </Link>
      ) : (
        <span aria-disabled="true">{i18n.t("Next")}</span>
      )}
    </nav>
  );
}
