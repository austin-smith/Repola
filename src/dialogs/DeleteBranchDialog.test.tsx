import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { toast } from "@/components/ui/toast";
import DeleteBranchDialog from "./DeleteBranchDialog";
import type {
  BranchDeletionPlan,
  BranchDeletionRequest,
  BranchDeletionResult,
  BranchInfo,
  BranchPullRequests,
} from "../ipc/types";

const ipc = vi.hoisted(() => ({
  prepareBranchDeletionReview: vi.fn(),
  executeBranchDeletion: vi.fn(),
  openExternalUrl: vi.fn(),
}));

vi.mock("../ipc/worktrees", () => ipc);

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
  branch({ name: "old-work", fullName: "refs/heads/old-work", upstream: null }),
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
      pushUrl: "https://example.com/repola.git",
      pullRequests: null,
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
      ...(request.deleteLocal ? ["git -C /repos/repola update-ref --no-deref -d refs/heads/feature aaaa"] : []),
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
      pullRequests: null,
      pushDestination: request.deleteRemote ? "f".repeat(64) : null,
      localReachability: null,
      remoteReachability: null,
      commands: [],
      warnings: [],
      confirmation: "confirm",
    },
    ...overrides,
  };
}

const result: BranchDeletionResult = {
  message: "Deleted local branch feature.",
  local: { target: "feature", deletedOid: "a".repeat(40), succeeded: true, unconfirmed: false, output: "", warning: null, finishCommands: [], recoveryCommands: ["git -C /repos/repola branch -- feature aaaa"] },
  remote: null,
  auditPath: null,
  auditWarning: null,
};

