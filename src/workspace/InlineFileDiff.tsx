import { useEffect, useState } from "react";
import { AlertTriangleIcon } from "lucide-react";
import { PatchDiff } from "@pierre/diffs/react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Skeleton } from "@/components/ui/skeleton";
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
import { useDelayedPending } from "./use-delayed-pending";

export function InlineFileDiff({
  machineId,
  repositoryPath,
  worktreePath,
  change,
  cache,
  scrollElement,
  selection,
  onSelectionChange,
}: {
  machineId: string;
  repositoryPath: string;
  worktreePath: string;
  change: FileChange;
  /**
   * Diffs already loaded for the current working-copy snapshot, keyed by path
   * token. The owner replaces the map whenever the snapshot changes, so an
   * entry is never older than the file list it was loaded for.
   */
  cache: Map<string, FileDiff>;
  /** The ancestor that scrolls this diff; large diffs window their rows against it. */
  scrollElement: HTMLElement | null;
  selection: FileCommitSelection;
  onSelectionChange: (selection: FileCommitSelection) => void;
}) {
  const cacheKey = change.path.token;
  const cached = cache.get(cacheKey) ?? null;
  // A load result is only meaningful for the cache (and therefore the
  // snapshot) it was requested under: when the snapshot is refreshed with the
  // same file selected, the previous result must not be shown while the new
  // request is in flight.
  const [loaded, setLoaded] = useState<{ cache: Map<string, FileDiff>; key: string; diff: FileDiff | null; error: string | null } | null>(null);
  const current = loaded?.cache === cache && loaded.key === cacheKey ? loaded : null;
  const diff = cached ?? current?.diff ?? null;
  const error = cached === null ? current?.error ?? null : null;
  const showSkeleton = useDelayedPending(diff === null && error === null);
  const { resolvedTheme } = useTheme();

  useEffect(() => {
    if (cache.has(cacheKey)) return;
    const controller = new AbortController();
    void fetchFileDiff(
      machineId,
      repositoryPath,
      worktreePath,
      change.path,
      controller.signal,
    )
      .then((next) => {
        cache.set(cacheKey, next);
        setLoaded({ cache, key: cacheKey, diff: next, error: null });
      })
      .catch((cause: unknown) => {
        if (!controller.signal.aborted) {
          setLoaded({ cache, key: cacheKey, diff: null, error: cause instanceof Error ? cause.message : String(cause) });
        }
      });
    return () => controller.abort();
  }, [cache, cacheKey, change.path, machineId, repositoryPath, worktreePath]);

  const updateHunkSelection = (target: PatchHunk, lineIndices: readonly number[]) => {
    if (!diff) return;
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
  if (!diff || showSkeleton) {
    // Most diffs arrive in a few tens of milliseconds; showing nothing for
    // that window reads as a plain content swap rather than a flash. Once the
    // skeleton has appeared it stays up for its minimum duration even if the
    // diff lands in the meantime, so it never blinks.
    if (!showSkeleton) return null;
    return (
      <div className="flex flex-col gap-2 p-4" role="status" aria-label="Loading diff">
        <Skeleton className="h-5 w-1/3" />
        <Skeleton className="h-24 w-full" />
        <Skeleton className="h-36 w-full" />
      </div>
    );
  }
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
