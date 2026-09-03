import { useState } from "react";
import {
  AlertTriangleIcon,
  Code2Icon,
  CopyIcon,
  FileDiffIcon,
  FolderOpenIcon,
  GitPullRequestIcon,
  ShieldCheckIcon,
  SquareTerminalIcon,
  Trash2Icon,
  UnlockKeyholeIcon,
  WrenchIcon,
} from "lucide-react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { toast } from "@/components/ui/toast";
import { TooltipButton } from "@/components/tooltip-button";
import { toMessage } from "@/lib/errors";
import { useShortPath } from "../app/environment";
import { formatAge, formatDate, formatMeasuredBytes, shortSha } from "../domain/format";
import { fileManagerName } from "../domain/platform";
import { actionForWorktree } from "../domain/inventory";
import { launchWorktreeTool } from "../ipc/app-preferences";
import { openExternalUrl, revealWorktree } from "../ipc/worktrees";
import type { ActionKind, PullRequestEvidence, PullRequestSummary, RemoteProvider, WorktreeRecord } from "../ipc/types";
import { useRepositoryContext } from "./context";
import { ActivityFact, DetailSection, StatusFact } from "./facts";
import { LazyDialog } from "./LazyDialog";
import { PullRequestDialog } from "./lazy";

export type PullState = PullRequestEvidence | "loading";

/**
 * The inventory detail pane. `worktree` is the row selected in the inventory table,
 * which is independent of the working copy chosen in the toolbar.
 */
