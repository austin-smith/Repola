import { invoke } from "@tauri-apps/api/core";
import type { WorkspaceContext } from "./types";

export function loadWorkspaceContext(): Promise<WorkspaceContext> {
  return invoke<WorkspaceContext>("load_workspace_context");
}

export function saveWorkspaceContext(context: WorkspaceContext): Promise<WorkspaceContext> {
  return invoke<WorkspaceContext>("save_workspace_context", { context });
}
