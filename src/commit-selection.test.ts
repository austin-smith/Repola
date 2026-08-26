import { describe, expect, it } from "vitest";
import type { FileChange, PatchHunk } from "./types";
import {
  commitSelectionFor,
  commitSelectionRequest,
  createCommitSelection,
  includeAllChanges,
  includeNoChanges,
  isIncludedInCommit,
  normalizePartialCommitSelection,
  reconcileCommitSelection,
  selectedLinesForHunk,
  setChangesIncluded,
} from "./commit-selection";

const change = (id: string, ignored = false): FileChange => ({
  id,
  path: { display: `${id}.txt`, token: id },
  previousPath: null,
  kind: "modified",
  indexStatus: ".",
  worktreeStatus: "M",
  staged: false,
  unstaged: true,
  conflicted: false,
  untracked: false,
  ignored,
  submodule: false,
  headMode: "100644",
  indexMode: "100644",
  worktreeMode: "100644",
  modeChange: null,
});

const hunk = (patch: string, index = 0): PatchHunk => ({ index, header: "@@ -1 +1 @@", patch });

describe("commit inclusion", () => {
  it("includes every non-ignored change by default", () => {
    const selections = createCommitSelection([change("a"), change("ignored", true), change("b")]);

    expect([...selections.keys()]).toEqual(["a", "b"]);
    expect(commitSelectionFor(selections, "a")).toBe(includeAllChanges);
    expect(isIncludedInCommit(commitSelectionFor(selections, "b"))).toBe(true);
  });

  it("preserves existing choices and includes newly detected changes", () => {
    const current = setChangesIncluded(createCommitSelection([change("a")]), new Set(["a"]), false);
    const reconciled = reconcileCommitSelection([change("a"), change("b")], current);

    expect(commitSelectionFor(reconciled, "a")).toBe(includeNoChanges);
    expect(commitSelectionFor(reconciled, "b")).toBe(includeAllChanges);
  });

  it("includes or excludes a highlighted set without changing other files", () => {
    const current = createCommitSelection([change("a"), change("b"), change("c")]);
    const next = setChangesIncluded(current, new Set(["a", "c"]), false);

    expect(commitSelectionFor(next, "a")).toBe(includeNoChanges);
    expect(commitSelectionFor(next, "b")).toBe(includeAllChanges);
    expect(commitSelectionFor(next, "c")).toBe(includeNoChanges);
  });

  it("normalizes line choices to all, none, or an exact partial selection", () => {
    const first = hunk("@@ -1,2 +1,2 @@\n-old\n+new\n context\n", 0);
    const second = hunk("@@ -5 +5 @@\n-before\n+after\n", 1);

    expect(normalizePartialCommitSelection([first, second], [
      { expectedPatch: first.patch, selectedLineIndices: [0, 1] },
      { expectedPatch: second.patch, selectedLineIndices: [0, 1] },
    ])).toBe(includeAllChanges);
    expect(normalizePartialCommitSelection([first, second], [])).toBe(includeNoChanges);

    const partial = normalizePartialCommitSelection([first, second], [
      { expectedPatch: first.patch, selectedLineIndices: [1] },
    ]);
    expect(partial).toEqual({
      kind: "partial",
      hunks: [{ expectedPatch: first.patch, selectedLineIndices: [1] }],
    });
    expect(selectedLinesForHunk(partial, first)).toEqual([1]);
    expect(selectedLinesForHunk(partial, second)).toEqual([]);
  });

  it("serializes only included files with reviewed status and exact partial lines", () => {
    const changes = [change("whole"), change("partial"), change("excluded")];
    const partialPatch = "@@ -1 +1 @@\n-before\n+after\n";
    const selections = new Map(createCommitSelection(changes));
    selections.set("partial", {
      kind: "partial",
      hunks: [{ expectedPatch: partialPatch, selectedLineIndices: [1] }],
    });
    selections.set("excluded", includeNoChanges);

    expect(commitSelectionRequest(changes, selections)).toEqual([
      {
        path: changes[0].path,
        previousPath: null,
        expectedIndexStatus: ".",
        expectedWorktreeStatus: "M",
        includeAll: true,
        hunks: [],
      },
      {
        path: changes[1].path,
        previousPath: null,
        expectedIndexStatus: ".",
        expectedWorktreeStatus: "M",
        includeAll: false,
        hunks: [{ expectedPatch: partialPatch, selectedLineIndices: [1] }],
      },
    ]);
  });
});
