import { describe, expect, it } from "vitest";
import type { RefreshResult, RepositorySummary, ScanResult, WorktreeRecord } from "../ipc/types";
import { computeTotals } from "./inventory";
import { mergeMeasurement, mergeRefresh } from "./refresh-merge";

function record(path = "linked", repositoryPath = "repo"): WorktreeRecord {
  return {
    id: `${repositoryPath}:${path}`, repositoryName: repositoryPath, repositoryPath, path,
    branch: "branch", head: "head", detached: false, isPrimary: false, exists: true,
    createdAtMs: 1, headCommitAtMs: 2, lastActivityAtMs: 5, headSubject: "commit",
    unpushedCommitCount: 0, sizeBytes: 100, sizeIncomplete: false,
    origin: { kind: "unattributed", id: path, label: path },
    status: { available: true, total: 0, staged: 0, unstaged: 0, untracked: 0, conflicted: 0 },
    registration: { kind: "healthy", reason: null },
    integration: { kind: "headContained", target: "main", summary: "Contained" },
    safety: { level: "review", label: "Review", reasons: [] },
  };
}
function repository(path = "repo"): RepositorySummary {
  return { id: path, name: path, path, remoteUrl: null, provider: "none", worktreeCount: 1, attentionCount: 0, conflictedCount: 0, allocatedBytes: 100, allocationIncomplete: false };
}
function scan(worktrees = [record()]): ScanResult {
  const paths = [...new Set(worktrees.map((item) => item.repositoryPath))];
  return { scannedAtMs: 1, repositoryPaths: paths, repositories: paths.map(repository), worktrees, totals: computeTotals(worktrees, paths.length), warnings: [] };
}
function result(overrides: Partial<RefreshResult> = {}): RefreshResult {
  return { refreshedAtMs: 10, repositories: [repository()], memberships: [record()], worktrees: [], successfulRepositoryPaths: ["repo"], errors: [], gitSignatures: [], workingCopy: null, ...overrides };
}

describe("refresh publication", () => {
  it("retains unrelated repositories and uninspected state, and preserves measured fields", () => {
    const current = scan([record(), record("sibling"), record("other", "second")]);
    const membership = { ...record("sibling"), status: { ...record().status, available: false }, sizeBytes: null };
    const inspected = { ...record(), status: { ...record().status, total: 1, unstaged: 1 }, sizeBytes: null, lastActivityAtMs: 2 };
    const next = mergeRefresh(current, result({ memberships: [record(), membership], worktrees: [inspected] }), ["repo", "second"]);
    expect(next.worktrees.find((item) => item.path === "other")).toBe(current.worktrees[2]);
    expect(next.worktrees.find((item) => item.path === "sibling")).toBe(current.worktrees[1]);
    expect(next.worktrees.find((item) => item.path === "linked")).toMatchObject({ sizeBytes: 100, lastActivityAtMs: 5, status: { total: 1 } });
    expect(next.totals).toMatchObject({ dirtyCount: 1, linkedSizeBytes: 300 });
    expect(next.repositories.find((item) => item.path === "repo")?.allocatedBytes).toBe(200);
  });

  it("removes only confirmed missing membership and keeps an inaccessible scope protected", () => {
    const current = scan([record(), record("other", "second")]);
    const next = mergeRefresh(current, result({ memberships: [], errors: [{ repositoryPath: "second", worktreePath: null, message: "offline" }] }), ["repo", "second"]);
    expect(next.worktrees).toHaveLength(1);
    expect(next.worktrees[0]).toMatchObject({ repositoryPath: "second", sizeBytes: 100, status: { available: false }, safety: { level: "protected" } });
    expect(next.repositories.map((item) => item.path)).toEqual(["repo", "second"]);
    expect(next.warnings).toEqual(["second: offline"]);
  });

  it("keeps a newly inaccessible registered repository visible and prunes unregistered scopes", () => {
    const next = mergeRefresh(scan([record("old", "removed")]), result({ repositories: [], memberships: [], successfulRepositoryPaths: [], errors: [{ repositoryPath: "repo", worktreePath: null, message: "missing" }] }), ["repo"]);
    expect(next.worktrees).toEqual([]);
    expect(next.repositories.map((item) => item.path)).toEqual(["repo"]);
    expect(next.repositories[0]).toMatchObject({ attentionCount: 1, allocationIncomplete: true });
    expect(next.warnings).toEqual(["repo: missing"]);
  });

  it("reuses unchanged records so background checks do not invalidate consumers", () => {
    const current = scan();
    expect(mergeRefresh(current, result({ worktrees: [record()] }), ["repo"]).worktrees[0]).toBe(current.worktrees[0]);
  });

  it("protects changed membership until inspection and does not mark healthy primary rows as attention", () => {
    const primary = { ...record("primary"), isPrimary: true, registration: { kind: "primary" as const, reason: null } };
    const current = scan([primary, record()]);
    const changed = { ...record(), head: "new-head", status: { ...record().status, available: false }, safety: { level: "protected" as const, label: "Pending", reasons: [] }, integration: { kind: "unknown" as const, target: null, summary: "Pending" } };
    const next = mergeRefresh(current, result({ memberships: [primary, changed] }), ["repo"]);
    expect(next.repositories[0].attentionCount).toBe(0);
    expect(next.worktrees.find((item) => item.path === "linked")).toMatchObject({ head: "new-head", status: { available: false }, safety: { level: "protected" }, headSubject: null, unpushedCommitCount: null });
  });

  it("removes stale warnings when a repository is unregistered", () => {
    const current = { ...scan([record("old", "removed")]), warnings: ["removed: offline", "old: unreadable"] };
    const next = mergeRefresh(current, result(), ["repo"]);
    expect(next.warnings).toEqual([]);
  });

  it("applies late measurement only to the current identity without overwriting Git data", () => {
    const current = scan([{ ...record(), branch: "new-branch", head: "new-head" }]);
    const measured = { repositoryPath: "repo", worktreePath: "linked", sizeBytes: 200, sizeIncomplete: true, lastActivityAtMs: 20 };
    const next = mergeMeasurement(current, measured, current.worktrees[0].id);
    expect(next.worktrees[0]).toMatchObject({ branch: "new-branch", head: "new-head", sizeBytes: 200, lastActivityAtMs: 20 });
    expect(next.totals.linkedSizeBytes).toBe(200);
    expect(next.repositories[0]).toMatchObject({ allocatedBytes: 200, allocationIncomplete: true });
    expect(mergeMeasurement(current, measured, "obsolete-identity")).toBe(current);
    expect(mergeMeasurement(scan([]), measured, current.worktrees[0].id).worktrees).toEqual([]);
  });
});
