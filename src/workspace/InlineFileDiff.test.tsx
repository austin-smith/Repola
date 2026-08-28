import { useState } from "react";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ThemeProvider } from "@/components/theme-provider";
import { InlineFileDiff } from "./InlineFileDiff";
import { includeAllChanges, includeNoChanges, selectableLineIndices, type FileCommitSelection } from "../domain/commit-selection";
import type { FileChange, FileDiff, PatchHunk } from "../ipc/types";

const ipc = vi.hoisted(() => ({ fetchFileDiff: vi.fn() }));
vi.mock("../ipc/worktrees", () => ipc);
vi.mock("@pierre/diffs/react", () => ({ PatchDiff: () => <div data-testid="patch-diff" /> }));

const header = "diff --git a/src/a.ts b/src/a.ts\n--- a/src/a.ts\n+++ b/src/a.ts\n";

const hunkOne = [
  "@@ -1,4 +1,5 @@ export function a() {",
  " const one = 1;",
  "-const two = 2;",
  "+const two = 22;",
  "+const three = 3;",
  " return one;",
  " }",
  "",
].join("\n");

const hunkTwo = [
  "@@ -20,3 +21,3 @@",
  " tail();",
  "-end",
  "\\ No newline at end of file",
  "+end;",
  "\\ No newline at end of file",
  "",
].join("\n");

const hunks: PatchHunk[] = [
  { index: 0, header: "@@ -1,4 +1,5 @@ export function a() {", patch: header + hunkOne },
  { index: 1, header: "@@ -20,3 +21,3 @@", patch: header + hunkTwo },
];

const diff: FileDiff = {
  patch: header + hunkOne + hunkTwo,
  truncated: false,
  binary: false,
  submodule: false,
  image: null,
  hunks,
  stagedHunks: [],
  unstagedHunks: hunks,
};

const change: FileChange = {
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
  headMode: null,
  indexMode: null,
  worktreeMode: null,
  modeChange: null,
};

// jsdom performs no layout, so the scroll container that windows the diff
// rows would report a zero-height viewport (the virtualizer reads
// offsetWidth/offsetHeight). Give it a viewport tall enough to hold every row
// so the tests exercise the full list.
const VIEWPORT = { offsetWidth: 800, offsetHeight: 100_000 };

function ScrollHost({ children }: { children: (scrollElement: HTMLDivElement | null) => React.ReactNode }) {
  const [scrollElement, setScrollElement] = useState<HTMLDivElement | null>(null);
  return (
    <div
      ref={(element) => {
        if (element) Object.defineProperties(element, { offsetWidth: { value: VIEWPORT.offsetWidth }, offsetHeight: { value: VIEWPORT.offsetHeight } });
        setScrollElement(element);
      }}
      style={{ overflow: "auto" }}
    >
      {children(scrollElement)}
    </div>
  );
}

function renderDiff(selection: FileCommitSelection, overrides: Partial<FileChange> = {}, cache = new Map<string, FileDiff>()) {
  const onSelectionChange = vi.fn();
  const tree = (diffCache: Map<string, FileDiff>) => (
    <ThemeProvider storageKey="test-theme">
      <ScrollHost>
        {(scrollElement) => (
          <InlineFileDiff
            machineId="local"
            repositoryPath="/tmp/repola"
            worktreePath="/tmp/repola"
            change={{ ...change, ...overrides }}
            cache={diffCache}
            scrollElement={scrollElement}
            selection={selection}
            onSelectionChange={onSelectionChange}
          />
        )}
      </ScrollHost>
    </ThemeProvider>
  );
  const { rerender } = render(tree(cache));
  return { onSelectionChange, rerenderWithCache: (next: Map<string, FileDiff>) => rerender(tree(next)) };
}

const hunkCheckboxes = () => screen.getAllByRole("checkbox", { name: /hunk from commit/ });
const lineCheckboxes = (group: HTMLElement) => within(group).getAllByRole("checkbox", { name: /^Select (added|deleted) line/ });

