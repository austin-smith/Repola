import { useEffect, useState } from "react";
import { AlertTriangleIcon } from "lucide-react";
import { PatchDiff } from "@pierre/diffs/react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Skeleton } from "@/components/ui/skeleton";
import { useTheme } from "@/components/theme-provider";
import type { CommitChangedFile, FileDiff } from "../ipc/types";
import { fetchCommitFileDiff } from "../ipc/worktrees";
import { FileDiffFallback } from "./FileDiffFallback";
import { useDelayedPending } from "./use-delayed-pending";

export function CommitFileDiffView({ machineId, repositoryPath, worktreePath, commit, file }: {
  machineId: string;
  repositoryPath: string;
  worktreePath: string;
  commit: string;
  file: CommitChangedFile;
}) {
  const [diff, setDiff] = useState<FileDiff | null>(null);
  const [error, setError] = useState<string | null>(null);
  const showSkeleton = useDelayedPending(diff === null && error === null);
  const { resolvedTheme } = useTheme();

  useEffect(() => {
    const controller = new AbortController();
    setDiff(null);
    setError(null);
    void fetchCommitFileDiff(machineId, repositoryPath, worktreePath, commit, file.path, controller.signal)
      .then(setDiff)
      .catch((cause: unknown) => { if (!controller.signal.aborted) setError(cause instanceof Error ? cause.message : String(cause)); });
    return () => controller.abort();
  }, [commit, file.path, file.path.token, machineId, repositoryPath, worktreePath]);

  if (error) return <Alert variant="destructive" className="m-4"><AlertTriangleIcon aria-hidden="true" /><AlertDescription>{error}</AlertDescription></Alert>;
  if (!diff || showSkeleton) return showSkeleton ? <div className="flex flex-col gap-3 p-4" role="status" aria-label="Loading diff"><Skeleton className="h-5 w-1/3" /><Skeleton className="h-40 w-full" /></div> : null;
  if (diff.binary || diff.submodule) return <FileDiffFallback diff={diff} />;
  if (!diff.patch) return <div className="grid h-full place-items-center text-sm text-muted-foreground">No textual lines to display.</div>;
  return (
    <div className="flex min-h-full flex-col gap-3 p-3">
      {diff.truncated ? <Alert variant="warning"><AlertTriangleIcon aria-hidden="true" /><AlertDescription>This commit diff exceeded 8 MiB and was truncated.</AlertDescription></Alert> : null}
      <PatchDiff patch={diff.patch} disableWorkerPool options={{ theme: { light: "github-light", dark: "github-dark" }, themeType: resolvedTheme }} />
    </div>
  );
}
