import { useEffect, useState } from "react";
import { AlertTriangleIcon } from "lucide-react";
import { PatchDiff } from "@pierre/diffs/react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Skeleton } from "@/components/ui/skeleton";
import { useTheme } from "@/components/theme-provider";
import type { WorktreeChanges, WorktreeRecord } from "./types";
import { useShortPath } from "./environment";
import { fetchWorktreeChanges } from "./worktrees";

interface DiffDialogProps {
  machineId: string;
  worktree: WorktreeRecord;
  onClose: () => void;
}

export function DiffDialog({ machineId, worktree, onClose }: DiffDialogProps) {
  const shortPath = useShortPath();
  const [changes, setChanges] = useState<WorktreeChanges | null>(null);
  const [error, setError] = useState<string | null>(null);
  const { resolvedTheme } = useTheme();

  useEffect(() => {
    const controller = new AbortController();
    fetchWorktreeChanges(machineId, worktree.repositoryPath, worktree.path, controller.signal)
      .then((next) => {
        setChanges(next);
      })
      .catch((cause: unknown) => {
        if (!controller.signal.aborted) setError(cause instanceof Error ? cause.message : String(cause));
      });
    return () => {
      controller.abort();
    };
  }, [machineId, worktree.id, worktree.path, worktree.repositoryPath]);

  const unavailableReason = error ?? (changes && !changes.available ? changes.reason ?? "The changes could not be read." : null);
  const empty = changes?.available && changes.patch === "" && changes.untracked.length === 0;

  return (
    <Dialog open onOpenChange={(open) => { if (!open) onClose(); }}>
      <DialogContent className="flex max-h-[88vh] flex-col sm:max-w-4xl">
        <DialogHeader>
          <span className="text-xs font-medium tracking-widest text-muted-foreground uppercase">Read-only review · {worktree.repositoryName}</span>
          <DialogTitle>{worktree.branch ?? "Detached HEAD"}</DialogTitle>
          <DialogDescription className="font-mono text-xs break-all">{shortPath(worktree.path)}</DialogDescription>
        </DialogHeader>

        <div className="flex min-h-0 flex-col gap-3 overflow-y-auto">
          {!changes && !error && (
            <div className="flex flex-col gap-2">
              <Skeleton className="h-4 w-1/3" />
              <Skeleton className="h-24 w-full" />
              <Skeleton className="h-24 w-full" />
            </div>
          )}
          {unavailableReason && (
            <Alert variant="destructive" role="alert">
              <AlertTriangleIcon aria-hidden="true" />
              <AlertDescription>{unavailableReason}</AlertDescription>
            </Alert>
          )}
          {empty && (
            <p className="text-sm text-muted-foreground">No tracked changes and no untracked files. The worktree may have changed since the last scan.</p>
          )}
          {changes?.truncated && (
            <Alert variant="warning">
              <AlertTriangleIcon aria-hidden="true" />
              <AlertDescription>The patch was truncated for display. Review the full diff in a terminal before acting on it.</AlertDescription>
            </Alert>
          )}
          {changes?.available && changes.patch !== "" && (
            <PatchDiff
              patch={changes.patch}
              disableWorkerPool
              options={{ theme: { light: "github-light", dark: "github-dark" }, themeType: resolvedTheme }}
            />
          )}
          {changes && changes.untracked.length > 0 && (
            <section className="flex flex-col gap-2">
              <h3 className="text-xs font-medium tracking-widest text-muted-foreground uppercase">
                {changes.untracked.length} untracked file{changes.untracked.length === 1 ? "" : "s"}
              </h3>
              <div className="flex flex-col gap-1 border bg-card p-2">
                {changes.untracked.map((path) => (
                  <code key={path} className="font-mono text-xs break-all text-muted-foreground">{path}</code>
                ))}
              </div>
            </section>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}