describe("InlineFileDiff", () => {
  afterEach(cleanup);
  beforeEach(() => {
    ipc.fetchFileDiff.mockReset();
    ipc.fetchFileDiff.mockResolvedValue(diff);
  });

  it("requests the diff for the change and renders one selectable group per hunk", async () => {
    renderDiff(includeAllChanges);
    const groups = await screen.findAllByRole("group", { name: "Select changed lines" });
    expect(groups).toHaveLength(2);
    expect(ipc.fetchFileDiff).toHaveBeenCalledExactlyOnceWith("local", "/tmp/repola", "/tmp/repola", change.path, expect.any(AbortSignal));
    expect(hunkCheckboxes().map((box) => box.getAttribute("aria-checked"))).toEqual(["true", "true"]);
    expect(lineCheckboxes(groups[0]).map((box) => box.getAttribute("aria-label"))).toEqual([
      "Select deleted line 2",
      "Select added line 2",
      "Select added line 3",
    ]);
    expect(within(groups[1]).getAllByText("\\")).toHaveLength(2);
  });

  it("toggling a hunk checkbox selects or clears every selectable line in that hunk only", async () => {
    const { onSelectionChange } = renderDiff(includeNoChanges);
    await screen.findAllByRole("group", { name: "Select changed lines" });
    fireEvent.click(hunkCheckboxes()[0]);
    expect(onSelectionChange).toHaveBeenLastCalledWith({
      kind: "partial",
      hunks: [{ expectedPatch: hunks[0].patch, selectedLineIndices: selectableLineIndices(hunks[0].patch) }],
    });
    cleanup();

    const all = renderDiff(includeAllChanges);
    await screen.findAllByRole("group", { name: "Select changed lines" });
    fireEvent.click(hunkCheckboxes()[1]);
    expect(all.onSelectionChange).toHaveBeenLastCalledWith({
      kind: "partial",
      hunks: [{ expectedPatch: hunks[0].patch, selectedLineIndices: selectableLineIndices(hunks[0].patch) }],
    });
  });

  it("toggling a line checkbox changes only that line and shows the hunk as indeterminate", async () => {
    const partial: FileCommitSelection = {
      kind: "partial",
      hunks: [{ expectedPatch: hunks[0].patch, selectedLineIndices: [1, 2] }],
    };
    const { onSelectionChange } = renderDiff(partial);
    const groups = await screen.findAllByRole("group", { name: "Select changed lines" });
    expect(hunkCheckboxes()[0]).toHaveAttribute("data-indeterminate");
    expect(hunkCheckboxes()[1]).toHaveAttribute("aria-checked", "false");

    fireEvent.click(lineCheckboxes(groups[0])[1]); // deselect "+const two = 22;" (line index 2)
    expect(onSelectionChange).toHaveBeenLastCalledWith({
      kind: "partial",
      hunks: [{ expectedPatch: hunks[0].patch, selectedLineIndices: [1] }],
    });

    fireEvent.click(lineCheckboxes(groups[1])[0]); // select "-end" in the second hunk
    expect(onSelectionChange).toHaveBeenLastCalledWith({
      kind: "partial",
      hunks: [
        { expectedPatch: hunks[0].patch, selectedLineIndices: [1, 2] },
        { expectedPatch: hunks[1].patch, selectedLineIndices: [1] },
      ],
    });
  });

  it("collapses a full selection back to `all` and an empty one to `none`", async () => {
    const partial: FileCommitSelection = {
      kind: "partial",
      hunks: [
        { expectedPatch: hunks[0].patch, selectedLineIndices: [1, 2, 3] },
        { expectedPatch: hunks[1].patch, selectedLineIndices: [1] },
      ],
    };
    const { onSelectionChange } = renderDiff(partial);
    const groups = await screen.findAllByRole("group", { name: "Select changed lines" });
    fireEvent.click(lineCheckboxes(groups[1])[1]);
    expect(onSelectionChange).toHaveBeenLastCalledWith(includeAllChanges);

    cleanup();
    const single = renderDiff({ kind: "partial", hunks: [{ expectedPatch: hunks[1].patch, selectedLineIndices: [1] }] });
    const again = await screen.findAllByRole("group", { name: "Select changed lines" });
    fireEvent.click(lineCheckboxes(again[1])[0]);
    expect(single.onSelectionChange).toHaveBeenLastCalledWith(includeNoChanges);
  });

  it.each([
    ["single hunk", hunks[0].patch],
    ["missing trailing newline markers", hunks[1].patch],
    ["change at end of file without trailing newline", "@@ -1 +1 @@\n-old\n\\ No newline at end of file\n+new\n\\ No newline at end of file"],
    ["context lines only", "@@ -1,2 +1,2 @@\n a\n b\n"],
    ["multi-hunk patch passed verbatim", diff.patch],
  ])("the UI line parser agrees with selectableLineIndices for %s", async (_label, patch) => {
    const single: PatchHunk[] = [{ index: 0, header: patch.split("\n")[0], patch }];
    ipc.fetchFileDiff.mockResolvedValue({ ...diff, patch, hunks: single, unstagedHunks: single });
    const { onSelectionChange } = renderDiff(includeNoChanges);
    const [group] = await screen.findAllByRole("group", { name: "Select changed lines" });
    const expected = selectableLineIndices(patch);

    // Every rendered checkbox reports its own index through onToggle; the set of
    // indices the UI produces must be exactly the set commit-selection accepts.
    const boxes = within(group).queryAllByRole("checkbox", { name: /^Select (added|deleted) line/ });
    expect(boxes).toHaveLength(expected.length);
    const reported: number[] = [];
    for (const box of boxes) {
      onSelectionChange.mockClear();
      fireEvent.click(box);
      const selection = onSelectionChange.mock.calls[0][0] as FileCommitSelection;
      expect(selection.kind).toBe(expected.length === 1 ? "all" : "partial");
      if (selection.kind === "partial") reported.push(...selection.hunks[0].selectedLineIndices);
    }
    if (expected.length !== 1) expect(reported).toEqual(expected);

    // Every "+"/"-" line in the body (and nothing else, e.g. "\ No newline") is selectable.
    const body = patch.slice(patch.indexOf("@@ ")).split("\n").slice(1);
    expect(expected.every((index) => /^[+-]/.test(body[index]))).toBe(true);
    expect(body.filter((line) => /^[+-]/.test(line))).toHaveLength(expected.length);
  });

  it("serves a cached diff synchronously and stores fresh loads in the cache", async () => {
    const cache = new Map<string, FileDiff>([[change.path.token, diff]]);
    renderDiff(includeAllChanges, {}, cache);
    expect(await screen.findAllByRole("group", { name: "Select changed lines" })).toHaveLength(2);
    expect(ipc.fetchFileDiff).not.toHaveBeenCalled();
    cleanup();

    const empty = new Map<string, FileDiff>();
    renderDiff(includeAllChanges, {}, empty);
    await screen.findAllByRole("group", { name: "Select changed lines" });
    expect(empty.get(change.path.token)).toBe(diff);
  });

  it("drops a loaded diff as soon as the snapshot cache it belongs to is replaced", async () => {
    const { rerenderWithCache } = renderDiff(includeAllChanges);
    await screen.findAllByRole("group", { name: "Select changed lines" });

    // A working-copy refresh with the same file selected hands the component a
    // fresh cache; the previous snapshot's hunks must not stay interactive.
    ipc.fetchFileDiff.mockReturnValue(new Promise(() => undefined));
    rerenderWithCache(new Map());
    expect(screen.queryByRole("group", { name: "Select changed lines" })).not.toBeInTheDocument();
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
    expect(ipc.fetchFileDiff).toHaveBeenCalledTimes(2);
  });

  it("keeps a shown skeleton up for its minimum duration even when the diff lands sooner", async () => {
    vi.useFakeTimers();
    try {
      let resolve: (value: FileDiff) => void = () => undefined;
      ipc.fetchFileDiff.mockReturnValue(new Promise<FileDiff>((next) => { resolve = next; }));
      renderDiff(includeAllChanges);
      await act(async () => { await vi.advanceTimersByTimeAsync(150); });
      expect(screen.getByRole("status", { name: "Loading diff" })).toBeInTheDocument();

      // The diff lands 50ms after the skeleton appeared. The minimum display
      // time counts from when the skeleton was shown, so it stays for the
      // remaining 200ms of the 250ms minimum, then the diff takes over.
      await act(async () => { await vi.advanceTimersByTimeAsync(50); });
      await act(async () => { resolve(diff); });
      expect(screen.getByRole("status", { name: "Loading diff" })).toBeInTheDocument();
      expect(screen.queryByRole("group", { name: "Select changed lines" })).not.toBeInTheDocument();

      await act(async () => { await vi.advanceTimersByTimeAsync(199); });
      expect(screen.getByRole("status", { name: "Loading diff" })).toBeInTheDocument();
      await act(async () => { await vi.advanceTimersByTimeAsync(1); });
      expect(screen.queryByRole("status", { name: "Loading diff" })).not.toBeInTheDocument();
      expect(screen.getAllByRole("group", { name: "Select changed lines" })).toHaveLength(2);
    } finally {
      vi.useRealTimers();
    }
  });

  it("only shows the loading skeleton when a diff is slow to arrive", async () => {
    vi.useFakeTimers();
    try {
      ipc.fetchFileDiff.mockReturnValue(new Promise(() => undefined));
      renderDiff(includeAllChanges);
      expect(screen.queryByRole("status", { name: "Loading diff" })).not.toBeInTheDocument();
      await act(async () => { await vi.advanceTimersByTimeAsync(149); });
      expect(screen.queryByRole("status", { name: "Loading diff" })).not.toBeInTheDocument();
      await act(async () => { await vi.advanceTimersByTimeAsync(1); });
      expect(screen.getByRole("status", { name: "Loading diff" })).toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  it("falls back to a read-only diff for renames and truncated patches, and surfaces load errors", async () => {
    renderDiff(includeAllChanges, { kind: "renamed", previousPath: { display: "src/old.ts", token: "src/old.ts" } });
    expect(await screen.findByTestId("patch-diff")).toBeInTheDocument();
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
    cleanup();

    ipc.fetchFileDiff.mockResolvedValue({ ...diff, truncated: true });
    renderDiff(includeAllChanges);
    expect(await screen.findByText(/truncated for display/)).toBeInTheDocument();
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
    cleanup();

    ipc.fetchFileDiff.mockRejectedValue(new Error("fatal: bad object"));
    renderDiff(includeAllChanges);
    expect(await screen.findByRole("alert")).toHaveTextContent("fatal: bad object");
  });
});
