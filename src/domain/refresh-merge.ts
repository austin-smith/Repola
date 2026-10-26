import type { RefreshResult, ScanResult, WorktreeRecord, WorktreeMeasurement } from "../ipc/types";
import { computeTotals } from "./inventory";

function sameRecord(left: WorktreeRecord, right: WorktreeRecord): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}

function keepMeasurement(next: WorktreeRecord, previous?: WorktreeRecord): WorktreeRecord {
  if (!previous || next.id !== previous.id || !next.exists) return next;
  return {
    ...next,
    sizeBytes: previous.sizeBytes,
    sizeIncomplete: previous.sizeIncomplete,
    lastActivityAtMs: Math.max(next.lastActivityAtMs ?? 0, previous.lastActivityAtMs ?? 0) || null,
  };
}

/** Only a successful, complete membership enumeration can remove a row. */
export function mergeRefresh(current: ScanResult | null, result: RefreshResult, registeredPaths: string[]): ScanResult {
  const successful = new Set(result.successfulRepositoryPaths);
  const registered = new Set(registeredPaths);
  const previous = new Map(current?.worktrees.filter((record) => registered.has(record.repositoryPath)).map((record) => [record.id, record]) ?? []);
  const records = new Map<string, WorktreeRecord>();
  for (const record of previous.values()) {
    if (!successful.has(record.repositoryPath)) records.set(record.id, record);
  }
  for (const membership of result.memberships) {
    const old = previous.get(membership.id);
    // Membership conveys registration and identity; it does not refresh status,
    // graph calculations, or measurement fields on an uninspected worktree.
    const changed = old && (old.head !== membership.head || old.branch !== membership.branch || old.exists !== membership.exists || old.registration.kind !== membership.registration.kind);
    const record = old ? {
      ...old,
      repositoryName: membership.repositoryName,
      branch: membership.branch,
      head: membership.head,
      detached: membership.detached,
      isPrimary: membership.isPrimary,
      exists: membership.exists,
      registration: membership.registration,
      ...(!membership.exists ? { sizeBytes: null, sizeIncomplete: false } : {}),
      ...(changed ? {
        status: { ...old.status, available: false },
        safety: membership.safety,
        integration: membership.integration,
        unpushedCommitCount: null,
        headSubject: null,
        headCommitAtMs: null,
      } : {}),
    } : membership;
    records.set(record.id, old && sameRecord(old, record) ? old : record);
  }
  for (const inspected of result.worktrees) {
    const old = previous.get(inspected.id);
    const record = keepMeasurement(inspected, old);
    records.set(record.id, old && sameRecord(old, record) ? old : record);
  }
  for (const error of result.errors) {
    for (const [id, record] of records) {
      if (record.repositoryPath !== error.repositoryPath || (error.worktreePath && record.path !== error.worktreePath)) continue;
      records.set(id, { ...record, status: { ...record.status, available: false }, safety: { level: "protected", label: "Inspection unavailable", reasons: [error.message] } });
    }
  }
  const worktrees = [...records.values()];
  const byRepository = new Map<string, WorktreeRecord[]>();
  for (const record of worktrees) {
    const members = byRepository.get(record.repositoryPath) ?? [];
    members.push(record);
    byRepository.set(record.repositoryPath, members);
  }
  const summaries = new Map(current?.repositories.filter((repository) => registered.has(repository.path)).map((repository) => [repository.path, repository]) ?? []);
  for (const repository of result.repositories) summaries.set(repository.path, repository);
  for (const error of result.errors) {
    if (error.worktreePath || summaries.has(error.repositoryPath)) continue;
    summaries.set(error.repositoryPath, { id: `unavailable:${error.repositoryPath}`, name: error.repositoryPath, path: error.repositoryPath, remoteUrl: null, provider: "none", worktreeCount: 0, attentionCount: 1, conflictedCount: 0, allocatedBytes: 0, allocationIncomplete: true });
  }
  const repositories = [...summaries.values()].map((repository) => {
    const members = byRepository.get(repository.path) ?? [];
    const unavailable = result.errors.some((error) => error.repositoryPath === repository.path && error.worktreePath === null);
    return {
      ...repository,
      worktreeCount: members.length,
      attentionCount: Math.max(unavailable ? 1 : 0, members.filter((record) => !record.exists || ["brokenLink", "locked", "missing", "prunable"].includes(record.registration.kind)).length),
      conflictedCount: members.filter((record) => record.status.conflicted > 0).length,
      allocatedBytes: members.reduce((total, record) => total + (record.sizeBytes ?? 0), 0),
      allocationIncomplete: unavailable || members.some((record) => !record.isPrimary && (record.sizeIncomplete || record.sizeBytes === null)),
    };
  });
  const refreshedWarningPrefixes = [...successful, ...result.errors.map((error) => error.repositoryPath)].map((path) => `${path}:`);
  for (const record of previous.values()) if (successful.has(record.repositoryPath)) refreshedWarningPrefixes.push(`${record.path}:`);
  for (const path of current?.repositoryPaths ?? []) if (!registered.has(path)) refreshedWarningPrefixes.push(`${path}:`);
  for (const record of current?.worktrees ?? []) if (!registered.has(record.repositoryPath)) refreshedWarningPrefixes.push(`${record.path}:`);
  return {
    scannedAtMs: result.refreshedAtMs,
    repositoryPaths: registeredPaths,
    repositories,
    worktrees,
    totals: computeTotals(worktrees, repositories.length),
    warnings: [
      ...(current?.warnings.filter((warning) => !refreshedWarningPrefixes.some((prefix) => warning.startsWith(prefix))) ?? []),
      ...result.errors.map((error) => `${error.worktreePath ?? error.repositoryPath}: ${error.message}`),
    ],
  };
}

/** Measurement results never replace a concurrently refreshed Git record. */
export function mergeMeasurement(current: ScanResult, measurement: WorktreeMeasurement, expectedId: string): ScanResult {
  const record = current.worktrees.find((item) => item.id === expectedId && item.path === measurement.worktreePath && item.repositoryPath === measurement.repositoryPath && item.exists);
  if (!record) return current;
  const worktrees = current.worktrees.map((item) => item !== record ? item : {
    ...item,
    sizeBytes: measurement.sizeBytes,
    sizeIncomplete: measurement.sizeIncomplete,
    lastActivityAtMs: Math.max(item.lastActivityAtMs ?? 0, measurement.lastActivityAtMs ?? 0) || null,
  });
  const repositories = current.repositories.map((repository) => {
    if (repository.path !== record.repositoryPath) return repository;
    const members = worktrees.filter((item) => item.repositoryPath === repository.path && !item.isPrimary);
    return {
      ...repository,
      allocatedBytes: members.reduce((total, item) => total + (item.sizeBytes ?? 0), 0),
      allocationIncomplete: members.some((item) => item.sizeIncomplete || item.sizeBytes === null),
    };
  });
  return { ...current, worktrees, repositories, totals: computeTotals(worktrees, repositories.length) };
}
