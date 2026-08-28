import { useState } from "react";
import { AlertTriangleIcon, ArrowDownIcon, ArrowUpIcon, CloudUploadIcon, RefreshCwIcon } from "lucide-react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Spinner } from "@/components/ui/spinner";
import { ToolbarButton } from "./ContextHeader";
import { useOptionalWorkingCopyState } from "./working-copy-state";

/** Pull / push / publish / fetch for the selected worktree, plus force-push. */
export function SyncControls() {
  const state = useOptionalWorkingCopyState();
  const [pendingForcePush, setPendingForcePush] = useState(false);
  const snapshot = state?.snapshot ?? null;
  const busy = state?.syncBusy ?? false;
  const blocked = !state || !snapshot?.remote || snapshot.operation !== null;
  const kind = state?.syncKind ?? "fetch";
  const remote = snapshot?.remote ?? "origin";
  const caption = kind === "publish" ? "Publish branch" : kind === "pull" ? `Pull ${remote}` : kind === "push" ? `Push ${remote}` : `Fetch ${remote}`;
  const detail = busy
    ? "Working…"
    : !snapshot?.remote
      ? "No remote"
      : kind === "publish"
        ? "Not on the remote yet"
        : kind === "pull"
          ? `${snapshot.behind} commit${snapshot.behind === 1 ? "" : "s"} behind`
          : kind === "push"
            ? `${snapshot.ahead} commit${snapshot.ahead === 1 ? "" : "s"} ahead`
            : "Up to date";
  const diverged = Boolean(snapshot?.upstream && snapshot.ahead > 0 && snapshot.behind > 0);
  return (
    <>
      <ToolbarButton
        caption={caption}
        className="w-44 flex-none"
        icon={busy
          ? <Spinner />
          : kind === "pull"
            ? <ArrowDownIcon />
            : kind === "push"
              ? <ArrowUpIcon />
              : kind === "publish"
                ? <CloudUploadIcon />
                : <RefreshCwIcon />}
        disabled={busy || blocked}
        onClick={() => { void state?.synchronize(); }}
        aria-label={state?.syncLabel ?? "Fetch"}
      >
        {detail}
      </ToolbarButton>
      {diverged ? (
        <Button variant="destructive" size="sm" disabled={busy || snapshot?.operation !== null} onClick={() => setPendingForcePush(true)}>Force…</Button>
      ) : null}
      {pendingForcePush && state && snapshot?.branch ? (
        <Dialog open onOpenChange={(open) => { if (!open && !busy) setPendingForcePush(false); }}>
          <DialogContent>
            <DialogHeader>
              <DialogTitle>Force-push {snapshot.branch}?</DialogTitle>
              <DialogDescription>Replace the remote branch with this local history. Repola uses an exact force-with-lease, so Git will refuse if the remote changed since your last fetch.</DialogDescription>
            </DialogHeader>
            <Alert variant="destructive"><AlertTriangleIcon aria-hidden="true" /><AlertDescription>This rewrites published history and may disrupt anyone using the remote commits.</AlertDescription></Alert>
            <DialogFooter>
              <Button variant="outline" disabled={busy} onClick={() => setPendingForcePush(false)}>Cancel</Button>
              <Button variant="destructive" disabled={busy || !snapshot.upstreamHead} onClick={() => { void state.synchronize("forcePush").then((ok) => { if (ok) setPendingForcePush(false); }); }}>
                {busy ? <Spinner data-icon="inline-start" /> : null}
                {busy ? "Pushing…" : "Force-push with Lease"}
              </Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      ) : null}
    </>
  );
}
