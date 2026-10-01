import {
  Clipboard,
  ExternalLink,
  FolderOpen,
  Info,
  Skull,
  Sparkles,
  type LucideIcon,
} from "lucide-react";
import { useEffect, useRef } from "react";

import type { Translator } from "../i18n";
import type { ActionId, ContextAction } from "../lib/actions";

const ICONS: Record<ActionId, LucideIcon> = {
  open: ExternalLink,
  reveal: FolderOpen,
  copyPath: Clipboard,
  copyName: Clipboard,
  copyPid: Clipboard,
  copyExePath: Clipboard,
  terminate: Skull,
  remember: Sparkles,
};

interface ContextMenuProps {
  actions: ContextAction[];
  position: { x: number; y: number };
  onAction: (action: ActionId) => void;
  onClose: () => void;
  t: Translator;
}

/**
 * The per-result action menu.
 *
 * Destructive entries are styled with the danger colour and are separated from
 * the safe ones by a divider, so "end process" can never be hit by accident.
 */
export function ContextMenu({ actions, position, onAction, onClose, t }: ContextMenuProps) {
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const onPointerDown = (event: MouseEvent) => {
      if (!menuRef.current?.contains(event.target as Node)) onClose();
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("mousedown", onPointerDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onPointerDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [onClose]);

  const safe = actions.filter((action) => !action.destructive);
  const destructive = actions.filter((action) => action.destructive);

  const renderAction = (action: ContextAction) => {
    const Icon = ICONS[action.id];
    return (
      <button
        key={action.id}
        type="button"
        className="lce-menu-item"
        data-destructive={action.destructive === true}
        onClick={() => onAction(action.id)}
      >
        <Icon size={14} aria-hidden />
        <span className="flex-1">{t(action.labelKey)}</span>
      </button>
    );
  };

  return (
    <div
      ref={menuRef}
      className="lce-menu fixed z-50"
      style={{ left: position.x, top: position.y }}
      role="menu"
    >
      {safe.map(renderAction)}
      {destructive.length > 0 && (
        <>
          <div className="lce-divider" />
          <div className="px-2 pb-1 pt-1 text-[10px] uppercase tracking-wide text-[var(--lce-muted)]">
            <span className="inline-flex items-center gap-1">
              <Info size={10} aria-hidden />
              {t("action.danger")}
            </span>
          </div>
          {destructive.map(renderAction)}
        </>
      )}
    </div>
  );
}