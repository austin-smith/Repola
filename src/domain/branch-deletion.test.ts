import { describe, expect, it } from "vitest";
import type { BranchDeletionPlan, BranchDeletionResult, BranchInfo } from "../ipc/types";
import {
  canExecuteDeletion,
  confirmationSatisfied,
  deletionButtonLabel,
  deletionNotice,
  describeObservedAt,
  initialDeletionBranch,
  initialScope,
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
  const step = (target: string, recoveryCommand: string | null, succeeded = true, warning: string | null = null) => ({
    target,
    deletedOid: "a".repeat(40),
    succeeded,
    output: succeeded ? "Deleted branch feature (was aaaa)." : "! [remote rejected] (stale info)",
    warning,
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

  it("keeps the local recovery when the remote step failed", () => {
    expect(deletionNotice(result({
      local: step("feature", "git branch -- feature aaaa"),
      remote: step("origin/feature", null, false),
      auditWarning: "disk full",
    }))).toEqual({
      type: "warning",
      title: "Deleted feature.",
      description: "! [remote rejected] (stale info) To restore: git branch -- feature aaaa Audit warning: disk full",
    });
  });

  it("warns about anything a successful step left undone", () => {
    const leftover = "feature was deleted, but its configuration was not removed.";
    expect(deletionNotice(result({ local: step("feature", "git branch -- feature aaaa", true, leftover) }))).toEqual({
      type: "warning",
      title: "Deleted feature.",
      description: `${leftover} To restore: git branch -- feature aaaa`,
    });
  });
});
