import type {
  DiscardPlanEntry,
  DiscardScope,
  FileChange,
  KeptChangeReason,
  MachineKind,
  RecoveryPoint,
  RecoveryPointKind,
  RecoveryRestoreEntry,
} from "../ipc/types";

/** Recovery points larger than this get an explicit size warning before discarding. */
export const LARGE_RECOVERY_POINT_BYTES = 100 * 1024 * 1024;

export function isLargeRecoveryPoint(bytes: number): boolean {
  return bytes > LARGE_RECOVERY_POINT_BYTES;
}

function isSubmodule(change: FileChange): boolean {
  return change.submodule || [change.headMode, change.indexMode, change.worktreeMode].includes("160000");
}

/** Git lists an untracked repository nested in the working tree as one directory entry ending in a slash. */
function isNestedRepository(change: FileChange): boolean {
  return change.untracked && change.path.display.endsWith("/");
}

/** Why one file cannot be discarded on its own, or null when it can. Mirrors the engine's refusals. */
export function fileDiscardBlocker(change: FileChange): string | null {
  if (change.conflicted) return "Resolve the conflict instead of discarding it.";
  if (change.ignored) return "Repola does not discard ignored files.";
  if (isSubmodule(change)) return "Repola leaves submodules alone because it cannot save their changes first.";
  if (isNestedRepository(change)) return "Repola leaves nested repositories alone because it cannot save them first.";
  return null;
}

/** Offer the least destructive scope first: unstaged edits when there are any. */
export function defaultDiscardScope(change: FileChange): DiscardScope {
  return change.unstaged || change.untracked ? "unstaged" : "all";
}

export function discardEffectLabel(entry: DiscardPlanEntry): string {
  switch (entry.effect) {
    case "remove":
      return entry.kind === "untracked" ? "Delete the untracked file" : "Remove the new file";
    case "unstage":
      return "Unstage the new file, which is already deleted";
    case "restoreStaged":
      return "Replace unstaged edits with the staged version";
    case "restoreCommitted":
      if (entry.kind === "renamed" && entry.previousPath) return `Restore ${entry.previousPath.display} and remove this name`;
      if (entry.kind === "deleted") return "Restore the deleted file";
      return "Restore the committed version";
  }
}

export function keptReasonLabel(reason: KeptChangeReason): string {
  return reason === "submodule"
    ? "Submodule: commit, stash, or reset it inside the submodule."
    : "Nested repository: move or delete it yourself if you no longer need it.";
}

export function recoveryPointKindLabel(kind: RecoveryPointKind): string {
  switch (kind) {
    case "discardFile":
      return "File discard";
    case "discardAll":
      return "Discard all";
    case "restore":
      return "Replaced by a restore";
  }
}

export function restoreEffectLabel(entry: RecoveryRestoreEntry): string {
  switch (entry.worktree) {
    case "create":
      return "Recreate the saved file";
    case "replace":
      return "Replace the current content";
    case "remove":
      return "Remove the current file";
    case "unchanged":
      return entry.indexChanges ? "Restore the saved staged state" : "Already matches";
  }
}

/** Entries a restore would actually change. */
export function restoreChanges(entries: readonly RecoveryRestoreEntry[]): RecoveryRestoreEntry[] {
  return entries.filter((entry) => entry.worktree !== "unchanged" || entry.indexChanges);
}

/** Recovery points are shared by every worktree of a repository; list this worktree's first. */
export function partitionRecoveryPoints(
  points: readonly RecoveryPoint[],
  worktreePath: string,
): { here: RecoveryPoint[]; elsewhere: RecoveryPoint[] } {
  const here: RecoveryPoint[] = [];
  const elsewhere: RecoveryPoint[] = [];
  for (const point of points) (point.worktreePath === worktreePath ? here : elsewhere).push(point);
  return { here, elsewhere };
}

/** Saved paths beyond the sample a recovery point lists. */
export function unlistedPathCount(point: RecoveryPoint): number {
  return Math.max(0, point.pathCount - point.paths.length);
}

/** Where recovery points live, in terms of the machine that owns the working copy. */
export function recoveryLocation(machineKind: MachineKind): string {
  return machineKind === "local" ? "in this repository on this computer" : "in this repository on the remote machine";
}
