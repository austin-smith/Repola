import { useEffect, useRef, useState } from "react";
import { ArchiveRestoreIcon, Trash2Icon } from "lucide-react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Spinner } from "@/components/ui/spinner";
import { toast } from "@/components/ui/toast";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { toMessage } from "@/lib/errors";
import { ActionableGitError } from "../components/ActionableGitError";
import {
  defaultDiscardScope,
  discardEffectLabel,
  isLargeRecoveryPoint,
  keptReasonLabel,
  recoveryLocation,
} from "../domain/discard";
import { formatBytes } from "../domain/format";
import type { DiscardPlan, DiscardResult, DiscardScope, DiscardTarget, FileChange, MachineKind } from "../ipc/types";
import { discardChanges, planDiscard } from "../ipc/worktrees";

interface DiscardDialogProps {
  machineId: string;
  machineKind: MachineKind;
  repositoryPath: string;
  worktreePath: string;
  /** The file to discard, or null to discard every change. */
  change: FileChange | null;
  /** Reports while the discard runs; the owner reloads the working copy once it ends. */
  onBusyChange: (busy: boolean) => void;
  onClose: () => void;
  /** Called with the result and the plan it carried out. */
  onDiscarded: (result: DiscardResult, plan: DiscardPlan) => void;
}

/**
 * Reviews a discard before it runs. The engine plans it read-only and returns
 * the exact paths, what happens to each, what is left alone, and a
 * fingerprint; confirming executes only if the working copy still matches.
 */
export default function DiscardDialog({ machineId, machineKind, repositoryPath, worktreePath, change, onBusyChange, onClose, onDiscarded }: DiscardDialogProps) {
  const [scope, setScope] = useState<DiscardScope>(() => (change ? defaultDiscardScope(change) : "all"));
  const [error, setError] = useState<string | null>(null);
  // A failure that arrives after the dialog closed is still reported.
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);
  const [busy, setBusy] = useState(false);
  // Bumped after a failed discard so the review reflects the working copy as it is now.
  const [review, setReview] = useState(0);
  // Each load is keyed by the review it answers, so a stale plan is never shown for a newer one.
  const reviewKey = `${scope}:${review}`;
  const [loaded, setLoaded] = useState<{ key: string; plan: DiscardPlan | null; error: string | null } | null>(null);
  const plan = loaded?.key === reviewKey ? loaded.plan : null;
  const planError = loaded?.key === reviewKey ? loaded.error : null;

  useEffect(() => {
    const controller = new AbortController();
    const target: DiscardTarget = change ? { kind: "file", path: change.path, scope } : { kind: "all" };
    void planDiscard(machineId, repositoryPath, worktreePath, target, controller.signal)
      .then((next) => { if (!controller.signal.aborted) setLoaded({ key: reviewKey, plan: next, error: null }); })
      .catch((cause: unknown) => { if (!controller.signal.aborted) setLoaded({ key: reviewKey, plan: null, error: toMessage(cause) }); });
    return () => controller.abort();
  }, [change, machineId, repositoryPath, reviewKey, scope, worktreePath]);

  const confirm = async () => {
    if (!plan) return;
    setBusy(true);
    onBusyChange(true);
    setError(null);
    try {
      const result = await discardChanges(machineId, repositoryPath, worktreePath, plan);
      onBusyChange(false);
      onDiscarded(result, plan);
    } catch (cause) {
      onBusyChange(false);
      if (!mounted.current) {
        toast.add({ type: "error", title: "Discard failed", description: toMessage(cause) });
        return;
      }
      setError(toMessage(cause));
      setReview((current) => current + 1);
    } finally {
      setBusy(false);
    }
  };

  const title = change
    ? "Discard changes to this file?"
    : plan
      ? `Discard ${plan.entries.length} changed file${plan.entries.length === 1 ? "" : "s"}?`
      : "Discard all changes?";

  return (
    <Dialog open onOpenChange={(open) => { if (!open && !busy) onClose(); }}>
      <DialogContent className="flex max-h-[86vh] flex-col sm:max-w-xl" showCloseButton={!busy}>
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>
            Repola saves everything it discards as a recovery point {recoveryLocation(machineKind)}, then checks that these files are still exactly as reviewed immediately before Git changes them.
          </DialogDescription>
        </DialogHeader>
        {change ? <code className="rounded-md border bg-muted p-3 font-mono text-xs break-all">{change.path.display}</code> : null}
        {change?.staged && change.unstaged ? (
          <ToggleGroup
            value={[scope]}
            onValueChange={(value) => {
              if (value[0] === "unstaged" || value[0] === "all") {
                setScope(value[0]);
                setError(null);
              }
            }}
            disabled={busy}
            className="grid grid-cols-2"
          >
            <ToggleGroupItem value="unstaged">Unstaged edits only</ToggleGroupItem>
            <ToggleGroupItem value="all">Staged and unstaged</ToggleGroupItem>
          </ToggleGroup>
        ) : null}
        {planError ? <ActionableGitError message={planError} /> : null}
        {!plan && !planError ? <div className="grid h-24 place-items-center"><Spinner className="size-5" /></div> : null}
        {plan ? (
          <>
            <section aria-label="Changes to discard" className="min-h-0 flex-1 overflow-y-auto border">
              {plan.entries.map((entry) => (
                <div key={entry.path.token} className="flex min-h-9 items-center gap-3 border-b px-3 py-1.5 text-xs last:border-b-0">
                  <span className="min-w-0 flex-1 truncate font-mono" title={entry.path.display}>{entry.path.display}</span>
                  <span className="shrink-0 text-muted-foreground">{discardEffectLabel(entry)}</span>
                </div>
              ))}
            </section>
            {plan.kept.length > 0 ? (
              <Alert>
                <AlertTitle>Left unchanged ({plan.kept.length})</AlertTitle>
                <AlertDescription>
                  <p>Repola leaves these alone, for the reason given with each.</p>
                  <ul className="mt-1.5 flex flex-col gap-1">
                    {plan.kept.map((kept) => (
                      <li key={kept.path.token}><code className="font-mono">{kept.path.display}</code> · {keptReasonLabel(kept.reason)}</li>
                    ))}
                  </ul>
                </AlertDescription>
              </Alert>
            ) : null}
            <Alert variant={isLargeRecoveryPoint(plan.backupBytes) ? "warning" : "default"}>
              <ArchiveRestoreIcon aria-hidden="true" />
              <AlertTitle>Recoverable from Discarded Changes</AlertTitle>
              <AlertDescription>
                {plan.backupBytes > 0
                  ? `The recovery point stores ${formatBytes(plan.backupBytes)} of new content.`
                  : "The discarded content is already stored in Git, so the recovery point adds almost nothing."}
                {isLargeRecoveryPoint(plan.backupBytes)
                  ? " That space stays in the repository until you delete the recovery point."
                  : null}
              </AlertDescription>
            </Alert>
          </>
        ) : null}
        {error ? <ActionableGitError message={error} /> : null}
        <DialogFooter>
          <Button variant="outline" disabled={busy} onClick={onClose}>Cancel</Button>
          <Button variant="destructive" disabled={busy || !plan} onClick={() => void confirm()}>
            {busy ? <Spinner data-icon="inline-start" /> : <Trash2Icon data-icon="inline-start" aria-hidden="true" />}
            {busy ? "Discarding…" : change ? "Discard Changes" : "Discard Everything"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
