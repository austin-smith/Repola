import { useEffect, useState } from "react";
import { AlertTriangleIcon } from "lucide-react";
import { PatchDiff } from "@pierre/diffs/react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { useTheme } from "@/components/theme-provider";
import type { FileChange, FileDiff, PatchHunk } from "../ipc/types";
import type { FileCommitSelection } from "../domain/commit-selection";
import {
  normalizePartialCommitSelection,
  selectedLinesForHunk,
} from "../domain/commit-selection";
import { fetchFileDiff } from "../ipc/worktrees";
import { FileDiffFallback } from "./FileDiffFallback";
import { SelectableHunkList } from "./SelectableHunkList";

export function InlineFileDiff({
  machineId,
  repositoryPath,
  worktreePath,
  change,
  diffKey,
  cache,
  scrollElement,
  selection,
  selectionDisabled = false,
  onSelectionChange,
}: {
  machineId: string;
  repositoryPath: string;
  worktreePath: string;
  change: FileChange;
  /**
   * Content identity of this change's diff (see `changeDiffKey`). The cache
   * is keyed by it, so a hit proves the stored diff still matches the
   * working copy and a refresh that changed nothing renders nothing new.
   */
  diffKey: string;
  /**
   * Diffs the owner carries across snapshot refreshes, keyed by content
   * identity. The owner prunes entries whose identity the current snapshot
   * no longer produces.
   */
  cache: Map<string, FileDiff>;
  /** The ancestor that scrolls this diff; large diffs window their rows against it. */
  scrollElement: HTMLElement | null;
  selection: FileCommitSelection;
  selectionDisabled?: boolean;
  onSelectionChange: (selection: FileCommitSelection) => void;
}) {
  const cached = cache.get(diffKey) ?? null;
  // The last completed load. When its key matches it is current (and carries
  // any load error). When only its file matches, it is the previous version
  // of the same file, kept on screen while the fresh one is in flight so a
  // refresh swaps content in place instead of blanking the pane. Acting on a
  // stale diff is safe: every mutation re-validates its expected patch
  // against the live working copy before touching anything.
  const [loaded, setLoaded] = useState<{ key: string; token: string; diff: FileDiff | null; error: string | null } | null>(null);
  const fresh = cached ?? (loaded?.key === diffKey ? loaded.diff : null);
  const diff = fresh ?? (loaded?.token === change.path.token ? loaded.diff : null);
  const error = fresh === null && loaded?.key === diffKey ? loaded.error : null;
  const { resolvedTheme } = useTheme();

  useEffect(() => {
    if (cache.has(diffKey)) return;
    const controller = new AbortController();
    void fetchFileDiff(
      machineId,
      repositoryPath,
      worktreePath,
      change.path,
      controller.signal,
    )
      .then((next) => {
        cache.set(diffKey, next);
        if (!controller.signal.aborted) {
          setLoaded({ key: diffKey, token: change.path.token, diff: next, error: null });
        }
      })
      .catch((cause: unknown) => {
        if (!controller.signal.aborted) {
          setLoaded({ key: diffKey, token: change.path.token, diff: null, error: cause instanceof Error ? cause.message : String(cause) });
        }
      });
    return () => controller.abort();
  }, [cache, change.path, diffKey, machineId, repositoryPath, worktreePath]);

  const updateHunkSelection = (target: PatchHunk, lineIndices: readonly number[]) => {
    if (!diff || selectionDisabled) return;
    const selections = diff.hunks.flatMap((hunk) => {
      const selected = hunk.patch === target.patch
        ? lineIndices
        : selectedLinesForHunk(selection, hunk);
      return selected.length > 0
        ? [{ expectedPatch: hunk.patch, selectedLineIndices: [...selected] }]
        : [];
    });
    onSelectionChange(normalizePartialCommitSelection(diff.hunks, selections));
  };

  if (error) {
    return (
      <Alert variant="destructive" className="m-4" role="alert">
        <AlertTriangleIcon aria-hidden="true" />
        <AlertDescription>{error}</AlertDescription>
      </Alert>
    );
  }
  // While a first load is in flight the pane stays empty; refreshes of an
  // already-shown file keep the previous diff up instead, so this only ever
  // reads as a plain content swap, never a flash.
  if (!diff) return null;
  if (diff.binary || diff.submodule) return <FileDiffFallback diff={diff} />;
  if (diff.patch === "") {
    return (
      <div className="grid min-h-full place-items-center p-8 text-center">
        <div>
          <strong className="text-sm">No textual lines to display</strong>
          <p className="mt-1 text-sm text-muted-foreground">The selected path may be an empty new file or have changed since the working copy refreshed.</p>
        </div>
      </div>
    );
  }
  const canSelectLines = !diff.truncated
    && diff.hunks.length > 0
    && change.kind !== "renamed"
    && change.kind !== "copied";
  return (
    <div className="flex min-h-full flex-col">
      {diff.truncated ? (
        <Alert variant="warning" className="m-3">
          <AlertTriangleIcon aria-hidden="true" />
          <AlertDescription>This file diff exceeded 8 MiB and was truncated for display.</AlertDescription>
        </Alert>
      ) : null}
      {canSelectLines ? (
        <SelectableHunkList
          hunks={diff.hunks}
          selection={selection}
          disabled={selectionDisabled}
          scrollElement={scrollElement}
          onHunkSelectionChange={updateHunkSelection}
        />
      ) : (
        <PatchDiff
          patch={diff.patch}
          disableWorkerPool
          options={{ theme: { light: "github-light", dark: "github-dark" }, themeType: resolvedTheme }}
        />
      )}
    </div>
  );
}
