import { useEffect, useState } from "react";
import { GitPullRequestCreateIcon, GitPullRequestIcon } from "lucide-react";
import { ActionableGitError } from "../components/ActionableGitError";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Field, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Spinner } from "@/components/ui/spinner";
import { Textarea } from "@/components/ui/textarea";
import { mutatePullRequest } from "../ipc/worktrees";
import type { PullRequestEvidence, PullRequestSummary, RemoteProvider, WorktreeRecord } from "../ipc/types";

export interface PullRequestDialogProps {
  open: boolean;
  mode: "create" | "checkout";
  machineId: string;
  provider: RemoteProvider;
  worktree: WorktreeRecord;
  pull: PullRequestSummary | null;
  onOpenChange: (open: boolean) => void;
  onComplete: (evidence: PullRequestEvidence, checkedOut: boolean) => Promise<void> | void;
}

export default function PullRequestDialog({
  open,
  mode,
  machineId,
  provider,
  worktree,
  pull,
  onOpenChange,
  onComplete,
}: PullRequestDialogProps) {
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");
  const [baseBranch, setBaseBranch] = useState("");
  const [draft, setDraft] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const providerName = provider === "gitHub" ? "GitHub" : "Azure DevOps";

  useEffect(() => {
    if (!open) return;
    setTitle(worktree.headSubject ?? "");
    setBody("");
    setBaseBranch("");
    setDraft(false);
    setError(null);
  }, [open, mode, worktree.headSubject]);

  const run = async () => {
    if (!worktree.branch || (mode === "checkout" && !pull)) return;
    setBusy(true);
    setError(null);
    try {
      const result = await mutatePullRequest(
        machineId,
        worktree.repositoryPath,
        worktree.path,
        mode,
        worktree.branch,
        worktree.head,
        mode === "create"
          ? { title: title.trim(), body, baseBranch: baseBranch.trim() || null, draft }
          : { expectedPull: pull },
      );
      await onComplete(result.evidence, mode === "checkout");
      onOpenChange(false);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  };

  const checkoutBlocked = mode === "checkout" && worktree.status.available && worktree.status.total > 0;
  return (
    <Dialog open={open} onOpenChange={(next) => { if (!busy) onOpenChange(next); }}>
      <DialogContent className="sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>{mode === "create" ? "Create Pull Request" : `Check Out Pull Request #${pull?.number ?? ""}`}</DialogTitle>
          <DialogDescription>
            {mode === "create"
              ? `${providerName} will create this from ${worktree.branch}. Authentication remains in the ${provider === "gitHub" ? "GitHub" : "Azure"} CLI on this machine.`
              : `Repola will re-fetch and compare the complete reviewed pull-request record before asking ${providerName} to check out its branch.`}
          </DialogDescription>
        </DialogHeader>

        {mode === "create" ? (
          <div className="flex flex-col gap-4">
            <Field>
              <FieldLabel htmlFor="pr-title">Title</FieldLabel>
              <Input id="pr-title" autoFocus value={title} maxLength={1024} onChange={(event) => setTitle(event.currentTarget.value)} />
            </Field>
            <Field>
              <FieldLabel htmlFor="pr-body">Description</FieldLabel>
              <Textarea id="pr-body" rows={7} value={body} onChange={(event) => setBody(event.currentTarget.value)} placeholder="Explain the change, context, and testing." />
            </Field>
            <Field>
              <FieldLabel htmlFor="pr-base">Base branch <span className="font-normal text-muted-foreground">(optional)</span></FieldLabel>
              <Input id="pr-base" value={baseBranch} onChange={(event) => setBaseBranch(event.currentTarget.value)} placeholder="Provider default branch" spellCheck={false} />
            </Field>
            <label className="flex items-center gap-2 text-sm">
              <Checkbox checked={draft} onCheckedChange={(value) => setDraft(value === true)} />
              Create as draft
            </label>
          </div>
        ) : (
          <div className="flex flex-col gap-3">
            <div className="border bg-muted/50 p-3">
              <strong className="text-sm">#{pull?.number} {pull?.title}</strong>
              <p className="mt-1 font-mono text-xs text-muted-foreground">{pull?.headBranch} → {pull?.baseBranch}</p>
            </div>
            <Alert variant={checkoutBlocked ? "warning" : "default"}>
              <GitPullRequestIcon aria-hidden="true" />
              <AlertTitle>{checkoutBlocked ? "Clean working copy required" : "Reviewed local state"}</AlertTitle>
              <AlertDescription>
                HEAD {worktree.head?.slice(0, 12) ?? "unborn"} · {worktree.status.total} changed file{worktree.status.total === 1 ? "" : "s"}. The action stops if either this state or the provider record changes.
              </AlertDescription>
            </Alert>
          </div>
        )}

        {error && <ActionableGitError message={error} />}
        <DialogFooter>
          <Button variant="outline" disabled={busy} onClick={() => onOpenChange(false)}>Cancel</Button>
          <Button disabled={busy || !worktree.branch || (mode === "create" ? !title.trim() : !pull || checkoutBlocked)} onClick={() => void run()}>
            {busy ? <Spinner data-icon="inline-start" /> : mode === "create" ? <GitPullRequestCreateIcon data-icon="inline-start" aria-hidden="true" /> : <GitPullRequestIcon data-icon="inline-start" aria-hidden="true" />}
            {busy ? "Revalidating…" : mode === "create" ? "Create Pull Request" : "Check Out Pull Request"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
