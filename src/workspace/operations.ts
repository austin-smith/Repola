import type { RepositoryOperation } from "../ipc/types";

export function operationLabel(operation: RepositoryOperation): string {
  switch (operation) {
    case "merge": return "Merge";
    case "rebase": return "Rebase";
    case "cherryPick": return "Cherry-pick";
    case "revert": return "Revert";
    case "bisect": return "Bisect";
    case "sequencer": return "Git sequence";
  }
}

export function operationSupportsSkip(operation: RepositoryOperation): boolean {
  return operation === "rebase" || operation === "cherryPick" || operation === "revert" || operation === "bisect";
}

export function operationGuidance(operation: RepositoryOperation, conflicted: boolean): string {
  if (conflicted) return "Resolve every conflicted file below. Repola will enable Continue when the index is ready.";
  if (operation === "bisect") return "Test this revision outside Repola, then use Git to mark it good or bad. You can skip an untestable revision or abort the bisect here.";
  if (operation === "sequencer") return "Git reported an internal sequence that Repola cannot identify safely. Finish it with Git, then refresh the working copy.";
  return "The index has no unresolved files. Continue to let Git complete this step, or abort to restore the pre-operation state.";
}
