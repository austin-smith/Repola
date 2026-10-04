import type { BranchDeletionPlan, BranchDeletionResult } from "../ipc/types";

// Kept apart from branch-deletion.ts so the startup bundle carries only what runs outside the dialog.

/** Why a reviewed branch is left out of a batch, which never forces a deletion or asks for a typed name. */
export function batchDeletionRefusal(plan: BranchDeletionPlan): string | null {
  if (plan.blockers.length > 0) return plan.blockers.join(" ");
  if (plan.confirmation === "confirm") return null;
  const reason = plan.requiresForce && plan.local
    ? `${plan.branchName} is not contained in ${plan.local.mergeReference}`
    : `Deleting ${plan.branchName} needs its name typed`;
  return `${reason}. Review it from the branch menu instead.`;
}

export interface DeletionNotice {
  type: "success" | "warning";
  title: string;
  description?: string;
}

/** Reports a finished deletion: how to restore what was deleted, or why the remote step failed. */
export function deletionNotice(result: BranchDeletionResult): DeletionNotice {
  if (result.remote !== null && !result.remote.succeeded) {
    return { type: "warning", title: result.message, description: result.remote.output };
  }
  const recovery = [result.local, result.remote]
    .flatMap((step) => (step?.recoveryCommand ? [step.recoveryCommand] : []))
    .join("; ");
  const description = [
    recovery && `To restore: ${recovery}`,
    result.auditWarning && `Audit warning: ${result.auditWarning}`,
  ].filter(Boolean).join(" ");
  return { type: "success", title: result.message, description: description || undefined };
}
