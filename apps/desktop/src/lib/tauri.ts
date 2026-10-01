import { invoke } from "@tauri-apps/api/core";

import type {
  IndexReport,
  IndexStatus,
  LocalEntity,
  ProviderStats,
  SearchResponse,
  Settings,
} from "./types";

/**
 * Whether the frontend is running inside the Tauri shell. `npm run dev` alone
 * renders the UI in a browser, where there is no search core to talk to; that
 * case shows an explicit message rather than fabricated results.
 */
export function isDesktopShell(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function requireShell(): void {
  if (!isDesktopShell()) {
    throw new Error(
      "The desktop shell is not attached. Run `npm run tauri:dev` to search this machine.",
    );
  }
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  requireShell();
  return invoke<T>(command, args);
}

export function search(
  query: string,
  types: string[],
  limit: number,
): Promise<SearchResponse> {
  return call<SearchResponse>("search", { query, types, limit });
}

export function getSettings(): Promise<Settings> {
  return call<Settings>("settings");
}

export function updateSettings(settings: Settings): Promise<Settings> {
  return call<Settings>("update_settings", { settings });
}

export function getIndexStatus(): Promise<IndexStatus> {
  return call<IndexStatus>("index_status");
}

export function rebuildIndex(): Promise<IndexReport> {
  return call<IndexReport>("rebuild_index");
}

export function updateIndex(): Promise<unknown> {
  return call<unknown>("update_index");
}

export function providerStats(): Promise<ProviderStats[]> {
  return call<ProviderStats[]>("providers");
}

export function openResult(entity: LocalEntity): Promise<void> {
  return call<void>("open_result", { entity });
}

export function revealResult(entity: LocalEntity): Promise<void> {
  return call<void>("reveal_result", { entity });
}

export function launchResult(entity: LocalEntity): Promise<void> {
  return call<void>("launch_result", { entity });
}

export function terminateProcess(pid: number, confirmed: boolean): Promise<void> {
  return call<void>("terminate_process", { pid, confirmed });
}

export function recordSelection(query: string, entityId: string): Promise<void> {
  return call<void>("record_selection", { query, entityId });
}

export function resetUsage(): Promise<void> {
  return call<void>("reset_usage");
}

/** Copy text to the clipboard, using the web API the WebView provides. */
export async function copyToClipboard(text: string): Promise<void> {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(text);
    return;
  }
  // Older WebViews need the fallback path.
  const area = document.createElement("textarea");
  area.value = text;
  area.setAttribute("readonly", "");
  area.style.position = "fixed";
  area.style.opacity = "0";
  document.body.appendChild(area);
  area.select();
  document.execCommand("copy");
  document.body.removeChild(area);
}