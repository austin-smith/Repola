import { FileQuestionIcon, GitCommitIcon, ImageIcon } from "lucide-react";
import type { FileDiff } from "../ipc/types";
import { formatBytes } from "../domain/format";

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
    const source = "data:" + diff.image.mimeType + ";base64," + diff.image.base64;
    return (
      <div className="flex min-h-full flex-col items-center justify-center gap-4 p-8">
        <div className="flex items-center gap-2 text-sm"><ImageIcon className="size-4 text-muted-foreground" aria-hidden="true" /><strong>{diff.image.label}</strong><span className="text-muted-foreground">· {formatBytes(diff.image.byteLength)}</span></div>
        <div className="grid max-h-[70vh] max-w-full place-items-center overflow-auto border bg-[repeating-conic-gradient(var(--muted)_0_25%,transparent_0_50%)_50%/16px_16px] p-4">
          <img src={source} alt={diff.image.label + " image preview"} className="max-h-[62vh] max-w-full object-contain" />
        </div>
        <p className="max-w-md text-center text-xs text-muted-foreground">Raster preview is bounded to 4 MiB. Pixel-level before/after comparison is not available yet.</p>
      </div>
    );
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
