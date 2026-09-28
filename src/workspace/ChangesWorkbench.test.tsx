import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
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
    headMode: null, indexMode: null, worktreeMode: null, modeChange: null }],
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
});
