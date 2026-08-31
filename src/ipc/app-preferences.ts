import { invoke } from "@tauri-apps/api/core";
import type { AppPreferences, ExternalToolAvailability } from "./types";

export function loadAppPreferences(): Promise<AppPreferences> {
  return invoke<AppPreferences>("load_app_preferences");
}

export function saveAppPreferences(preferences: AppPreferences): Promise<AppPreferences> {
  return invoke<AppPreferences>("save_app_preferences", { preferences });
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
