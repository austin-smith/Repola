import { describe, expect, it } from "vitest";
import { actionForWorktree, computeTotals, filterAndSortWorktrees, isRemovable, matchesState } from "./inventory";
import type { WorktreeRecord } from "../ipc/types";

function worktree(overrides: Partial<WorktreeRecord> = {}): WorktreeRecord {
  return {
    id: "repo:/tmp/linked",
    repositoryName: "repo",
    repositoryPath: "/tmp/repo",
    path: "/tmp/linked",
    branch: "old-work",
    head: "1234567890abcdef",
    detached: false,
    isPrimary: false,
    exists: true,
    createdAtMs: 1,
    headCommitAtMs: 2,
    lastActivityAtMs: 3,
    headSubject: "fixture",
    unpushedCommitCount: 0,
    sizeBytes: 1024,
    sizeIncomplete: false,
    origin: { kind: "unattributed", id: "linked", label: "Linked" },
    status: { available: true, total: 0, staged: 0, unstaged: 0, untracked: 0, conflicted: 0 },
    registration: { kind: "healthy", reason: null },
    integration: { kind: "headContained", target: "main", summary: "HEAD is contained by main." },
    safety: { level: "review", label: "Clean — review", reasons: [] },
    ...overrides,
  };
}

describe("inventory review logic", () => {
  it("sorts and filters on actual last activity", () => {
    const now = 100 * 86_400_000;
    const recentlyTouched = worktree({ id: "recent", createdAtMs: 0, lastActivityAtMs: now - 2 * 86_400_000 });
    const idle = worktree({ id: "idle", createdAtMs: now, lastActivityAtMs: now - 90 * 86_400_000 });
    expect(filterAndSortWorktrees([recentlyTouched, idle], { age: 30, query: "", repositoryPath: "all", state: "all" }, now).map((item) => item.id)).toEqual(["idle"]);
  });

  it("does not call primary or broken registrations clean", () => {
    expect(matchesState(worktree({ isPrimary: true }), "clean")).toBe(false);
    expect(matchesState(worktree({ registration: { kind: "brokenLink", reason: null } }), "clean")).toBe(false);
  });

  it("never offers metadata pruning while a registration is locked", () => {
    const action = actionForWorktree(worktree({ registration: { kind: "locked", reason: "agent" } }));
    expect(action?.kind).toBe("unlock");
  });

  it("uses repository paths to distinguish same-name repositories", () => {
    const first = worktree({ id: "first", repositoryPath: "/one/repo" });
    const second = worktree({ id: "second", repositoryPath: "/two/repo" });
    expect(filterAndSortWorktrees([first, second], { age: 0, query: "", repositoryPath: "/two/repo", state: "all" }, 10).map((item) => item.id)).toEqual(["second"]);
  });

  it("searches self-described agent metadata without knowing agent IDs", () => {
    const agentWorktree = worktree({
      origin: { kind: "agent", id: "future-agent", label: "Future Agent" },
    });
    expect(filterAndSortWorktrees([agentWorktree], { age: 0, query: "future-agent", repositoryPath: "all", state: "all" }, 10)).toHaveLength(1);
  });

  it("only marks clean healthy linked worktrees as removable", () => {
    expect(isRemovable(worktree())).toBe(true);
    expect(isRemovable(worktree({ isPrimary: true }))).toBe(false);
    expect(isRemovable(worktree({ detached: true }))).toBe(false);
    expect(isRemovable(worktree({ status: { available: true, total: 2, staged: 1, unstaged: 1, untracked: 0, conflicted: 0 } }))).toBe(false);
    expect(isRemovable(worktree({ registration: { kind: "locked", reason: null } }))).toBe(false);
  });

  it("computes partial-scan totals the way the backend does", () => {
    const totals = computeTotals([
      worktree({ id: "primary", isPrimary: true }),
      worktree({ id: "clean", sizeBytes: 100 }),
      worktree({ id: "dirty", sizeBytes: 50, status: { available: true, total: 1, staged: 0, unstaged: 1, untracked: 0, conflicted: 0 } }),
      worktree({ id: "missing", exists: false, sizeBytes: null, registration: { kind: "missing", reason: null } }),
    ], 2);
    expect(totals).toEqual({
      repositoryCount: 2,
      primaryCount: 1,
      linkedCount: 3,
      existingLinkedCount: 2,
      cleanCount: 2,
      dirtyCount: 1,
      missingCount: 1,
      prunableCount: 0,
      brokenLinkCount: 0,
      linkedSizeBytes: 150,
    });
  });
});
