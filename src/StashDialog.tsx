import { useEffect, useState } from "react";
import { ArchiveIcon, ArrowDownToLineIcon, LayersIcon, Trash2Icon } from "lucide-react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Field, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Spinner } from "@/components/ui/spinner";
import { TooltipButton } from "@/components/tooltip-button";
import { ActionableGitError } from "./ActionableGitError";
import type { RepositorySummary, StashEntry, StashMutationKind, WorkingCopySnapshot, WorktreeRecord } from "./types";
import { loadStashes, mutateStash } from "./worktrees";

export function StashDialog({
  machineId,
  repository,
  worktree,
  snapshot,
  onSnapshot,
  onClose,
}: {
  machineId: string;
  repository: RepositorySummary;
  worktree: WorktreeRecord;
  snapshot: WorkingCopySnapshot;
  onSnapshot: (snapshot: WorkingCopySnapshot) => void;
  onClose: () => void;
}) {
  const [stashes, setStashes] = useState<StashEntry[] | null>(null);
  const [currentSnapshot, setCurrentSnapshot] = useState(snapshot);
  const [message, setMessage] = useState("");
  const [includeUntracked, setIncludeUntracked] = useState(true);
  const [selectedChangeIds, setSelectedChangeIds] = useState<ReadonlySet<string>>(
    () => new Set(snapshot.changes.filter((change) => !change.ignored).map((change) => change.id)),
  );
  const [busy, setBusy] = useState<string | null>(null);
  const [reviewDrop, setReviewDrop] = useState<StashEntry | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const controller = new AbortController();
    void loadStashes(machineId, repository.path, worktree.path, controller.signal)
      .then(setStashes)
      .catch((cause: unknown) => {
        if (!controller.signal.aborted) setError(cause instanceof Error ? cause.message : String(cause));
      });
    return () => controller.abort();
  }, [machineId, repository.path, worktree.id, worktree.path]);

  const run = async (kind: StashMutationKind, stash: StashEntry | null = null) => {
    const key = stash ? `${kind}-${stash.oid}` : kind;
    setBusy(key);
    setError(null);
    try {
      const result = await mutateStash(machineId, {
        repositoryPath: repository.path,
        worktreePath: worktree.path,
        kind,
        message: kind === "push" ? message.trim() || null : null,
        includeUntracked: kind === "push" && includeUntracked,
        paths: kind === "push"
          ? currentSnapshot.changes
            .filter((change) => selectedChangeIds.has(change.id) && !change.ignored && (!change.untracked || includeUntracked))
            .map((change) => change.path)
          : [],
        stashIndex: stash?.index ?? null,
        expectedStashOid: stash?.oid ?? null,
        expectedHead: currentSnapshot.head,
      });
      setCurrentSnapshot(result.snapshot);
      setSelectedChangeIds(new Set(
        result.snapshot.changes.filter((change) => !change.ignored).map((change) => change.id),
      ));
      setStashes(result.stashes);
      setMessage("");
      setReviewDrop(null);
      onSnapshot(result.snapshot);
      if (result.conflicted) {
        setError("The stash was restored with conflicts. Resolve every conflicted file before continuing.");
      }
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(null);
    }
  };

  const changes = currentSnapshot.changes.filter((change) => !change.ignored);
  const selectedChanges = changes.filter(
    (change) => selectedChangeIds.has(change.id) && (!change.untracked || includeUntracked),
  );
  const hasChanges = selectedChanges.length > 0;
  const hasUntracked = changes.some((change) => selectedChangeIds.has(change.id) && change.untracked);
  const allSelected = changes.length > 0 && changes.every((change) => selectedChangeIds.has(change.id));
  const toggleChange = (id: string) => {
    setSelectedChangeIds((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };
  const toggleAll = () => {
    setSelectedChangeIds(allSelected ? new Set() : new Set(changes.map((change) => change.id)));
  };

  return (
    <Dialog open onOpenChange={(open) => { if (!open && busy === null) onClose(); }}>
      <DialogContent className="flex max-h-[86vh] flex-col sm:max-w-2xl" showCloseButton={busy === null}>
        <DialogHeader>
          <DialogTitle>Stashes</DialogTitle>
          <DialogDescription>Temporarily set work aside or restore it in {worktree.branch ?? "this working copy"}. Restores preserve the stash’s index state.</DialogDescription>
        </DialogHeader>

        {error ? <ActionableGitError message={error} /> : null}

        <form className="flex flex-col border bg-muted/35" onSubmit={(event) => { event.preventDefault(); void run("push"); }}>
          <div className="flex items-end gap-2 p-3">
            <Field className="min-w-0 flex-1">
              <FieldLabel htmlFor="stash-message">Stash selected changes</FieldLabel>
              <Input id="stash-message" value={message} onChange={(event) => setMessage(event.currentTarget.value)} placeholder="Optional description" maxLength={998} disabled={busy !== null} />
            </Field>
            <label className="flex h-8 items-center gap-2 whitespace-nowrap text-xs text-muted-foreground">
              <Checkbox checked={includeUntracked} disabled={busy !== null || !hasUntracked} onCheckedChange={(value) => setIncludeUntracked(value === true)} />
              Include untracked
            </label>
            <Button type="submit" size="sm" disabled={busy !== null || !hasChanges}>
              {busy === "push" ? <Spinner data-icon="inline-start" /> : <ArchiveIcon data-icon="inline-start" aria-hidden="true" />}
              {busy === "push" ? "Stashing…" : "Stash Selected"}
            </Button>
          </div>
          {changes.length > 0 ? (
            <div className="border-t bg-background/60">
              <label className="flex h-9 items-center gap-2 border-b px-3 text-xs font-medium">
                <Checkbox checked={allSelected} disabled={busy !== null} onCheckedChange={toggleAll} />
                {selectedChanges.length} of {changes.length} changed file{changes.length === 1 ? "" : "s"} selected
              </label>
              <div className="max-h-32 overflow-y-auto">
                {changes.map((change) => (
                  <label key={change.id} className="flex h-8 items-center gap-2 border-b px-3 text-xs last:border-b-0">
                    <Checkbox checked={selectedChangeIds.has(change.id)} disabled={busy !== null} onCheckedChange={() => toggleChange(change.id)} />
                    <span className="min-w-0 flex-1 truncate font-mono">{change.path.display}</span>
                    <Badge variant={change.conflicted ? "destructive" : change.untracked ? "outline" : "secondary"}>
                      {change.conflicted ? "conflict" : change.untracked ? "untracked" : change.staged && change.unstaged ? "staged + unstaged" : change.staged ? "staged" : "modified"}
                    </Badge>
                  </label>
                ))}
              </div>
            </div>
          ) : null}
        </form>

        <section className="min-h-0 overflow-y-auto border" aria-label="Saved stashes">
          {stashes === null && !error ? <div className="grid h-28 place-items-center"><Spinner className="size-5" /></div> : null}
          {stashes?.length === 0 ? (
            <div className="flex h-28 flex-col items-center justify-center gap-2 text-center">
              <LayersIcon className="size-5 text-muted-foreground" aria-hidden="true" />
              <p className="text-sm text-muted-foreground">No saved stashes in this repository.</p>
            </div>
          ) : null}
          {stashes?.map((stash) => (
            <div key={stash.oid} className="flex min-h-16 items-center gap-3 border-b px-3 py-2 last:border-b-0">
              <Badge variant="outline" className="font-mono">{`stash@{${stash.index}}`}</Badge>
              <div className="min-w-0 flex-1">
                <strong className="block truncate text-sm font-medium">{stash.subject}</strong>
                <span className="text-xs text-muted-foreground">{new Date(stash.createdAt).toLocaleString()} · {stash.oid.slice(0, 7)}</span>
              </div>
              <Button variant="outline" size="sm" disabled={busy !== null || hasChanges} onClick={() => void run("apply", stash)}>
                {busy === `apply-${stash.oid}` ? <Spinner data-icon="inline-start" /> : <ArrowDownToLineIcon data-icon="inline-start" aria-hidden="true" />}Apply
              </Button>
              <Button variant="outline" size="sm" disabled={busy !== null || hasChanges} onClick={() => void run("pop", stash)}>
                {busy === `pop-${stash.oid}` ? <Spinner data-icon="inline-start" /> : null}Pop
              </Button>
              <TooltipButton variant="ghost" size="icon-sm" className="text-destructive" disabled={busy !== null} aria-label={`Review dropping ${stash.subject}`} tooltip="Drop stash" onClick={() => setReviewDrop(stash)}><Trash2Icon aria-hidden="true" /></TooltipButton>
            </div>
          ))}
        </section>

        {reviewDrop ? (
          <Alert variant="destructive">
            <Trash2Icon aria-hidden="true" />
            <AlertTitle>Permanently drop this stash?</AlertTitle>
            <AlertDescription className="flex flex-col gap-3">
              <span>{reviewDrop.subject}. Git may eventually prune the underlying objects; Repola cannot promise recovery.</span>
              <span className="flex gap-2"><Button variant="outline" size="sm" disabled={busy !== null} onClick={() => setReviewDrop(null)}>Keep stash</Button><Button variant="destructive" size="sm" disabled={busy !== null} onClick={() => void run("drop", reviewDrop)}>{busy === `drop-${reviewDrop.oid}` ? <Spinner data-icon="inline-start" /> : <Trash2Icon data-icon="inline-start" aria-hidden="true" />}Drop Stash</Button></span>
            </AlertDescription>
          </Alert>
        ) : null}

        <DialogFooter><Button variant="outline" disabled={busy !== null} onClick={onClose}>Close</Button></DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
