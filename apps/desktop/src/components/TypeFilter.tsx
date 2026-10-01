import {
  AppWindow,
  File as FileIcon,
  Folder,
  LayoutGrid,
  MonitorCog,
  Package,
  type LucideIcon,
} from "lucide-react";

import type { Translator } from "../i18n";
import { FILTER_TYPES, type EntityType } from "../lib/types";

const ICONS: Record<"all" | EntityType, LucideIcon> = {
  all: LayoutGrid,
  file: FileIcon,
  directory: Folder,
  application: Package,
  process: MonitorCog,
  service: AppWindow,
  window: AppWindow,
};

interface TypeFilterProps {
  active: "all" | EntityType;
  onChange: (value: "all" | EntityType) => void;
  counts: Partial<Record<"all" | EntityType, number>>;
  t: Translator;
}

/** The type filter row. `Tab`/`Shift+Tab` cycles it from the keyboard. */
export function TypeFilter({ active, onChange, counts, t }: TypeFilterProps) {
  return (
    <div
      className="flex flex-wrap items-center gap-1.5 border-b border-[var(--lce-border)] px-4 py-2"
      role="tablist"
      aria-label={t("search.all")}
    >
      {FILTER_TYPES.map((filter) => {
        const Icon = ICONS[filter.key];
        const count = counts[filter.key];
        return (
          <button
            key={filter.key}
            type="button"
            role="tab"
            aria-selected={active === filter.key}
            className="lce-chip"
            data-active={active === filter.key}
            onClick={() => onChange(filter.key)}
          >
            <Icon size={12} aria-hidden />
            {t(filter.labelKey)}
            {count !== undefined && <span className="opacity-60">{count}</span>}
          </button>
        );
      })}
    </div>
  );
}