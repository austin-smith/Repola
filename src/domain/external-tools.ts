import type { ExternalTool } from "../ipc/types";

export function resolveAvailableToolId(preferredId: string | null, tools: readonly ExternalTool[]): string | null {
  if (preferredId && tools.some((tool) => tool.id === preferredId)) return preferredId;
  return tools[0]?.id ?? null;
}

export function toolLabels(tools: readonly ExternalTool[]): Record<string, string> {
  return Object.fromEntries(tools.map((tool) => [tool.id, tool.label]));
}
