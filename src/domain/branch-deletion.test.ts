import { describe, expect, it } from "vitest";
import type { BranchDeletionPlan, BranchDeletionResult, BranchInfo, BranchPullRequests, LocalBranchDeletion, OpenPullRequest, RemoteBranchDeletion } from "../ipc/types";
import {
  canExecuteDeletion,
  confirmationReasons,
  confirmationSatisfied,
  deletionButtonLabel,
  deletionFailure,
  deletionNotice,
  describeObservedAt,
  initialDeletionBranch,
  initialScope,
  openPullRequests,
  pullRequestConsequence,
  pullRequestOverflow,
  scopeOptions,
} from "./branch-deletion";

function branch(overrides: Partial<BranchInfo>): BranchInfo {
  return {
    name: "feature",
    fullName: "refs/heads/feature",
    head: "a".repeat(40),
    remote: false,
    current: false,
    upstream: null,
    ahead: 0,
    behind: 0,
    occupiedWorktreePath: null,
    ...overrides,
  };
}

function plan(overrides: Partial<BranchDeletionPlan>): BranchDeletionPlan {
  return {
    repositoryPath: "/repos/repola",
    worktreePath: "/repos/repola",
    branchRef: "refs/heads/feature",
    branchName: "feature",
    deleteLocal: true,
    deleteRemote: false,
    local: null,
    remote: null,
    remoteUnavailableReason: null,
    requiresForce: false,
    confirmation: "confirm",
    commands: [],
    warnings: [],
    blockers: [],
    fingerprint: {
      localTip: "a".repeat(40),
      mergeReferenceOid: null,
      requiresForce: false,
      remote: null,
      remoteRef: null,
      remoteOid: null,
      pullRequests: null,
      morePullRequests: false,
      pushDestination: null,
      localReachability: null,
      remoteReachability: null,
      commands: [],
      warnings: [],
      confirmation: "confirm",
    },
    ...overrides,
  };
}

describe("branch deletion scope", () => {
  it("offers a free local branch and its upstream but preselects only the local branch", () => {
    const published = branch({ upstream: "origin/feature" });
    expect(scopeOptions(published, "/repos/repola")).toEqual({
      local: { available: true, reason: null },
      remote: { available: true, reason: null, label: "origin/feature" },
    });
    expect(initialScope(published, "/repos/repola")).toEqual({ deleteLocal: true, deleteRemote: false });
  });

  it("explains occupancy by this worktree or another one", () => {
    const here = scopeOptions(branch({ occupiedWorktreePath: "/repos/repola" }), "/repos/repola");
    expect(here.local.available).toBe(false);
    expect(here.local.reason).toMatch(/this worktree/);
    const elsewhere = scopeOptions(branch({ occupiedWorktreePath: "/worktrees/other" }), "/repos/repola");
    expect(elsewhere.local.reason).toContain("/worktrees/other");
    expect(initialScope(branch({ occupiedWorktreePath: "/worktrees/other" }), "/repos/repola"))
      .toEqual({ deleteLocal: false, deleteRemote: false });
  });

  it("only offers a remote deletion when there is a remote branch", () => {
    expect(scopeOptions(branch({}), "/repos/repola").remote).toEqual({
      available: false,
      reason: "feature has no upstream branch.",
      label: null,
    });
    const remote = branch({ name: "origin/feature", fullName: "refs/remotes/origin/feature", remote: true });
    expect(scopeOptions(remote, "/repos/repola").local.available).toBe(false);
    expect(initialScope(remote, "/repos/repola")).toEqual({ deleteLocal: false, deleteRemote: true });
  });

  it("opens on the first local branch that is free to delete", () => {
    const branches = [
      branch({ name: "main", fullName: "refs/heads/main", current: true, occupiedWorktreePath: "/repos/repola" }),
      branch({ name: "busy", fullName: "refs/heads/busy", occupiedWorktreePath: "/worktrees/busy" }),
      branch({ name: "free", fullName: "refs/heads/free" }),
    ];
    expect(initialDeletionBranch(branches, "/repos/repola")?.name).toBe("free");
    expect(initialDeletionBranch(branches.slice(0, 2), "/repos/repola")?.name).toBe("busy");
    expect(initialDeletionBranch([], "/repos/repola")).toBeNull();
  });
});

