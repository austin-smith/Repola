import type { BranchDeletionPlan, FollowUpAction } from "../ipc/types";

// Kept apart from branch-deletion.ts so the startup bundle carries only what runs outside the dialog.

/**
 * Why a reviewed local-only deletion is left out of a batch, which never forces a deletion. A
 * local-only review asks for the typed branch name exactly when the branch needs force.
 */
export function batchDeletionRefusal(plan: BranchDeletionPlan): string | null {
  if (plan.blockers.length > 0) return plan.blockers.join(" ");
  if (plan.confirmation === "confirm") return null;
  return `${plan.branchName} is not contained in ${plan.local?.mergeReference ?? "the branch it is checked against"}. Review it from the branch menu instead.`;
}

/** A finished worktree removal and the branch it left behind, if any could be reviewed. */
export interface Removal {
  repositoryPath: string;
  followUp: FollowUpAction | null;
}

/**
 * The branches a batch of removals left behind, each reviewed from a checkout that still exists.
 * A follow-up names the checkout that remained right after its own removal, which a later removal
 * in the batch may take away. The latest removal in a repository saw every earlier one, so its
 * checkout is the one left at the end; when it found none, nothing remains to review from.
 */
export function settleFollowUps(removals: Removal[]): FollowUpAction[] {
  const checkouts = new Map(removals.map((removal) => [removal.repositoryPath, removal.followUp?.worktreePath]));
  return removals.flatMap(({ repositoryPath, followUp }) => {
    const worktreePath = checkouts.get(repositoryPath);
    return followUp && worktreePath ? [{ ...followUp, worktreePath }] : [];
  });
}

/** Whether a rejected `executeBranchDeletion` may have deleted anyway: only the engine's own answer says it did not. */
export function deletionOutcomeUnknown(cause: unknown): boolean {
  return !(typeof cause === "object" && cause !== null && "outcomeKnown" in cause && cause.outcomeKnown === true);
}
