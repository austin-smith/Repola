import { describe, expect, it } from "vitest";
import { groupWorktreesForPicker, matchesWorktreeQuery, worktreeBranchLabel, worktreeFolderName } from "./worktree-picker";
import type { WorktreeRecord } from "../ipc/types";

function worktree(overrides: Partial<WorktreeRecord> = {}): WorktreeRecord {
  return {
    id: "repo:/tmp/linked",
    repositoryName: "repo",
    repositoryPath: "/tmp/repo",
    path: "/tmp/t3code-30dacb81",
    branch: "t3code/fix-contents-pane-reload",
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

describe("worktree picker", () => {
  it("names worktrees by their folder on every platform", () => {
    expect(worktreeFolderName("/Users/me/.t3/worktrees/repola/t3code-30dacb81")).toBe("t3code-30dacb81");
    expect(worktreeFolderName("C:\\Users\\me\\repola\\")).toBe("repola");
    expect(worktreeFolderName("C:/Users/me/repola")).toBe("repola");
  });

  it("labels detached worktrees by their short HEAD", () => {
    expect(worktreeBranchLabel(worktree({ branch: null, detached: true }))).toBe("Detached at 12345678");
  });

  it("searches both the folder name and the branch, ignoring case", () => {
    const item = worktree();
    expect(matchesWorktreeQuery(item, "30DACB")).toBe(true);
    expect(matchesWorktreeQuery(item, "contents-pane")).toBe(true);
    expect(matchesWorktreeQuery(item, "  ")).toBe(true);
    expect(matchesWorktreeQuery(item, "tmp")).toBe(false);
    expect(matchesWorktreeQuery(worktree({ branch: null }), "detached at 1234")).toBe(true);
  });

  it("groups the main worktree ahead of linked worktrees and drops empty groups", () => {
    const main = worktree({ id: "main", isPrimary: true });
    const linked = worktree({ id: "linked" });
    expect(groupWorktreesForPicker([linked, main]).map((group) => [group.value, group.items.map((item) => item.id)]))
      .toEqual([["Main Worktree", ["main"]], ["Linked Worktrees", ["linked"]]]);
    expect(groupWorktreesForPicker([linked]).map((group) => group.value)).toEqual(["Linked Worktrees"]);
  });
});
