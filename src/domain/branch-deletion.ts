import type { BranchDeletionPlan, BranchDeletionResult, BranchInfo } from "../ipc/types";

export interface DeletionScope {
  deleteLocal: boolean;
  deleteRemote: boolean;
}

export interface ScopeOption {
  available: boolean;
  /** Why the option cannot be chosen, when it is unavailable. */
  reason: string | null;
}

export interface BranchScopeOptions {
  local: ScopeOption;
  remote: ScopeOption & { label: string | null };
}

/** Which parts of a branch can be chosen for deletion before the engine reviews it. */
export function scopeOptions(branch: BranchInfo, worktreePath: string): BranchScopeOptions {
  if (branch.remote) {
    return {
      local: { available: false, reason: "A remote-tracking branch has no local branch to delete." },
      remote: { available: true, reason: null, label: branch.name },
    };
  }
  const occupiedReason = branch.occupiedWorktreePath === null
    ? null
    : branch.occupiedWorktreePath === worktreePath
      ? "Checked out in this worktree. Switch to another branch first."
      : `Checked out at ${branch.occupiedWorktreePath}. Switch that worktree to another branch first.`;
  return {
    local: { available: occupiedReason === null, reason: occupiedReason },
    remote: branch.upstream === null
      ? { available: false, reason: `${branch.name} has no upstream branch.`, label: null }
      : { available: true, reason: null, label: branch.upstream },
  };
}

/** Remote deletion is never preselected; the local branch is when it can be deleted. */
export function initialScope(branch: BranchInfo, worktreePath: string): DeletionScope {
  const options = scopeOptions(branch, worktreePath);
  return {
    deleteLocal: options.local.available,
    deleteRemote: branch.remote,
  };
}

export function hasScope(scope: DeletionScope): boolean {
  return scope.deleteLocal || scope.deleteRemote;
}

/** The branch the dialog opens on: the first local branch that is free to delete. */
export function initialDeletionBranch(branches: BranchInfo[], worktreePath: string): BranchInfo | null {
  return branches.find((branch) => !branch.remote && scopeOptions(branch, worktreePath).local.available)
    ?? branches.find((branch) => !branch.current)
    ?? null;
}

export function confirmationSatisfied(plan: BranchDeletionPlan, typed: string): boolean {
  return plan.confirmation === "confirm" || typed === plan.branchName;
}

export function canExecuteDeletion(plan: BranchDeletionPlan, typed: string): boolean {
  return plan.blockers.length === 0 && confirmationSatisfied(plan, typed);
}

export function deletionButtonLabel(plan: BranchDeletionPlan): string {
  if (plan.deleteLocal && plan.deleteRemote) return plan.requiresForce ? "Force Delete Local and Remote" : "Delete Local and Remote";
  if (plan.deleteRemote) return "Delete Remote Branch";
  return plan.requiresForce ? "Force Delete Branch" : "Delete Branch";
}

/** Describes how old a Unix-seconds timestamp is, for labelling remote-tracking freshness. */
export function describeObservedAt(seconds: number | null, nowMs = Date.now()): string {
  if (seconds === null) return "unknown";
  const elapsed = Math.max(0, Math.floor(nowMs / 1000) - seconds);
  if (elapsed < 60) return "just now";
  if (elapsed < 3_600) return plural(Math.floor(elapsed / 60), "minute");
  if (elapsed < 86_400) return plural(Math.floor(elapsed / 3_600), "hour");
  return plural(Math.floor(elapsed / 86_400), "day");
}

function plural(count: number, unit: string): string {
  return `${count} ${unit}${count === 1 ? "" : "s"} ago`;
}

export interface DeletionNotice {
  type: "success" | "warning";
  title: string;
  description?: string;
}

/**
 * Reports a finished deletion: why a step failed or left something undone, how to restore what
 * was deleted, and whether the audit log missed it.
 */
export function deletionNotice(result: BranchDeletionResult): DeletionNotice {
  const steps = [result.local, result.remote].flatMap((step) => (step ? [step] : []));
  const problems = steps.flatMap((step) => [step.succeeded ? "" : step.output, step.warning ?? ""]).filter(Boolean);
  const recovery = steps.flatMap((step) => (step.recoveryCommand ? [step.recoveryCommand] : [])).join("; ");
  const description = [
    ...problems,
    recovery && `To restore: ${recovery}`,
    result.auditWarning && `Audit warning: ${result.auditWarning}`,
  ].filter(Boolean).join(" ");
  return { type: problems.length > 0 ? "warning" : "success", title: result.message, description: description || undefined };
}
