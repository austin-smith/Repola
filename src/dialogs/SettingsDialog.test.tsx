import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ThemeProvider } from "@/components/theme-provider";
import { SettingsDialog } from "./SettingsDialog";
import type { AppPreferences } from "../ipc/types";

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
  updateAppPreferences: async (update: (current: AppPreferences) => AppPreferences) =>
    preferenceMocks.saveAppPreferences(update(await preferenceMocks.loadAppPreferences())),
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

function renderSettings(profiles = machines) {
  return render(<ThemeProvider storageKey="test-theme"><SettingsDialog machines={profiles} selectedMachineId="local" busy={false}
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
    await act(async () => fireEvent.click(within(ai).getByRole("button", { name: "Claude" })));
    expect(preferenceMocks.saveAppPreferences).toHaveBeenCalledTimes(1);
    fireEvent.click(signing);
    expect(tools.querySelector('[data-slot="spinner"]')).toBeNull();
    await act(async () => rejectSave(new Error("Cannot save AI settings")));
    expect(signing).toBeChecked();
    expect(within(ai).getByRole("button", { name: "Codex" })).toHaveAttribute("aria-pressed", "true");
    expect(within(ai).getByRole("alert")).toHaveTextContent("Cannot save AI settings");
    await act(async () => fireEvent.click(save));
    expect(preferenceMocks.saveAppPreferences).toHaveBeenLastCalledWith(expect.objectContaining({
      defaultSignCommits: true, textGenerationSelections: {},
    }));
  });

  it("keeps controls enabled during rapid switches and rolls back to the last successful save", async () => {
    let resolveFirst!: () => void;
    let rejectSecond!: (error: Error) => void;
    preferenceMocks.saveAppPreferences
      .mockImplementationOnce((preferences) => new Promise((resolve) => { resolveFirst = () => resolve(preferences); }))
      .mockImplementationOnce(() => new Promise((_, reject) => { rejectSecond = reject; }));
    await act(async () => { renderSettings(); });
    const codex = screen.getByRole("button", { name: "Codex" });
    const claude = screen.getByRole("button", { name: "Claude" });
    const model = screen.getByRole("button", { name: /^Model / });
    await act(async () => fireEvent.click(claude));
    expect(codex).toBeEnabled();
    expect(claude).toBeEnabled();
    expect(model).toBeEnabled();
    expect(screen.getByRole("button", { name: "Save preferences" })).toBeEnabled();
    await act(async () => fireEvent.click(codex));
    expect(preferenceMocks.saveAppPreferences).toHaveBeenCalledTimes(2);
    expect(codex).toHaveAttribute("aria-pressed", "true");
    await act(async () => resolveFirst());
    // An older response must not change the latest visible choice.
    expect(codex).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: /^Model / })).toBe(model);
    await act(async () => rejectSecond(new Error("Cannot save latest choice")));
    expect(claude).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("alert")).toHaveTextContent("Cannot save latest choice");
  });

  it("ignores a superseded failure without reverting or flashing the latest choice", async () => {
    let rejectFirst!: (error: Error) => void;
    let resolveSecond!: () => void;
    preferenceMocks.saveAppPreferences
      .mockImplementationOnce(() => new Promise((_, reject) => { rejectFirst = reject; }))
      .mockImplementationOnce((preferences) => new Promise((resolve) => { resolveSecond = () => resolve(preferences); }));
    await act(async () => { renderSettings(); });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Claude" })));
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Codex" })));
    await act(async () => rejectFirst(new Error("Old save failed")));
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Codex" })).toHaveAttribute("aria-pressed", "true");
    await act(async () => resolveSecond());
    expect(screen.getByRole("button", { name: "Codex" })).toBeEnabled();
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
    await act(async () => { renderSettings(); });

    expect(screen.getByRole("heading", { name: "AI" })).toBeInTheDocument();
    expect(screen.queryByText("Integrations")).not.toBeInTheDocument();
    expect(screen.getByText("GPT-5.6-Luna")).toBeInTheDocument();
    expect(screen.queryByText(/Ready on/)).not.toBeInTheDocument();
    expect(screen.getByText("Low")).toBeInTheDocument();
    expect(screen.queryByText("Fast and affordable agentic coding model.")).not.toBeInTheDocument();
    expect(codexMocks.loadTextGenerationStatus).toHaveBeenCalledWith("local", null, expect.any(AbortSignal));
    expect(preferenceMocks.saveAppPreferences).not.toHaveBeenCalled();
  });

  it("persists an explicit commit-message model for the selected machine", async () => {
    await act(async () => { renderSettings(); });

    const modelTrigger = screen.getByText("GPT-5.6-Luna").closest("button");
    expect(modelTrigger).not.toBeNull();
    if (!modelTrigger) throw new Error("model trigger not found");
    await act(async () => fireEvent.click(modelTrigger));
    const option = screen.getByRole("menuitemradio", { name: "GPT-5.6-Sol" });
    await act(async () => {
      fireEvent.pointerDown(option, { button: 0 });
      fireEvent.pointerUp(option, { button: 0 });
      fireEvent.click(option, { button: 0 });
    });

    expect(preferenceMocks.saveAppPreferences).toHaveBeenCalledWith(expect.objectContaining({
      textGenerationSelections: { local: { provider: "codex", selections: { codex: { model: "gpt-5.6-sol", reasoningEffort: "low" } } } },
    }));
  });

  it("keeps older and specialized Codex models out of the normal picker", async () => {
    await act(async () => { renderSettings(); });

    const modelTrigger = screen.getByText("GPT-5.6-Luna").closest("button");
    expect(modelTrigger).not.toBeNull();
    if (!modelTrigger) throw new Error("model trigger not found");
    await act(async () => fireEvent.click(modelTrigger));
    expect(screen.queryByRole("menuitemradio", { name: "GPT-5.4" })).not.toBeInTheDocument();
    await act(async () => fireEvent.click(screen.getByRole("menuitem", { name: "More models…" })));
    expect(screen.getByRole("menuitemradio", { name: "GPT-5.4" })).toBeInTheDocument();
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
    await act(async () => { renderSettings(machines.slice(0, 1)); });

    const editor = screen.getByText("Cursor").closest("button");
    expect(editor).not.toBeNull();
    expect(screen.getByText("Ghostty").closest("button")).not.toBeNull();
    if (!editor) throw new Error("editor trigger not found");

    await act(async () => fireEvent.click(editor));
    expect(screen.getByRole("option", { name: "Visual Studio Code" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "Xcode" })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "Zed" })).not.toBeInTheDocument();
  });
});
