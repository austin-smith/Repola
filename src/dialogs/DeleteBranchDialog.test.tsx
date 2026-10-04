import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import DeleteBranchDialog from "./DeleteBranchDialog";
import type {
  BranchDeletionPlan,
  BranchDeletionRequest,
  BranchDeletionResult,
  BranchInfo,
  RepositorySummary,
  WorkingCopySnapshot,
  WorktreeRecord,
} from "../ipc/types";

const ipc = vi.hoisted(() => ({
  prepareBranchDeletionReview: vi.fn(),
  executeBranchDeletion: vi.fn(),
}));

vi.mock("../ipc/worktrees", () => ipc);

const repository: RepositorySummary = {
  id: "repo-1",
  name: "repola",
  path: "/repos/repola",
  remoteUrl: null,
  provider: "none",
  worktreeCount: 1,
  attentionCount: 0,
  conflictedCount: 0,
  allocatedBytes: 1024,
  allocationIncomplete: false,
};

const worktree: WorktreeRecord = {
  id: "/repos/repola",
  repositoryName: "repola",
  repositoryPath: "/repos/repola",
  path: "/repos/repola",
  branch: "main",
  head: "1".repeat(40),
  detached: false,
  isPrimary: true,
  exists: true,
  createdAtMs: null,
  headCommitAtMs: null,
  lastActivityAtMs: null,
  headSubject: null,
  unpushedCommitCount: null,
  sizeBytes: null,
  sizeIncomplete: false,
  origin: { kind: "unattributed", id: "primary", label: "Primary" },
  status: { available: true, total: 0, staged: 0, unstaged: 0, untracked: 0, conflicted: 0 },
  registration: { kind: "healthy", reason: null },
  integration: { kind: "headContained", target: "main", summary: "" },
  safety: { level: "review", label: "", reasons: [] },
};

function branch(overrides: Partial<BranchInfo>): BranchInfo {
  return {
    name: "feature",
    fullName: "refs/heads/feature",
    head: "a".repeat(40),
    remote: false,
    current: false,
    upstream: "origin/feature",
    ahead: 0,
    behind: 0,
    occupiedWorktreePath: null,
    ...overrides,
  };
}

const branches = [
  branch({ name: "main", fullName: "refs/heads/main", current: true, occupiedWorktreePath: "/repos/repola", upstream: "origin/main" }),
  branch({}),
  branch({ name: "origin/feature", fullName: "refs/remotes/origin/feature", remote: true, upstream: null }),
];

function planFor(request: BranchDeletionRequest, overrides: Partial<BranchDeletionPlan> = {}): BranchDeletionPlan {
  return {
    repositoryPath: request.repositoryPath,
    worktreePath: request.worktreePath,
    branchRef: request.branchRef,
    branchName: "feature",
    deleteLocal: request.deleteLocal,
    deleteRemote: request.deleteRemote,
    local: request.deleteLocal
      ? {
          name: "feature",
          tip: "a".repeat(40),
          mergeReference: "origin/feature",
          mergeReferenceKind: "upstream",
          mergeReferenceOid: "a".repeat(40),
          containedInMergeReference: true,
          defaultTarget: "origin/main",
          containedInDefaultTarget: false,
          occupiedWorktreePath: null,
          isDefaultBranch: false,
          upstream: "origin/feature",
          exclusiveCommitCount: 0,
          exclusiveCommitCountCapped: false,
        }
      : null,
    remote: {
      remote: "origin",
      remoteRef: "refs/heads/feature",
      trackingRef: "refs/remotes/origin/feature",
      displayName: "origin/feature",
      expectedOid: "a".repeat(40),
      trackingRefUpdatedAt: null,
      lastFetchedAt: null,
      isRemoteDefaultBranch: false,
      trackedBy: [],
      exclusiveCommitCount: request.deleteLocal ? 2 : 0,
      exclusiveCommitCountCapped: false,
    },
    remoteUnavailableReason: null,
    requiresForce: false,
    confirmation: request.deleteLocal && request.deleteRemote ? "typeBranchName" : "confirm",
    commands: [
      ...(request.deleteLocal ? ["git -C /repos/repola branch -d -- feature"] : []),
      ...(request.deleteRemote ? ["git -C /repos/repola push --porcelain '--force-with-lease=refs/heads/feature:aaaa' -- origin :refs/heads/feature"] : []),
    ],
    warnings: [],
    blockers: [],
    fingerprint: {
      localTip: request.deleteLocal ? "a".repeat(40) : null,
      mergeReferenceOid: null,
      requiresForce: false,
      remote: request.deleteRemote ? "origin" : null,
      remoteRef: request.deleteRemote ? "refs/heads/feature" : null,
      remoteOid: request.deleteRemote ? "a".repeat(40) : null,
      confirmation: "confirm",
    },
    ...overrides,
  };
}

const snapshot: WorkingCopySnapshot = {
  repositoryPath: "/repos/repola",
  worktreePath: "/repos/repola",
  head: "1".repeat(40),
  branch: "main",
  upstream: "origin/main",
  upstreamHead: "1".repeat(40),
  remote: "origin",
  ahead: 0,
  behind: 0,
  changes: [],
  operation: null,
};

