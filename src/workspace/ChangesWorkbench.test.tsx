import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ChangesWorkbench } from "./ChangesWorkbench";
import type { GeneratedCommitMessage, WorkingCopySnapshot } from "../ipc/types";

const ipc = vi.hoisted(() => ({
  fetchWorkingCopy: vi.fn(), generateCommitMessage: vi.fn(), synchronizeWorkingCopy: vi.fn(),
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
  DiscardDialog: ({ change, onBusyChange }: { change: { path: { display: string } } | null; onBusyChange: (busy: boolean) => void }) => (
    <div role="dialog" aria-label="Discard review">
      {change ? change.path.display : "every change"}
      <button type="button" onClick={() => onBusyChange(true)}>Start discard</button>
      <button type="button" onClick={() => onBusyChange(false)}>End discard</button>
    </div>
  ),
  DiscardedChangesDialog: ({ initialPointId }: { initialPointId: string | null }) => <div role="dialog" aria-label="Discarded changes">{initialPointId ?? "no point"}</div>,
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

  it("keeps secondary actions in the menu and confirms before discarding", async () => {
    await act(async () => { render(<ChangesWorkbench />); });
    expect(screen.queryByRole("button", { name: "Stashes" })).not.toBeInTheDocument();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "More change actions" })));
    expect(screen.getByRole("menuitem", { name: "Stashes" })).toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: "Force push…" })).not.toBeInTheDocument();
    await act(async () => fireEvent.click(screen.getByRole("menuitem", { name: "Discard all changes…" })));
    expect(screen.getByRole("dialog", { name: "Discard review" })).toHaveTextContent("every change");
  });

  it("holds refreshes back while a discard runs and reloads once it ends", async () => {
    let notify!: (event: { machineId: string }) => void;
    ipc.onWorktreeChanged.mockImplementation(async (listener) => { notify = listener; return () => undefined; });
    await act(async () => { render(<ChangesWorkbench />); });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "More change actions" })));
    await act(async () => fireEvent.click(screen.getByRole("menuitem", { name: "Discard all changes…" })));
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Start discard" })));
    const loads = ipc.fetchWorkingCopy.mock.calls.length;
    await act(async () => notify({ machineId: "local" }));
    expect(ipc.fetchWorkingCopy).toHaveBeenCalledTimes(loads);
    // Whether the discard succeeded or failed, the working copy loads again.
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "End discard" })));
    expect(ipc.fetchWorkingCopy).toHaveBeenCalledTimes(loads + 1);
  });

  it("opens the recovery point a discard's toast asks for, but not another working copy's", async () => {
    await act(async () => { render(<ChangesWorkbench />); });
    const show = (worktreePath: string, pointId: string) => window.dispatchEvent(
      new CustomEvent("repola:show-recovery-point", { detail: { machineId: "local", worktreePath, pointId } }),
    );
    await act(async () => show("/elsewhere", "refs/repola/discarded/other"));
    expect(screen.queryByRole("dialog", { name: "Discarded changes" })).not.toBeInTheDocument();
    await act(async () => show("/repo", "refs/repola/discarded/mine"));
    expect(screen.getByRole("dialog", { name: "Discarded changes" })).toHaveTextContent("refs/repola/discarded/mine");
  });

  it("opens the discarded changes from the menu", async () => {
    await act(async () => { render(<ChangesWorkbench />); });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "More change actions" })));
    await act(async () => fireEvent.click(screen.getByRole("menuitem", { name: "Discarded changes…" })));
    expect(screen.getByRole("dialog", { name: "Discarded changes" })).toHaveTextContent("no point");
  });

  it("offers force push for diverged branches through its existing confirmation", async () => {
    ipc.fetchWorkingCopy.mockResolvedValue({ ...snapshot, remote: "origin", upstream: "origin/main", upstreamHead: "remote-head", ahead: 1, behind: 1 });
    await act(async () => { render(<ChangesWorkbench />); });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "More change actions" })));
    await act(async () => fireEvent.click(screen.getByRole("menuitem", { name: "Force push…" })));
    expect(screen.getByRole("dialog", { name: "Force-push main?" })).toBeInTheDocument();
    expect(ipc.synchronizeWorkingCopy).not.toHaveBeenCalled();
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

  it("cancels generation when a pull replaces the snapshot without a watcher refresh", async () => {
    // The reload once the pull ends never answers, so only the pull's own
    // snapshot can cancel the generation.
    ipc.fetchWorkingCopy
      .mockResolvedValueOnce({ ...snapshot, remote: "origin", upstream: "origin/main", behind: 1 })
      .mockImplementation(() => new Promise(() => undefined));
    let complete!: (message: GeneratedCommitMessage) => void;
    ipc.generateCommitMessage.mockImplementation(() => new Promise((resolve) => { complete = resolve; }));
    ipc.synchronizeWorkingCopy.mockResolvedValue({ snapshot: { ...snapshot, head: "new-head" }, output: "" });
    render(<ChangesWorkbench />);
    const generate = await screen.findByRole("button", { name: "Generate commit message" });
    const summary = screen.getByPlaceholderText("Summary (required)");
    fireEvent.change(summary, { target: { value: "existing message" } });
    fireEvent.click(generate);
    const signal = ipc.generateCommitMessage.mock.calls[0][2] as AbortSignal;
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Pull 1" })));
    expect(ipc.synchronizeWorkingCopy).toHaveBeenCalled();
    expect(signal.aborted).toBe(true);
    await act(async () => complete({ subject: "outdated message", body: "" }));
    expect(summary).toHaveValue("existing message");
    expect(summary).toBeEnabled();
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

describe("change type filter", () => {
  const change = snapshot.changes[0];
  const mixed: WorkingCopySnapshot = {
    ...snapshot,
    changes: [
      { ...change, id: "src/app.ts", path: { display: "src/app.ts", token: "src/app.ts" } },
      { ...change, id: "new.txt", path: { display: "new.txt", token: "new.txt" }, kind: "untracked", indexStatus: "?", worktreeStatus: "?", untracked: true },
      { ...change, id: "old.txt", path: { display: "old.txt", token: "old.txt" }, kind: "deleted", worktreeStatus: "D" },
    ],
  };
  const listedIds = (container: HTMLElement) => (
    [...container.querySelectorAll("[data-change-id]")].map((row) => row.getAttribute("data-change-id"))
  );
  // Every IPC mock resolves immediately, so flushing the render settles the
  // workbench without polling against the clock.
  const renderWorkbench = async () => {
    let view!: ReturnType<typeof render>;
    await act(async () => { view = render(<ChangesWorkbench />); });
    return view;
  };

  const openTypeMenu = async () => {
    await act(async () => fireEvent.click(screen.getByRole("button", { name: /Filter by change type/ })));
  };
  const closeTypeMenu = async () => {
    await act(async () => fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" }));
  };
  const chooseType = async (name: string) => {
    await openTypeMenu();
    await act(async () => fireEvent.click(screen.getByRole("menuitemcheckbox", { name })));
    await closeTypeMenu();
  };

  afterEach(cleanup);
  beforeEach(() => {
    vi.clearAllMocks();
    ipc.fetchWorkingCopy.mockResolvedValue(mixed);
    ipc.watchWorktree.mockResolvedValue(undefined);
    ipc.unwatchWorktree.mockResolvedValue(undefined);
    ipc.onWorktreeChanged.mockResolvedValue(() => undefined);
  });

  it("offers no type filter when every change has the same type", async () => {
    ipc.fetchWorkingCopy.mockResolvedValue(snapshot);
    await renderWorkbench();
    expect(screen.getByText("file", { selector: "strong" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Filter by change type/ })).not.toBeInTheDocument();
  });

  it("lists only the pressed types, previews a listed file, and keeps hidden files in the commit", async () => {
    const { container } = await renderWorkbench();
    expect(screen.queryByRole("menuitemcheckbox")).not.toBeInTheDocument();
    await openTypeMenu();
    expect(screen.getByRole("menuitemcheckbox", { name: "Modified (1)" })).toBeInTheDocument();
    await closeTypeMenu();
    expect(screen.getByText("src/app.ts", { selector: "strong" })).toBeInTheDocument();

    await chooseType("Added (1)");
    expect(listedIds(container)).toEqual(["new.txt"]);
    expect(screen.getByText("new.txt", { selector: "strong" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Commit 3 files to main" })).toBeInTheDocument();

    await chooseType("Deleted (1)");
    expect(listedIds(container)).toEqual(["new.txt", "old.txt"]);

    await openTypeMenu();
    expect(screen.getByRole("menuitemcheckbox", { name: "Added (1)" })).toBeChecked();
    expect(screen.getByRole("menuitemcheckbox", { name: "Deleted (1)" })).toBeChecked();
    await act(async () => fireEvent.click(screen.getByRole("menuitemcheckbox", { name: "All change types" })));
    expect(screen.getByRole("menuitemcheckbox", { name: "All change types" })).toBeChecked();
    await closeTypeMenu();
    expect(listedIds(container)).toEqual(["src/app.ts", "new.txt", "old.txt"]);
  });

  it("never acts on selected files the filter hides", async () => {
    const { container } = await renderWorkbench();
    const list = container.querySelector<HTMLElement>("[aria-keyshortcuts]");
    if (!list) throw new Error("changes list missing");
    fireEvent.keyDown(list, { key: "a", metaKey: true });
    await chooseType("Added (1)");
    fireEvent.keyDown(list, { key: " " });
    expect(listedIds(container)).toEqual(["new.txt"]);
    expect(screen.getByRole("button", { name: "Commit 2 files to main" })).toBeInTheDocument();
    await chooseType("Added (1)");
    expect(screen.getByRole("checkbox", { name: "Exclude src/app.ts from commit" })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: "Exclude old.txt from commit" })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: "Include new.txt in commit" })).not.toBeChecked();
  });

  it("stops filtering by a type once a refresh removes its last file", async () => {
    let notify!: (event: { machineId: string }) => void;
    ipc.onWorktreeChanged.mockImplementation(async (listener) => { notify = listener; return () => undefined; });
    const { container } = await renderWorkbench();
    await chooseType("Deleted (1)");
    expect(listedIds(container)).toEqual(["old.txt"]);

    ipc.fetchWorkingCopy.mockResolvedValue({ ...mixed, changes: mixed.changes.filter((entry) => entry.kind !== "deleted") });
    await act(async () => notify({ machineId: "local" }));
    expect(listedIds(container)).toEqual(["src/app.ts", "new.txt"]);
    await openTypeMenu();
    expect(screen.queryByRole("menuitemcheckbox", { name: /^Deleted/ })).not.toBeInTheDocument();
  });
});
