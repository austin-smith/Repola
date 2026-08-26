import type { CommitFileSelectionRequest, FileChange, PatchHunk } from "../ipc/types";

export interface CommitHunkSelection {
  readonly expectedPatch: string;
  readonly selectedLineIndices: readonly number[];
}

export type FileCommitSelection =
  | { readonly kind: "all" }
  | { readonly kind: "none" }
  | { readonly kind: "partial"; readonly hunks: readonly CommitHunkSelection[] };

export type CommitSelectionMap = ReadonlyMap<string, FileCommitSelection>;

export const includeAllChanges: FileCommitSelection = { kind: "all" };
export const includeNoChanges: FileCommitSelection = { kind: "none" };

export function createCommitSelection(changes: readonly FileChange[]): CommitSelectionMap {
  return new Map(changes.filter((change) => !change.ignored).map((change) => [change.id, includeAllChanges]));
}

export function reconcileCommitSelection(
  changes: readonly FileChange[],
  current: CommitSelectionMap,
): CommitSelectionMap {
  return new Map(changes.filter((change) => !change.ignored).map((change) => [
    change.id,
    current.get(change.id) ?? includeAllChanges,
  ]));
}

export function commitSelectionFor(
  selections: CommitSelectionMap,
  changeId: string,
): FileCommitSelection {
  return selections.get(changeId) ?? includeAllChanges;
}

export function isIncludedInCommit(selection: FileCommitSelection): boolean {
  return selection.kind !== "none";
}

export function includedChangeCount(
  changes: readonly FileChange[],
  selections: CommitSelectionMap,
): number {
  return changes.filter((change) => isIncludedInCommit(commitSelectionFor(selections, change.id))).length;
}

export function commitSelectionRequest(
  changes: readonly FileChange[],
  selections: CommitSelectionMap,
): CommitFileSelectionRequest[] {
  return changes.flatMap((change) => {
    const selection = commitSelectionFor(selections, change.id);
    if (selection.kind === "none") return [];
    return [{
      path: change.path,
      previousPath: change.previousPath,
      expectedIndexStatus: change.indexStatus,
      expectedWorktreeStatus: change.worktreeStatus,
      includeAll: selection.kind === "all",
      hunks: selection.kind === "partial"
        ? selection.hunks.map((hunk) => ({
            expectedPatch: hunk.expectedPatch,
            selectedLineIndices: [...hunk.selectedLineIndices],
          }))
        : [],
    }];
  });
}

export function setChangesIncluded(
  selections: CommitSelectionMap,
  changeIds: ReadonlySet<string>,
  included: boolean,
): CommitSelectionMap {
  const next = new Map(selections);
  const selection = included ? includeAllChanges : includeNoChanges;
  changeIds.forEach((id) => next.set(id, selection));
  return next;
}

export function normalizePartialCommitSelection(
  hunks: readonly PatchHunk[],
  selections: readonly CommitHunkSelection[],
): FileCommitSelection {
  const selectedByPatch = new Map(selections.map((selection) => [
    selection.expectedPatch,
    new Set(selection.selectedLineIndices),
  ]));
  let selectableLineCount = 0;
  let selectedLineCount = 0;
  const partialHunks: CommitHunkSelection[] = [];

  for (const hunk of hunks) {
    const selectable = selectableLineIndices(hunk.patch);
    const selected = selectedByPatch.get(hunk.patch) ?? new Set<number>();
    const validSelected = selectable.filter((index) => selected.has(index));
    selectableLineCount += selectable.length;
    selectedLineCount += validSelected.length;
    if (validSelected.length > 0) {
      partialHunks.push({ expectedPatch: hunk.patch, selectedLineIndices: validSelected });
    }
  }

  if (selectableLineCount === 0 || selectedLineCount === selectableLineCount) return includeAllChanges;
  if (selectedLineCount === 0) return includeNoChanges;
  return { kind: "partial", hunks: partialHunks };
}

export function selectedLinesForHunk(
  selection: FileCommitSelection,
  hunk: PatchHunk,
): readonly number[] {
  if (selection.kind === "all") return selectableLineIndices(hunk.patch);
  if (selection.kind === "none") return [];
  return selection.hunks.find((candidate) => candidate.expectedPatch === hunk.patch)?.selectedLineIndices ?? [];
}

export function selectableLineIndices(patch: string): number[] {
  const hunkOffset = patch.indexOf("@@ ");
  if (hunkOffset < 0) return [];
  const lines = patch.slice(hunkOffset).split("\n");
  return lines.slice(1).flatMap((line, index) => (
    line.startsWith("+") || line.startsWith("-") ? [index] : []
  ));
}
