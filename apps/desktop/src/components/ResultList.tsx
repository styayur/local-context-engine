import {
  AppWindow,
  File as FileIcon,
  Folder,
  MonitorCog,
  Package,
  type LucideIcon,
} from "lucide-react";
import { useEffect, useRef } from "react";

import type { Translator } from "../i18n";
import type { EntityType, SearchResult } from "../lib/types";
import { Highlight } from "./Highlight";

const ICONS: Record<EntityType, LucideIcon> = {
  file: FileIcon,
  directory: Folder,
  application: Package,
  process: MonitorCog,
  service: AppWindow,
  window: AppWindow,
};

interface ResultListProps {
  results: SearchResult[];
  selectedIndex: number;
  onSelect: (index: number) => void;
  onActivate: (result: SearchResult) => void;
  onContextMenu: (result: SearchResult, position: { x: number; y: number }) => void;
  t: Translator;
}

/** The result list. Rows are plain buttons so the keyboard and mouse agree. */
export function ResultList({
  results,
  selectedIndex,
  onSelect,
  onActivate,
  onContextMenu,
  t,
}: ResultListProps) {
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const row = container.querySelector<HTMLElement>(`[data-index="${selectedIndex}"]`);
    row?.scrollIntoView({ block: "nearest" });
  }, [selectedIndex]);

  return (
    <div ref={containerRef} className="lce-scroll px-2 py-2" role="listbox" tabIndex={-1}>
      {results.map((result, index) => {
        const Icon = ICONS[result.entity_type];
        return (
          <div
            key={result.id}
            role="option"
            aria-selected={index === selectedIndex}
            data-index={index}
            data-selected={index === selectedIndex}
            className="lce-row"
            onClick={() => {
              onSelect(index);
              onActivate(result);
            }}
            onMouseEnter={() => onSelect(index)}
            onContextMenu={(event) => {
              event.preventDefault();
              onSelect(index);
              onContextMenu(result, { x: event.clientX, y: event.clientY });
            }}
          >
            <Icon size={16} className="text-[var(--lce-muted)]" aria-hidden />
            <div className="min-w-0">
              <div className="truncate">
                <Highlight
                  text={result.display_name}
                  ranges={result.match_ranges}
                  applicable={result.matched_field === "name" && result.display_name === result.name}
                />
              </div>
              <div className="truncate text-[var(--lce-muted)]">{result.subtitle}</div>
            </div>
            <span className="text-[11px] uppercase tracking-wide text-[var(--lce-muted)]">
              {t(`type.${result.entity_type}`)}
            </span>
          </div>
        );
      })}
    </div>
  );
}