import { describe, expect, it } from "vitest";
import type { DiscardPlanEntry, FileChange, RecoveryPoint, RecoveryRestoreEntry } from "../ipc/types";
import {
  defaultDiscardScope,
  discardEffectLabel,
  fileDiscardBlocker,
  isLargeRecoveryPoint,
  LARGE_RECOVERY_POINT_BYTES,
  partitionRecoveryPoints,
  recoveryLocation,
  restoreChanges,
  restoreEffectLabel,
  unlistedPathCount,
} from "./discard";

function change(overrides: Partial<FileChange> = {}): FileChange {
  return {
    id: "a.txt",
    path: { display: "a.txt", token: "612e747874" },
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
    ...overrides,
  };
}

function point(overrides: Partial<RecoveryPoint> = {}): RecoveryPoint {
  return {
    id: "refs/repola/discarded/20261003T142501Z-abc",
    oid: "0".repeat(40),
    kind: "discardFile",
    summary: "Discarded changes to a.txt",
    createdAt: "2026-10-03T14:25:01.123Z",
    worktreePath: "/work/main",
    head: null,
    pathCount: 1,
    paths: [{ display: "a.txt", token: "612e747874" }],
    storedBytes: 10,
    ...overrides,
  };
}

describe("discard rules", () => {
  it("blocks exactly the files the engine refuses to discard on their own", () => {
    expect(fileDiscardBlocker(change())).toBeNull();
    expect(fileDiscardBlocker(change({ untracked: true, kind: "untracked" }))).toBeNull();
    expect(fileDiscardBlocker(change({ conflicted: true }))).toMatch(/conflict/);
    expect(fileDiscardBlocker(change({ submodule: true }))).toMatch(/submodules/);
    expect(fileDiscardBlocker(change({ indexMode: "160000" }))).toMatch(/submodules/);
    expect(fileDiscardBlocker(change({ untracked: true, path: { display: "vendor/", token: "76656e646f722f" } }))).toMatch(/nested repositories/);
  });

  it("offers unstaged edits before staged ones", () => {
    expect(defaultDiscardScope(change())).toBe("unstaged");
    expect(defaultDiscardScope(change({ untracked: true }))).toBe("unstaged");
    expect(defaultDiscardScope(change({ staged: true, unstaged: false }))).toBe("all");
  });

  it("describes each effect in terms of what happens to the file", () => {
    const entry = (overrides: Partial<DiscardPlanEntry>): DiscardPlanEntry => ({
      path: { display: "b.txt", token: "622e747874" },
      effect: "restoreCommitted",
      onDisk: true,
      tracked: true,
      ...overrides,
    });
    expect(discardEffectLabel(entry({}))).toBe("Restore the committed version");
    expect(discardEffectLabel(entry({ onDisk: false }))).toBe("Restore the deleted file");
    expect(discardEffectLabel(entry({ effect: "restoreStaged" }))).toBe("Replace unstaged edits with the staged version");
    expect(discardEffectLabel(entry({ effect: "restoreStaged", onDisk: false }))).toBe("Restore the staged version of the deleted file");
    expect(discardEffectLabel(entry({ effect: "remove", tracked: false }))).toBe("Delete the untracked file");
    expect(discardEffectLabel(entry({ effect: "remove" }))).toBe("Remove the new file");
    expect(discardEffectLabel(entry({ effect: "unstage", onDisk: false }))).toBe("Unstage the new file, which is already deleted");
    expect(discardEffectLabel(entry({ effect: "restoreCommittedStaged", onDisk: false }))).toBe("Restore the committed version in the index; sparse checkout keeps the file off disk");
  });

  it("warns only above the large recovery point threshold", () => {
    expect(isLargeRecoveryPoint(LARGE_RECOVERY_POINT_BYTES)).toBe(false);
    expect(isLargeRecoveryPoint(LARGE_RECOVERY_POINT_BYTES + 1)).toBe(true);
  });

  it("names where recovery points are stored for the owning machine", () => {
    expect(recoveryLocation("local")).toBe("in this repository on this computer");
    expect(recoveryLocation("ssh")).toBe("in this repository on the remote machine");
  });
});

describe("recovery points", () => {
  it("counts only entries a restore would change", () => {
    const entries: RecoveryRestoreEntry[] = [
      { path: { display: "a", token: "61" }, worktree: "unchanged", indexChanges: false },
      { path: { display: "b", token: "62" }, worktree: "unchanged", indexChanges: true },
      { path: { display: "c", token: "63" }, worktree: "create", indexChanges: false },
      { path: { display: "d", token: "64" }, worktree: "replace", indexChanges: true },
      { path: { display: "e", token: "65" }, worktree: "createFolder", indexChanges: false },
    ];
    expect(restoreChanges(entries).map((entry) => entry.path.display)).toEqual(["b", "c", "d", "e"]);
    expect(entries.map(restoreEffectLabel)).toEqual([
      "Already matches",
      "Restore the saved staged state",
      "Recreate the saved file",
      "Replace the current content and the saved staged state",
      "Recreate the saved folder",
    ]);
  });

  it("separates this worktree's recovery points from its siblings' and keeps their order", () => {
    const first = point({ id: "1" });
    const sibling = point({ id: "2", worktreePath: "/work/feature" });
    const last = point({ id: "3" });
    expect(partitionRecoveryPoints([first, sibling, last], "/work/main")).toEqual({ here: [first, last], elsewhere: [sibling] });
  });

  it("reports saved paths beyond the listed sample", () => {
    expect(unlistedPathCount(point({ pathCount: 25, paths: Array.from({ length: 20 }, (_, index) => ({ display: `${index}`, token: "" })) }))).toBe(5);
    expect(unlistedPathCount(point())).toBe(0);
  });
});
