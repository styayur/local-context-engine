import { Database, Zap } from "lucide-react";

import type { Translator } from "../i18n";
import type { IndexStatus, SearchResponse } from "../lib/types";

interface StatusBarProps {
  response: SearchResponse | null;
  indexStatus: IndexStatus | null;
  t: Translator;
}

/** The footer: how long the last search took and what is being searched. */
export function StatusBar({ response, indexStatus, t }: StatusBarProps) {
  const backend =
    indexStatus === null
      ? "…"
      : indexStatus.ready
        ? `${indexStatus.backend} · ${indexStatus.entries}`
        : t("settings.indexNotBuilt");

  return (
    <div className="flex items-center justify-between gap-3 border-t border-[var(--lce-border)] px-4 py-1.5 text-[11px] text-[var(--lce-muted)]">
      <div className="flex items-center gap-3">
        <span className="flex items-center gap-1">
          <Zap size={11} aria-hidden />
          {response ? t("search.elapsed", { ms: response.elapsed_ms.toFixed(1) }) : "—"}
        </span>
        {response && (
          <span className="truncate">
            {t("search.compiled")}: <code>{response.compiled}</code>
          </span>
        )}
      </div>
      <div className="flex items-center gap-3">
        {response?.truncated && <span>{t("search.truncated")}</span>}
        <span className="flex items-center gap-1">
          <Database size={11} aria-hidden />
          {backend}
        </span>
      </div>
    </div>
  );
}