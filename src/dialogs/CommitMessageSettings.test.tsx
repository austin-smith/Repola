import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CommitMessageSettings } from "./CommitMessageSettings";
import type { TextGenerationStatus } from "../ipc/types";

const mocks = vi.hoisted(() => ({ loadTextGenerationStatus: vi.fn() }));
vi.mock("../ipc/worktrees", () => mocks);
const machine = { id: "local", name: "This computer", kind: "local" as const, enabled: true, ssh: null };
const ready: TextGenerationStatus = {
  status: "ready", version: null,
  recommendedSelection: { provider: "codex", model: "test-model", reasoningEffort: null },
  models: [{ model: "test-model", displayName: "Test model", description: "Marketing copy", isDefault: true, recommendedForCommitMessages: true, upgrade: null, defaultReasoningEffort: null, supportedReasoningEfforts: [] }],
};

describe("CommitMessageSettings", () => {
  afterEach(cleanup);
  beforeEach(() => { mocks.loadTextGenerationStatus.mockReset().mockResolvedValue(ready); });

  it("places refresh beside the provider toggle with no empty footer row", async () => {
    render(<CommitMessageSettings machine={machine} selection={null} disabled={false} onChange={vi.fn()} />);
    await screen.findByText("Test model");
    const refresh = screen.getByRole("button", { name: "Check Codex again" });
    const providerRow = screen.getByRole("button", { name: "Codex" }).closest('[data-slot="field"]');
    expect(refresh.closest('[data-slot="field"]')).toBe(providerRow);
    const modelRow = screen.getByRole("button", { name: "Model Test model" }).closest('[data-slot="field-group"]');
    expect(modelRow?.nextElementSibling).toBeNull();
  });

  it.each(["codex", "claude"] as const)("reserves the form layout while %s loads", (provider) => {
    mocks.loadTextGenerationStatus.mockReturnValue(new Promise(() => {}));
    render(<CommitMessageSettings machine={machine} selection={{ provider, model: null, reasoningEffort: null }} disabled={false} onChange={vi.fn()} />);
    const loading = screen.getByRole("status", { name: `Loading ${provider === "codex" ? "Codex" : "Claude"} settings` });
    expect(loading).toHaveAttribute("aria-busy", "true");
    expect(loading.querySelectorAll('[data-slot="skeleton"]')).toHaveLength(2);
    expect(screen.queryByText(/Checking (Codex|Claude)/)).not.toBeInTheDocument();
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
  });

  it("keeps loaded controls mounted during refresh and a failed refresh", async () => {
    let rejectRefresh!: (error: Error) => void;
    mocks.loadTextGenerationStatus.mockResolvedValueOnce(ready)
      .mockImplementationOnce(() => new Promise((_, reject) => { rejectRefresh = reject; }));
    render(<CommitMessageSettings machine={machine} selection={null} disabled={false} onChange={vi.fn()} />);
    const model = await screen.findByRole("button", { name: /^Model / });
    fireEvent.click(screen.getByRole("button", { name: "Check Codex again" }));
    expect(screen.getByRole("button", { name: /^Model / })).toBe(model);
    expect(screen.queryByRole("status", { name: "Loading Codex settings" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Check Codex again" })).toBeDisabled();
    await act(async () => rejectRefresh(new Error("Connection lost")));
    expect(screen.getByRole("alert")).toHaveTextContent("Connection lost");
    expect(screen.getByRole("button", { name: /^Model / })).toBe(model);
    expect(screen.getByRole("button", { name: "Check Codex again" })).toBeEnabled();
  });

  it("replaces initial skeletons with an actionable discovery error", async () => {
    mocks.loadTextGenerationStatus.mockRejectedValue(new Error("Connection lost"));
    render(<CommitMessageSettings machine={machine} selection={null} disabled={false} onChange={vi.fn()} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("Connection lost");
    expect(screen.queryByRole("status", { name: "Loading Codex settings" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Check Codex again" })).toBeEnabled();
  });

  it("selects a provider without requiring it to be installed", async () => {
    const onChange = vi.fn();
    render(<CommitMessageSettings machine={machine} selection={null} disabled={false} onChange={onChange} />);
    await screen.findByText("Test model");
    fireEvent.click(screen.getByRole("button", { name: "Claude" }));
    expect(onChange).toHaveBeenCalledWith({ provider: "claude", model: null, reasoningEffort: null });
    expect(screen.queryByText("Marketing copy")).not.toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "Reasoning" })).toBeDisabled();
  });

  it("aborts old provider discovery and ignores late responses", async () => {
    let resolveOld!: (status: TextGenerationStatus) => void;
    mocks.loadTextGenerationStatus.mockImplementationOnce(() => new Promise((resolve) => { resolveOld = resolve; }))
      .mockResolvedValueOnce({ ...ready, status: "signedOut", models: [], recommendedSelection: null });
    const props = { machine, disabled: false, onChange: vi.fn() };
    const view = render(<CommitMessageSettings {...props} selection={null} />);
    const oldSignal = mocks.loadTextGenerationStatus.mock.calls[0][2] as AbortSignal;
    view.rerender(<CommitMessageSettings {...props} selection={{ provider: "claude", model: null, reasoningEffort: null }} />);
    expect(await screen.findByText("Run claude auth login on This computer.")).toBeInTheDocument();
    expect(oldSignal.aborted).toBe(true);
    await act(async () => resolveOld(ready));
    expect(screen.queryByText("Test model")).not.toBeInTheDocument();
  });

  it("pairs model and reasoning in a responsive row", async () => {
    mocks.loadTextGenerationStatus.mockResolvedValue({
      ...ready,
      recommendedSelection: { ...ready.recommendedSelection, reasoningEffort: "low" },
      models: [{ ...ready.models[0], defaultReasoningEffort: "low", supportedReasoningEfforts: [{ reasoningEffort: "low", description: "" }] }],
    });
    render(<CommitMessageSettings machine={machine} selection={null} disabled={false} onChange={vi.fn()} />);
    const model = await screen.findByRole("button", { name: /^Model / });
    const reasoning = screen.getByRole("combobox", { name: "Reasoning" });
    const row = model.closest('[data-slot="field-group"]');
    expect(row).toBe(reasoning.closest('[data-slot="field-group"]'));
    expect(row).toHaveClass("grid", "grid-cols-1", "@sm/field-group:grid-cols-[minmax(0,2fr)_minmax(0,1fr)]");
  });

  it("preserves the reasoning column when the model has no reasoning options", async () => {
    render(<CommitMessageSettings machine={machine} selection={null} disabled={false} onChange={vi.fn()} />);
    const model = await screen.findByRole("button", { name: /^Model / });
    expect(model.closest('[data-slot="field-group"]')).toHaveClass("grid-cols-1");
    expect(model.closest('[data-slot="field-group"]')).toHaveClass("@sm/field-group:grid-cols-[minmax(0,2fr)_minmax(0,1fr)]");
    expect(screen.getByRole("combobox", { name: "Reasoning" })).toBeDisabled();
    expect(screen.getByText("Not applicable")).toBeInTheDocument();
  });

  it("reuses each provider's loaded discovery without another skeleton or request", async () => {
    const claude = { ...ready, recommendedSelection: { provider: "claude", model: "claude-model", reasoningEffort: null }, models: [{ ...ready.models[0], model: "claude-model", displayName: "Claude model" }] };
    mocks.loadTextGenerationStatus.mockResolvedValueOnce(ready).mockResolvedValueOnce(claude);
    const props = { machine, disabled: false, onChange: vi.fn() };
    const view = render(<CommitMessageSettings {...props} selection={null} />);
    await screen.findByText("Test model");
    view.rerender(<CommitMessageSettings {...props} selection={{ provider: "claude", model: null, reasoningEffort: null }} />);
    await screen.findByText("Claude model");
    view.rerender(<CommitMessageSettings {...props} selection={null} />);
    expect(screen.getByText("Test model")).toBeInTheDocument();
    expect(screen.queryByRole("status", { name: "Loading Codex settings" })).not.toBeInTheDocument();
    view.rerender(<CommitMessageSettings {...props} selection={{ provider: "claude", model: null, reasoningEffort: null }} />);
    expect(screen.getByText("Claude model")).toBeInTheDocument();
    expect(mocks.loadTextGenerationStatus).toHaveBeenCalledTimes(2);
  });

  it("does not reuse discovery from another machine", async () => {
    const props = { disabled: false, onChange: vi.fn(), selection: null };
    const view = render(<CommitMessageSettings {...props} machine={machine} />);
    await screen.findByText("Test model");
    mocks.loadTextGenerationStatus.mockReturnValue(new Promise(() => {}));
    view.rerender(<CommitMessageSettings {...props} machine={{ ...machine, id: "remote" }} />);
    expect(screen.getByRole("status", { name: "Loading Codex settings" })).toBeInTheDocument();
    expect(screen.queryByText("Test model")).not.toBeInTheDocument();
    expect(mocks.loadTextGenerationStatus).toHaveBeenLastCalledWith("remote", "codex", expect.any(AbortSignal));
  });

  it("checks remote providers on the selected machine", async () => {
    const remote = { ...machine, id: "remote-id", name: "Build server" };
    render(<CommitMessageSettings machine={remote} selection={{ provider: "claude", model: null, reasoningEffort: null }} disabled={false} onChange={vi.fn()} />);
    await waitFor(() => expect(mocks.loadTextGenerationStatus).toHaveBeenCalledWith("remote-id", "claude", expect.any(AbortSignal)));
  });

  it("shows an unavailable saved model instead of pretending the default is selected", async () => {
    render(<CommitMessageSettings machine={machine} selection={{ provider: "codex", model: "removed-model", reasoningEffort: null }} disabled={false} onChange={vi.fn()} />);
    expect(await screen.findByText("This saved model is unavailable. Choose another model.")).toBeInTheDocument();
    expect(screen.getByText("removed-model")).toBeInTheDocument();
  });

  it("keeps older models inside the submenu and displays the selected older model without expanding the list", async () => {
    const legacy = { ...ready.models[0], model: "legacy", displayName: "Legacy model", recommendedForCommitMessages: false, isDefault: false };
    mocks.loadTextGenerationStatus.mockResolvedValue({ ...ready, models: [...ready.models, legacy] });
    const onChange = vi.fn();
    const props = { machine, disabled: false, onChange };
    const view = render(<CommitMessageSettings {...props} selection={null} />);
    const trigger = await screen.findByRole("button", { name: "Model Test model" });
    expect(screen.queryByRole("button", { name: /Other models/ })).not.toBeInTheDocument();
    fireEvent.click(trigger);
    expect(await screen.findByRole("menuitemradio", { name: "Test model" })).toHaveAttribute("aria-checked", "true");
    expect(screen.queryByRole("menuitemradio", { name: "Legacy model" })).not.toBeInTheDocument();
    const more = await screen.findByRole("menuitem", { name: "More models…" });
    fireEvent.keyDown(more, { key: "ArrowRight" });
    fireEvent.click(await screen.findByRole("menuitemradio", { name: "Legacy model" }));
    expect(onChange).toHaveBeenCalledWith({ provider: "codex", model: "legacy", reasoningEffort: null });
    await waitFor(() => expect(screen.queryByRole("menu")).not.toBeInTheDocument());
    view.rerender(<CommitMessageSettings {...props} selection={{ provider: "codex", model: "legacy", reasoningEffort: null }} />);
    fireEvent.click(screen.getByRole("button", { name: "Model Legacy model" }));
    await screen.findByRole("menuitemradio", { name: "Test model" });
    expect(screen.queryByRole("menuitemradio", { name: "Legacy model" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("menuitem", { name: "More models…" }));
    expect(await screen.findByRole("menuitemradio", { name: "Legacy model" })).toHaveAttribute("aria-checked", "true");
  });
});
