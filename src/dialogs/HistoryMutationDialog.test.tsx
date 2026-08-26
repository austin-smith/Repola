import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import HistoryMutationDialog, { type HistoryTarget } from "./HistoryMutationDialog";
import type { FileChange, HistoryMutationResult, RepositorySummary, WorkingCopySnapshot, WorktreeRecord } from "../ipc/types";

const ipc = vi.hoisted(() => ({
  fetchWorkingCopy: vi.fn(),
  mutateHistory: vi.fn(),
}));

vi.mock("../ipc/worktrees", () => ipc);

const repository: RepositorySummary = {
  id: "repo",
  name: "repola",
  path: "/tmp/repola",
  remoteUrl: null,
  provider: "unknown" as RepositorySummary["provider"],
  worktreeCount: 1,
  attentionCount: 0,
  conflictedCount: 0,
  allocatedBytes: 0,
  allocationIncomplete: false,
};

const worktree = {
  id: "wt",
  repositoryName: "repola",
  repositoryPath: "/tmp/repola",
  path: "/tmp/repola-feature",
  branch: "feature",
  head: "abc123",
} as WorktreeRecord;

const change: FileChange = {
  id: "src/a.ts",
  path: { display: "src/a.ts", token: "src/a.ts" },
  previousPath: null,
  kind: "modified",
  indexStatus: ".",
  worktreeStatus: "M",
  staged: false,
  unstaged: true,
  conflicted: false,
  untracked: false,
  ignored: false,
  submodule: false,
  headMode: null,
  indexMode: null,
  worktreeMode: null,
  modeChange: null,
};

function snapshot(overrides: Partial<WorkingCopySnapshot> = {}): WorkingCopySnapshot {
  return {
    repositoryPath: repository.path,
    worktreePath: worktree.path,
    head: "abc123def456",
    branch: "feature",
    upstream: null,
    upstreamHead: null,
    remote: null,
    ahead: 0,
    behind: 0,
    changes: [],
    operation: null,
    ...overrides,
  };
}

const targets: HistoryTarget[] = [{ id: "t1", oid: "0123456789abcdef", label: "main" }];

function result(overrides: Partial<HistoryMutationResult> = {}): HistoryMutationResult {
  return { snapshot: snapshot(), succeeded: true, output: "", conflicted: false, ...overrides };
}

function renderDialog(kind: "resetHard" | "merge" | "rebase", props: Partial<Parameters<typeof HistoryMutationDialog>[0]> = {}) {
  const onClose = vi.fn();
  const onCompleted = vi.fn();
  render(
    <HistoryMutationDialog
      machineId="local"
      repository={repository}
      worktree={worktree}
      kinds={[kind]}
      targets={targets}
      initialKind={kind}
      title="History action"
      onClose={onClose}
      onCompleted={onCompleted}
      {...props}
    />,
  );
  return { onClose, onCompleted };
}

