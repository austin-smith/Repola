import { useEffect, useMemo, useState } from "react";
import { AlertTriangleIcon, GitMergeIcon, RotateCcwIcon } from "lucide-react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Field, FieldDescription, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { ActionableGitError } from "../components/ActionableGitError";
import type { HistoryMutationKind, HistoryMutationResult, RepositorySummary, WorkingCopySnapshot, WorktreeRecord } from "../ipc/types";
import { shortSha } from "../domain/format";
import { fetchWorkingCopy, mutateHistory } from "../ipc/worktrees";
import { toMessage } from "@/lib/errors";

export interface HistoryTarget {
  id: string;
  oid: string;
  label: string;
  detail?: string;
}

interface HistoryMutationDialogProps {
  machineId: string;
  repository: RepositorySummary;
  worktree: WorktreeRecord;
  kinds: HistoryMutationKind[];
  targets: HistoryTarget[];
  initialKind: HistoryMutationKind;
  title: string;
  onClose: () => void;
  onCompleted: (result: HistoryMutationResult, kind: HistoryMutationKind, target: HistoryTarget) => void | Promise<void>;
}

const actionCopy: Record<HistoryMutationKind, { label: string; verb: string; description: string; clean: boolean; destructive: boolean }> = {
  merge: {
    label: "Merge",
    verb: "Merge",
    description: "Bring the target into the current branch. Git creates a merge commit when a fast-forward is not possible.",
    clean: true,
    destructive: false,
  },
  squashMerge: {
    label: "Squash and merge",
    verb: "Squash",
    description: "Apply the target as staged changes without preserving its individual commits. Review and create the final commit from Changes.",
    clean: true,
    destructive: false,
  },
  rebase: {
    label: "Rebase current branch",
    verb: "Rebase",
    description: "Replay the current branch on the target. This rewrites the current branch’s commits and may require conflict resolution.",
    clean: true,
    destructive: true,
  },
  cherryPick: {
    label: "Cherry-pick commit",
    verb: "Cherry-pick",
    description: "Apply this commit on the current branch as a new commit, preserving its message and authorship.",
    clean: true,
    destructive: false,
  },
  revert: {
    label: "Revert commit",
    verb: "Revert",
    description: "Create a new commit that reverses this commit without removing existing history.",
    clean: true,
    destructive: false,
  },
  resetSoft: {
    label: "Soft reset",
    verb: "Reset softly",
    description: "Move the current branch to this commit while preserving the current index and working tree. The removed commits become staged changes.",
    clean: false,
    destructive: false,
  },
  resetMixed: {
    label: "Mixed reset",
    verb: "Reset",
    description: "Move the current branch to this commit and rebuild the index. Working-tree files remain, but the current staging selection is lost.",
    clean: false,
    destructive: true,
  },
  resetHard: {
    label: "Hard reset",
    verb: "Reset permanently",
    description: "Move the current branch to this commit and replace tracked working-tree and submodule content. Uncommitted tracked changes are permanently discarded.",
    clean: false,
    destructive: true,
  },
};

