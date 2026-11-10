"use client";
import DOMPurify from "dompurify";
import { useMemo } from "react";
import { useI18n } from "@spitzli/i18n/react";

export function MessageHTML({ html }: { html: string }) {
  const { t } = useI18n();
  const document = useMemo(() => {
    const clean = DOMPurify.sanitize(html, {
      ALLOWED_TAGS: [
        "p",
        "br",
        "div",
        "span",
        "strong",
        "b",
        "em",
        "i",
        "u",
        "s",
        "blockquote",
        "pre",
        "code",
        "hr",
        "h1",
        "h2",
        "h3",
        "h4",
        "ul",
        "ol",
        "li",
        "table",
        "thead",
        "tbody",
        "tfoot",
        "tr",
        "td",
        "th",
        "caption",
      ],
      ALLOWED_ATTR: ["colspan", "rowspan", "dir", "lang"],
      ALLOW_DATA_ATTR: false,
    });
    return `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'"><style>body{font:15px/1.65 Arial,sans-serif;color:#1b293c;overflow-wrap:anywhere;margin:12px}table{max-width:100%;border-collapse:collapse}td,th{padding:6px}pre{white-space:pre-wrap}blockquote{border-left:2px solid #dce3ec;padding-left:16px}</style></head><body>${clean}</body></html>`;
  }, [html]);
  return (
    <>
      <p className="muted">
        {t("External images and active content are blocked.")}
      </p>
      <iframe
        className="webmail-html"
        title={t("Message content")}
        sandbox=""
        referrerPolicy="no-referrer"
        srcDoc={document}
      />
    </>
  );
}