export function WorktreeDetails({
  worktree,
  provider,
  pull,
  now,
  busy,
  actionError,
  onReviewAction,
  onCheckPulls,
  onPullChanged,
  onViewChanges,
}: {
  worktree: WorktreeRecord;
  provider: RemoteProvider;
  pull: PullState | null;
  now: number;
  busy: boolean;
  actionError: string | null;
  onReviewAction: (kind: ActionKind, worktree: WorktreeRecord) => void;
  onCheckPulls: (worktree: WorktreeRecord) => void;
  onPullChanged: (evidence: PullRequestEvidence, checkedOut: boolean) => Promise<void>;
  onViewChanges: (worktree: WorktreeRecord) => void;
}) {
  const { machineId, machineKind } = useRepositoryContext();
  const shortPath = useShortPath();
  const [opening, setOpening] = useState(false);
  const [openingTool, setOpeningTool] = useState<"editor" | "terminal" | null>(null);
  const open = async () => {
    setOpening(true);
    try {
      await revealWorktree(worktree.path);
    } catch (cause) {
      toast.add({ type: "error", title: "Could not open the worktree", description: toMessage(cause) });
    } finally {
      setOpening(false);
    }
  };
  const launchTool = async (tool: "editor" | "terminal") => {
    setOpeningTool(tool);
    try {
      const message = await launchWorktreeTool(machineId, worktree.path, tool);
      toast.add({ type: "success", title: tool === "editor" ? "Editor opened" : "Terminal opened", description: message });
    } catch (cause) {
      toast.add({ type: "error", title: `Could not open ${tool}`, description: toMessage(cause) });
    } finally {
      setOpeningTool(null);
    }
  };
  const dirty = worktree.status.available && worktree.status.total > 0;
  const pullLookupSupported = worktree.branch !== null && (provider === "gitHub" || provider === "azureDevOps");
  const safetyAlertVariant = ({ protected: "destructive", review: "success", repair: "warning", metadataOnly: "default" } as const)[worktree.safety.level];

  return (
    <div className="flex min-h-full flex-col gap-6 p-5">
      <div className="flex flex-col gap-2">
        <span className="text-xs font-medium tracking-widest text-brand uppercase">{worktree.repositoryName}</span>
        <h2 className="text-lg leading-tight font-medium break-all">{worktree.branch ?? "Detached HEAD"}</h2>
        <code className="font-mono text-xs break-all text-muted-foreground">{shortPath(worktree.path)}</code>
        <div className="flex flex-wrap gap-1.5">
          <Button variant="default" size="sm" disabled={!worktree.exists || openingTool !== null} onClick={() => void launchTool("editor")}>
            {openingTool === "editor" ? <Spinner data-icon="inline-start" /> : <Code2Icon data-icon="inline-start" aria-hidden="true" />}
            Open in Editor
          </Button>
          <TooltipButton
            variant="outline"
            size="sm"
            disabled={!worktree.exists || openingTool !== null || machineKind === "ssh"}
            onClick={() => void launchTool("terminal")}
            tooltip={machineKind === "ssh" ? "Interactive remote shells remain in your terminal's SSH workflow." : null}
          >
            {openingTool === "terminal" ? <Spinner data-icon="inline-start" /> : <SquareTerminalIcon data-icon="inline-start" aria-hidden="true" />}
            Terminal
          </TooltipButton>
          <TooltipButton
            variant="outline"
            size="sm"
            disabled={!worktree.exists || opening || machineKind === "ssh"}
            onClick={() => void open()}
            tooltip={machineKind === "ssh" ? "This path belongs to the remote machine." : null}
          >
            {opening ? <Spinner data-icon="inline-start" /> : <FolderOpenIcon data-icon="inline-start" aria-hidden="true" />}
            Show in {fileManagerName()}
          </TooltipButton>
        </div>
      </div>

      <DetailSection title="Safety">
        <Alert variant={safetyAlertVariant}>
          <ShieldCheckIcon aria-hidden="true" />
          <AlertTitle>{worktree.safety.label}</AlertTitle>
          <AlertDescription>
            {worktree.safety.reasons.map((reason) => <p key={reason}>{reason}</p>)}
          </AlertDescription>
        </Alert>
      </DetailSection>

      <DetailSection title="Local state">
        <dl className="grid grid-cols-4 gap-px border bg-border">
          <StatusFact label="Changed" value={worktree.status.available ? String(worktree.status.total) : "Unknown"} />
          <StatusFact label="Staged" value={worktree.status.available ? String(worktree.status.staged) : "—"} />
          <StatusFact label="Unstaged" value={worktree.status.available ? String(worktree.status.unstaged) : "—"} />
          <StatusFact label="Untracked" value={worktree.status.available ? String(worktree.status.untracked) : "—"} />
        </dl>
        {dirty && (
          <Button variant="outline" size="sm" className="self-start" onClick={() => onViewChanges(worktree)}>
            <FileDiffIcon data-icon="inline-start" aria-hidden="true" />
            View Changes…
          </Button>
        )}
      </DetailSection>

      <DetailSection title="Integration">
        <p className="text-sm leading-relaxed">{worktree.integration.summary}</p>
        {worktree.unpushedCommitCount !== null && worktree.unpushedCommitCount > 0 && (
          <p className="text-sm text-warning">
            {worktree.unpushedCommitCount} commit{worktree.unpushedCommitCount === 1 ? "" : "s"} on this branch {worktree.unpushedCommitCount === 1 ? "is" : "are"} not on any remote.
          </p>
        )}
        <p className="text-xs leading-relaxed text-muted-foreground">Commit containment is not the same thing as a merged or completed pull request.</p>
        {pullLookupSupported && <PullRequestPanel provider={provider} worktree={worktree} pull={pull} onCheckPulls={onCheckPulls} onChanged={onPullChanged} />}
      </DetailSection>

      <DetailSection title="Activity">
        <dl className="flex flex-col">
          <ActivityFact label="Last activity" value={`${formatDate(worktree.lastActivityAtMs)} · ${formatAge(worktree.lastActivityAtMs, now)}`} />
          <ActivityFact label="Folder created" value={formatDate(worktree.createdAtMs)} />
          <ActivityFact label="HEAD commit" value={`${formatDate(worktree.headCommitAtMs)} · ${shortSha(worktree.head)}`} />
          <ActivityFact label="Subject" value={worktree.headSubject ?? "Unavailable"} />
          <ActivityFact label="Allocated size" value={formatMeasuredBytes(worktree.sizeBytes, worktree.sizeIncomplete)} tooltip={worktree.sizeIncomplete ? "Some files could not be measured; this is a lower bound." : undefined} />
          <ActivityFact label="Registration" value={worktree.registration.kind.replace(/([a-z])([A-Z])/g, "$1 $2").toLowerCase()} />
        </dl>
      </DetailSection>

      <ManagementAction worktree={worktree} busy={busy} error={actionError} onReview={onReviewAction} />
    </div>
  );
}
const pullStateBadgeVariant: Record<string, "success" | "secondary" | "outline" | "destructive"> = {
  merged: "success",
  open: "secondary",
  draft: "outline",
  closed: "destructive",
};

