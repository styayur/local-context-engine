import { CircleSlash, HardDriveDownload, Search } from "lucide-react";

import type { Translator } from "../i18n";

interface EmptyStateProps {
  mode: "idle" | "empty";
  indexReady: boolean;
  rebuilding: boolean;
  onBuildIndex: () => void;
  t: Translator;
}

/** The two states that are not a result list: nothing typed, nothing found. */
export function EmptyState({
  mode,
  indexReady,
  rebuilding,
  onBuildIndex,
  t,
}: EmptyStateProps) {
  const Icon = mode === "idle" ? Search : CircleSlash;
  const title = mode === "idle" ? t("search.idle.title") : t("search.empty.title");
  const hint = mode === "idle" ? t("search.idle.hint") : t("search.empty.hint");

  return (
    <div className="flex h-full flex-col items-center justify-center gap-2 px-8 text-center">
      <Icon size={26} className="text-[var(--lce-muted)]" aria-hidden />
      <p className="text-[15px] font-medium">{title}</p>
      <p className="max-w-md text-[var(--lce-muted)]">{hint}</p>

      {!indexReady && (
        <div className="mt-4 flex flex-col items-center gap-2">
          <p className="text-[var(--lce-muted)]">{t("search.indexHint")}</p>
          <button
            type="button"
            className="lce-chip"
            data-active="true"
            disabled={rebuilding}
            onClick={onBuildIndex}
          >
            <HardDriveDownload size={13} aria-hidden />
            {rebuilding ? t("search.indexBuilding") : t("search.indexAction")}
          </button>
        </div>
      )}
    </div>
  );
}