function renderDialog(onDeleted = vi.fn(), onClose = vi.fn(), initialBranchRef?: string) {
  render(
    <DeleteBranchDialog
      machineId="local"
      repositoryPath="/repos/repola"
      worktreePath="/repos/repola"
      branches={branches}
      initialBranchRef={initialBranchRef}
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
    vi.restoreAllMocks();
  });

  it("reviews only the local branch until the remote is explicitly chosen", async () => {
    const { onDeleted, onClose } = renderDialog();

    const remote = screen.getByRole("checkbox", { name: "Remote branch origin/feature" });
    expect(remote).not.toBeChecked();
    expect(screen.getByRole("checkbox", { name: "Local branch feature" })).toBeChecked();
    await screen.findByText(/update-ref --no-deref -d refs\/heads\/feature/);
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

  it("opens on the branch it was asked to review", async () => {
    renderDialog(vi.fn(), vi.fn(), "refs/heads/old-work");

    await waitFor(() => expect(ipc.prepareBranchDeletionReview).toHaveBeenCalledWith(
      "local",
      expect.objectContaining({ branchRef: "refs/heads/old-work", deleteLocal: true, deleteRemote: false }),
      expect.any(AbortSignal),
    ));
    expect(screen.getByRole("checkbox", { name: "Local branch old-work" })).toBeChecked();
  });

  it("requires the exact branch name before deleting both copies", async () => {
    renderDialog();
    await screen.findByText(/update-ref --no-deref -d refs\/heads\/feature/);
    fireEvent.click(screen.getByRole("checkbox", { name: "Remote branch origin/feature" }));

    await screen.findByText(/push --porcelain/);
    expect(screen.getByText(/Repola does not fetch during review/)).toBeInTheDocument();
    expect(screen.getByText("https://example.com/repola.git")).toBeInTheDocument();
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
    await screen.findByText(/update-ref --no-deref -d refs\/heads\/feature/);
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

  it("reports a completed deletion however long the caller takes to follow up", async () => {
    const notify = vi.spyOn(toast, "add");
    const onDeleted = vi.fn(() => new Promise<void>(() => undefined));
    const { onClose } = renderDialog(onDeleted);
    await screen.findByText(/update-ref --no-deref -d refs\/heads\/feature/);

    fireEvent.click(screen.getByRole("button", { name: "Delete Branch" }));

    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(notify).toHaveBeenCalledWith(expect.objectContaining({ type: "success", title: "Deleted local branch feature." }));
    expect(onDeleted).toHaveBeenCalledWith(result);
    expect(screen.queryByText("The branch was not deleted")).not.toBeInTheDocument();
  });

  it("shows a refused execution and reviews the branch again", async () => {
    ipc.executeBranchDeletion.mockRejectedValue({ message: "The branch changed after this deletion was reviewed.", outcomeKnown: true });
    const { onDeleted } = renderDialog();
    await screen.findByText(/update-ref --no-deref -d refs\/heads\/feature/);
    expect(ipc.prepareBranchDeletionReview).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole("button", { name: "Delete Branch" }));

    expect(await screen.findByText("The branch was not deleted")).toBeInTheDocument();
    await waitFor(() => expect(ipc.prepareBranchDeletionReview).toHaveBeenCalledTimes(2));
    expect(onDeleted).not.toHaveBeenCalled();
  });

  it("does not claim a deletion it lost track of was refused", async () => {
    ipc.executeBranchDeletion.mockRejectedValue({
      message: "The operation on \"build box\" exceeded its 120-second deadline. The deletion may have completed anyway. Review the branch before trying again.",
      outcomeKnown: false,
    });
    const { onDeleted } = renderDialog();
    await screen.findByText(/update-ref --no-deref -d refs\/heads\/feature/);

    fireEvent.click(screen.getByRole("button", { name: "Delete Branch" }));

    expect(await screen.findByText("Repola could not confirm whether the branch was deleted")).toBeInTheDocument();
    expect(screen.queryByText("The branch was not deleted")).not.toBeInTheDocument();
    await waitFor(() => expect(ipc.prepareBranchDeletionReview).toHaveBeenCalledTimes(2));
    expect(onDeleted).not.toHaveBeenCalled();
  });

  /** Reviews the remote branch as the provider answered about pull requests. */
  function reviewRemoteWith(check: BranchPullRequests) {
    ipc.prepareBranchDeletionReview.mockImplementation((_machine: string, request: BranchDeletionRequest) => {
      const plan = planFor(request);
      return Promise.resolve({
        ...plan,
        remote: plan.remote && { ...plan.remote, pullRequests: request.deleteRemote ? check : null },
        confirmation: request.deleteRemote ? "typeBranchName" : plan.confirmation,
      });
    });
  }

  it("lists the open pull requests a remote deletion affects and why the name must be typed", async () => {
    reviewRemoteWith({
      status: "checked",
      provider: "gitHub",
      pulls: [
        { repository: "octo/app", number: 42, title: "Ship it", url: "https://github.com/octo/app/pull/42", relation: "source", from: "octo/app:feature", into: "octo/app:main" },
        { repository: "octo/app", number: 43, title: "Stacked", url: null, relation: "target", from: "octo/app:next", into: "octo/app:feature" },
      ],
      moreThanListed: true,
    });
    renderDialog();
    await screen.findByText(/update-ref --no-deref -d refs\/heads\/feature/);
    expect(screen.queryByText(/open pull request/)).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("checkbox", { name: "Remote branch origin/feature" }));

    expect(await screen.findByText("2 open pull requests use origin/feature")).toBeInTheDocument();
    expect(screen.getByText(/closes them on GitHub/)).toBeInTheDocument();
    expect(screen.getByText("from octo/app:feature into octo/app:main")).toBeInTheDocument();
    expect(screen.getByText("from octo/app:next into octo/app:feature")).toBeInTheDocument();
    expect(screen.getByText(/2 open pull requests use the remote branch\./)).toBeInTheDocument();
    expect(screen.getByText("GitHub has more open pull requests that use this branch than Repola lists here.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "#42" }));
    expect(ipc.openExternalUrl).toHaveBeenCalledWith("https://github.com/octo/app/pull/42");
  });

  it("says when it could not check for open pull requests", async () => {
    reviewRemoteWith({ status: "unavailable", reason: "The gh CLI is not installed on this machine." });
    renderDialog();
    await screen.findByText(/update-ref --no-deref -d refs\/heads\/feature/);
    fireEvent.click(screen.getByRole("checkbox", { name: "Remote branch origin/feature" }));

    expect(await screen.findByText("Could not check for open pull requests")).toBeInTheDocument();
    expect(screen.getByText("The gh CLI is not installed on this machine.")).toBeInTheDocument();
  });
});
