import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { AlertTriangleIcon } from "lucide-react";
import { PatchDiff } from "@pierre/diffs/react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Spinner } from "@/components/ui/spinner";
import { useTheme } from "@/components/theme-provider";
import type { CommitChangedFile, FileDiff } from "../ipc/types";
import { fetchCommitFileDiff } from "../ipc/worktrees";
import { DiffOptionsMenu } from "./DiffOptionsMenu";
import { FileDiffFallback } from "./FileDiffFallback";
import { OnlyWhitespaceChanged, truncatedDiffWhitespaceReason } from "./HiddenWhitespace";
import { useDelayedPending } from "./use-delayed-pending";

export function CommitFileDiffView({ machineId, repositoryPath, worktreePath, commit, file, hideWhitespace, onHideWhitespaceChange, controlsElement }: {
  machineId: string;
  repositoryPath: string;
  worktreePath: string;
  commit: string;
  file: CommitChangedFile;
  /** Whether the diff is shown without whitespace changes (`git diff -w`). */
  hideWhitespace: boolean;
  onHideWhitespaceChange: (hide: boolean) => void;
  /** Where the diff's presentation controls render, in the owner's header. */
  controlsElement: HTMLElement | null;
}) {
  // Loads are keyed by file and display mode. The previous load of the same
  // file stays on screen while its other display mode loads, so toggling
  // whitespace swaps content in place instead of blanking the pane (and
  // unmounting the control that toggled it).
  const identity = `${commit}\u0000${file.path.token}`;
  const [loaded, setLoaded] = useState<{ identity: string; hideWhitespace: boolean; diff: FileDiff | null; error: string | null } | null>(null);
  const current = loaded?.identity === identity ? loaded : null;
  const settled = current?.hideWhitespace === hideWhitespace;
  const diff = current?.diff ?? null;
  const error = settled ? current.error : null;
  const showLoading = useDelayedPending(diff === null && error === null);
  const { resolvedTheme } = useTheme();
  const optionsTrigger = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    const controller = new AbortController();
    const settle = (diff: FileDiff | null, error: string | null) => {
      if (!controller.signal.aborted) setLoaded({ identity, hideWhitespace, diff, error });
    };
    void fetchCommitFileDiff(machineId, repositoryPath, worktreePath, commit, file.path, { ignoreWhitespace: hideWhitespace }, controller.signal)
      .then((next) => settle(next, null))
      .catch((cause: unknown) => settle(null, cause instanceof Error ? cause.message : String(cause)));
    return () => controller.abort();
  }, [commit, file.path, hideWhitespace, identity, machineId, repositoryPath, worktreePath]);

  const renderControls = (whitespaceUnavailable: string | null) => (controlsElement ? createPortal(
    <DiffOptionsMenu
      hideWhitespace={hideWhitespace}
      onHideWhitespaceChange={onHideWhitespaceChange}
      whitespaceUnavailable={whitespaceUnavailable}
      triggerRef={optionsTrigger}
    />,
    controlsElement,
  ) : null);
  const showWhitespace = () => {
    onHideWhitespaceChange(false);
    optionsTrigger.current?.focus();
  };

  if (error) {
    return (
      <>
        {/* A filtered load that failed must still offer the way back. */}
        {hideWhitespace ? renderControls(null) : null}
        <Alert variant="destructive" className="m-4"><AlertTriangleIcon aria-hidden="true" /><AlertDescription>{error}</AlertDescription></Alert>
      </>
    );
  }
  if (!diff || showLoading) return showLoading ? <div className="flex h-full items-center justify-center gap-2 text-sm text-muted-foreground" role="status"><Spinner aria-hidden="true" />Loading diff…</div> : null;
  // Presentation controls do not apply to binary, image, or submodule
  // fallbacks, so they are not offered there.
  if (diff.binary || diff.submodule) return <FileDiffFallback diff={diff} />;
  const controls = renderControls(diff.truncated ? truncatedDiffWhitespaceReason : null);
  // Describe the diff on screen, which may still be the other mode's.
  if (!diff.patch && current?.hideWhitespace) return <>{controls}<OnlyWhitespaceChanged onShowWhitespace={showWhitespace} /></>;
  if (!diff.patch) return <div className="grid h-full place-items-center text-sm text-muted-foreground">No textual lines to display.</div>;
  return (
    <div className="flex min-h-full flex-col gap-3 p-3">
      {controls}
      {diff.truncated ? <Alert variant="warning"><AlertTriangleIcon aria-hidden="true" /><AlertDescription>This commit diff exceeded 8 MiB and was truncated.</AlertDescription></Alert> : null}
      <PatchDiff patch={diff.patch} disableWorkerPool options={{ theme: { light: "github-light", dark: "github-dark" }, themeType: resolvedTheme }} />
    </div>
  );
}
