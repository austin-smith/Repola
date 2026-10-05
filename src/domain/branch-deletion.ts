import { toMessage } from "../lib/errors";
import { deletionOutcomeUnknown } from "./branch-deletion-outcomes";
import type {
  BranchDeletionFailure,
  BranchDeletionPlan,
  BranchDeletionResult,
  BranchInfo,
  OpenPullRequest,
  RemoteProvider,
} from "../ipc/types";

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
 * Reports a finished deletion: why a step failed or left something undone, how to finish it, how
 * to restore what was deleted, and whether the audit log missed it.
 */
export function deletionNotice(result: BranchDeletionResult): DeletionNotice {
  const steps = [result.local, result.remote].flatMap((step) => (step ? [step] : []));
  const problems = steps.flatMap((step) => [step.succeeded ? "" : step.output, step.warning ?? ""]).filter(Boolean);
  const finish = steps.flatMap((step) => step.finishCommands).join("; ");
  const recovery = steps.flatMap((step) => step.recoveryCommands).join("; ");
  const description = [
    ...problems,
    finish && `To finish: ${finish}`,
    recovery && `To restore: ${recovery}`,
    result.auditWarning && `Audit warning: ${result.auditWarning}`,
  ].filter(Boolean).join(" ");
  return { type: problems.length > 0 ? "warning" : "success", title: result.message, description: description || undefined };
}

/** Why `executeBranchDeletion` rejected, and whether the deletion may have happened anyway. */
export function deletionFailure(cause: unknown): BranchDeletionFailure {
  return { message: toMessage(cause), outcomeKnown: !deletionOutcomeUnknown(cause) };
}

/** The open pull requests that use the remote branch this plan deletes as their source or target. */
export function openPullRequests(plan: BranchDeletionPlan): OpenPullRequest[] {
  const check = plan.deleteRemote ? plan.remote?.pullRequests : null;
  return check?.status === "checked" ? check.pulls : [];
}

/** What deleting their source branch does to open pull requests on this provider. */
export function pullRequestConsequence(provider: RemoteProvider, count: number): string {
  const them = count === 1 ? "it" : "them";
  switch (provider) {
    case "gitHub":
      return `Deleting the branch closes ${them} on GitHub. Restoring the branch lets ${them} be reopened.`;
    case "azureDevOps":
      return `Deleting the branch leaves ${them} unable to complete on Azure DevOps until the branch is restored.`;
    default:
      return `Deleting the branch removes the source of ${them}.`;
  }
}

/** Why the review asks for the typed branch name before deleting. */
export function confirmationReasons(plan: BranchDeletionPlan): string[] {
  const reasons: string[] = [];
  const { local } = plan;
  if (plan.requiresForce && local) {
    reasons.push(local.exclusiveCommitCount > 0
      ? "This forced deletion discards commits that no other ref contains."
      : `git branch -d would refuse, because ${local.name} is not contained in ${local.mergeReference}.`);
  }
  const remote = plan.deleteRemote ? plan.remote : null;
  if (remote && remote.exclusiveCommitCount > 0) reasons.push("Commits on the remote branch exist in no ref that remains after this deletion.");
  const pulls = openPullRequests(plan).length;
  if (pulls === 1) reasons.push("An open pull request uses the remote branch.");
  if (pulls > 1) reasons.push(`${pulls} open pull requests use the remote branch.`);
  const check = plan.deleteRemote ? plan.remote?.pullRequests : null;
  if (check?.status === "checked" && check.moreThanListed) reasons.push("The provider has more open pull requests than Repola lists.");
  return reasons;
}

/** Says that the provider has more open pull requests than the review lists. */
export function pullRequestOverflow(provider: RemoteProvider): string {
  const where = provider === "azureDevOps" ? "Azure DevOps" : provider === "gitHub" ? "GitHub" : "The provider";
  return `${where} has more open pull requests that use this branch than Repola lists here.`;
}
