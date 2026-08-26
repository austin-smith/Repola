import { ageInDays } from "./format";
import type { ActionKind, RegistrationKind, ScanResult, WorktreeRecord } from "./types";

export type AgeFilter = 0 | 30 | 90 | 180 | 365;
export type StateFilter = "all" | "clean" | "changed" | "attention";

export interface InventoryFilters {
  age: AgeFilter;
  query: string;
  repositoryPath: string;
  state: StateFilter;
}

export interface ActionDescriptor {
  kind: ActionKind;
  label: string;
  description: string;
  destructive: boolean;
}

const attentionKinds = new Set<RegistrationKind>(["brokenLink", "locked", "missing", "prunable"]);

export function matchesState(worktree: WorktreeRecord, filter: StateFilter): boolean {
  if (filter === "all") return true;
  if (filter === "clean") {
    return !worktree.isPrimary
      && worktree.registration.kind === "healthy"
      && worktree.status.available
      && worktree.status.total === 0;
  }
  if (filter === "changed") return worktree.status.available && worktree.status.total > 0;
  return attentionKinds.has(worktree.registration.kind);
}

export function filterAndSortWorktrees(
  worktrees: WorktreeRecord[],
  filters: InventoryFilters,
  now: number,
): WorktreeRecord[] {
  const normalized = filters.query.trim().toLowerCase();
  const filtered: WorktreeRecord[] = [];
  for (const worktree of worktrees) {
    if (filters.repositoryPath !== "all" && worktree.repositoryPath !== filters.repositoryPath) continue;
    if (filters.age !== 0 && (ageInDays(worktree.lastActivityAtMs, now) ?? -1) < filters.age) continue;
    if (!matchesState(worktree, filters.state)) continue;
    if (normalized) {
      const values = [
        worktree.repositoryName,
        worktree.branch,
        worktree.path,
        worktree.head,
        worktree.headSubject,
        worktree.origin.label,
        worktree.origin.id,
      ];
      if (!values.some((value) => value?.toLowerCase().includes(normalized))) continue;
    }
    filtered.push(worktree);
  }
  return filtered.sort((left, right) => {
    const leftDate = left.lastActivityAtMs ?? Number.MAX_SAFE_INTEGER;
    const rightDate = right.lastActivityAtMs ?? Number.MAX_SAFE_INTEGER;
    return leftDate - rightDate
      || left.repositoryName.localeCompare(right.repositoryName)
      || left.path.localeCompare(right.path);
  });
}

export function isRemovable(worktree: WorktreeRecord): boolean {
  return actionForWorktree(worktree)?.kind === "remove";
}

export function computeTotals(
  worktrees: WorktreeRecord[],
  repositoryCount: number,
): ScanResult["totals"] {
  const totals = {
    repositoryCount,
    primaryCount: 0,
    linkedCount: 0,
    existingLinkedCount: 0,
    cleanCount: 0,
    dirtyCount: 0,
    missingCount: 0,
    prunableCount: 0,
    brokenLinkCount: 0,
    linkedSizeBytes: 0,
  };
  for (const worktree of worktrees) {
    if (worktree.isPrimary) {
      totals.primaryCount += 1;
      continue;
    }
    totals.linkedCount += 1;
    if (worktree.exists) totals.existingLinkedCount += 1;
    else totals.missingCount += 1;
    if (worktree.status.available) {
      if (worktree.status.total === 0) totals.cleanCount += 1;
      else totals.dirtyCount += 1;
    }
    if (worktree.registration.kind === "prunable") totals.prunableCount += 1;
    if (worktree.registration.kind === "brokenLink") totals.brokenLinkCount += 1;
    totals.linkedSizeBytes += worktree.sizeBytes ?? 0;
  }
  return totals;
}

export function actionForWorktree(worktree: WorktreeRecord): ActionDescriptor | null {
  if (worktree.registration.kind === "brokenLink") {
    return { kind: "repair", label: "Review Repair…", description: "Update Git pointers after a repository move.", destructive: false };
  }
  if (worktree.registration.kind === "locked") {
    return { kind: "unlock", label: "Review Unlock…", description: "Unlock this registration before cleanup.", destructive: false };
  }
  if (worktree.registration.kind === "prunable") {
    return { kind: "pruneRepository", label: "Review Metadata Prune…", description: "Remove stale metadata across this repository.", destructive: true };
  }
  if (!worktree.isPrimary
    && worktree.registration.kind === "healthy"
    && worktree.status.available
    && worktree.status.total === 0
    && !worktree.detached) {
    return { kind: "remove", label: "Review Removal…", description: "Run a fresh preflight before Git removes the directory.", destructive: true };
  }
  return null;
}
