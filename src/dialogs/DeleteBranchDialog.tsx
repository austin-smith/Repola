import { useEffect, useMemo, useState } from "react";
import { AlertTriangleIcon, BanIcon, Trash2Icon } from "lucide-react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Field, FieldDescription, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectGroup, SelectItem, SelectLabel, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { toast } from "@/components/ui/toast";
import { toMessage } from "@/lib/errors";
import { ActionableGitError } from "../components/ActionableGitError";
import {
  canExecuteDeletion,
  deletionButtonLabel,
  deletionNotice,
  describeObservedAt,
  hasScope,
  initialDeletionBranch,
  initialScope,
  scopeOptions,
  type DeletionScope,
} from "../domain/branch-deletion";
import { shortSha } from "../domain/format";
import { executeBranchDeletion, prepareBranchDeletionReview } from "../ipc/worktrees";
import type { BranchDeletionPlan, BranchDeletionResult, BranchInfo } from "../ipc/types";

interface DeleteBranchDialogProps {
  machineId: string;
  repositoryPath: string;
  /** The worktree whose HEAD Git compares a branch with when it has no upstream. */
  worktreePath: string;
  branches: BranchInfo[];
  /** Opens on this branch, or on no branch once it is gone, instead of the first one that is free to delete. */
  initialBranchRef?: string;
  onClose: () => void;
  /** Runs after the dialog has closed and reported the outcome. */
  onDeleted: (result: BranchDeletionResult) => void | Promise<void>;
}

const noScope: DeletionScope = { deleteLocal: false, deleteRemote: false };

/** A review belongs to the exact branch, scope, and attempt it was requested for. */
interface Review {
  key: string;
  plan: BranchDeletionPlan | null;
  error: string | null;
}

