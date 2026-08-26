import { describe, expect, it } from "vitest";
import { resolveAvailableToolId, toolLabels } from "./external-tools";
import type { ExternalTool } from "./types";

const tools: ExternalTool[] = [
  { id: "cursor", label: "Cursor", supportsRemoteWorkspaces: true },
  { id: "vscode", label: "Visual Studio Code", supportsRemoteWorkspaces: true },
];

describe("external tools", () => {
  it("preserves an available preference", () => {
    expect(resolveAvailableToolId("vscode", tools)).toBe("vscode");
  });

  it("falls back in discovery order when a preference is unavailable", () => {
    expect(resolveAvailableToolId("uninstalled-editor", tools)).toBe("cursor");
    expect(resolveAvailableToolId(null, tools)).toBe("cursor");
  });

  it("returns null when no supported tool is installed", () => {
    expect(resolveAvailableToolId("vscode", [])).toBeNull();
  });

  it("provides human labels to the Base UI select", () => {
    expect(toolLabels(tools)).toEqual({ cursor: "Cursor", vscode: "Visual Studio Code" });
  });
});
