import { invoke } from "@tauri-apps/api/core";
import type { AppPreferences, ExternalToolAvailability } from "./types";

// Keep read/modify/write updates ordered, including across settings dialog mounts.
let pendingUpdates: Promise<void> = Promise.resolve();

export function loadAppPreferences(): Promise<AppPreferences> {
  return pendingUpdates.then(() => invoke<AppPreferences>("load_app_preferences"));
}

export function updateAppPreferences(update: (current: AppPreferences) => AppPreferences): Promise<AppPreferences> {
  const result = pendingUpdates.then(async () => {
    const current = await invoke<AppPreferences>("load_app_preferences");
    return invoke<AppPreferences>("save_app_preferences", { preferences: update(current) });
  });
  // A failed update must not prevent the next choice from being saved.
  pendingUpdates = result.then(() => undefined, () => undefined);
  return result;
}

export function loadExternalTools(): Promise<ExternalToolAvailability> {
  return invoke<ExternalToolAvailability>("load_external_tools");
}

export function launchWorktreeTool(machineId: string, path: string, tool: "editor" | "terminal"): Promise<string> {
  return invoke<string>("launch_worktree_tool", { machineId, path, tool });
}

export function openFileInEditor(machineId: string, worktreePath: string, pathToken: string, remoteOs: string | null): Promise<string> {
  return invoke<string>("open_file_in_editor", { machineId, worktreePath, pathToken, remoteOs });
}