describe("HistoryMutationDialog", () => {
  afterEach(cleanup);

  beforeEach(() => {
    ipc.fetchWorkingCopy.mockReset();
    ipc.mutateHistory.mockReset();
    ipc.fetchWorkingCopy.mockResolvedValue(snapshot());
    ipc.mutateHistory.mockResolvedValue(result());
  });

  it("keeps a hard reset disabled until the exact `RESET <branch>` phrase is typed", async () => {
    renderDialog("resetHard");
    const confirm = screen.getByRole("button", { name: "Reset permanently" });
    const input = await screen.findByLabelText("Type RESET feature to confirm");
    expect(confirm).toBeDisabled();

    fireEvent.change(input, { target: { value: "reset feature" } });
    expect(confirm).toBeDisabled();
    fireEvent.change(input, { target: { value: "RESET main" } });
    expect(confirm).toBeDisabled();
    fireEvent.change(input, { target: { value: "RESET feature" } });
    expect(confirm).toBeEnabled();
    expect(ipc.mutateHistory).not.toHaveBeenCalled();
  });

  it("blocks clean-tree actions while the worktree is dirty, but not resets", async () => {
    ipc.fetchWorkingCopy.mockResolvedValue(snapshot({ changes: [change] }));
    renderDialog("merge");
    expect(await screen.findByText("Clean working copy required")).toBeInTheDocument();
    expect(screen.getByText("1 changed file")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Merge" })).toBeDisabled();
    cleanup();

    renderDialog("resetHard");
    const input = await screen.findByLabelText("Type RESET feature to confirm");
    expect(screen.queryByText("Clean working copy required")).not.toBeInTheDocument();
    fireEvent.change(input, { target: { value: "RESET feature" } });
    expect(screen.getByRole("button", { name: "Reset permanently" })).toBeEnabled();
  });

  it("ignores ignored files when judging cleanliness", async () => {
    ipc.fetchWorkingCopy.mockResolvedValue(snapshot({ changes: [{ ...change, ignored: true }] }));
    renderDialog("merge");
    expect(await screen.findByText("Clean")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Merge" })).toBeEnabled();
  });

  it("refuses to run while another Git operation is in progress", async () => {
    ipc.fetchWorkingCopy.mockResolvedValue(snapshot({ operation: "rebase" }));
    renderDialog("merge");
    expect(await screen.findByText("Finish the current Git operation")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Merge" })).toBeDisabled();
  });

  it("calls mutateHistory with the reviewed snapshot, kind, and target oid, then reports and closes", async () => {
    const reviewed = snapshot();
    ipc.fetchWorkingCopy.mockResolvedValue(reviewed);
    const { onClose, onCompleted } = renderDialog("resetHard");
    const input = await screen.findByLabelText("Type RESET feature to confirm");
    fireEvent.change(input, { target: { value: "RESET feature" } });
    fireEvent.click(screen.getByRole("button", { name: "Reset permanently" }));

    await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
    expect(ipc.mutateHistory).toHaveBeenCalledExactlyOnceWith("local", reviewed, "resetHard", "0123456789abcdef");
    expect(onCompleted).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ succeeded: true }), "resetHard", targets[0]);
  });

  it("renders IPC failures as an actionable error and keeps the dialog open", async () => {
    ipc.mutateHistory.mockRejectedValue(new Error("fatal: not possible to fast-forward, aborting."));
    const { onClose, onCompleted } = renderDialog("merge");
    const confirm = await screen.findByRole("button", { name: "Merge" });
    await waitFor(() => expect(confirm).toBeEnabled());
    fireEvent.click(confirm);

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("fatal: not possible to fast-forward, aborting.");
    expect(onCompleted).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Merge" })).toBeEnabled();
  });

  it("surfaces a working-copy preflight failure and never enables the action", async () => {
    ipc.fetchWorkingCopy.mockRejectedValue(new Error("fatal: not a git repository"));
    renderDialog("rebase");
    expect(await screen.findByRole("alert")).toHaveTextContent("fatal: not a git repository");
    expect(screen.getByRole("button", { name: "Rebase" })).toBeDisabled();
  });

  it("hands a conflicted result to onCompleted so the caller can route to resolution", async () => {
    const conflicted = result({ succeeded: false, conflicted: true, snapshot: snapshot({ operation: "rebase" }) });
    ipc.mutateHistory.mockResolvedValue(conflicted);
    const onNeedsResolution = vi.fn();
    const { onClose } = renderDialog("rebase", {
      onCompleted: (outcome) => { if (outcome.conflicted || outcome.snapshot.operation) onNeedsResolution(); },
    });
    const confirm = await screen.findByRole("button", { name: "Rebase" });
    await waitFor(() => expect(confirm).toBeEnabled());
    fireEvent.click(confirm);

    await waitFor(() => expect(onNeedsResolution).toHaveBeenCalledOnce());
    expect(ipc.mutateHistory).toHaveBeenCalledExactlyOnceWith("local", expect.anything(), "rebase", "0123456789abcdef");
    expect(onClose).toHaveBeenCalledOnce();
  });
});