export default function DeleteBranchDialog({
  machineId,
  repositoryPath,
  worktreePath,
  branches,
  initialBranchRef,
  onClose,
  onDeleted,
}: DeleteBranchDialogProps) {
  const [branchRef, setBranchRef] = useState(() => (initialBranchRef === undefined
    ? initialDeletionBranch(branches, worktreePath)
    : branches.find((branch) => branch.fullName === initialBranchRef))?.fullName ?? null);
  const selected = branches.find((branch) => branch.fullName === branchRef) ?? null;
  const [scope, setScope] = useState<DeletionScope>(() => (selected ? initialScope(selected, worktreePath) : noScope));
  const [revision, setRevision] = useState(0);
  const [review, setReview] = useState<Review | null>(null);
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  const [executeError, setExecuteError] = useState<string | null>(null);
  const options = selected ? scopeOptions(selected, worktreePath) : null;
  const items = useMemo(() => Object.fromEntries(branches.map((branch) => [branch.fullName, branch.name])), [branches]);
  const { deleteLocal, deleteRemote } = scope;
  const reviewKey = branchRef && hasScope(scope) ? `${branchRef}\0${deleteLocal}\0${deleteRemote}\0${revision}` : null;
  const current = review !== null && review.key === reviewKey ? review : null;
  const plan = current?.plan ?? null;
  const planError = current?.error ?? null;
  const reviewing = reviewKey !== null && current === null;

  useEffect(() => {
    if (!branchRef || reviewKey === null) return;
    const controller = new AbortController();
    void prepareBranchDeletionReview(machineId, {
      repositoryPath,
      worktreePath,
      branchRef,
      deleteLocal,
      deleteRemote,
    }, controller.signal)
      .then((next) => setReview({ key: reviewKey, plan: next, error: null }))
      .catch((cause: unknown) => {
        if (!controller.signal.aborted) setReview({ key: reviewKey, plan: null, error: toMessage(cause) });
      });
    return () => controller.abort();
  }, [branchRef, deleteLocal, deleteRemote, machineId, repositoryPath, reviewKey, worktreePath]);

  const changeScope = (next: DeletionScope) => {
    setScope(next);
    setTyped("");
    setExecuteError(null);
  };

  const selectBranch = (fullName: string | null) => {
    const branch = branches.find((item) => item.fullName === fullName);
    if (!branch) return;
    setBranchRef(branch.fullName);
    changeScope(initialScope(branch, worktreePath));
  };

  const run = async () => {
    if (!plan || !canExecuteDeletion(plan, typed) || busy) return;
    setBusy(true);
    setExecuteError(null);
    let result: BranchDeletionResult;
    try {
      result = await executeBranchDeletion(machineId, plan, plan.confirmation === "typeBranchName" ? typed : null);
    } catch (cause) {
      // Whatever made execution refuse is reviewed again before another attempt.
      setExecuteError(toMessage(cause));
      setTyped("");
      setRevision((value) => value + 1);
      setBusy(false);
      return;
    }
    // The deletion has happened, so nothing the caller does next may report it as refused.
    onClose();
    toast.add(deletionNotice(result));
    await onDeleted(result);
  };

  const localBranches = branches.filter((branch) => !branch.remote);
  const remoteBranches = branches.filter((branch) => branch.remote);

  return (
    <Dialog open onOpenChange={(open) => { if (!open && !busy) onClose(); }}>
      <DialogContent className="max-h-[calc(100dvh-2rem)] grid-cols-1 overflow-y-auto sm:max-w-lg" showCloseButton={!busy}>
        <DialogHeader>
          <DialogTitle>Delete a branch</DialogTitle>
          <DialogDescription>Review exactly what will be deleted. Repola checks everything again immediately before deleting.</DialogDescription>
        </DialogHeader>

        <Field>
          <FieldLabel htmlFor="delete-branch-target">Branch</FieldLabel>
          <Select items={items} value={branchRef} disabled={busy} onValueChange={selectBranch}>
            <SelectTrigger id="delete-branch-target" className="w-full" aria-label="Branch to delete">
              <SelectValue placeholder="Select a branch…" />
            </SelectTrigger>
            <SelectContent>
              {localBranches.length > 0 ? (
                <SelectGroup>
                  <SelectLabel>Local branches</SelectLabel>
                  {localBranches.map((branch) => (
                    <SelectItem key={branch.fullName} value={branch.fullName}>
                      <span className="truncate">{branch.name}</span>
                      {branch.occupiedWorktreePath !== null ? <span className="ml-auto pl-2 text-xs text-muted-foreground">checked out</span> : null}
                    </SelectItem>
                  ))}
                </SelectGroup>
              ) : null}
              {remoteBranches.length > 0 ? (
                <SelectGroup>
                  <SelectLabel>Remote branches</SelectLabel>
                  {remoteBranches.map((branch) => (
                    <SelectItem key={branch.fullName} value={branch.fullName}>
                      <span className="truncate">{branch.name}</span>
                    </SelectItem>
                  ))}
                </SelectGroup>
              ) : null}
            </SelectContent>
          </Select>
        </Field>

        {selected && options ? (
          <fieldset className="flex min-w-0 flex-col gap-2 border p-3">
            <legend className="px-1 text-xs font-medium text-muted-foreground">Delete</legend>
            <ScopeChoice
              label={`Local branch ${selected.remote ? "" : selected.name}`.trim()}
              checked={scope.deleteLocal}
              disabled={busy || !options.local.available}
              reason={options.local.reason}
              onChange={(checked) => changeScope({ ...scope, deleteLocal: checked })}
            />
            <ScopeChoice
              label={options.remote.label ? `Remote branch ${options.remote.label}` : "Remote branch"}
              checked={scope.deleteRemote}
              disabled={busy || !options.remote.available}
              reason={options.remote.reason ?? "Deletes the branch on the remote for everyone who uses it."}
              onChange={(checked) => changeScope({ ...scope, deleteRemote: checked })}
            />
          </fieldset>
        ) : null}

        {selected && !hasScope(scope) ? <p className="text-sm text-muted-foreground">Choose the local branch, the remote branch, or both.</p> : null}
        {reviewing ? <div className="flex items-center gap-2 text-sm text-muted-foreground"><Spinner />Reviewing…</div> : null}
        {planError ? <ActionableGitError message={planError} /> : null}
        {plan ? <PlanReview plan={plan} /> : null}

        {plan && plan.blockers.length === 0 && plan.confirmation === "typeBranchName" ? (
          <Field>
            <FieldLabel htmlFor="delete-branch-confirmation">Type {plan.branchName} to confirm</FieldLabel>
            <Input
              id="delete-branch-confirmation"
              value={typed}
              disabled={busy}
              autoComplete="off"
              autoCapitalize="off"
              spellCheck={false}
              onChange={(event) => setTyped(event.currentTarget.value)}
            />
            <FieldDescription>
              {plan.requiresForce ? "This forced deletion discards commits that no other ref contains." : "Commits on the remote branch exist in no ref that remains after this deletion."}
            </FieldDescription>
          </Field>
        ) : null}

        {executeError ? (
          <Alert variant="destructive" role="alert">
            <AlertTriangleIcon aria-hidden="true" />
            <AlertTitle>The branch was not deleted</AlertTitle>
            <AlertDescription className="whitespace-pre-wrap break-words">{executeError}</AlertDescription>
          </Alert>
        ) : null}

        <DialogFooter>
          <Button variant="outline" disabled={busy} onClick={onClose}>Cancel</Button>
          <Button variant="destructive" disabled={!plan || !canExecuteDeletion(plan, typed) || busy} onClick={() => void run()}>
            {busy ? <Spinner data-icon="inline-start" /> : <Trash2Icon data-icon="inline-start" aria-hidden="true" />}
            {busy ? "Revalidating…" : plan ? deletionButtonLabel(plan) : "Delete Branch"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function ScopeChoice({ label, checked, disabled, reason, onChange }: {
  label: string;
  checked: boolean;
  disabled: boolean;
  reason: string | null;
  onChange: (checked: boolean) => void;
}) {
  return (
    <div className="flex flex-col gap-0.5">
      <label className="flex items-center gap-2 text-sm">
        <Checkbox checked={checked} disabled={disabled} onCheckedChange={(value) => onChange(value === true)} />
        <span className="min-w-0 truncate">{label}</span>
      </label>
      {reason ? <p className="pl-6 text-xs text-muted-foreground">{reason}</p> : null}
    </div>
  );
}

function PlanReview({ plan }: { plan: BranchDeletionPlan }) {
  const { local } = plan;
  const remote = plan.deleteRemote ? plan.remote : null;
  return (
    <div className="flex min-w-0 flex-col gap-3">
      <dl className="flex flex-col border bg-card px-3">
        {local ? (
          <>
            <Fact label={`Local ${local.name}`} value={`at ${shortSha(local.tip)}`} />
            <Fact label={`Contained in ${local.mergeReference}`} value={local.containedInMergeReference ? "Yes" : "No"} />
            {local.defaultTarget !== null && local.containedInDefaultTarget !== null ? (
              <Fact label={`Contained in ${local.defaultTarget}`} value={local.containedInDefaultTarget ? "Yes" : "No"} />
            ) : null}
            <Fact label="Commits only on this branch" value={commitCount(local.exclusiveCommitCount, local.exclusiveCommitCountCapped)} />
          </>
        ) : null}
        {remote ? (
          <>
            <Fact label={`Remote ${remote.displayName}`} value={`at ${shortSha(remote.expectedOid)}`} />
            <Fact label="Last fetch" value={describeObservedAt(remote.lastFetchedAt)} />
            <Fact label={`${remote.displayName} last changed`} value={describeObservedAt(remote.trackingRefUpdatedAt)} />
          </>
        ) : null}
      </dl>
      {remote ? (
        <p className="text-xs text-muted-foreground">
          Repola does not fetch during review. Git deletes {remote.displayName} only if {remote.remote} still has it at {shortSha(remote.expectedOid)}; if it moved or is already gone, the push is rejected and nothing on the remote changes.
        </p>
      ) : null}
      {plan.blockers.map((blocker) => (
        <Alert key={blocker} variant="destructive">
          <BanIcon aria-hidden="true" />
          <AlertTitle>Cannot delete</AlertTitle>
          <AlertDescription>{blocker}</AlertDescription>
        </Alert>
      ))}
      {plan.warnings.map((warning) => (
        <Alert key={warning} variant="warning">
          <AlertTriangleIcon aria-hidden="true" />
          <AlertDescription>{warning}</AlertDescription>
        </Alert>
      ))}
      {plan.commands.length > 0 ? (
        <div className="flex flex-col gap-1.5">
          <span className="text-xs font-medium tracking-widest text-muted-foreground uppercase">Exact {plan.commands.length === 1 ? "command" : "commands, in order"}</span>
          <code className="border-l-2 border-foreground bg-muted p-2 font-mono text-xs whitespace-pre-wrap break-all">{plan.commands.join("\n")}</code>
        </div>
      ) : null}
    </div>
  );
}

function Fact({ label, value }: { label: string; value: string }) {
  return (
    <div className="grid grid-cols-[minmax(0,1fr)_auto] gap-3 border-b py-2 last:border-b-0">
      <dt className="truncate text-xs text-muted-foreground">{label}</dt>
      <dd className="m-0 text-right text-xs">{value}</dd>
    </div>
  );
}

function commitCount(count: number, capped: boolean): string {
  return capped ? `more than ${count}` : String(count);
}
