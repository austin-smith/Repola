import { useEffect, useMemo, useRef, useState } from "react";
import { ArchiveRestoreIcon, ArrowLeftIcon, CopyIcon, RotateCcwIcon, Trash2Icon } from "lucide-react";
import { PatchDiff } from "@pierre/diffs/react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Spinner } from "@/components/ui/spinner";
import { toast } from "@/components/ui/toast";
import { useTheme } from "@/components/theme-provider";
import { toMessage } from "@/lib/errors";
import { cn } from "@/lib/utils";
import { ActionableGitError } from "../components/ActionableGitError";
import {
  partitionRecoveryPoints,
  recoveryLocation,
  recoveryPointKindLabel,
  restoreChanges,
  restoreEffectLabel,
  unlistedPathCount,
} from "../domain/discard";
import { formatBytes } from "../domain/format";
import type {
  GitPath,
  MachineKind,
  RecoveryFileDiff,
  RecoveryPoint,
  RecoveryRestorePlan,
} from "../ipc/types";
import {
  deleteRecoveryPoints,
  loadRecoveryFileDiff,
  loadRecoveryPoints,
  planRecoveryRestore,
  restoreRecoveryPoint,
} from "../ipc/worktrees";

interface DiscardedChangesDialogProps {
  machineId: string;
  machineKind: MachineKind;
  repositoryPath: string;
  worktreePath: string;
  /** The recovery point to show first, such as the one a discard just created. */
  initialPointId?: string | null;
  /** Reports while a restore or deletion runs; the owner reloads the working copy once it ends. */
  onBusyChange: (busy: boolean) => void;
  onClose: () => void;
}

type View =
  | { kind: "list" }
  // `review` distinguishes each review, so a plan is shown only for the one it answers.
  | { kind: "restore"; point: RecoveryPoint; review: number }
  | { kind: "delete"; points: RecoveryPoint[] };

/**
 * Lists the recovery points Repola saved before discarding or overwriting
 * content, and restores or deletes them through reviewed actions.
 */