const result: BranchDeletionResult = {
  message: "Deleted local branch feature.",
  local: { target: "feature", deletedOid: "a".repeat(40), succeeded: true, output: "", recoveryCommand: "git -C /repos/repola branch -- feature aaaa" },
  remote: null,
  branches: [],
  snapshot,
  auditPath: null,
  auditWarning: null,
};

function renderDialog(onDeleted = vi.fn(), onClose = vi.fn()) {
  render(
    <DeleteBranchDialog
      machineId="local"
      repository={repository}
      worktree={worktree}
      branches={branches}
      onClose={onClose}
      onDeleted={onDeleted}
    />,
  );
  return { onDeleted, onClose };
}

describe("DeleteBranchDialog", () => {
  beforeEach(() => {
    ipc.prepareBranchDeletionReview.mockImplementation((_machine: string, request: BranchDeletionRequest) => Promise.resolve(planFor(request)));
    ipc.executeBranchDeletion.mockResolvedValue(result);
  });
  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("reviews only the local branch until the remote is explicitly chosen", async () => {
    const { onDeleted, onClose } = renderDialog();

    const remote = screen.getByRole("checkbox", { name: "Remote branch origin/feature" });
    expect(remote).not.toBeChecked();
    expect(screen.getByRole("checkbox", { name: "Local branch feature" })).toBeChecked();
    await screen.findByText(/branch -d -- feature/);
    expect(ipc.prepareBranchDeletionReview).toHaveBeenLastCalledWith("local", {
      repositoryPath: "/repos/repola",
      worktreePath: "/repos/repola",
      branchRef: "refs/heads/feature",
      deleteLocal: true,
      deleteRemote: false,
    }, expect.any(AbortSignal));
    expect(screen.queryByText(/push --porcelain/)).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Delete Branch" }));
    await waitFor(() => expect(onDeleted).toHaveBeenCalledWith(result));
    expect(ipc.executeBranchDeletion).toHaveBeenCalledWith("local", expect.objectContaining({ deleteLocal: true, deleteRemote: false }), null);
    expect(onClose).toHaveBeenCalled();
  });

  it("requires the exact branch name before deleting both copies", async () => {
    renderDialog();
    await screen.findByText(/branch -d -- feature/);
    fireEvent.click(screen.getByRole("checkbox", { name: "Remote branch origin/feature" }));

    await screen.findByText(/push --porcelain/);
    expect(screen.getByText(/Repola does not fetch during review/)).toBeInTheDocument();
    const confirm = screen.getByRole("button", { name: "Delete Local and Remote" });
    expect(confirm).toBeDisabled();
    const input = screen.getByRole("textbox", { name: "Type feature to confirm" });
    fireEvent.change(input, { target: { value: "Feature" } });
    expect(confirm).toBeDisabled();
    fireEvent.change(input, { target: { value: "feature" } });
    expect(confirm).toBeEnabled();
    fireEvent.click(confirm);

    await waitFor(() => expect(ipc.executeBranchDeletion).toHaveBeenCalledWith(
      "local",
      expect.objectContaining({ deleteLocal: true, deleteRemote: true }),
      "feature",
    ));
  });

  it("never offers to run a blocked plan", async () => {
    ipc.prepareBranchDeletionReview.mockImplementation((_machine: string, request: BranchDeletionRequest) => Promise.resolve(
      planFor(request, { blockers: ["feature is checked out in the worktree at /worktrees/feature."] }),
    ));
    renderDialog();

    expect(await screen.findByText("Cannot delete")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Delete Branch" })).toBeDisabled();
  });

  it("explains why a branch checked out here can only be deleted on the remote", async () => {
    renderDialog();
    await screen.findByText(/branch -d -- feature/);
    fireEvent.click(screen.getByRole("combobox", { name: "Branch to delete" }));
    const option = await screen.findByRole("option", { name: /^main.*checked out/ });
    fireEvent.pointerDown(option, { button: 0 });
    fireEvent.pointerUp(option, { button: 0 });
    fireEvent.click(option, { button: 0 });

    const local = screen.getByRole("checkbox", { name: "Local branch main" });
    expect(local).not.toBeChecked();
    expect(local).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByText(/Checked out in this worktree/)).toBeInTheDocument();
    expect(screen.getByText("Choose the local branch, the remote branch, or both.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Delete Branch" })).toBeDisabled();
  });

  it("shows a refused execution and reviews the branch again", async () => {
    ipc.executeBranchDeletion.mockRejectedValue(new Error("The branch changed after this deletion was reviewed."));
    const { onDeleted } = renderDialog();
    await screen.findByText(/branch -d -- feature/);
    expect(ipc.prepareBranchDeletionReview).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole("button", { name: "Delete Branch" }));

    expect(await screen.findByText("The branch was not deleted")).toBeInTheDocument();
    await waitFor(() => expect(ipc.prepareBranchDeletionReview).toHaveBeenCalledTimes(2));
    expect(onDeleted).not.toHaveBeenCalled();
  });
});
