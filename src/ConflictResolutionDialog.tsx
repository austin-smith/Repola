import { useEffect, useState } from "react";
import { AlertTriangleIcon, CheckIcon, CombineIcon, FilePenLineIcon, Trash2Icon } from "lucide-react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Spinner } from "@/components/ui/spinner";
import { Textarea } from "@/components/ui/textarea";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type { ConflictFile, ConflictResolutionKind, FileChange, RepositorySummary, WorkingCopySnapshot, WorktreeRecord } from "./types";
import { loadConflictFile, resolveConflict } from "./worktrees";
import { formatBytes } from "./format";
import { toMessage } from "@/lib/errors";

interface ConflictResolutionDialogProps {
  machineId: string;
  repository: RepositorySummary;
  worktree: WorktreeRecord;
  snapshot: WorkingCopySnapshot;
  change: FileChange;
  initialKind: ConflictResolutionKind;
  onClose: () => void;
  onSnapshot: (snapshot: WorkingCopySnapshot) => void;
}

const descriptions: Record<ConflictResolutionKind, string> = {
  ours: "Replace the file with Git’s ours stage. During a rebase, ours is the branch being rebased onto.",
  theirs: "Replace the file with Git’s theirs stage. During a rebase, theirs is the commit being replayed.",
  both: "Run Git’s three-way union merge for this file, retaining both sides only where their edits conflict, then stage the result.",
  manual: "Edit the conflicted UTF-8 file here. Repola revalidates the exact original content before writing and staging your result.",
  markResolved: "Keep the working file exactly as it is and stage it. Conflict markers are not removed or reinterpreted.",
  remove: "Remove the path from the working copy and stage that deletion as the resolution.",
};

export default function ConflictResolutionDialog({ machineId, repository, worktree, snapshot, change, initialKind, onClose, onSnapshot }: ConflictResolutionDialogProps) {
  const [kind, setKind] = useState(initialKind);
  const [file, setFile] = useState<ConflictFile | null>(null);
  const [manualContent, setManualContent] = useState("");
  const [fileError, setFileError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    const controller = new AbortController();
    void loadConflictFile(machineId, repository.path, worktree.path, change.path, controller.signal)
      .then((next) => {
        setFile(next);
        setManualContent(next.content);
      })
      .catch((cause: unknown) => {
        if (!controller.signal.aborted) setFileError(toMessage(cause));
      });
    return () => controller.abort();
  }, [change.path, machineId, repository.path, worktree.path]);

  const textRequired = kind === "both" || kind === "manual";
  const canRun = !busy && (!textRequired || file !== null) && (kind !== "manual" || manualContent.length <= 2 * 1024 * 1024);
  const unresolved = snapshot.changes.filter((item) => item.conflicted).length;

  const run = async () => {
    if (!canRun) return;
    setBusy(true);
    setActionError(null);
    try {
      const next = await resolveConflict(
        machineId,
        repository.path,
        worktree.path,
        change.path,
        kind,
        snapshot.head,
        textRequired ? file?.content ?? null : null,
        kind === "manual" ? manualContent : null,
      );
      onSnapshot(next);
      onClose();
    } catch (cause) {
      setActionError(toMessage(cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open onOpenChange={(open) => { if (!open && !busy) onClose(); }}>
      <DialogContent className="flex max-h-[88vh] flex-col sm:max-w-3xl" showCloseButton={!busy}>
        <DialogHeader>
          <DialogTitle>Resolve {change.path.display}</DialogTitle>
          <DialogDescription>{descriptions[kind]}</DialogDescription>
        </DialogHeader>

        <div className="flex items-center gap-2 text-xs text-muted-foreground">
          <Badge variant="destructive">{unresolved} unresolved</Badge>
          <Tooltip>
            <TooltipTrigger render={<code className="min-w-0 flex-1 truncate font-mono" tabIndex={0} />}>
              {change.path.display}
            </TooltipTrigger>
            <TooltipContent>{change.path.display}</TooltipContent>
          </Tooltip>
          {file ? <span>{formatBytes(file.byteLength)}</span> : null}
        </div>

        <ToggleGroup className="grid shrink-0 grid-cols-6" value={[kind]} onValueChange={(value) => { const next = value[0] as ConflictResolutionKind | undefined; if (next) { setKind(next); setActionError(null); } }} disabled={busy}>
          <ToggleGroupItem value="ours">Ours</ToggleGroupItem>
          <ToggleGroupItem value="theirs">Theirs</ToggleGroupItem>
          <ToggleGroupItem value="both"><CombineIcon aria-hidden="true" />Both</ToggleGroupItem>
          <ToggleGroupItem value="manual"><FilePenLineIcon aria-hidden="true" />Manual</ToggleGroupItem>
          <ToggleGroupItem value="markResolved"><CheckIcon aria-hidden="true" />As-is</ToggleGroupItem>
          <ToggleGroupItem value="remove" className="text-destructive"><Trash2Icon aria-hidden="true" />Remove</ToggleGroupItem>
        </ToggleGroup>

        {kind === "manual" ? (
          file ? (
            <Textarea
              value={manualContent}
              onChange={(event) => setManualContent(event.currentTarget.value)}
              className="min-h-80 flex-1 resize-none font-mono text-xs leading-relaxed"
              aria-label="Manual conflict resolution"
              spellCheck={false}
              disabled={busy}
            />
          ) : <div className="grid min-h-80 flex-1 place-items-center"><Spinner className="size-5" /></div>
        ) : kind === "both" ? (
          <Alert>
            <CombineIcon aria-hidden="true" />
            <AlertTitle>Three-way union resolution</AlertTitle>
            <AlertDescription>Common lines appear once. When both sides changed the same region, Git keeps ours followed by theirs. The generated file is staged only after the original conflicted content is revalidated.</AlertDescription>
          </Alert>
        ) : null}

        {fileError && textRequired ? <Alert variant="warning"><AlertTriangleIcon aria-hidden="true" /><AlertTitle>Text resolution unavailable</AlertTitle><AlertDescription>{fileError}</AlertDescription></Alert> : null}
        {actionError ? <Alert variant="destructive" role="alert"><AlertTriangleIcon aria-hidden="true" /><AlertTitle>The conflict was not changed</AlertTitle><AlertDescription>{actionError}</AlertDescription></Alert> : null}

        <DialogFooter>
          <Button variant="outline" disabled={busy} onClick={onClose}>Cancel</Button>
          <Button variant={kind === "remove" ? "destructive" : "default"} disabled={!canRun} onClick={() => void run()}>
            {busy ? <Spinner data-icon="inline-start" /> : kind === "remove" ? <Trash2Icon data-icon="inline-start" aria-hidden="true" /> : <CheckIcon data-icon="inline-start" aria-hidden="true" />}
            {busy ? "Revalidating…" : kind === "remove" ? "Remove and Stage" : "Apply and Stage"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

