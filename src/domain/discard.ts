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
      return entry.tracked ? "Remove the new file" : "Delete the untracked file";
    case "unstage":
      return "Unstage the new file, which is already deleted";
    case "restoreStaged":
      return entry.onDisk ? "Replace unstaged edits with the staged version" : "Restore the staged version of the deleted file";
    case "restoreCommitted":
      return entry.onDisk ? "Restore the committed version" : "Restore the deleted file";
    case "restoreCommittedStaged":
      return "Restore the committed version in the index; sparse checkout keeps the file off disk";
  }
}

export function keptReasonLabel(reason: KeptChangeReason): string {
  switch (reason) {
    case "submodule":
      return "Submodule: commit, stash, or reset it inside the submodule.";
    case "nestedRepository":
      return "Nested repository: move or delete it yourself if you no longer need it.";
    case "fileFolderConflict":
      return "A file in one place and a folder in another: sort it out with Git first.";
  }
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
  const staged = entry.indexChanges ? " and the saved staged state" : "";
  switch (entry.worktree) {
    case "create":
      return `Recreate the saved file${staged}`;
    case "replace":
      return `Replace the current content${staged}`;
    case "remove":
      return entry.indexChanges ? "Remove the current file and restore the saved staged state" : "Remove the current file";
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
