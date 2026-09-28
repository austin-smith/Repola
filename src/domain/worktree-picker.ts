import { shortSha } from "./format";
import type { WorktreeRecord } from "../ipc/types";

export interface WorktreePickerGroup {
  value: string;
  items: WorktreeRecord[];
}

/** The worktree's folder name, accepting either separator because Git prints `/` on Windows. */
export function worktreeFolderName(path: string): string {
  const segments = path.split(/[\\/]/).filter((segment) => segment.length > 0);
  return segments[segments.length - 1] ?? path;
}

export function worktreeBranchLabel(worktree: WorktreeRecord): string {
  return worktree.branch ?? `Detached at ${shortSha(worktree.head)}`;
}

/** Matches the query against both values the picker shows: the folder name and the branch. */
export function matchesWorktreeQuery(worktree: WorktreeRecord, query: string): boolean {
  const normalized = query.trim().toLowerCase();
  if (!normalized) return true;
  return [worktreeFolderName(worktree.path), worktreeBranchLabel(worktree)]
    .some((value) => value.toLowerCase().includes(normalized));
}

export function groupWorktreesForPicker(worktrees: WorktreeRecord[]): WorktreePickerGroup[] {
  const groups: WorktreePickerGroup[] = [
    { value: "Main Worktree", items: worktrees.filter((worktree) => worktree.isPrimary) },
    { value: "Linked Worktrees", items: worktrees.filter((worktree) => !worktree.isPrimary) },
  ];
  return groups.filter((group) => group.items.length > 0);
}
