import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ChangesWorkbench } from "./ChangesWorkbench";
import type { GeneratedCommitMessage, WorkingCopySnapshot } from "../ipc/types";

const ipc = vi.hoisted(() => ({
  fetchWorkingCopy: vi.fn(), generateCommitMessage: vi.fn(),
  watchWorktree: vi.fn(), unwatchWorktree: vi.fn(), onWorktreeChanged: vi.fn(),
}));
vi.mock("../ipc/worktrees", () => ipc);
vi.mock("../ipc/app-preferences", () => ({
  loadAppPreferences: async () => ({ defaultSignCommits: false, editorId: null }),
  loadExternalTools: async () => ({ editors: [], terminals: [] }),
}));
vi.mock("../app/environment", () => ({ usePathSeparator: () => "/" }));
vi.mock("./context", () => ({ useWorkingCopy: () => ({
  machineId: "local", machineKind: "local", machineOs: null,
  repository: { path: "/repo" }, worktree: { id: "main", path: "/repo" },
}) }));
vi.mock("./lazy", () => ({
  InlineFileDiff: ({ selectionDisabled }: { selectionDisabled: boolean }) => <input type="checkbox" aria-label="Select diff line" disabled={selectionDisabled} />,
}));

const snapshot: WorkingCopySnapshot = {
  repositoryPath: "/repo", worktreePath: "/repo", head: "abc", branch: "main",
  upstream: null, upstreamHead: null, remote: null, ahead: 0, behind: 0, operation: null,
  changes: [{ id: "file", path: { display: "file", token: "file" }, previousPath: null,
    kind: "modified", indexStatus: ".", worktreeStatus: "M", staged: false, unstaged: true,
    conflicted: false, untracked: false, ignored: false, submodule: false,
    headMode: null, indexMode: null, worktreeMode: null, modeChange: null, worktreeStamp: "unchanged" }],
};

