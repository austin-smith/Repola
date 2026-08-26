import { useEffect, useState } from "react";
import { AlertTriangleIcon } from "lucide-react";
import { PatchDiff } from "@pierre/diffs/react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Checkbox } from "@/components/ui/checkbox";
import { Skeleton } from "@/components/ui/skeleton";
import { useTheme } from "@/components/theme-provider";
import type { FileChange, FileDiff, PatchHunk } from "../ipc/types";
import type { FileCommitSelection } from "../domain/commit-selection";
import {
  normalizePartialCommitSelection,
  selectableLineIndices,
  selectedLinesForHunk,
} from "../domain/commit-selection";
import { fetchFileDiff } from "../ipc/worktrees";
import { FileDiffFallback } from "./FileDiffFallback";

export function InlineFileDiff({
  machineId,
  repositoryPath,
  worktreePath,
  change,
  selection,
  onSelectionChange,
}: {
  machineId: string;
  repositoryPath: string;
  worktreePath: string;
  change: FileChange;
  selection: FileCommitSelection;
  onSelectionChange: (selection: FileCommitSelection) => void;
}) {
  const [diff, setDiff] = useState<FileDiff | null>(null);
  const [error, setError] = useState<string | null>(null);
  const { resolvedTheme } = useTheme();

  useEffect(() => {
    const controller = new AbortController();
    setDiff(null);
    setError(null);
    void fetchFileDiff(
      machineId,
      repositoryPath,
      worktreePath,
      change.path,
      controller.signal,
    )
      .then(setDiff)
      .catch((cause: unknown) => {
        if (!controller.signal.aborted) {
          setError(cause instanceof Error ? cause.message : String(cause));
        }
      });
    return () => controller.abort();
  }, [change.path, change.path.token, machineId, repositoryPath, worktreePath]);

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
  if (!diff) {
    return (
      <div className="flex flex-col gap-2 p-4">
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
    <div className="flex min-h-full flex-col gap-3 p-3">
      {diff.truncated ? (
        <Alert variant="warning">
          <AlertTriangleIcon aria-hidden="true" />
          <AlertDescription>This file diff exceeded 8 MiB and was truncated for display.</AlertDescription>
        </Alert>
      ) : null}
      {canSelectLines ? (
        <section className="flex flex-col gap-3" aria-label="Changes included in commit">
          {diff.hunks.map((hunk) => {
            const selected = selectedLinesForHunk(selection, hunk);
            const selectable = selectableLineIndices(hunk.patch);
            const allSelected = selected.length === selectable.length;
            return (
              <div key={hunk.index} className="overflow-hidden rounded-md border">
                <div className="flex items-center gap-3 border-b bg-muted/50 px-3 py-2">
                  <Checkbox
                    checked={allSelected}
                    indeterminate={selected.length > 0 && !allSelected}
                    onCheckedChange={(checked) => updateHunkSelection(hunk, checked ? selectable : [])}
                    aria-label={`${allSelected ? "Exclude" : "Include"} hunk from commit`}
                  />
                  <code className="min-w-0 flex-1 truncate font-mono text-xs text-muted-foreground">{hunk.header}</code>
                </div>
                <SelectableHunk
                  patch={hunk.patch}
                  selected={[...selected]}
                  onToggle={(index) => {
                    const next = new Set(selected);
                    if (next.has(index)) next.delete(index);
                    else next.add(index);
                    updateHunkSelection(hunk, [...next].sort((left, right) => left - right));
                  }}
                />
              </div>
            );
          })}
        </section>
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

interface SelectableLine {
  index: number;
  oldLine: number | null;
  newLine: number | null;
  prefix: string;
  content: string;
  selectable: boolean;
}

function SelectableHunk({ patch, selected, onToggle }: { patch: string; selected: number[]; onToggle: (index: number) => void }) {
  const lines = parseSelectableLines(patch);
  const selectedSet = new Set(selected);
  return (
    <div className="overflow-x-auto bg-card font-mono text-xs" role="group" aria-label="Select changed lines">
      {lines.map((line) => (
        <label key={line.index} className={`grid min-w-max grid-cols-[36px_44px_44px_minmax(420px,1fr)] border-b last:border-b-0 ${line.prefix === "+" ? "bg-success/10" : line.prefix === "-" ? "bg-destructive/10" : ""} ${line.selectable ? "cursor-pointer hover:bg-accent" : ""}`}>
          <span className="grid place-items-center border-r bg-muted/40">{line.selectable ? <Checkbox checked={selectedSet.has(line.index)} onCheckedChange={() => onToggle(line.index)} aria-label={`Select ${line.prefix === "+" ? "added" : "deleted"} line ${line.newLine ?? line.oldLine}`} /> : null}</span>
          <span className="border-r px-2 py-1 text-right text-muted-foreground select-none">{line.oldLine}</span>
          <span className="border-r px-2 py-1 text-right text-muted-foreground select-none">{line.newLine}</span>
          <code className="px-2 py-1 whitespace-pre"><span className={line.prefix === "+" ? "text-success" : line.prefix === "-" ? "text-destructive" : "text-muted-foreground"}>{line.prefix}</span>{line.content}</code>
        </label>
      ))}
    </div>
  );
}

function parseSelectableLines(patch: string): SelectableLine[] {
  const hunkOffset = patch.indexOf("@@ ");
  if (hunkOffset < 0) return [];
  const hunk = patch.slice(hunkOffset);
  const headerEnd = hunk.indexOf("\n");
  const match = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(hunk.slice(0, headerEnd));
  if (!match) return [];
  let oldLine = Number(match[1]);
  let newLine = Number(match[2]);
  return hunk.slice(headerEnd + 1).split("\n").filter((line, index, values) => index < values.length - 1 || line !== "").map((line, index) => {
    const prefix = line.slice(0, 1);
    const result: SelectableLine = {
      index,
      oldLine: prefix === "+" ? null : oldLine,
      newLine: prefix === "-" ? null : newLine,
      prefix,
      content: line.slice(1),
      selectable: prefix === "+" || prefix === "-",
    };
    if (prefix !== "+") oldLine += 1;
    if (prefix !== "-") newLine += 1;
    return result;
  });
}
