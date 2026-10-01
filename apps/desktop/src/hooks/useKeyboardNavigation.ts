import { useEffect } from "react";

export interface KeyboardHandlers {
  /** Move the selection by `delta` rows, clamped by the caller. */
  onMove: (delta: number) => void;
  /** Activate the selected row. */
  onActivate: () => void;
  /** Close the topmost overlay, or clear the query when nothing is open. */
  onEscape: () => void;
  /** Cycle through the type filters. */
  onCycleFilter?: (delta: number) => void;
  /** Open settings. */
  onSettings?: () => void;
  /** Toggle the context menu for the selected row. */
  onContextMenu?: () => void;
  /** Whether the handler should act at all (false while a dialog is open). */
  enabled: boolean;
}

/**
 * Keyboard-first navigation, matching the shortcuts a developer tool is
 * expected to have:
 *
 * | key             | action              |
 * |-----------------|---------------------|
 * | `ArrowUp`/`Down`| move the selection  |
 * | `Enter`         | open the selection  |
 * | `Shift+Enter`   | open the context menu |
 * | `Tab`/`Shift+Tab`| cycle the filters  |
 * | `Ctrl+,`        | settings            |
 * | `Escape`        | close / clear       |
 */
export function useKeyboardNavigation(handlers: KeyboardHandlers): void {
  useEffect(() => {
    if (!handlers.enabled) return;

    const listener = (event: KeyboardEvent) => {
      switch (event.key) {
        case "ArrowDown":
          event.preventDefault();
          handlers.onMove(1);
          break;
        case "ArrowUp":
          event.preventDefault();
          handlers.onMove(-1);
          break;
        case "Enter":
          event.preventDefault();
          if (event.shiftKey) handlers.onContextMenu?.();
          else handlers.onActivate();
          break;
        case "Tab":
          event.preventDefault();
          handlers.onCycleFilter?.(event.shiftKey ? -1 : 1);
          break;
        case "Escape":
          event.preventDefault();
          handlers.onEscape();
          break;
        case ",":
          if (event.ctrlKey || event.metaKey) {
            event.preventDefault();
            handlers.onSettings?.();
          }
          break;
        default:
          break;
      }
    };

    window.addEventListener("keydown", listener);
    return () => window.removeEventListener("keydown", listener);
  }, [handlers]);
}