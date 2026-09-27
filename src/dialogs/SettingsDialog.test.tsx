import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ThemeProvider } from "@/components/theme-provider";
import { SettingsDialog } from "./SettingsDialog";

const preferenceMocks = vi.hoisted(() => ({
  loadAppPreferences: vi.fn(),
  loadExternalTools: vi.fn(),
  saveAppPreferences: vi.fn(),
}));

const codexMocks = vi.hoisted(() => ({
  loadTextGenerationStatus: vi.fn(),
}));

vi.mock("../ipc/app-preferences", () => ({
  ...preferenceMocks,
  launchWorktreeTool: vi.fn(),
}));

vi.mock("../ipc/worktrees", () => codexMocks);

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

function renderSettings() {
  return render(<ThemeProvider storageKey="test-theme"><SettingsDialog machines={machines} selectedMachineId="local" busy={false}
    onClose={vi.fn()} onRemoveMachine={async () => true} onMoveMachine={async () => undefined}
    onSaveMachine={async () => true} onTestMachine={async () => null} /></ThemeProvider>);
}

describe("SettingsDialog", () => {
  afterEach(cleanup);

  beforeEach(() => {
    vi.clearAllMocks();
    preferenceMocks.loadAppPreferences.mockResolvedValue({
      version: 5,
      editorId: "uninstalled-editor",
      terminalId: "ghostty",
      defaultSignCommits: false,
      textGenerationSelections: {},
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
    codexMocks.loadTextGenerationStatus.mockResolvedValue({
      status: "ready",
      version: "codex-cli 0.153.0",
      models: [
        {
          model: "gpt-5.6-sol",
          displayName: "GPT-5.6-Sol",
          description: "Reliable agentic workhorse for everyday tasks.",
          isDefault: true,
          recommendedForCommitMessages: true,
          upgrade: null,
          defaultReasoningEffort: "low",
          supportedReasoningEfforts: [
            { reasoningEffort: "low", description: "Fast responses with lighter reasoning" },
          ],
        },
        {
          model: "gpt-5.6-luna",
          displayName: "GPT-5.6-Luna",
          description: "Fast and affordable agentic coding model.",
          isDefault: false,
          recommendedForCommitMessages: true,
          upgrade: null,
          defaultReasoningEffort: "medium",
          supportedReasoningEfforts: [
            { reasoningEffort: "low", description: "Fast responses with lighter reasoning" },
            { reasoningEffort: "medium", description: "Balances speed and reasoning depth" },
          ],
        },
        {
          model: "gpt-5.4",
          displayName: "GPT-5.4",
          description: "Previous generation model.",
          isDefault: false,
          recommendedForCommitMessages: false,
          upgrade: "gpt-5.6-terra",
          defaultReasoningEffort: "medium",
          supportedReasoningEfforts: [
            { reasoningEffort: "low", description: "Fast responses with lighter reasoning" },
            { reasoningEffort: "medium", description: "Balances speed and reasoning depth" },
          ],
        },
      ],
      recommendedSelection: { provider: "codex", model: "gpt-5.6-luna", reasoningEffort: "low" },
    });
  });

  it("rolls back only AI settings on save failure, preserving edits made while saving", async () => {
    let rejectSave!: (error: Error) => void;
    preferenceMocks.saveAppPreferences.mockImplementationOnce(() => new Promise((_, reject) => { rejectSave = reject; }));
    await act(async () => { renderSettings(); });
    const ai = screen.getByText("AI").closest("section");
    const tools = screen.getByText("Tools & Git").closest("section");
    if (!ai || !tools) throw new Error("Settings sections missing");
    const signing = within(tools).getByRole("checkbox", { name: "Sign commits by default using Git configuration" });
    const save = within(tools).getByRole("button", { name: "Save preferences" });
    fireEvent.click(within(ai).getByRole("button", { name: "Claude" }));
    await waitFor(() => expect(preferenceMocks.saveAppPreferences).toHaveBeenCalledTimes(1));
    fireEvent.click(signing);
    expect(tools.querySelector('[data-slot="spinner"]')).toBeNull();
    await act(async () => rejectSave(new Error("Cannot save AI settings")));
    expect(signing).toBeChecked();
    expect(within(ai).getByRole("button", { name: "Codex" })).toHaveAttribute("aria-pressed", "true");
    expect(within(ai).getByRole("alert")).toHaveTextContent("Cannot save AI settings");
    fireEvent.click(save);
    await waitFor(() => expect(preferenceMocks.saveAppPreferences).toHaveBeenLastCalledWith(expect.objectContaining({
      defaultSignCommits: true, textGenerationSelections: {},
    })));
  });

  it("restores saved per-provider choices after switching and reopening settings", async () => {
    let stored = { version: 5, editorId: "cursor", terminalId: "ghostty", defaultSignCommits: false,
      textGenerationSelections: { local: { provider: "codex", selections: {
        codex: { model: "gpt-5.6-sol", reasoningEffort: "low" },
        claude: { model: "claude-model", reasoningEffort: null },
      } } } };
    preferenceMocks.loadAppPreferences.mockImplementation(async () => structuredClone(stored));
    preferenceMocks.saveAppPreferences.mockImplementation(async (preferences) => { stored = structuredClone(preferences); return preferences; });
    const view = renderSettings();
    await act(async () => undefined);
    const ai = screen.getByText("AI").closest("section");
    if (!ai) throw new Error("AI section missing");
    expect(within(ai).getByText("GPT-5.6-Sol")).toBeInTheDocument();
    await act(async () => fireEvent.click(within(ai).getByRole("button", { name: "Claude" })));
    expect(stored.textGenerationSelections.local.selections.codex.model).toBe("gpt-5.6-sol");
    expect(stored.textGenerationSelections.local.provider).toBe("claude");
    view.unmount();
    await act(async () => { renderSettings(); });
    const reopened = screen.getByText("AI").closest("section");
    if (!reopened) throw new Error("AI section missing after reopening");
    await act(async () => fireEvent.click(within(reopened).getByRole("button", { name: "Codex" })));
    expect(within(reopened).getByText("GPT-5.6-Sol")).toBeInTheDocument();
    expect(stored.textGenerationSelections.local.provider).toBe("codex");
    expect(stored.textGenerationSelections.local.selections.claude.model).toBe("claude-model");
  });

  it("lists local and remote machines without repository discovery settings", () => {
    render(
      <ThemeProvider storageKey="test-theme">
        <SettingsDialog
          machines={machines}
          selectedMachineId="local"
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

  it("shows Codex readiness for the selected machine", async () => {
    render(
      <ThemeProvider storageKey="test-theme">
        <SettingsDialog
          machines={machines}
          selectedMachineId="local"
          busy={false}
          onClose={() => undefined}
          onRemoveMachine={async () => true}
          onMoveMachine={async () => undefined}
          onSaveMachine={async () => true}
          onTestMachine={async () => null}
        />
      </ThemeProvider>,
    );

    expect(screen.getByRole("heading", { name: "AI" })).toBeInTheDocument();
    expect(screen.queryByText("Integrations")).not.toBeInTheDocument();
    expect(await screen.findByText("GPT-5.6-Luna")).toBeInTheDocument();
    expect(screen.queryByText(/Ready on/)).not.toBeInTheDocument();
    expect(screen.getByText("Low")).toBeInTheDocument();
    expect(screen.queryByText("Fast and affordable agentic coding model.")).not.toBeInTheDocument();
    expect(codexMocks.loadTextGenerationStatus).toHaveBeenCalledWith("local", "codex", expect.any(AbortSignal));
  });

  it("persists an explicit commit-message model for the selected machine", async () => {
    render(
      <ThemeProvider storageKey="test-theme">
        <SettingsDialog
          machines={machines}
          selectedMachineId="local"
          busy={false}
          onClose={() => undefined}
          onRemoveMachine={async () => true}
          onMoveMachine={async () => undefined}
          onSaveMachine={async () => true}
          onTestMachine={async () => null}
        />
      </ThemeProvider>,
    );

    const selectedModel = await screen.findByText("GPT-5.6-Luna");
    const modelTrigger = selectedModel.closest("button");
    expect(modelTrigger).not.toBeNull();
    if (!modelTrigger) throw new Error("model trigger not found");
    fireEvent.click(modelTrigger);
    const option = await screen.findByRole("menuitemradio", { name: "GPT-5.6-Sol" });
    fireEvent.pointerDown(option, { button: 0 });
    fireEvent.pointerUp(option, { button: 0 });
    fireEvent.click(option, { button: 0 });

    await waitFor(() => expect(preferenceMocks.saveAppPreferences).toHaveBeenCalledWith(expect.objectContaining({
      textGenerationSelections: { local: { provider: "codex", selections: { codex: { model: "gpt-5.6-sol", reasoningEffort: "low" } } } },
    })));
  });

  it("keeps older and specialized Codex models out of the normal picker", async () => {
    render(
      <ThemeProvider storageKey="test-theme">
        <SettingsDialog
          machines={machines}
          selectedMachineId="local"
          busy={false}
          onClose={() => undefined}
          onRemoveMachine={async () => true}
          onMoveMachine={async () => undefined}
          onSaveMachine={async () => true}
          onTestMachine={async () => null}
        />
      </ThemeProvider>,
    );

    const selectedModel = await screen.findByText("GPT-5.6-Luna");
    const modelTrigger = selectedModel.closest("button");
    expect(modelTrigger).not.toBeNull();
    if (!modelTrigger) throw new Error("model trigger not found");
    fireEvent.click(modelTrigger);
    expect(screen.queryByRole("menuitemradio", { name: "GPT-5.4" })).not.toBeInTheDocument();
    fireEvent.click(await screen.findByRole("menuitem", { name: "More models…" }));
    expect(await screen.findByRole("menuitemradio", { name: "GPT-5.4" })).toBeInTheDocument();
  });

  it("changes the theme from the Appearance section", () => {
    localStorage.removeItem("test-theme");
    render(
      <ThemeProvider storageKey="test-theme">
        <SettingsDialog
          machines={machines.slice(0, 1)}
          selectedMachineId="local"
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
          selectedMachineId="local"
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
    if (!editor) throw new Error("editor trigger not found");

    fireEvent.click(editor);
    expect(await screen.findByRole("option", { name: "Visual Studio Code" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "Xcode" })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "Zed" })).not.toBeInTheDocument();
  });
});
