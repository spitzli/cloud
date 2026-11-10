"use client";
import * as React from "react";

import Link from "next/link";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { useI18n } from "@spitzli/i18n/react";

/** Keep the editor mounted while changing its available screen space. */
export function DatabaseWorkspaceFrame({
  name,
  environment,
  permission,
  back,
  children,
}: {
  name: string;
  environment: string;
  permission: string;
  back: string;
  children: ReactNode;
}) {
  const { t } = useI18n();
  const root = useRef<HTMLElement>(null);
  const focusButton = useRef<HTMLButtonElement>(null);
  const [expanded, setExpanded] = useState(false);
  const [fullscreen, setFullscreen] = useState(false);
  useEffect(() => {
    const changed = () =>
      setFullscreen(document.fullscreenElement === root.current);
    document.addEventListener("fullscreenchange", changed);
    return () => document.removeEventListener("fullscreenchange", changed);
  }, []);
  useEffect(() => {
    if (!expanded && !fullscreen) return;
    const overflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    // Hide surrounding chrome from keyboard and assistive technology too.
    const siblings: { element: HTMLElement; inert: boolean }[] = [];
    let element: HTMLElement | null = root.current;
    while (element && element !== document.body) {
      for (const sibling of element.parentElement?.children ?? []) {
        if (sibling !== element && sibling instanceof HTMLElement) {
          siblings.push({ element: sibling, inert: sibling.inert });
          sibling.setAttribute("inert", "");
        }
      }
      element = element.parentElement;
    }
    return () => {
      document.body.style.overflow = overflow;
      for (const sibling of siblings)
        sibling.element.toggleAttribute("inert", sibling.inert);
    };
  }, [expanded, fullscreen]);
  const toggleFullscreen = async () => {
    try {
      if (document.fullscreenElement === root.current)
        await document.exitFullscreen();
      else {
        setExpanded(true);
        if (root.current?.requestFullscreen)
          await root.current.requestFullscreen();
      }
    } catch {
      setExpanded(true);
    }
  };
  const leaveFocus = () => {
    setExpanded(false);
    focusButton.current?.focus();
  };
  return (
    <section
      ref={root}
      className={`database-workspace-frame${expanded ? " is-expanded" : ""}`}
      aria-label={t("Database workspace")}
    >
      <header className="database-workspace-heading">
        <Link
          href={back}
          className="database-back"
          aria-label={t("Back to databases")}
          title={t("Back to databases")}
        >
          <svg
            width="18"
            height="18"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.6"
            aria-hidden="true"
          >
            <path d="m14 6-6 6 6 6M8 12h13" />
          </svg>
        </Link>
        <div className="database-workspace-identity">
          <h1>{name}</h1>
          <div>
            <span>{environment}</span>
            <span className="database-permission">{permission}</span>
          </div>
        </div>
        <div className="database-display-controls">
          {!fullscreen && (
            <button
              ref={focusButton}
              type="button"
              aria-label={t(expanded ? "Exit focus mode" : "Focus mode")}
              aria-pressed={expanded}
              onClick={() => (expanded ? leaveFocus() : setExpanded(true))}
              title={t(expanded ? "Exit focus mode" : "Focus mode")}
            >
              <svg
                width="16"
                height="16"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.6"
                aria-hidden="true"
              >
                <rect x="3" y="4" width="18" height="16" rx="2" />
                <path d="M8 4v16" />
              </svg>
              <span>{t(expanded ? "Exit focus mode" : "Focus mode")}</span>
            </button>
          )}
          <button
            type="button"
            onClick={() => {
              void toggleFullscreen();
            }}
            aria-label={t(fullscreen ? "Exit fullscreen" : "Fullscreen")}
            aria-pressed={fullscreen}
            title={t(fullscreen ? "Exit fullscreen" : "Fullscreen")}
          >
            <svg
              width="16"
              height="16"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.6"
              aria-hidden="true"
            >
              <path d="M8 3H3v5m13-5h5v5M3 16v5h5m13-5v5h-5" />
            </svg>
            <span>{t(fullscreen ? "Exit fullscreen" : "Fullscreen")}</span>
          </button>
        </div>
      </header>
      <div className="database-workspace-content">{children}</div>
    </section>
  );
}
