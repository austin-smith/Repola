import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ThemeProvider } from "@/components/theme-provider";
import { SettingsDialog } from "./SettingsDialog";

const preferenceMocks = vi.hoisted(() => ({
  loadAppPreferences: vi.fn(),
  loadExternalTools: vi.fn(),
  saveAppPreferences: vi.fn(),
}));

vi.mock("../ipc/app-preferences", () => ({
  ...preferenceMocks,
  launchWorktreeTool: vi.fn(),
}));

const machines = [
  { id: "local", name: "This computer", kind: "local" as const, enabled: true, ssh: null },
  {
    id: "28b36699-7f53-40a4-9b21-7892b19045c0",
    name: "Build server",
    kind: "ssh" as const,
    enabled: true,
    ssh: { host: "buildbox", user: "deploy", port: 2222 },
  },
];

describe("SettingsDialog", () => {
  afterEach(cleanup);

  beforeEach(() => {
    preferenceMocks.loadAppPreferences.mockResolvedValue({
      version: 2,
      editorId: "uninstalled-editor",
      terminalId: "ghostty",
      defaultSignCommits: false,
    });
    preferenceMocks.loadExternalTools.mockResolvedValue({
      editors: [
        { id: "cursor", label: "Cursor", supportsRemoteWorkspaces: true },
        { id: "vscode", label: "Visual Studio Code", supportsRemoteWorkspaces: true },
        { id: "xcode", label: "Xcode", supportsRemoteWorkspaces: false },
      ],
      terminals: [
        { id: "terminal", label: "Terminal", supportsRemoteWorkspaces: false },
        { id: "ghostty", label: "Ghostty", supportsRemoteWorkspaces: false },
      ],
    });
    preferenceMocks.saveAppPreferences.mockImplementation(async (preferences) => preferences);
  });

  it("lists local and remote machines without repository discovery settings", () => {
    render(
      <ThemeProvider storageKey="test-theme">
        <SettingsDialog
          machines={machines}
          busy={false}
          onClose={() => undefined}
          onRemoveMachine={async () => true}
          onMoveMachine={async () => undefined}
          onSaveMachine={async () => true}
          onTestMachine={async () => null}
        />
      </ThemeProvider>,
    );

    expect(screen.getByRole("heading", { name: "Settings" })).toBeInTheDocument();
    expect(screen.getByText("This computer")).toBeInTheDocument();
    expect(screen.getByText("deploy@buildbox:2222")).toBeInTheDocument();
    expect(screen.queryByText(/folders/i)).not.toBeInTheDocument();
  });

  it("changes the theme from the Appearance section", () => {
    localStorage.removeItem("test-theme");
    render(
      <ThemeProvider storageKey="test-theme">
        <SettingsDialog
          machines={machines.slice(0, 1)}
          busy={false}
          onClose={() => undefined}
          onRemoveMachine={async () => true}
          onMoveMachine={async () => undefined}
          onSaveMachine={async () => true}
          onTestMachine={async () => null}
        />
      </ThemeProvider>,
    );
    fireEvent.click(screen.getByRole("radio", { name: "Dark" }));
    expect(localStorage.getItem("test-theme")).toBe("dark");
    expect(document.documentElement.classList.contains("dark")).toBe(true);
  });

  it("shows detected applications with human labels and falls back from an unavailable preference", async () => {
    render(
      <ThemeProvider storageKey="test-theme">
        <SettingsDialog
          machines={machines.slice(0, 1)}
          busy={false}
          onClose={() => undefined}
          onRemoveMachine={async () => true}
          onMoveMachine={async () => undefined}
          onSaveMachine={async () => true}
          onTestMachine={async () => null}
        />
      </ThemeProvider>,
    );

    const editorValue = await screen.findByText("Cursor");
    const terminalValue = await screen.findByText("Ghostty");
    const editor = editorValue.closest("button");
    expect(editor).not.toBeNull();
    expect(terminalValue.closest("button")).not.toBeNull();

    fireEvent.click(editor!);
    expect(await screen.findByRole("option", { name: "Visual Studio Code" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "Xcode" })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "Zed" })).not.toBeInTheDocument();
  });
});
