import { describe, expect, it } from "vitest";
import type { BranchDeletionPlan, BranchDeletionResult, LocalBranchDeletion } from "../ipc/types";
import { batchDeletionRefusal, deletionNotice } from "./branch-deletion-outcomes";

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

describe("deletionNotice", () => {
  const step = (target: string, recoveryCommand: string | null, succeeded = true) => ({
    target,
    deletedOid: "a".repeat(40),
    succeeded,
    output: succeeded ? "" : "! [remote rejected] (stale info)",
    recoveryCommand,
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
      local: step("feature", "git branch -- feature aaaa"),
      remote: step("origin/feature", "git push -- origin aaaa:refs/heads/feature"),
      auditWarning: "disk full",
    }))).toEqual({
      type: "success",
      title: "Deleted feature.",
      description: "To restore: git branch -- feature aaaa; git push -- origin aaaa:refs/heads/feature Audit warning: disk full",
    });
  });

  it("warns with Git's output when the remote step failed", () => {
    expect(deletionNotice(result({
      local: step("feature", "git branch -- feature aaaa"),
      remote: step("origin/feature", null, false),
    }))).toEqual({ type: "warning", title: "Deleted feature.", description: "! [remote rejected] (stale info)" });
  });
});
