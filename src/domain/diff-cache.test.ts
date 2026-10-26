import { describe, expect, it } from "vitest";
import { changeDiffKey, exactDiffDisplay, retainDiffEntries, workingCopySnapshotsEqual } from "./diff-cache";
import type { FileChange, FileDiff, WorkingCopySnapshot } from "../ipc/types";

function change(overrides: Partial<FileChange> = {}): FileChange {
  return {
    id: "src/a.ts",
    path: { display: "src/a.ts", token: "src/a.ts" },
    previousPath: null,
    kind: "modified",
    indexStatus: ".",
    worktreeStatus: "M",
    staged: false,
    unstaged: true,
    conflicted: false,
    untracked: false,
    ignored: false,
    submodule: false,
    headMode: "100644",
    indexMode: "100644",
    worktreeMode: "100644",
    modeChange: null,
    headOid: "abc",
    indexOid: "def",
    worktreeStamp: "f:12:1700000000.000000001",
    ...overrides,
  };
}

function snapshot(changes: FileChange[]): WorkingCopySnapshot {
  return {
    repositoryPath: "/tmp/repola",
    worktreePath: "/tmp/repola",
    head: "abc123",
    branch: "main",
    upstream: null,
    upstreamHead: null,
    remote: null,
    ahead: 0,
    behind: 0,
    changes,
    operation: null,
  };
}

const diff: FileDiff = {
  patch: "@@ -1 +1 @@\n-a\n+b\n",
  truncated: false,
  binary: false,
  submodule: false,
  image: null,
  hunks: [],
  stagedHunks: [],
  unstagedHunks: [],
};

describe("changeDiffKey", () => {
  it("is stable across snapshot generations when every content signal agrees", () => {
    expect(changeDiffKey(change(), 1, exactDiffDisplay)).toBe(changeDiffKey(change(), 2, exactDiffDisplay));
  });

  it.each([
    ["worktree stamp", { worktreeStamp: "f:13:1700000009.000000001" }],
    ["index object id", { indexOid: "0ther" }],
    ["head object id", { headOid: "0ther" }],
    ["status", { indexStatus: "M" }],
    ["kind", { kind: "deleted" as const }],
    ["mode", { worktreeMode: "100755" }],
    ["path", { path: { display: "src/b.ts", token: "src/b.ts" } }],
  ])("changes when the %s changes", (_label, overrides) => {
    expect(changeDiffKey(change(overrides), 1, exactDiffDisplay)).not.toBe(changeDiffKey(change(), 1, exactDiffDisplay));
  });

  it("separates whitespace-hidden diffs from exact diffs", () => {
    const hidden = { ignoreWhitespace: true };
    expect(changeDiffKey(change(), 1, hidden)).toBe(changeDiffKey(change(), 2, hidden));
    expect(changeDiffKey(change(), 1, hidden)).not.toBe(changeDiffKey(change(), 1, exactDiffDisplay));
    const unversioned = change({ worktreeStamp: undefined });
    expect(changeDiffKey(unversioned, 1, hidden)).not.toBe(changeDiffKey(unversioned, 1, exactDiffDisplay));
  });

  it("never matches across generations without a stamp, because unversioned content cannot be proven fresh", () => {
    const unversioned = change({ worktreeStamp: undefined });
    expect(changeDiffKey(unversioned, 1, exactDiffDisplay)).toBe(changeDiffKey(unversioned, 1, exactDiffDisplay));
    expect(changeDiffKey(unversioned, 1, exactDiffDisplay)).not.toBe(changeDiffKey(unversioned, 2, exactDiffDisplay));
  });
});

describe("retainDiffEntries", () => {
  it("keeps entries the new change list still identifies and drops the rest", () => {
    const kept = change();
    const edited = change({ id: "src/b.ts", path: { display: "src/b.ts", token: "src/b.ts" } });
    const cache = new Map([
      [changeDiffKey(kept, 1, exactDiffDisplay), diff],
      [changeDiffKey(edited, 1, exactDiffDisplay), diff],
      ["versioned no-longer-listed", diff],
    ]);
    const editedNow = { ...edited, worktreeStamp: "f:99:1700000099.000000000" };
    const next = retainDiffEntries(cache, [kept, editedNow], 2);
    expect([...next.keys()]).toEqual([changeDiffKey(kept, 2, exactDiffDisplay)]);
    expect(next.get(changeDiffKey(kept, 2, exactDiffDisplay))).toBe(diff);
  });

  it("keeps every display variant of an unchanged file", () => {
    const kept = change();
    const hidden = { ignoreWhitespace: true };
    const hiddenDiff = { ...diff, patch: "" };
    const cache = new Map([
      [changeDiffKey(kept, 1, exactDiffDisplay), diff],
      [changeDiffKey(kept, 1, hidden), hiddenDiff],
    ]);
    const next = retainDiffEntries(cache, [kept], 2);
    expect(next.get(changeDiffKey(kept, 2, exactDiffDisplay))).toBe(diff);
    expect(next.get(changeDiffKey(kept, 2, hidden))).toBe(hiddenDiff);
  });
});

describe("workingCopySnapshotsEqual", () => {
  it("proves two stamped snapshots equal regardless of object identity", () => {
    expect(workingCopySnapshotsEqual(snapshot([change()]), snapshot([change()]))).toBe(true);
  });

  it("detects a content change that status output alone would hide", () => {
    const before = snapshot([change()]);
    const after = snapshot([change({ worktreeStamp: "f:12:1700000009.000000001" })]);
    expect(workingCopySnapshotsEqual(before, after)).toBe(false);
  });

  it("never trusts snapshots without stamps, since identical output can hide an edit", () => {
    const unversioned = snapshot([change({ worktreeStamp: undefined })]);
    expect(workingCopySnapshotsEqual(unversioned, snapshot([change({ worktreeStamp: undefined })]))).toBe(false);
  });

  it("sees header-only changes such as a moved head", () => {
    const moved = { ...snapshot([change()]), head: "def456" };
    expect(workingCopySnapshotsEqual(snapshot([change()]), moved)).toBe(false);
  });
});