describe("branch deletion confirmation", () => {
  it("requires the exact branch name only when the plan asks for it", () => {
    expect(confirmationSatisfied(plan({}), "")).toBe(true);
    const forced = plan({ requiresForce: true, confirmation: "typeBranchName" });
    expect(confirmationSatisfied(forced, "")).toBe(false);
    expect(confirmationSatisfied(forced, "Feature")).toBe(false);
    expect(confirmationSatisfied(forced, "feature ")).toBe(false);
    expect(confirmationSatisfied(forced, "feature")).toBe(true);
  });

  it("never executes a blocked plan", () => {
    expect(canExecuteDeletion(plan({ blockers: ["checked out"] }), "feature")).toBe(false);
    expect(canExecuteDeletion(plan({}), "")).toBe(true);
  });

  it("labels the exact scope and force", () => {
    expect(deletionButtonLabel(plan({}))).toBe("Delete Branch");
    expect(deletionButtonLabel(plan({ requiresForce: true }))).toBe("Force Delete Branch");
    expect(deletionButtonLabel(plan({ deleteRemote: true }))).toBe("Delete Local and Remote");
    expect(deletionButtonLabel(plan({ deleteLocal: false, deleteRemote: true }))).toBe("Delete Remote Branch");
  });
});

describe("describeObservedAt", () => {
  const now = 1_800_000_000_000;
  it("describes how long ago a ref was observed", () => {
    expect(describeObservedAt(null, now)).toBe("unknown");
    expect(describeObservedAt(now / 1000 - 5, now)).toBe("just now");
    expect(describeObservedAt(now / 1000 - 60, now)).toBe("1 minute ago");
    expect(describeObservedAt(now / 1000 - 7_200, now)).toBe("2 hours ago");
    expect(describeObservedAt(now / 1000 - 3 * 86_400, now)).toBe("3 days ago");
    expect(describeObservedAt(now / 1000 + 30, now)).toBe("just now");
  });
});

describe("deletionNotice", () => {
  const step = (target: string, recoveryCommands: string[], succeeded = true, warning: string | null = null, finishCommands: string[] = []) => ({
    target,
    deletedOid: "a".repeat(40),
    succeeded,
    unconfirmed: false,
    output: succeeded ? "Deleted branch feature (was aaaa)." : "! [remote rejected] (stale info)",
    warning,
    finishCommands,
    recoveryCommands,
  });
  const result = (overrides: Partial<BranchDeletionResult>): BranchDeletionResult => ({
    message: "Deleted feature.",
    local: null,
    remote: null,
    auditPath: null,
    auditWarning: null,
    ...overrides,
  });

  it("lists every way to restore what was deleted", () => {
    expect(deletionNotice(result({
      local: step("feature", ["git branch -- feature aaaa", "git config --local --add branch.feature.remote origin"]),
      remote: step("origin/feature", ["git push -- origin aaaa:refs/heads/feature"]),
      auditWarning: "disk full",
    }))).toEqual({
      type: "success",
      title: "Deleted feature.",
      description: "To restore: git branch -- feature aaaa; git config --local --add branch.feature.remote origin; git push -- origin aaaa:refs/heads/feature Audit warning: disk full",
    });
  });

  it("keeps the local recovery when the remote step failed", () => {
    expect(deletionNotice(result({
      local: step("feature", ["git branch -- feature aaaa"]),
      remote: step("origin/feature", [], false),
      auditWarning: "disk full",
    }))).toEqual({
      type: "warning",
      title: "Deleted feature.",
      description: "! [remote rejected] (stale info) To restore: git branch -- feature aaaa Audit warning: disk full",
    });
  });

  it("warns that an interrupted step may have deleted the branch, and how to restore it", () => {
    const interrupted = {
      ...step("origin/feature", ["git push -- origin aaaa:refs/heads/feature"], false),
      unconfirmed: true,
      output: "git was cancelled. The push had started, so origin may have deleted refs/heads/feature.",
    };
    expect(deletionNotice(result({ message: "Repola could not confirm whether remote branch origin/feature was deleted.", remote: interrupted }))).toEqual({
      type: "warning",
      title: "Repola could not confirm whether remote branch origin/feature was deleted.",
      description: "git was cancelled. The push had started, so origin may have deleted refs/heads/feature. To restore: git push -- origin aaaa:refs/heads/feature",
    });
  });

  it("warns about anything a successful step left undone and how to finish it", () => {
    const leftover = "feature was deleted, but its configuration was not removed.";
    const finish = "git config --local --remove-section branch.feature";
    expect(deletionNotice(result({ local: step("feature", ["git branch -- feature aaaa"], true, leftover, [finish]) }))).toEqual({
      type: "warning",
      title: "Deleted feature.",
      description: `${leftover} To finish: ${finish} To restore: git branch -- feature aaaa`,
    });
  });
});

