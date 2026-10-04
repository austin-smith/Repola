import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { HistoryWorkbench } from "./HistoryWorkbench";
import type { CommitChangedFile, CommitSummary } from "../ipc/types";

const ipc = vi.hoisted(() => ({ loadBranches: vi.fn(), loadHistory: vi.fn(), loadCommitFiles: vi.fn() }));
vi.mock("../ipc/worktrees", () => ipc);
vi.mock("./context", () => ({ useWorkingCopy: () => ({
  machineId: "local", machineKind: "local", machineOs: null,
  repository: { path: "/repo" }, worktree: { id: "main", path: "/repo", branch: "main" },
  refreshWorkspace: async () => undefined, showChanges: () => undefined,
}) }));
vi.mock("./lazy", () => ({
  CommitFileDiffView: ({ file }: { file: CommitChangedFile }) => <p>Diff of {file.path.display}</p>,
}));

const commit = (oid: string, subject: string): CommitSummary => ({
  oid, parents: ["parent"], authorName: "Repola Test", authorEmail: "repola@example.invalid",
  authoredAt: "2026-10-01T00:00:00Z", committedAt: "2026-10-01T00:00:00Z", signature: "unsigned", subject, body: "",
});
const changed = (path: string, kind: CommitChangedFile["kind"], status: string): CommitChangedFile => ({
  id: path, path: { display: path, token: path }, previousPath: null, kind, status,
});
const filesByCommit: Record<string, CommitChangedFile[]> = {
  first: [changed("src/app.ts", "modified", "M"), changed("new.txt", "added", "A"), changed("old.txt", "deleted", "D")],
  second: [changed("other.ts", "modified", "M"), changed("another.txt", "added", "A")],
};

// Every IPC mock resolves immediately, so flushing the render settles the view
// without polling against the clock.
async function renderHistory() {
  await act(async () => { render(<HistoryWorkbench />); });
}

describe("commit file type filter", () => {
  afterEach(cleanup);
  beforeEach(() => {
    vi.clearAllMocks();
    ipc.loadBranches.mockResolvedValue([]);
    ipc.loadHistory.mockResolvedValue({ commits: [commit("first", "First commit"), commit("second", "Second commit")], nextCursor: null });
    ipc.loadCommitFiles.mockImplementation(async (_machine, _repository, _worktree, oid: string) => filesByCommit[oid]);
  });

  it("lists only the pressed types and diffs a listed file", async () => {
    await renderHistory();
    expect(screen.getByText("Diff of src/app.ts")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Added (1)" }));
    expect(screen.getByText("Diff of new.txt")).toBeInTheDocument();
    expect(screen.queryByTitle("src/app.ts")).not.toBeInTheDocument();
    expect(screen.queryByTitle("old.txt")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Added (1)" }));
    expect(screen.getByTitle("src/app.ts")).toBeInTheDocument();
    expect(screen.getByTitle("old.txt")).toBeInTheDocument();
  });

  it("keeps the filter while browsing commits and ignores types a commit lacks", async () => {
    await renderHistory();
    expect(screen.getByText("Diff of src/app.ts")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Deleted (1)" }));
    fireEvent.click(screen.getByRole("button", { name: "Added (1)" }));
    expect(screen.getByTitle("new.txt")).toBeInTheDocument();
    expect(screen.getByTitle("old.txt")).toBeInTheDocument();

    await act(async () => fireEvent.click(screen.getByText("Second commit")));
    expect(screen.getByText("Diff of another.txt")).toBeInTheDocument();
    expect(screen.queryByTitle("other.ts")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Deleted/ })).not.toBeInTheDocument();
  });
});
