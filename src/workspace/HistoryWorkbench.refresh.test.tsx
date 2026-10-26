import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { HistoryWorkbench } from "./HistoryWorkbench";
import type { CommitSummary, HistoryPage } from "../ipc/types";

const ipc = vi.hoisted(() => ({ loadBranches: vi.fn(), loadHistory: vi.fn(), loadCommitFiles: vi.fn() }));
vi.mock("../ipc/worktrees", () => ipc);
const context = vi.hoisted(() => ({
  machineId: "local", repository: { path: "/repo" }, worktree: { id: "main", path: "/repo" },
  gitRevision: 0, statusRevision: 0, refreshWorkspace: vi.fn(), showChanges: vi.fn(),
}));
vi.mock("./context", () => ({ useWorkingCopy: () => context }));
vi.mock("./lazy", () => ({
  CommitFileDiffView: () => null,
  HistoryMutationDialog: ({ targets }: { targets: { oid: string }[] }) => <div data-testid="reviewed-history-target">{targets[0].oid}</div>,
}));
function commit(oid: string): CommitSummary {
  return { oid, parents: [], authorName: "User", authorEmail: "user@example.test", authoredAt: "2026-01-01", committedAt: "2026-01-01", signature: "unsigned", subject: `commit ${oid}`, body: "" };
}

describe("history refresh", () => {
  afterEach(cleanup);
  beforeEach(() => {
    vi.resetAllMocks();
    context.gitRevision = 0;
    context.statusRevision = 0;
    ipc.loadBranches.mockResolvedValue([]);
    ipc.loadCommitFiles.mockResolvedValue([]);
    ipc.loadHistory.mockResolvedValue({ commits: [commit("a"), commit("b")], nextCursor: null });
  });

  it("ignores file status changes and preserves selected commit on metadata refresh", async () => {
    const view = render(<HistoryWorkbench />);
    fireEvent.click(await screen.findByRole("button", { name: /commit b/ }));
    await waitFor(() => expect(ipc.loadCommitFiles).toHaveBeenLastCalledWith("local", "/repo", "/repo", "b", expect.any(AbortSignal)));
    context.statusRevision++;
    view.rerender(<HistoryWorkbench />);
    expect(ipc.loadHistory).toHaveBeenCalledTimes(1);
    ipc.loadHistory.mockResolvedValue({ commits: [commit("c"), commit("a"), commit("b")], nextCursor: null });
    context.gitRevision++;
    view.rerender(<HistoryWorkbench />);
    await screen.findByRole("button", { name: /commit c/ });
    expect(screen.getByRole("heading", { name: "commit b" })).toBeInTheDocument();
    expect(ipc.loadCommitFiles).toHaveBeenCalledTimes(2);
  });

  it("rejects an older pagination response after the graph changes", async () => {
    ipc.loadHistory.mockResolvedValueOnce({ commits: [commit("a")], nextCursor: "a" });
    const view = render(<HistoryWorkbench />);
    let complete!: (page: HistoryPage) => void;
    ipc.loadHistory.mockImplementationOnce(() => new Promise((resolve) => { complete = resolve; }));
    fireEvent.click(await screen.findByRole("button", { name: "Load older commits" }));
    ipc.loadHistory.mockResolvedValueOnce({ commits: [commit("new")], nextCursor: null });
    context.gitRevision++;
    view.rerender(<HistoryWorkbench />);
    await screen.findByRole("button", { name: /commit new/ });
    await act(async () => complete({ commits: [commit("obsolete")], nextCursor: null }));
    expect(screen.queryByRole("button", { name: /commit obsolete/ })).not.toBeInTheDocument();
  });

  it("retains the selected older commit when refreshing already loaded pages", async () => {
    ipc.loadHistory.mockResolvedValueOnce({ commits: [commit("a")], nextCursor: "a" });
    const view = render(<HistoryWorkbench />);
    ipc.loadHistory.mockResolvedValueOnce({ commits: [commit("b")], nextCursor: null });
    fireEvent.click(await screen.findByRole("button", { name: "Load older commits" }));
    fireEvent.click(await screen.findByRole("button", { name: /commit b/ }));
    ipc.loadHistory.mockResolvedValueOnce({ commits: [commit("new")], nextCursor: "new" });
    ipc.loadHistory.mockResolvedValueOnce({ commits: [commit("a"), commit("b")], nextCursor: null });
    context.gitRevision++;
    view.rerender(<HistoryWorkbench />);
    await screen.findByRole("button", { name: /commit new/ });
    expect(screen.getByRole("heading", { name: "commit b" })).toBeInTheDocument();
  });

  it("keeps the reviewed destructive target when a refresh changes history selection", async () => {
    const view = render(<HistoryWorkbench />);
    fireEvent.click(await screen.findByRole("button", { name: /commit b/ }));
    fireEvent.click(screen.getByRole("button", { name: "Commit actions" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Reset branch to commit…" }));
    expect(screen.getByTestId("reviewed-history-target")).toHaveTextContent("b");
    ipc.loadHistory.mockResolvedValue({ commits: [commit("replacement")], nextCursor: null });
    context.gitRevision++;
    view.rerender(<HistoryWorkbench />);
    await screen.findByRole("button", { name: /commit replacement/ });
    expect(screen.getByTestId("reviewed-history-target")).toHaveTextContent("b");
  });
});