describe("commit-message generation", () => {
  afterEach(cleanup);
  beforeEach(() => {
    vi.clearAllMocks();
    ipc.fetchWorkingCopy.mockResolvedValue(snapshot);
    ipc.watchWorktree.mockResolvedValue(undefined);
    ipc.unwatchWorktree.mockResolvedValue(undefined);
    ipc.onWorktreeChanged.mockResolvedValue(() => undefined);
  });

  it("fills the commit form from the generated subject and body", async () => {
    ipc.generateCommitMessage.mockResolvedValue({
      subject: "improve commit generation",
      body: "- use standard Git terminology",
    } satisfies GeneratedCommitMessage);
    render(<ChangesWorkbench />);
    const generate = await screen.findByRole("button", { name: "Generate commit message" });
    await act(async () => fireEvent.click(generate));
    expect(screen.getByPlaceholderText("Summary (required)")).toHaveValue("improve commit generation");
    expect(screen.getByPlaceholderText("Description")).toHaveValue("- use standard Git terminology");
  });

  it("locks selection, offers cancellation, and ignores a late cancelled response", async () => {
    let complete!: (message: GeneratedCommitMessage) => void;
    ipc.generateCommitMessage.mockImplementation(() => new Promise((resolve) => { complete = resolve; }));
    render(<ChangesWorkbench />);
    const generate = await screen.findByRole("button", { name: "Generate commit message" });
    const summary = screen.getByPlaceholderText("Summary (required)");
    fireEvent.change(summary, { target: { value: "my existing message" } });
    fireEvent.click(generate);
    expect(screen.getByRole("checkbox", { name: "Select diff line" })).toBeDisabled();
    expect(summary).toBeDisabled();
    const cancel = screen.getByRole("button", { name: "Cancel commit message generation" });
    expect(cancel).toBeEnabled();
    fireEvent.click(cancel);
    const signal = ipc.generateCommitMessage.mock.calls[0][2] as AbortSignal;
    expect(signal.aborted).toBe(true);
    expect(summary).toBeDisabled(); // Keep the form locked until the operation settles.
    await act(async () => complete({ subject: "stale result", body: "discard me" }));
    expect(summary).toHaveValue("my existing message");
    expect(summary).toBeEnabled();
    expect(screen.getByRole("checkbox", { name: "Select diff line" })).toBeEnabled();
    expect(screen.getByPlaceholderText("Description")).toHaveValue("");
  });

  it("aborts generation when the workbench is closed", async () => {
    ipc.generateCommitMessage.mockImplementation(() => new Promise(() => undefined));
    const view = render(<ChangesWorkbench />);
    fireEvent.click(await screen.findByRole("button", { name: "Generate commit message" }));
    const signal = ipc.generateCommitMessage.mock.calls[0][2] as AbortSignal;
    view.unmount();
    expect(signal.aborted).toBe(true);
  });

  it("refreshes external changes and cancels generation instead of applying a message for the old selection", async () => {
    let notify!: (event: { machineId: string }) => void;
    ipc.onWorktreeChanged.mockImplementation(async (listener) => { notify = listener; return () => undefined; });
    let complete!: (message: GeneratedCommitMessage) => void;
    ipc.generateCommitMessage.mockImplementation(() => new Promise((resolve) => { complete = resolve; }));
    render(<ChangesWorkbench />);
    const generate = await screen.findByRole("button", { name: "Generate commit message" });
    const summary = screen.getByPlaceholderText("Summary (required)");
    fireEvent.change(summary, { target: { value: "existing message" } });
    fireEvent.click(generate);
    const signal = ipc.generateCommitMessage.mock.calls[0][2] as AbortSignal;
    const changed = { ...snapshot, changes: [...snapshot.changes, { ...snapshot.changes[0], id: "new-file", path: { display: "new-file", token: "new-file" } }] };
    ipc.fetchWorkingCopy.mockResolvedValue(changed);
    await act(async () => {
      notify({ machineId: "local" });
    });
    await screen.findByText("new-file");
    expect(signal.aborted).toBe(true);
    expect(summary).toBeDisabled();
    expect(ipc.generateCommitMessage.mock.calls[0][1].includedChanges).toHaveLength(1);
    await act(async () => complete({ subject: "change file", body: "" }));
    expect(summary).toHaveValue("existing message");
    expect(screen.getByRole("button", { name: "Commit 2 files to main" })).toBeEnabled();
  });

  it("cancels generation when a previously started reload discovers a changed selection", async () => {
    render(<ChangesWorkbench />);
    const generate = await screen.findByRole("button", { name: "Generate commit message" });
    let reload!: (value: WorkingCopySnapshot) => void;
    ipc.fetchWorkingCopy.mockImplementationOnce(() => new Promise((resolve) => { reload = resolve; }));
    fireEvent(window, new Event("focus"));
    await waitFor(() => expect(ipc.fetchWorkingCopy).toHaveBeenCalledTimes(2));
    let complete!: (message: GeneratedCommitMessage) => void;
    ipc.generateCommitMessage.mockImplementation(() => new Promise((resolve) => { complete = resolve; }));
    await act(async () => fireEvent.click(generate));
    const signal = ipc.generateCommitMessage.mock.calls[0][2] as AbortSignal;
    await act(async () => reload({ ...snapshot, changes: [...snapshot.changes, { ...snapshot.changes[0], id: "late-file", path: { display: "late-file", token: "late-file" } }] }));
    expect(signal.aborted).toBe(true);
    expect(screen.getByText("late-file")).toBeInTheDocument();
    await act(async () => complete({ subject: "outdated message", body: "" }));
    expect(screen.getByPlaceholderText("Summary (required)")).toHaveValue("");
  });

  it("does not cancel generation for a focus refresh with an unchanged snapshot", async () => {
    let complete!: (message: GeneratedCommitMessage) => void;
    ipc.generateCommitMessage.mockImplementation(() => new Promise((resolve) => { complete = resolve; }));
    render(<ChangesWorkbench />);
    fireEvent.click(await screen.findByRole("button", { name: "Generate commit message" }));
    const signal = ipc.generateCommitMessage.mock.calls[0][2] as AbortSignal;
    await act(async () => fireEvent(window, new Event("focus")));
    expect(ipc.fetchWorkingCopy).toHaveBeenCalledTimes(2);
    expect(signal.aborted).toBe(false);
    await act(async () => complete({ subject: "change file", body: "" }));
    expect(screen.getByPlaceholderText("Summary (required)")).toHaveValue("change file");
    expect(screen.getByRole("button", { name: "Commit 1 file to main" })).toBeEnabled();
  });

  it("waits for a pending refresh before applying a completed generation", async () => {
    render(<ChangesWorkbench />);
    const generate = await screen.findByRole("button", { name: "Generate commit message" });
    let reload!: (value: WorkingCopySnapshot) => void;
    ipc.fetchWorkingCopy.mockImplementationOnce(() => new Promise((resolve) => { reload = resolve; }));
    fireEvent(window, new Event("focus"));
    await waitFor(() => expect(ipc.fetchWorkingCopy).toHaveBeenCalledTimes(2));
    ipc.generateCommitMessage.mockResolvedValue({ subject: "outdated message", body: "" });
    await act(async () => fireEvent.click(generate));
    const summary = screen.getByPlaceholderText("Summary (required)");
    expect(summary).toHaveValue("");
    expect(summary).toBeDisabled();
    await act(async () => reload({ ...snapshot, changes: [...snapshot.changes, { ...snapshot.changes[0], id: "late-file", path: { display: "late-file", token: "late-file" } }] }));
    expect(ipc.generateCommitMessage.mock.calls[0][2].aborted).toBe(true);
    expect(summary).toHaveValue("");
    expect(summary).toBeEnabled();
    expect(screen.getByText("late-file")).toBeInTheDocument();
  });

  it("can cancel while a completed generation waits for snapshot validation", async () => {
    render(<ChangesWorkbench />);
    const generate = await screen.findByRole("button", { name: "Generate commit message" });
    let reload!: (value: WorkingCopySnapshot) => void;
    ipc.fetchWorkingCopy.mockImplementationOnce(() => new Promise((resolve) => { reload = resolve; }));
    fireEvent(window, new Event("focus"));
    await waitFor(() => expect(ipc.fetchWorkingCopy).toHaveBeenCalledTimes(2));
    ipc.generateCommitMessage.mockResolvedValue({ subject: "cancelled message", body: "" });
    await act(async () => fireEvent.click(generate));
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Cancel commit message generation" })));
    expect(screen.getByPlaceholderText("Summary (required)")).toBeEnabled();
    expect(screen.getByPlaceholderText("Summary (required)")).toHaveValue("");
    await act(async () => reload(snapshot));
    expect(screen.getByPlaceholderText("Summary (required)")).toHaveValue("");
  });

  it.each([false, true])("discards generation on refresh failure (provider finished first: %s)", async (providerFinishedFirst) => {
    render(<ChangesWorkbench />);
    const generate = await screen.findByRole("button", { name: "Generate commit message" });
    let rejectReload!: (error: Error) => void;
    let complete!: (message: GeneratedCommitMessage) => void;
    ipc.fetchWorkingCopy.mockImplementationOnce(() => new Promise((_, reject) => { rejectReload = reject; }));
    ipc.generateCommitMessage.mockImplementation(() => new Promise((resolve) => { complete = resolve; }));
    fireEvent(window, new Event("focus"));
    await waitFor(() => expect(ipc.fetchWorkingCopy).toHaveBeenCalledTimes(2));
    fireEvent.click(generate);
    const message = { subject: "unverified message", body: "" };
    if (providerFinishedFirst) await act(async () => complete(message));
    await act(async () => rejectReload(new Error("Could not refresh working copy")));
    expect(ipc.generateCommitMessage.mock.calls[0][2].aborted).toBe(true);
    if (!providerFinishedFirst) await act(async () => complete(message));
    expect(screen.getByPlaceholderText("Summary (required)")).toHaveValue("");
    expect(screen.getByPlaceholderText("Summary (required)")).toBeEnabled();
    expect(screen.getByText("Could not refresh working copy")).toBeInTheDocument();
  });
});
