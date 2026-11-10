"use client";
import { useEffect, useRef, useTransition, type ReactNode } from "react";
import { usePathname, useRouter, useSearchParams } from "next/navigation";
import { useI18n } from "@spitzli/i18n/react";
import { searchFormURL, fieldValueFromURL } from "./url";
/** Keeps native inputs mounted while server results update; no client index of private global data. */
export function LiveSearchForm({
  children,
  path,
  className,
  pageKey = "page",
  role = "search",
}: {
  children: ReactNode;
  path?: string;
  className?: string;
  pageKey?: string;
  role?: "search";
}) {
  const router = useRouter(),
    pathname = usePathname(),
    params = useSearchParams(),
    { t } = useI18n();
  const form = useRef<HTMLFormElement>(null),
    timer = useRef<ReturnType<typeof setTimeout> | null>(null),
    composing = useRef(false),
    edits = useRef(new Map<string, string>());
  const [pending, startTransition] = useTransition();
  const cancel = () => {
    if (timer.current) clearTimeout(timer.current);
    timer.current = null;
  };
  const commit = () => {
    cancel();
    if (!form.current || composing.current) return;
    const url = searchFormURL(
      path ?? pathname,
      new FormData(form.current),
      pageKey,
    );
    startTransition(() => router.replace(url, { scroll: false }));
  };
  const sync = (
    force = false,
    url = new URLSearchParams(window.location.search),
  ) => {
    if (!form.current || (composing.current && !force)) return;
    for (const el of form.current.elements) {
      if (
        !(el instanceof HTMLInputElement || el instanceof HTMLSelectElement) ||
        !el.name
      )
        continue;
      const fallback =
        el.dataset.searchDefault ??
        (el instanceof HTMLInputElement
          ? el.type === "hidden"
            ? ""
            : el.defaultValue
          : (el.options[0]?.value ?? ""));
      const value = fieldValueFromURL(
        url.get(el.name),
        fallback,
        edits.current.get(el.name),
        force,
      );
      if (value === undefined) continue;
      edits.current.delete(el.name);
      if (el.value !== value) el.value = value;
    }
  };
  const urlQuery = params.toString();
  useEffect(() => {
    sync();
  }, [urlQuery]);
  useEffect(() => {
    const back = () => {
      cancel();
      edits.current.clear();
      sync(true);
    };
    const navigate = (event: MouseEvent) => {
      if (
        event.defaultPrevented ||
        event.button !== 0 ||
        event.metaKey ||
        event.ctrlKey ||
        event.shiftKey ||
        event.altKey
      )
        return;
      const anchor =
        event.target instanceof Element ? event.target.closest("a") : null;
      if (
        !(anchor instanceof HTMLAnchorElement) ||
        (anchor.target && anchor.target !== "_self") ||
        anchor.hasAttribute("download")
      )
        return;
      const destination = new URL(anchor.href);
      if (destination.origin !== window.location.origin) return;
      cancel();
      edits.current.clear();
      if (form.current?.contains(anchor)) sync(true, destination.searchParams);
    };
    document.addEventListener("click", navigate, true);
    window.addEventListener("popstate", back);
    return () => {
      cancel();
      window.removeEventListener("popstate", back);
      document.removeEventListener("click", navigate, true);
    };
  }, []);
  return (
    <form
      ref={form}
      action={path ?? pathname}
      method="get"
      className={className}
      role={role}
      aria-busy={pending}
      onSubmit={(e) => {
        e.preventDefault();
        commit();
      }}
      onCompositionStart={() => {
        composing.current = true;
        cancel();
      }}
      onCompositionEnd={() => {
        composing.current = false;
        commit();
      }}
      onChange={(e) => {
        const field = e.target;
        if (
          (field instanceof HTMLInputElement ||
            field instanceof HTMLSelectElement) &&
          field.name
        )
          edits.current.set(field.name, field.value);
        cancel();
        if (composing.current) return;
        const immediate = e.target instanceof HTMLSelectElement;
        timer.current = setTimeout(commit, immediate ? 0 : 180);
      }}
    >
      {children}
      <span className="sr-only" aria-live="polite">
        {pending ? t("Searching…") : ""}
      </span>
    </form>
  );
}