export default function HistoryMutationDialog({
  machineId,
  repository,
  worktree,
  kinds,
  targets,
  initialKind,
  title,
  onClose,
  onCompleted,
}: HistoryMutationDialogProps) {
  const [kind, setKind] = useState(initialKind);
  const [targetId, setTargetId] = useState(targets[0]?.id ?? "");
  const [snapshot, setSnapshot] = useState<WorkingCopySnapshot | null>(null);
  const [loadingError, setLoadingError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirmation, setConfirmation] = useState("");

  useEffect(() => {
    const controller = new AbortController();
    void fetchWorkingCopy(machineId, repository.path, worktree.path, controller.signal)
      .then(setSnapshot)
      .catch((cause: unknown) => {
        if (!controller.signal.aborted) setLoadingError(toMessage(cause));
      });
    return () => controller.abort();
  }, [machineId, repository.path, worktree.path]);

  const target = useMemo(
    () => targets.find((item) => item.id === targetId) ?? targets[0] ?? null,
    [targetId, targets],
  );
  const copy = actionCopy[kind];
  const visibleChanges = snapshot?.changes.filter((change) => !change.ignored) ?? [];
  const cleanBlocked = copy.clean && visibleChanges.length > 0;
  const hardConfirmation = snapshot?.branch ? `RESET ${snapshot.branch}` : "RESET DETACHED HEAD";
  const confirmed = kind !== "resetHard" || confirmation === hardConfirmation;
  const canRun = snapshot !== null
    && target !== null
    && snapshot.operation === null
    && snapshot.head !== null
    && snapshot.branch !== null
    && !cleanBlocked
    && confirmed
    && !busy;

  const run = async () => {
    if (!canRun || !snapshot || !target) return;
    setBusy(true);
    setActionError(null);
    try {
      const result = await mutateHistory(machineId, snapshot, kind, target.oid);
      await onCompleted(result, kind, target);
      onClose();
    } catch (cause) {
      setActionError(toMessage(cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open onOpenChange={(open) => { if (!open && !busy) onClose(); }}>
      <DialogContent className="sm:max-w-xl" showCloseButton={!busy}>
        <DialogHeader>
          <div className="flex items-center gap-3">
            <div className={copy.destructive ? "grid size-9 place-items-center bg-destructive/10 text-destructive" : "grid size-9 place-items-center bg-brand/10 text-brand"}>
              {copy.destructive ? <AlertTriangleIcon className="size-4" aria-hidden="true" /> : <GitMergeIcon className="size-4" aria-hidden="true" />}
            </div>
            <div>
              <span className="text-xs font-medium tracking-widest text-muted-foreground uppercase">Reviewed Git action</span>
              <DialogTitle>{title}</DialogTitle>
            </div>
          </div>
          <DialogDescription>{copy.description}</DialogDescription>
        </DialogHeader>

        {kinds.length > 1 ? (
          <Field>
            <FieldLabel htmlFor="history-action-kind">Action</FieldLabel>
            <Select items={Object.fromEntries(kinds.map((item) => [item, actionCopy[item].label]))} value={kind} disabled={busy} onValueChange={(value) => { if (value) { setKind(value as HistoryMutationKind); setConfirmation(""); setActionError(null); } }}>
              <SelectTrigger id="history-action-kind"><SelectValue>{copy.label}</SelectValue></SelectTrigger>
              <SelectContent><SelectGroup>{kinds.map((item) => <SelectItem key={item} value={item}>{actionCopy[item].label}</SelectItem>)}</SelectGroup></SelectContent>
            </Select>
          </Field>
        ) : null}

        {targets.length > 1 ? (
          <Field>
            <FieldLabel htmlFor="history-action-target">Target</FieldLabel>
            <Select items={Object.fromEntries(targets.map((item) => [item.id, item.label]))} value={target?.id ?? null} disabled={busy} onValueChange={(value) => { if (value) { setTargetId(value); setActionError(null); } }}>
              <SelectTrigger id="history-action-target"><SelectValue>{target?.label ?? "Select a target"}</SelectValue></SelectTrigger>
              <SelectContent><SelectGroup>{targets.map((item) => <SelectItem key={item.id} value={item.id}><span className="flex min-w-0 items-center gap-2"><span className="truncate">{item.label}</span><code className="ml-auto font-mono text-xs text-muted-foreground">{shortSha(item.oid)}</code></span></SelectItem>)}</SelectGroup></SelectContent>
            </Select>
          </Field>
        ) : null}

        <div className="grid grid-cols-3 gap-px border bg-border">
          <ReviewFact label="Current branch" value={snapshot?.branch ?? (snapshot ? "Detached HEAD" : "Loading…")} />
          <ReviewFact label="Current HEAD" value={snapshot ? shortSha(snapshot.head) : "Loading…"} mono />
          <ReviewFact label="Target" value={target ? `${target.label} · ${shortSha(target.oid)}` : "Unavailable"} />
        </div>

        {snapshot ? (
          <div className="flex flex-wrap items-center gap-2 border bg-card px-3 py-2 text-xs text-muted-foreground">
            <span>Reviewed working copy</span>
            <Badge variant={visibleChanges.length === 0 ? "secondary" : "outline"}>{visibleChanges.length === 0 ? "Clean" : `${visibleChanges.length} changed file${visibleChanges.length === 1 ? "" : "s"}`}</Badge>
            {snapshot.operation ? <Badge variant="destructive">{snapshot.operation} in progress</Badge> : null}
          </div>
        ) : null}

        {cleanBlocked ? (
          <Alert variant="warning">
            <AlertTriangleIcon aria-hidden="true" />
            <AlertTitle>Clean working copy required</AlertTitle>
            <AlertDescription>Commit or stash all local changes before {copy.label.toLowerCase()}.</AlertDescription>
          </Alert>
        ) : null}
        {snapshot?.operation ? (
          <Alert variant="warning">
            <AlertTriangleIcon aria-hidden="true" />
            <AlertTitle>Finish the current Git operation</AlertTitle>
            <AlertDescription>Continue or abort the {snapshot.operation} from Changes before starting another history action.</AlertDescription>
          </Alert>
        ) : null}
        {kind === "resetHard" ? (
          <Field>
            <FieldLabel htmlFor="hard-reset-confirmation">Type {hardConfirmation} to confirm</FieldLabel>
            <Input id="hard-reset-confirmation" value={confirmation} disabled={busy} autoComplete="off" spellCheck={false} onChange={(event) => setConfirmation(event.currentTarget.value)} />
            <FieldDescription>This verifies intent; Repola separately revalidates the exact HEAD and complete changed-file status set immediately before Git runs.</FieldDescription>
          </Field>
        ) : null}

        {loadingError || actionError ? <ActionableGitError message={(actionError ?? loadingError) as string} /> : null}

        <DialogFooter>
          <Button variant="outline" disabled={busy} onClick={onClose}>Cancel</Button>
          <Button variant={copy.destructive ? "destructive" : "default"} disabled={!canRun} onClick={() => void run()}>
            {busy ? <Spinner data-icon="inline-start" /> : kind.startsWith("reset") ? <RotateCcwIcon data-icon="inline-start" aria-hidden="true" /> : <GitMergeIcon data-icon="inline-start" aria-hidden="true" />}
            {busy ? "Revalidating…" : copy.verb}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function ReviewFact({ label, value, mono = false }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="flex min-w-0 flex-col gap-1 bg-card p-2.5">
      <span className="text-xs text-muted-foreground">{label}</span>
      <Tooltip>
        <TooltipTrigger
          render={<span className={mono ? "truncate font-mono text-xs font-medium" : "truncate text-xs font-medium"} tabIndex={0} />}
        >
          {value}
        </TooltipTrigger>
        <TooltipContent>{value}</TooltipContent>
      </Tooltip>
    </div>
  );
}

