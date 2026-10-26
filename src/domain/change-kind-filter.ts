import type { FileChangeKind } from "../ipc/types";

/** A kind a changed-file list can be filtered by. Untracked files are additions. */
export type ChangeKindFilterKind = Exclude<FileChangeKind, "untracked">;

export interface ChangeKindCount {
  readonly kind: ChangeKindFilterKind;
  readonly count: number;
}

interface KindedFile {
  readonly kind: FileChangeKind;
}

/** Display order of the filter, most common kinds first. */
const changeKindFilterOrder: readonly ChangeKindFilterKind[] = [
  "added",
  "modified",
  "deleted",
  "renamed",
  "copied",
  "typeChanged",
  "unmerged",
  "unknown",
  "ignored",
];

export function changeKindFilterKind(kind: FileChangeKind): ChangeKindFilterKind {
  return kind === "untracked" ? "added" : kind;
}

export function isChangeKindFilterKind(value: string): value is ChangeKindFilterKind {
  return changeKindFilterOrder.some((kind) => kind === value);
}

/** Counts the kinds present in a list, in display order, omitting kinds with no files. */
export function countChangeKinds(files: readonly KindedFile[]): ChangeKindCount[] {
  const counts = new Map<ChangeKindFilterKind, number>();
  for (const file of files) {
    const kind = changeKindFilterKind(file.kind);
    counts.set(kind, (counts.get(kind) ?? 0) + 1);
  }
  return changeKindFilterOrder.flatMap((kind) => {
    const count = counts.get(kind);
    return count === undefined ? [] : [{ kind, count }];
  });
}

/**
 * The selected kinds that still have files. A kind whose files have all gone
 * stops filtering, so a refresh can never leave the list hidden behind a
 * toggle that is no longer shown.
 */
export function activeChangeKinds(
  selected: readonly ChangeKindFilterKind[],
  counts: readonly ChangeKindCount[],
): ChangeKindFilterKind[] {
  return selected.filter((kind) => counts.some((count) => count.kind === kind));
}

/** Keeps the files of the active kinds; no active kind keeps every file. */
export function filterByChangeKind<File extends KindedFile>(
  files: readonly File[],
  active: readonly ChangeKindFilterKind[],
): readonly File[] {
  if (active.length === 0) return files;
  const kinds = new Set(active);
  return files.filter((file) => kinds.has(changeKindFilterKind(file.kind)));
}