function PullRequestPanel({
  provider,
  worktree,
  pull,
  onCheckPulls,
  onChanged,
}: {
  provider: RemoteProvider;
  worktree: WorktreeRecord;
  pull: PullState | null;
  onCheckPulls: (worktree: WorktreeRecord) => void;
  onChanged: (evidence: PullRequestEvidence, checkedOut: boolean) => Promise<void>;
}) {
  const { machineId } = useRepositoryContext();
  const [dialog, setDialog] = useState<{ mode: "create" | "checkout"; pull: PullRequestSummary | null } | null>(null);
  const copyLink = async (url: string) => {
    try {
      await navigator.clipboard.writeText(url);
      toast.add({ type: "success", title: "Pull-request link copied" });
    } catch (cause) {
      toast.add({ type: "error", title: "Could not copy the link", description: toMessage(cause) });
    }
  };
  if (pull === "loading") {
    return (
      <p className="flex items-center gap-2 text-xs text-muted-foreground">
        <Spinner className="size-3" />
        Checking pull requests for {worktree.branch}…
      </p>
    );
  }
  if (pull === null) {
    return (
      <Button variant="outline" size="sm" className="self-start" onClick={() => onCheckPulls(worktree)}>
        <GitPullRequestIcon data-icon="inline-start" aria-hidden="true" />
        Check Pull Requests…
      </Button>
    );
  }
  return (
    <div className="flex flex-col gap-3 border bg-card p-3">
      {pull.status === "fetched" && pull.pulls.length === 0 && (
        <div className="flex items-center justify-between gap-3">
          <p className="text-xs text-muted-foreground">No pull requests found for {pull.branch}.</p>
          <Button size="xs" onClick={() => setDialog({ mode: "create", pull: null })}>Create Pull Request</Button>
        </div>
      )}
      {pull.status === "fetched" && pull.pulls.map((item) => (
        <div key={item.number} className="flex flex-col gap-2 border-b pb-3 last:border-b-0 last:pb-0">
          <div className="flex items-start gap-2">
            <Badge variant={pullStateBadgeVariant[item.state] ?? "outline"}>{item.state}</Badge>
            <strong className="min-w-0 flex-1 text-xs leading-relaxed">#{item.number} {item.title}</strong>
          </div>
          <p className="font-mono text-[11px] text-muted-foreground">{item.headBranch || pull.branch} → {item.baseBranch || "default branch"}</p>
          <div className="flex flex-wrap gap-1.5 text-[11px]">
            {item.reviewState && <Badge variant={item.reviewState === "approved" ? "success" : item.reviewState === "changes_requested" ? "destructive" : "outline"}>Review: {item.reviewState.replace(/_/g, " ")}</Badge>}
            {item.checksTotal > 0 && <Badge variant={item.checksFailed > 0 ? "destructive" : item.checksPending > 0 ? "warning" : "success"}>Checks: {item.checksPassed} passed{item.checksPending > 0 ? ` · ${item.checksPending} pending` : ""}{item.checksFailed > 0 ? ` · ${item.checksFailed} failed` : ""}</Badge>}
            {item.mergeState && <Badge variant={item.mergeState === "queued" ? "secondary" : "outline"}>Merge: {item.mergeState.replace(/_/g, " ")}</Badge>}
          </div>
          {item.linkedIssues.length > 0 && (
            <div className="flex flex-wrap items-center gap-1 text-[11px] text-muted-foreground">
              <span>Closes</span>
              {item.linkedIssues.map((issue) => issue.url
                ? <TooltipButton key={issue.number} variant="link" size="xs" className="h-auto p-0" tooltip={issue.title} onClick={() => void openExternalUrl(issue.url as string)}>#{issue.number}</TooltipButton>
                : <span key={issue.number}>#{issue.number}</span>)}
            </div>
          )}
          <div className="flex flex-wrap gap-1">
            {item.url && <Button variant="outline" size="xs" onClick={() => void openExternalUrl(item.url as string)}><FolderOpenIcon data-icon="inline-start" aria-hidden="true" />Open</Button>}
            {item.url && <Button variant="ghost" size="xs" onClick={() => void copyLink(item.url as string)}><CopyIcon data-icon="inline-start" aria-hidden="true" />Copy link</Button>}
            {matchesOpenPull(item) && <Button variant="ghost" size="xs" onClick={() => setDialog({ mode: "checkout", pull: item })}>Check out…</Button>}
          </div>
        </div>
      ))}
      {pull.status !== "fetched" && (
        <div className="flex flex-col gap-2">
          <p className="flex items-start gap-1.5 text-xs text-warning">
            <AlertTriangleIcon className="mt-0.5 size-3 shrink-0" aria-hidden="true" />
            {pull.detail ?? "Pull-request state could not be verified."}
          </p>
          {(pull.status === "cliMissing" || pull.status === "notAuthenticated") && (
            <div className="border bg-muted/50 p-2 text-xs">
              <strong>{pull.status === "cliMissing" ? "Provider CLI required" : "Provider sign-in required"}</strong>
              <p className="mt-1 text-muted-foreground">
                {provider === "gitHub"
                  ? "Install GitHub CLI, then run gh auth login on this machine. For GitHub Enterprise, select the repository host when prompted or pass --hostname. Repola never stores the token."
                  : "Install the Azure CLI with the Azure DevOps extension, then run az login and az devops login as appropriate on this machine. Repola never stores the token."}
              </p>
            </div>
          )}
        </div>
      )}
      <div className="flex items-center justify-between gap-2">
        <Button variant="link" size="xs" className="px-0" onClick={() => onCheckPulls(worktree)}>Re-check</Button>
        {pull.status === "fetched" && pull.pulls.length > 0 && <Button variant="outline" size="xs" onClick={() => setDialog({ mode: "create", pull: null })}>New Pull Request…</Button>}
      </div>
      {dialog ? (
        <LazyDialog onClose={() => setDialog(null)}>
          <PullRequestDialog open mode={dialog.mode} machineId={machineId} provider={provider} worktree={worktree} pull={dialog.pull} onOpenChange={(open) => { if (!open) setDialog(null); }} onComplete={onChanged} />
        </LazyDialog>
      ) : null}
    </div>
  );
}

function matchesOpenPull(pull: PullRequestSummary): boolean {
  return pull.state === "open" || pull.state === "draft";
}

function ManagementAction({
  worktree,
  busy,
  error,
  onReview,
}: {
  worktree: WorktreeRecord;
  busy: boolean;
  error: string | null;
  onReview: (kind: ActionKind, worktree: WorktreeRecord) => void;
}) {
  const action = actionForWorktree(worktree);
  const Icon = action?.kind === "repair" ? WrenchIcon : action?.kind === "unlock" ? UnlockKeyholeIcon : Trash2Icon;
  return (
    <div className="-mx-5 -mb-5 mt-auto flex flex-col gap-2.5 border-t bg-muted/50 p-4">
      <div className="flex flex-col gap-1">
        <strong className="text-sm font-medium">Management actions</strong>
        <span className="text-xs text-muted-foreground">{action?.description ?? "This worktree is protected by its current state."}</span>
      </div>
      {action ? (
        <Button variant={action.destructive ? "destructive" : "outline"} disabled={busy} onClick={() => onReview(action.kind, worktree)}>
          {busy ? <Spinner data-icon="inline-start" /> : <Icon data-icon="inline-start" aria-hidden="true" />}
          {busy ? "Running preflight…" : action.label}
        </Button>
      ) : (
        <Button variant="outline" disabled>Action unavailable</Button>
      )}
      {error && (
        <Alert variant="destructive" role="alert">
          <AlertTriangleIcon aria-hidden="true" />
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}
    </div>
  );
}