describe("deletionFailure", () => {
  it("keeps the engine's word on whether the deletion happened", () => {
    expect(deletionFailure({ message: "The branch deletion is blocked.", outcomeKnown: true })).toEqual({
      message: "The branch deletion is blocked.",
      outcomeKnown: true,
    });
    expect(deletionFailure({ message: "The SSH connection closed.", outcomeKnown: false }).outcomeKnown).toBe(false);
  });

  it("treats any other failure as one whose outcome is unknown", () => {
    expect(deletionFailure(new Error("IPC bridge unavailable"))).toEqual({ message: "IPC bridge unavailable", outcomeKnown: false });
    expect(deletionFailure("lost")).toEqual({ message: "lost", outcomeKnown: false });
  });
});

describe("open pull requests", () => {
  const remote = (pullRequests: BranchPullRequests | null): RemoteBranchDeletion => ({
    remote: "origin",
    remoteRef: "refs/heads/feature",
    trackingRef: "refs/remotes/origin/feature",
    displayName: "origin/feature",
    expectedOid: "a".repeat(40),
    pushUrl: "https://github.com/octo/app.git",
    pullRequests,
    trackingRefUpdatedAt: null,
    lastFetchedAt: null,
    isRemoteDefaultBranch: false,
    trackedBy: [],
    exclusiveCommitCount: 0,
    exclusiveCommitCountCapped: false,
  });
  const pull = (number: number): OpenPullRequest => ({
    repository: "octo/app",
    number,
    title: `Pull ${number}`,
    url: null,
    relation: "source",
    from: "octo/app:feature",
    into: "octo/app:main",
  });

  it("lists them only when the remote branch is being deleted", () => {
    const checked: BranchPullRequests = { status: "checked", provider: "gitHub", pulls: [pull(1), pull(2)], moreThanListed: false };
    expect(openPullRequests(plan({ deleteRemote: true, remote: remote(checked) })).map((open) => open.number)).toEqual([1, 2]);
    expect(openPullRequests(plan({ deleteRemote: false, remote: remote(checked) }))).toEqual([]);
    expect(openPullRequests(plan({ deleteRemote: true, remote: remote({ status: "unavailable", reason: "offline" }) }))).toEqual([]);
  });

  it("says when the provider has more than are listed", () => {
    expect(pullRequestOverflow("azureDevOps")).toBe("Azure DevOps has more open pull requests that use this branch than Repola lists here.");
  });

  it("says what deleting the branch does to them on each provider", () => {
    expect(pullRequestConsequence("gitHub", 1)).toBe("Deleting the branch closes it on GitHub. Restoring the branch lets it be reopened.");
    expect(pullRequestConsequence("azureDevOps", 2)).toBe("Deleting the branch leaves them unable to complete on Azure DevOps until the branch is restored.");
  });

  it("explains every reason the branch name must be typed", () => {
    const local = (exclusiveCommitCount: number): LocalBranchDeletion => ({
      name: "feature",
      tip: "a".repeat(40),
      mergeReference: "origin/feature",
      mergeReferenceKind: "upstream",
      mergeReferenceOid: "b".repeat(40),
      containedInMergeReference: false,
      defaultTarget: null,
      containedInDefaultTarget: null,
      occupiedWorktreePath: null,
      isDefaultBranch: false,
      upstream: "origin/feature",
      exclusiveCommitCount,
      exclusiveCommitCountCapped: false,
    });
    const pulls: BranchPullRequests = { status: "checked", provider: "gitHub", pulls: [pull(1), pull(2)], moreThanListed: false };
    expect(confirmationReasons(plan({ requiresForce: true, local: local(2), deleteRemote: true, remote: remote(pulls) }))).toEqual([
      "This forced deletion discards commits that no other ref contains.",
      "2 open pull requests use the remote branch.",
    ]);
    // Every commit is on another ref; only Git's upstream check asks for force.
    expect(confirmationReasons(plan({ requiresForce: true, local: local(0) }))).toEqual([
      "git branch -d would refuse, because feature is not contained in origin/feature.",
    ]);
    expect(confirmationReasons(plan({ deleteRemote: true, remote: remote({ status: "checked", provider: "azureDevOps", pulls: [], moreThanListed: true }) }))).toEqual([
      "The provider has more open pull requests than Repola lists.",
    ]);
    expect(confirmationReasons(plan({ deleteRemote: true, remote: remote({ status: "checked", provider: "gitHub", pulls: [pull(1)], moreThanListed: false }) }))).toEqual([
      "An open pull request uses the remote branch.",
    ]);
  });
});
