import type { EntityType, LocalEntity, SearchResult } from "./types";

/** One entry in the context menu. */
export interface ContextAction {
  /** Stable identifier used by the click handler. */
  id: ActionId;
  /** Translation key for the label. */
  labelKey: string;
  /** Whether the action destroys data and therefore needs confirmation. */
  destructive?: boolean;
}

export type ActionId =
  | "open"
  | "reveal"
  | "copyPath"
  | "copyName"
  | "copyPid"
  | "copyExePath"
  | "terminate"
  | "remember";

function pathOf(result: SearchResult): string | null {
  return result.path;
}

/**
 * The actions offered for a result, decided by its entity type.
 *
 * Services are deliberately read-only: a search tool that can stop a system
 * service is a search tool nobody should install.
 */
export function actionsFor(result: SearchResult): ContextAction[] {
  const actions: ContextAction[] = [];
  const type: EntityType = result.entity_type;

  switch (type) {
    case "file":
    case "directory":
      actions.push({ id: "open", labelKey: "action.open" });
      if (pathOf(result)) actions.push({ id: "reveal", labelKey: "action.reveal" });
      actions.push({ id: "copyPath", labelKey: "action.copyPath" });
      actions.push({ id: "copyName", labelKey: "action.copyName" });
      break;
    case "application":
      actions.push({ id: "open", labelKey: "action.launch" });
      if (pathOf(result)) actions.push({ id: "reveal", labelKey: "action.openLocation" });
      actions.push({ id: "copyPath", labelKey: "action.copyPath" });
      break;
    case "process":
      if (pathOf(result)) actions.push({ id: "reveal", labelKey: "action.openLocation" });
      actions.push({ id: "copyPid", labelKey: "action.copyPid" });
      if (pathOf(result)) actions.push({ id: "copyExePath", labelKey: "action.copyExePath" });
      actions.push({ id: "terminate", labelKey: "action.terminate", destructive: true });
      break;
    case "service":
      if (pathOf(result)) actions.push({ id: "reveal", labelKey: "action.openLocation" });
      if (pathOf(result)) actions.push({ id: "copyExePath", labelKey: "action.copyExePath" });
      break;
    case "window":
      actions.push({ id: "copyName", labelKey: "action.copyName" });
      actions.push({ id: "copyPid", labelKey: "action.copyPid" });
      break;
  }

  actions.push({ id: "remember", labelKey: "action.recordSelection" });
  return actions;
}

/** The text an action copies, or `null` when it copies nothing. */
export function clipboardTextFor(action: ActionId, result: SearchResult): string | null {
  switch (action) {
    case "copyPath":
    case "copyExePath":
      return result.path ?? null;
    case "copyName":
      return result.name;
    case "copyPid":
      return result.metadata["pid"] ?? null;
    default:
      return null;
  }
}

/** Whether an action opens the entity rather than copying from it. */
export function isOpenAction(action: ActionId): boolean {
  return action === "open";
}

/** The pid of a process or window result, when it has one. */
export function pidOf(result: SearchResult): number | null {
  const raw = result.metadata["pid"];
  if (!raw) return null;
  const value = Number.parseInt(raw, 10);
  return Number.isFinite(value) ? value : null;
}

/** Whether this result can be revealed in the file manager. */
export function canReveal(entity: LocalEntity): boolean {
  return "path" in entity || entity.kind === "file" || entity.kind === "directory";
}