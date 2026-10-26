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

  it("displays the detected provider without saving it, and pins an explicit choice without reloading", async () => {
    mocks.loadTextGenerationStatus.mockResolvedValue({ ...ready, recommendedSelection: { provider: "claude", model: "test-model", reasoningEffort: null } });
    const onChange = vi.fn();
    const props = { machine, disabled: false, onChange };
    const view = render(<CommitMessageSettings {...props} selection={null} />);
    const model = await screen.findByRole("button", { name: "Model Test model" });
    expect(mocks.loadTextGenerationStatus).toHaveBeenCalledWith("local", null, expect.any(AbortSignal));
    expect(screen.getByRole("button", { name: "Claude" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: "Codex" })).toHaveAttribute("aria-pressed", "false");
    expect(onChange).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Claude" }));
    const explicit = { provider: "claude" as const, model: null, reasoningEffort: null };
    expect(onChange).toHaveBeenCalledWith(explicit);
    view.rerender(<CommitMessageSettings {...props} selection={explicit} />);
    expect(screen.getByRole("button", { name: "Model Test model" })).toBe(model);
    expect(mocks.loadTextGenerationStatus).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: "Check Claude again" }));
    await waitFor(() => expect(mocks.loadTextGenerationStatus).toHaveBeenLastCalledWith("local", "claude", expect.any(AbortSignal)));
  });

  it("rechecks automatic availability without saving a preference", async () => {
    const onChange = vi.fn();
    mocks.loadTextGenerationStatus.mockResolvedValueOnce(ready).mockResolvedValueOnce({ ...ready, recommendedSelection: { provider: "claude", model: "test-model", reasoningEffort: null } });
    render(<CommitMessageSettings machine={machine} selection={null} disabled={false} onChange={onChange} />);
    await screen.findByText("Test model");
    fireEvent.click(screen.getByRole("button", { name: "Check Codex again" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Claude" })).toHaveAttribute("aria-pressed", "true"));
    expect(mocks.loadTextGenerationStatus).toHaveBeenLastCalledWith("local", null, expect.any(AbortSignal));
    expect(onChange).not.toHaveBeenCalled();
  });

  it("shows setup guidance with no selected provider when neither is ready", async () => {
    const detail = "No AI provider is ready. Install the Codex CLI or Claude Code on this machine.";
    mocks.loadTextGenerationStatus.mockResolvedValue({ status: "unavailable", detail, version: null, models: [], recommendedSelection: null });
    const onChange = vi.fn();
    render(<CommitMessageSettings machine={machine} selection={null} disabled={false} onChange={onChange} />);
    await screen.findByText(detail);
    for (const name of ["Codex", "Claude"]) expect(screen.getByRole("button", { name })).toHaveAttribute("aria-pressed", "false");
    expect(screen.getByRole("button", { name: "Model Unavailable" })).toBeDisabled();
    expect(onChange).not.toHaveBeenCalled();
  });

  it("keeps an unavailable saved provider selected without probing a fallback", async () => {
    mocks.loadTextGenerationStatus.mockResolvedValue({ status: "notInstalled", version: null, models: [], recommendedSelection: null });
    const onChange = vi.fn();
    render(<CommitMessageSettings machine={machine} selection={{ provider: "codex", model: null, reasoningEffort: null }} disabled={false} onChange={onChange} />);
    await screen.findByText("Install the Codex CLI on This computer.");
    expect(screen.getByRole("button", { name: "Codex" })).toHaveAttribute("aria-pressed", "true");
    expect(mocks.loadTextGenerationStatus).toHaveBeenCalledExactlyOnceWith("local", "codex", expect.any(AbortSignal));
    expect(onChange).not.toHaveBeenCalled();
  });

  it("waits for preferences before discovering providers", async () => {
    const props = { machine, onChange: vi.fn() };
    const view = render(<CommitMessageSettings {...props} selection={null} disabled />);
    expect(mocks.loadTextGenerationStatus).not.toHaveBeenCalled();
    view.rerender(<CommitMessageSettings {...props} selection={{ provider: "claude", model: null, reasoningEffort: null }} disabled={false} />);
    await waitFor(() => expect(mocks.loadTextGenerationStatus).toHaveBeenCalledExactlyOnceWith("local", "claude", expect.any(AbortSignal)));
  });

  it("shows decorative provider logos without changing button names or selection", async () => {
    render(<CommitMessageSettings machine={machine} selection={null} disabled={false} onChange={vi.fn()} />);
    await screen.findByText("Test model");
    for (const name of ["Codex", "Claude"]) {
      const button = screen.getByRole("button", { name });
      const logo = button.querySelector("svg");
      expect(button).toHaveTextContent(name);
      expect(logo).toHaveAttribute("aria-hidden", "true");
      expect(logo).toHaveAttribute("focusable", "false");
      if (name === "Codex") {
        expect(logo).toHaveAttribute("fill", "currentColor");
      } else {
        expect(logo?.querySelector("path")).toHaveAttribute("fill", "#D97757");
      }
      expect(logo).toHaveAttribute("viewBox", "0 0 24 24");
      expect(button).toHaveAttribute("aria-pressed", String(name === "Codex"));
    }
  });

  it("places refresh beside the provider toggle with no empty footer row", async () => {
    render(<CommitMessageSettings machine={machine} selection={null} disabled={false} onChange={vi.fn()} />);
    await screen.findByText("Test model");
    const refresh = screen.getByRole("button", { name: "Check Codex again" });
    const providerRow = screen.getByRole("button", { name: "Codex" }).closest('[data-slot="field"]');
    expect(refresh.closest('[data-slot="field"]')).toBe(providerRow);
    const modelRow = screen.getByRole("button", { name: "Model Test model" }).closest('[data-slot="field-group"]');
    expect(modelRow?.nextElementSibling).toBeNull();
  });

  it("keeps exactly one provider selected with the standard default controls", async () => {
    const onChange = vi.fn();
    const props = { machine, disabled: false, onChange };
    const view = render(<CommitMessageSettings {...props} selection={{ provider: "codex", model: null, reasoningEffort: null }} />);
    await screen.findByText("Test model");
    const codex = screen.getByRole("button", { name: "Codex" });
    const claude = screen.getByRole("button", { name: "Claude" });
    for (const button of [codex, claude]) {
      expect(button).toHaveAttribute("data-variant", "default");
      expect(button.querySelectorAll("svg")).toHaveLength(1);
    }
    fireEvent.click(codex);
    expect(onChange).not.toHaveBeenCalled();
    expect(codex).toHaveAttribute("aria-pressed", "true");
    view.rerender(<CommitMessageSettings {...props} selection={{ provider: "claude", model: null, reasoningEffort: null }} />);
    expect(codex).toHaveAttribute("aria-pressed", "false");
    expect(claude).toHaveAttribute("aria-pressed", "true");
  });

  it.each(["codex", "claude"] as const)("reserves the form layout while %s loads", (provider) => {
    mocks.loadTextGenerationStatus.mockReturnValue(new Promise(() => {}));
    render(<CommitMessageSettings machine={machine} selection={{ provider, model: null, reasoningEffort: null }} disabled={false} onChange={vi.fn()} />);
    const loading = screen.getByRole("status", { name: `Loading ${provider === "codex" ? "Codex" : "Claude"} settings` });
    expect(loading).toHaveAttribute("aria-busy", "true");
    expect(loading.querySelectorAll('[data-slot="skeleton"]')).toHaveLength(2);
    expect(screen.queryByText(/Checking (Codex|Claude)/)).not.toBeInTheDocument();
    expect(screen.getByRole("combobox")).toHaveClass("invisible");
    expect(screen.getByRole("combobox")).toBeDisabled();
    for (const skeleton of loading.querySelectorAll('[data-slot="skeleton"]')) {
      expect(skeleton).toHaveClass("animate-none");
    }
  });

  it("keeps loaded controls mounted during refresh and a failed refresh", async () => {
    let rejectRefresh!: (error: Error) => void;
    mocks.loadTextGenerationStatus.mockResolvedValueOnce(ready)
      .mockImplementationOnce(() => new Promise((_, reject) => { rejectRefresh = reject; }));
    render(<CommitMessageSettings machine={machine} selection={null} disabled={false} onChange={vi.fn()} />);
    const model = await screen.findByRole("button", { name: "Model Test model" });
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
    expect(screen.queryByRole("status", { name: "Loading AI provider settings" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Check AI providers again" })).toBeEnabled();
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
    const model = await screen.findByRole("button", { name: "Model Test model" });
    const reasoning = screen.getByRole("combobox", { name: "Reasoning" });
    const row = model.closest('[data-slot="field-group"]');
    expect(row).toBe(reasoning.closest('[data-slot="field-group"]'));
    expect(row).toHaveClass("grid", "grid-cols-1", "@sm/field-group:grid-cols-[minmax(0,2fr)_minmax(0,1fr)]");
  });

  it("preserves the reasoning column when the model has no reasoning options", async () => {
    render(<CommitMessageSettings machine={machine} selection={null} disabled={false} onChange={vi.fn()} />);
    const model = await screen.findByRole("button", { name: "Model Test model" });
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

  it("preserves controls and provider focus through cold and cached switches", async () => {
    let resolveClaude!: (status: TextGenerationStatus) => void;
    const codex: TextGenerationStatus = {
      ...ready,
      recommendedSelection: { provider: "codex", model: "test-model", reasoningEffort: "low" },
      models: [{ ...ready.models[0], defaultReasoningEffort: "low", supportedReasoningEfforts: [{ reasoningEffort: "low", description: "" }] }],
    };
    mocks.loadTextGenerationStatus.mockResolvedValueOnce(codex)
      .mockImplementationOnce(() => new Promise((resolve) => { resolveClaude = resolve; }));
    const props = { machine, disabled: false, onChange: vi.fn() };
    const view = render(<CommitMessageSettings {...props} selection={null} />);
    const model = await screen.findByRole("button", { name: "Model Test model" });
    const reasoning = screen.getByRole("combobox", { name: "Reasoning" });
    const claude = screen.getByRole("button", { name: "Claude" });
    claude.focus();
    const claudeSelection = { provider: "claude" as const, model: null, reasoningEffort: null };
    view.rerender(<CommitMessageSettings {...props} selection={claudeSelection} />);
    expect(claude).toHaveFocus();
    expect(model).toBeInTheDocument();
    expect(reasoning).toBeInTheDocument();
    expect(model).toHaveClass("invisible");
    await act(async () => resolveClaude({ ...ready, recommendedSelection: { provider: "claude", model: "test-model", reasoningEffort: null } }));
    expect(screen.getByRole("button", { name: "Model Test model" })).toBe(model);
    expect(screen.getByRole("combobox", { name: "Reasoning" })).toBe(reasoning);
    expect(reasoning).toHaveTextContent("Not applicable");
    view.rerender(<CommitMessageSettings {...props} selection={null} />);
    expect(screen.getByRole("combobox", { name: "Reasoning" })).toBe(reasoning);
    expect(reasoning).toHaveTextContent("Low");
    expect(reasoning).toBeEnabled();
    expect(model).not.toHaveClass("invisible");
    expect(mocks.loadTextGenerationStatus).toHaveBeenCalledTimes(2);
  });

  it("does not reuse discovery from another machine", async () => {
    const props = { disabled: false, onChange: vi.fn(), selection: null };
    const view = render(<CommitMessageSettings {...props} machine={machine} />);
    await screen.findByText("Test model");
    mocks.loadTextGenerationStatus.mockReturnValue(new Promise(() => {}));
    view.rerender(<CommitMessageSettings {...props} machine={{ ...machine, id: "remote" }} />);
    expect(screen.getByRole("status", { name: "Loading AI provider settings" })).toBeInTheDocument();
    expect(screen.queryByText("Test model")).not.toBeInTheDocument();
    expect(mocks.loadTextGenerationStatus).toHaveBeenLastCalledWith("remote", null, expect.any(AbortSignal));
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
