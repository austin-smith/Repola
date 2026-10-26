import { describe, expect, it } from "vitest";
import type { BranchDeletionPlan, FollowUpAction, LocalBranchDeletion } from "../ipc/types";
import { batchDeletionRefusal, deletionOutcomeUnknown, settleFollowUps } from "./branch-deletion-outcomes";

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

function localDetails(): LocalBranchDeletion {
  return {
    name: "feature",
    tip: "a".repeat(40),
    mergeReference: "origin/feature",
    mergeReferenceKind: "upstream",
    mergeReferenceOid: "b".repeat(40),
    containedInMergeReference: true,
    defaultTarget: "origin/main",
    containedInDefaultTarget: false,
    occupiedWorktreePath: null,
    isDefaultBranch: false,
    upstream: "origin/feature",
    exclusiveCommitCount: 0,
    exclusiveCommitCountCapped: false,
  };
}

describe("batch branch deletion", () => {
  it("keeps only branches that need neither force nor a typed name", () => {
    expect(batchDeletionRefusal(plan({}))).toBeNull();
    expect(batchDeletionRefusal(plan({ blockers: ["feature is checked out.", "Another reason."] })))
      .toBe("feature is checked out. Another reason.");
  });

  it("sends a branch that needs force to the branch menu", () => {
    const forced = plan({
      requiresForce: true,
      confirmation: "typeBranchName",
      local: { ...localDetails(), mergeReference: "main", containedInMergeReference: false },
    });
    expect(batchDeletionRefusal(forced)).toBe("feature is not contained in main. Review it from the branch menu instead.");
  });
});

describe("settleFollowUps", () => {
  const followUp = (repositoryPath: string, branch: string, worktreePath: string): FollowUpAction => ({
    repositoryPath,
    worktreePath,
    branch,
    description: `Branch ${branch} was retained.`,
  });

  it("reviews every retained branch from the checkout left after the batch", () => {
    const settled = settleFollowUps([
      { repositoryPath: "/repos/bare.git", followUp: followUp("/repos/bare.git", "one", "/work/two") },
      { repositoryPath: "/repos/other", followUp: followUp("/repos/other", "solo", "/repos/other") },
      { repositoryPath: "/repos/bare.git", followUp: followUp("/repos/bare.git", "two", "/work/three") },
    ]);

    expect(settled.map(({ branch, worktreePath }) => [branch, worktreePath])).toEqual([
      ["one", "/work/three"],
      ["solo", "/repos/other"],
      ["two", "/work/three"],
    ]);
  });

  it("drops a repository's branches once no checkout remains to review them from", () => {
    const settled = settleFollowUps([
      { repositoryPath: "/repos/bare.git", followUp: followUp("/repos/bare.git", "one", "/work/two") },
      { repositoryPath: "/repos/bare.git", followUp: null },
      { repositoryPath: "/repos/other", followUp: followUp("/repos/other", "solo", "/repos/other") },
    ]);

    expect(settled.map(({ branch }) => branch)).toEqual(["solo"]);
  });
});

describe("deletionOutcomeUnknown", () => {
  it("trusts only the engine's word that a deletion did not happen", () => {
    expect(deletionOutcomeUnknown({ message: "The branch deletion is blocked.", outcomeKnown: true })).toBe(false);
    expect(deletionOutcomeUnknown({ message: "The SSH connection closed.", outcomeKnown: false })).toBe(true);
    expect(deletionOutcomeUnknown(new Error("IPC bridge unavailable"))).toBe(true);
    expect(deletionOutcomeUnknown("lost")).toBe(true);
  });
});
