import type { BranchDeletionPlan, BranchDeletionResult } from "../ipc/types";

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