export default function DiscardedChangesDialog({
  machineId,
  machineKind,
  repositoryPath,
  worktreePath,
  initialPointId = null,
  onBusyChange,
  onClose,
}: DiscardedChangesDialogProps) {
  const [points, setPoints] = useState<RecoveryPoint[] | null>(null);
  // Older points left out because the list would not fit in one response.
  const [omitted, setOmitted] = useState(0);
  // Bumped to load the list again after a restore or deletion changed it.
  const [listing, setListing] = useState(0);
  const [listError, setListError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // An outcome that arrives after the dialog closed is still reported.
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);
  const report = (message: string) => {
    if (mounted.current) setError(message);
    else toast.add({ type: "error", title: "Discarded Changes", description: message });
  };
  const [activeId, setActiveId] = useState<string | null>(initialPointId);
  const [checked, setChecked] = useState<ReadonlySet<string>>(() => new Set());
  const [view, setView] = useState<View>({ kind: "list" });
  const [busy, setBusyState] = useState(false);
  const setBusy = (value: boolean) => {
    setBusyState(value);
    onBusyChange(value);
  };

  useEffect(() => {
    const controller = new AbortController();
    void loadRecoveryPoints(machineId, repositoryPath, worktreePath, controller.signal)
      .then((next) => {
        if (controller.signal.aborted) return;
        setPoints(next.points);
        setOmitted(next.omitted);
        setListError(null);
        setActiveId((current) => (current && next.points.some((point) => point.id === current) ? current : next.points[0]?.id ?? null));
      })
      .catch((cause: unknown) => { if (!controller.signal.aborted) setListError(toMessage(cause)); });
    return () => controller.abort();
  }, [listing, machineId, repositoryPath, worktreePath]);

  const { here, elsewhere } = useMemo(() => partitionRecoveryPoints(points ?? [], worktreePath), [points, worktreePath]);
  const active = points?.find((point) => point.id === activeId) ?? null;

  const reviews = useRef(0);
  const reviewRestore = (point: RecoveryPoint) => {
    setError(null);
    reviews.current += 1;
    setView({ kind: "restore", point, review: reviews.current });
  };

  // Leaving or replacing a review aborts its plan, and a plan is shown only
  // for the review that requested it.
  const reviewing = view.kind === "restore" ? view : null;
  const [loadedPlan, setLoadedPlan] = useState<{ review: number; plan: RecoveryRestorePlan | null; error: string | null } | null>(null);
  const plan = reviewing && loadedPlan?.review === reviewing.review ? loadedPlan.plan : null;
  const planError = reviewing && loadedPlan?.review === reviewing.review ? loadedPlan.error : null;
  useEffect(() => {
    if (!reviewing) return;
    const controller = new AbortController();
    const { point, review } = reviewing;
    void planRecoveryRestore(machineId, repositoryPath, worktreePath, { id: point.id, oid: point.oid }, controller.signal)
      .then((next) => { if (!controller.signal.aborted) setLoadedPlan({ review, plan: next, error: null }); })
      .catch((cause: unknown) => { if (!controller.signal.aborted) setLoadedPlan({ review, plan: null, error: toMessage(cause) }); });
    return () => controller.abort();
  }, [machineId, repositoryPath, reviewing, worktreePath]);

  const restore = async (plan: RecoveryRestorePlan) => {
    setBusy(true);
    setError(null);
    try {
      const result = await restoreRecoveryPoint(machineId, repositoryPath, worktreePath, plan);
      const replaced = result.replaced;
      setView({ kind: "list" });
      setListing((current) => current + 1);
      toast.add({
        type: "success",
        title: "Discarded changes restored",
        description: replaced
          ? "What the restore replaced is saved as a new recovery point."
          : plan.point.summary,
      });
    } catch (cause) {
      // Show the failure against a fresh review of the working copy as it is now.
      reviewRestore(plan.point);
      report(toMessage(cause));
    } finally {
      setBusy(false);
    }
  };

  const remove = async (selected: RecoveryPoint[]) => {
    setBusy(true);
    setError(null);
    try {
      await deleteRecoveryPoints(
        machineId,
        repositoryPath,
        worktreePath,
        selected.map((point) => ({ id: point.id, oid: point.oid })),
      );
      const deleted = new Set(selected.map((point) => point.id));
      const remaining = (points ?? []).filter((point) => !deleted.has(point.id));
      setPoints(remaining);
      setChecked(new Set());
      setActiveId((current) => (current && remaining.some((point) => point.id === current) ? current : remaining[0]?.id ?? null));
      setView({ kind: "list" });
      // The list loads again, so older points it left out can take their place.
      setListing((current) => current + 1);
    } catch (cause) {
      report(toMessage(cause));
    } finally {
      setBusy(false);
    }
  };

  const toggleChecked = (id: string) => {
    setChecked((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const renderPoint = (point: RecoveryPoint) => (
    <div
      key={point.id}
      className={cn("flex min-h-14 items-center gap-2 border-b px-3 py-2 last:border-b-0", point.id === activeId && "bg-accent")}
    >
      <Checkbox
        checked={checked.has(point.id)}
        disabled={busy}
        onCheckedChange={() => toggleChecked(point.id)}
        aria-label={`Select “${point.summary}” for deletion`}
      />
      <button type="button" className="min-w-0 flex-1 text-left" aria-pressed={point.id === activeId} onClick={() => setActiveId(point.id)}>
        <span className="block truncate text-sm">{point.summary}</span>
        <time className="block text-xs text-muted-foreground" dateTime={point.createdAt}>{new Date(point.createdAt).toLocaleString()}</time>
      </button>
    </div>
  );

  const checkedPoints = (points ?? []).filter((point) => checked.has(point.id));

  return (
    <Dialog open onOpenChange={(open) => { if (!open && !busy) onClose(); }}>
      <DialogContent className="flex h-[min(86vh,44rem)] flex-col sm:max-w-4xl" showCloseButton={!busy}>
        <DialogHeader>
          <DialogTitle>Discarded Changes</DialogTitle>
          <DialogDescription>
            When you discard changes, Repola first saves them as a recovery point {recoveryLocation(machineKind)}, and a restore saves whatever it replaces the same way. Recovery points stay until you delete them.
          </DialogDescription>
        </DialogHeader>
        {error ? <ActionableGitError message={error} /> : null}
        {view.kind === "list" && listError ? <ActionableGitError message={listError} /> : null}
        {view.kind === "delete" ? (
          <DeleteReview points={view.points} busy={busy} onBack={() => setView({ kind: "list" })} onConfirm={() => void remove(view.points)} />
        ) : view.kind === "restore" ? (
          <RestoreReview
            // A fresh review starts without the previous review's previews.
            key={view.review}
            machineId={machineId}
            repositoryPath={repositoryPath}
            worktreePath={worktreePath}
            plan={plan}
            planError={planError}
            busy={busy}
            onBack={() => setView({ kind: "list" })}
            onConfirm={(plan) => void restore(plan)}
          />
        ) : (
          <>
            <div className="grid min-h-0 flex-1 grid-cols-[minmax(0,2fr)_minmax(0,3fr)] border">
              <section aria-label="Recovery points" className="min-h-0 overflow-y-auto border-r">
                {points === null && !listError ? <div className="grid h-32 place-items-center"><Spinner className="size-5" /></div> : null}
                {points?.length === 0 ? <p className="p-6 text-center text-sm text-muted-foreground">Nothing has been discarded in this repository yet.</p> : null}
                {here.map(renderPoint)}
                {elsewhere.length > 0 ? (
                  <>
                    <h3 className="border-b bg-muted/40 px-3 py-1.5 text-xs font-medium text-muted-foreground">From other worktrees</h3>
                    {elsewhere.map(renderPoint)}
                  </>
                ) : null}
                {omitted > 0 ? (
                  <p className="px-3 py-2 text-xs text-muted-foreground">
                    {omitted} older recovery point{omitted === 1 ? "" : "s"} not shown. Delete recovery points you no longer need to see {omitted === 1 ? "it" : "them"}.
                  </p>
                ) : null}
              </section>
              <section aria-label="Recovery point details" className="min-h-0 overflow-y-auto p-4">
                {active ? (
                  <PointDetails point={active} currentWorktree={active.worktreePath === worktreePath} busy={busy} onRestore={() => reviewRestore(active)} />
                ) : (
                  <p className="text-sm text-muted-foreground">Select a recovery point to see what it holds.</p>
                )}
              </section>
            </div>
            <DialogFooter>
              <Button variant="outline" disabled={busy || checkedPoints.length === 0} onClick={() => setView({ kind: "delete", points: checkedPoints })}>
                <Trash2Icon data-icon="inline-start" aria-hidden="true" />
                {checkedPoints.length > 0 ? `Delete ${checkedPoints.length}…` : "Delete…"}
              </Button>
              <Button variant="outline" disabled={busy} onClick={onClose}>Close</Button>
            </DialogFooter>
          </>
        )}
      </DialogContent>
    </Dialog>
  );
}

function PointDetails({ point, currentWorktree, busy, onRestore }: { point: RecoveryPoint; currentWorktree: boolean; busy: boolean; onRestore: () => void }) {
  const copyReference = () => {
    void navigator.clipboard.writeText(point.id).catch((cause: unknown) => (
      toast.add({ type: "error", title: "Could not copy to the clipboard", description: toMessage(cause) })
    ));
  };
  const unlisted = unlistedPathCount(point);
  return (
    <div className="flex flex-col gap-3">
      <div className="flex items-start gap-2">
        <strong className="min-w-0 flex-1 text-sm">{point.summary}</strong>
        <Badge variant="secondary">{recoveryPointKindLabel(point.kind)}</Badge>
      </div>
      <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1.5 text-xs">
        <dt className="text-muted-foreground">Saved</dt>
        <dd><time dateTime={point.createdAt}>{new Date(point.createdAt).toLocaleString()}</time></dd>
        <dt className="text-muted-foreground">Stored as</dt>
        <dd className="flex min-w-0 items-center gap-1">
          <code className="min-w-0 truncate font-mono" title={point.id}>{point.id}</code>
          <Button variant="ghost" size="icon-xs" onClick={copyReference} aria-label="Copy reference name"><CopyIcon aria-hidden="true" /></Button>
        </dd>
        <dt className="text-muted-foreground">New content</dt>
        <dd>{formatBytes(point.storedBytes)}</dd>
        {!currentWorktree ? (
          <>
            <dt className="text-muted-foreground">Worktree</dt>
            <dd className="truncate font-mono" title={point.worktreePath}>{point.worktreePath}</dd>
          </>
        ) : null}
      </dl>
      <div>
        <h4 className="text-xs font-medium text-muted-foreground">{point.pathCount} saved path{point.pathCount === 1 ? "" : "s"}</h4>
        <ul className="mt-1 flex flex-col gap-0.5 font-mono text-xs">
          {point.paths.map((path) => <li key={path.token} className="truncate" title={path.display}>{path.display}</li>)}
          {unlisted > 0 ? <li className="text-muted-foreground">and {unlisted} more</li> : null}
        </ul>
      </div>
      {!currentWorktree ? (
        <Alert>
          <AlertDescription>This was saved from another worktree. Restoring writes its files into the current worktree.</AlertDescription>
        </Alert>
      ) : null}
      <p className="text-xs leading-relaxed text-muted-foreground">
        Git can read it without Repola: <code className="font-mono">git ls-tree -r {point.id}</code> lists the saved working-tree files under <code className="font-mono">worktree/</code>.
      </p>
      <div>
        <Button size="sm" disabled={busy} onClick={onRestore}>
          <RotateCcwIcon data-icon="inline-start" aria-hidden="true" />
          Review Restore…
        </Button>
      </div>
    </div>
  );
}

function RestoreReview({
  machineId,
  repositoryPath,
  worktreePath,
  plan,
  planError,
  busy,
  onBack,
  onConfirm,
}: {
  machineId: string;
  repositoryPath: string;
  worktreePath: string;
  plan: RecoveryRestorePlan | null;
  planError: string | null;
  busy: boolean;
  onBack: () => void;
  onConfirm: (plan: RecoveryRestorePlan) => void;
}) {
  const changes = plan ? restoreChanges(plan.entries) : [];
  const [previewPath, setPreviewPath] = useState<GitPath | null>(null);
  const shownPath = previewPath ?? changes[0]?.path ?? null;
  const [preview, setPreview] = useState<{ token: string; diff: RecoveryFileDiff | null; error: string | null } | null>(null);
  const { resolvedTheme } = useTheme();
  const point = plan?.point ?? null;

  useEffect(() => {
    if (!point || !shownPath) return;
    const controller = new AbortController();
    void loadRecoveryFileDiff(machineId, repositoryPath, worktreePath, { id: point.id, oid: point.oid }, shownPath, controller.signal)
      .then((diff) => { if (!controller.signal.aborted) setPreview({ token: shownPath.token, diff, error: null }); })
      .catch((cause: unknown) => { if (!controller.signal.aborted) setPreview({ token: shownPath.token, diff: null, error: toMessage(cause) }); });
    return () => controller.abort();
  }, [machineId, point, repositoryPath, shownPath, worktreePath]);

  const shown = preview && shownPath && preview.token === shownPath.token ? preview : null;
  return (
    <>
      {planError ? <ActionableGitError message={planError} /> : null}
      {!plan && !planError ? <div className="grid min-h-0 flex-1 place-items-center"><Spinner className="size-5" /></div> : null}
      {plan ? (
        <>
          <p className="text-sm">
            {changes.length === 0 && plan.omitted === 0
              ? "This working copy already matches the recovery point."
              : `Restoring “${plan.point.summary}” changes ${changes.length} listed path${changes.length === 1 ? "" : "s"}. Whatever it replaces is saved as a new recovery point first.`}
            {plan.omitted > 0 ? ` ${plan.omitted} more saved path${plan.omitted === 1 ? " is" : "s are"} too many to list here; the restore covers ${plan.omitted === 1 ? "it" : "them"} too.` : null}
          </p>
          <div className="grid min-h-0 flex-1 grid-cols-[minmax(0,2fr)_minmax(0,3fr)] border">
            <section aria-label="Paths to restore" className="min-h-0 overflow-y-auto border-r">
              {changes.map((entry) => (
                <button
                  key={entry.path.token}
                  type="button"
                  className={cn("flex w-full flex-col items-start border-b px-3 py-1.5 text-left last:border-b-0", entry.path.token === shownPath?.token && "bg-accent")}
                  aria-pressed={entry.path.token === shownPath?.token}
                  onClick={() => setPreviewPath(entry.path)}
                >
                  <span className="w-full truncate font-mono text-xs" title={entry.path.display}>{entry.path.display}</span>
                  <span className="text-xs text-muted-foreground">{restoreEffectLabel(entry)}</span>
                </button>
              ))}
            </section>
            <section aria-label="Restore preview" className="min-h-0 overflow-auto">
              {shownPath && !shown ? <div className="grid h-24 place-items-center"><Spinner className="size-5" /></div> : null}
              {shown?.error ? <ActionableGitError message={shown.error} className="m-3" /> : null}
              {shown?.diff?.truncated ? <p className="p-4 text-sm text-muted-foreground">This file is too large to preview.</p> : null}
              {shown?.diff && !shown.diff.truncated && (shown.diff.binary || shown.diff.patch === "")
                ? <p className="p-4 text-sm text-muted-foreground">{shown.diff.binary ? "Binary content changes." : "Only the staged state changes."}</p>
                : null}
              {shown?.diff && !shown.diff.truncated && !shown.diff.binary && shown.diff.patch !== "" ? (
                <PatchDiff
                  patch={shown.diff.patch}
                  disableWorkerPool
                  options={{ theme: { light: "github-light", dark: "github-dark" }, themeType: resolvedTheme }}
                />
              ) : null}
            </section>
          </div>
        </>
      ) : null}
      <DialogFooter>
        <Button variant="outline" disabled={busy} onClick={onBack}>
          <ArrowLeftIcon data-icon="inline-start" aria-hidden="true" />
          Back
        </Button>
        <Button disabled={busy || !plan || (changes.length === 0 && plan.omitted === 0)} onClick={() => { if (plan) onConfirm(plan); }}>
          {busy ? <Spinner data-icon="inline-start" /> : <ArchiveRestoreIcon data-icon="inline-start" aria-hidden="true" />}
          {busy ? "Restoring…" : plan && plan.omitted > 0 ? "Restore" : `Restore ${changes.length} Path${changes.length === 1 ? "" : "s"}`}
        </Button>
      </DialogFooter>
    </>
  );
}

function DeleteReview({ points, busy, onBack, onConfirm }: { points: RecoveryPoint[]; busy: boolean; onBack: () => void; onConfirm: () => void }) {
  return (
    <>
      <p className="text-sm">Delete {points.length} recovery point{points.length === 1 ? "" : "s"}?</p>
      <section aria-label="Recovery points to delete" className="min-h-0 flex-1 overflow-y-auto border">
        {points.map((point) => (
          <div key={point.id} className="border-b px-3 py-2 last:border-b-0">
            <span className="block truncate text-sm">{point.summary}</span>
            <code className="block truncate font-mono text-xs text-muted-foreground">{point.id}</code>
          </div>
        ))}
      </section>
      <Alert variant="destructive">
        <Trash2Icon aria-hidden="true" />
        <AlertDescription>Their saved content can no longer be restored, and Git removes it during a later garbage collection. If any of them changed since you selected it, nothing is deleted.</AlertDescription>
      </Alert>
      <DialogFooter>
        <Button variant="outline" disabled={busy} onClick={onBack}>Cancel</Button>
        <Button variant="destructive" disabled={busy} onClick={onConfirm}>
          {busy ? <Spinner data-icon="inline-start" /> : <Trash2Icon data-icon="inline-start" aria-hidden="true" />}
          {busy ? "Deleting…" : `Delete Recovery Point${points.length === 1 ? "" : "s"}`}
        </Button>
      </DialogFooter>
    </>
  );
}
