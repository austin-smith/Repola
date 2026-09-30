import { FileQuestionIcon, GitCommitIcon } from "lucide-react";
import type { FileDiff } from "../ipc/types";
import { ImageComparisonView } from "./ImageComparisonView";

export function FileDiffFallback({ diff }: { diff: FileDiff }) {
  if (diff.submodule) {
    return (
      <div className="grid min-h-full place-items-center p-8">
        <div className="flex w-full max-w-xl flex-col items-center gap-3 text-center">
          <span className="grid size-12 place-items-center rounded-full bg-muted"><GitCommitIcon className="size-5 text-muted-foreground" aria-hidden="true" /></span>
          <strong className="text-sm">Submodule commit changed</strong>
          <p className="text-sm text-muted-foreground">The working copy points this submodule at a different commit. Its nested file changes belong to the submodule repository.</p>
          {diff.patch ? <pre className="mt-2 w-full overflow-x-auto border bg-card p-3 text-left font-mono text-xs whitespace-pre-wrap">{diff.patch}</pre> : null}
        </div>
      </div>
    );
  }
  if (diff.image) {
    return <ImageComparisonView comparison={diff.image} />;
  }
  return (
    <div className="grid min-h-full place-items-center p-8">
      <div className="flex max-w-md flex-col items-center gap-3 text-center">
        <span className="grid size-12 place-items-center rounded-full bg-muted"><FileQuestionIcon className="size-5 text-muted-foreground" aria-hidden="true" /></span>
        <strong className="text-sm">Binary file changed</strong>
        <p className="text-sm text-muted-foreground">Repola detected binary content. This format has no safe line-by-line or raster preview.</p>
      </div>
    </div>
  );
}
