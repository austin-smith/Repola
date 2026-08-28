import { Suspense, useCallback, useDeferredValue, useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import {
  AlertTriangleIcon,
  ArchiveIcon,
  FileDiffIcon,
  GitCommitIcon,
  RefreshCwIcon,
  Settings2Icon,
  ShieldCheckIcon,
  Trash2Icon,
  UnlockKeyholeIcon,
} from "lucide-react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Textarea } from "@/components/ui/textarea";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { toast } from "@/components/ui/toast";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { TooltipButton } from "@/components/tooltip-button";
import { cn } from "@/lib/utils";
import { toMessage } from "@/lib/errors";
import { ActionableGitError } from "../components/ActionableGitError";
import { StashDialog } from "../dialogs/StashDialog";
import { arrowKeyChangeTarget, emptyChangeSelection, isSelectAllChangesShortcut, isToggleSelectedChangesShortcut, selectAllChanges, singleChangeSelection, updateChangeSelection } from "../domain/change-selection";
import {
  commitSelectionFor,
  commitSelectionRequest,
  createCommitSelection,
  includedChangeCount,
  isIncludedInCommit,
  reconcileCommitSelection,
  setChangesIncluded,
  type CommitSelectionMap,
  type FileCommitSelection,
} from "../domain/commit-selection";
import { shortSha } from "../domain/format";
import { SELECT_ALL_EVENT } from "../domain/select-all";
import { loadAppPreferences } from "../ipc/app-preferences";
import {
  commitWorkingCopy,
  discardAll,
  discardFile,
  fetchWorkingCopy,
  mutateRepositoryOperation,
  onWorktreeChanged,
  synchronizeWorkingCopy,
  undoLatestCommit,
  unwatchWorktree,
  watchWorktree,
} from "../ipc/worktrees";
import type { CommitSigning, ConflictResolutionKind, DiscardScope, FileDiff, RepositoryOperationAction, SyncKind, WorkingCopySnapshot } from "../ipc/types";
import { parseCommitPeople, parseCommitTrailers } from "./commit-form";
import { useWorkingCopy } from "./context";
import { sectionHeadingClass, signingItems } from "./labels";
import { LazyDialog } from "./LazyDialog";
import { ConflictResolutionDialog, DiffDialog, InlineFileDiff } from "./lazy";
import { operationGuidance, operationLabel, operationSupportsSkip } from "./operations";

export function ChangesWorkbench() {
  const { machineId, repository, worktree } = useWorkingCopy();
  // File diffs are only meaningful for the snapshot they were loaded against,
  // so the cache is stored with the snapshot and replaced whenever it is.
  const [workingCopy, setWorkingCopy] = useState<{ snapshot: WorkingCopySnapshot | null; diffCache: Map<string, FileDiff> }>(() => ({ snapshot: null, diffCache: new Map() }));
  const { snapshot, diffCache } = workingCopy;
  const setSnapshot = useCallback((next: WorkingCopySnapshot | null) => setWorkingCopy({ snapshot: next, diffCache: new Map() }), []);
  const [changeSelection, setChangeSelection] = useState(emptyChangeSelection);
  const [commitSelections, setCommitSelections] = useState<CommitSelectionMap>(() => new Map());
  const changesListRef = useRef<HTMLDivElement>(null);
  const [busyPath, setBusyPath] = useState<string | null>(null);
  const [commitBusy, setCommitBusy] = useState(false);
  const [syncBusy, setSyncBusy] = useState(false);
  const [operationBusy, setOperationBusy] = useState(false);
  const [pendingForcePush, setPendingForcePush] = useState(false);
  const [pendingUndo, setPendingUndo] = useState(false);
  const [pendingDiscardAll, setPendingDiscardAll] = useState(false);
  const [summary, setSummary] = useState("");
  const [description, setDescription] = useState("");
  const [amend, setAmend] = useState(false);
  const [commitOptionsOpen, setCommitOptionsOpen] = useState(false);
  const [authorName, setAuthorName] = useState("");
  const [authorEmail, setAuthorEmail] = useState("");
  const [coAuthors, setCoAuthors] = useState("");
  const [trailers, setTrailers] = useState("");
  const [signing, setSigning] = useState<CommitSigning>("default");
  const [error, setError] = useState<string | null>(null);
  const [diffOpen, setDiffOpen] = useState(false);
  const [stashOpen, setStashOpen] = useState(false);
  const [pendingResolution, setPendingResolution] = useState<{
    kind: ConflictResolutionKind;
    change: WorkingCopySnapshot["changes"][number];
  } | null>(null);
  const [pendingDiscard, setPendingDiscard] = useState<{
    change: WorkingCopySnapshot["changes"][number];
    scope: DiscardScope;
  } | null>(null);
  const [pendingOperationAction, setPendingOperationAction] = useState<RepositoryOperationAction | null>(null);

  useEffect(() => {
    const controller = new AbortController();
    setSnapshot(null);
    setChangeSelection(emptyChangeSelection);
    setCommitSelections(new Map());
    setError(null);
    void fetchWorkingCopy(machineId, repository.path, worktree.path, controller.signal)
      .then((next) => {
        setSnapshot(next);
        setChangeSelection(singleChangeSelection(next.changes.find((change) => !change.ignored)?.id ?? null));
        setCommitSelections(createCommitSelection(next.changes));
      })
      .catch((cause: unknown) => {
        if (!controller.signal.aborted) setError(toMessage(cause));
      });
    return () => controller.abort();
  }, [machineId, repository.path, setSnapshot, worktree.id, worktree.path]);

  // Mutations replace the snapshot with their own result, so a disk-triggered reload
  // while one is running would only race it.
  const mutating = busyPath !== null || commitBusy || syncBusy || operationBusy;
  const mutatingRef = useRef(mutating);
  useEffect(() => { mutatingRef.current = mutating; }, [mutating]);
  const reloadController = useRef<AbortController | null>(null);
  const reloadSnapshot = useCallback(() => {
    if (mutatingRef.current) return;
    reloadController.current?.abort();
    const controller = new AbortController();
    reloadController.current = controller;
    void fetchWorkingCopy(machineId, repository.path, worktree.path, controller.signal)
      .then((next) => {
        if (controller.signal.aborted || mutatingRef.current) return;
        setSnapshot(next);
        setChangeSelection((current) => {
          const ids = new Set(next.changes.map((change) => change.id));
          return current.activeId && ids.has(current.activeId)
            ? current
            : singleChangeSelection(next.changes.find((change) => !change.ignored)?.id ?? null);
        });
      })
      .catch((cause: unknown) => {
        if (!controller.signal.aborted) setError(toMessage(cause));
      })
      .finally(() => {
        if (reloadController.current === controller) reloadController.current = null;
      });
  }, [machineId, repository.path, setSnapshot, worktree.path]);
  useEffect(() => () => reloadController.current?.abort(), []);

  // Refresh when files change on disk (local machines) and whenever the window regains focus,
  // so edits and Git commands made outside Repola show up without a manual refresh.
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    void watchWorktree(machineId, worktree.path).catch(() => undefined);
    void onWorktreeChanged((event) => {
      if (!disposed && event.machineId === machineId) reloadSnapshot();
    }).then((dispose) => {
      if (disposed) dispose();
      else unlisten = dispose;
    });
    const onFocus = () => reloadSnapshot();
    window.addEventListener("focus", onFocus);
    return () => {
      disposed = true;
      unlisten?.();
      window.removeEventListener("focus", onFocus);
      void unwatchWorktree().catch(() => undefined);
    };
  }, [machineId, reloadSnapshot, worktree.path]);

  useEffect(() => {
    let active = true;
    void loadAppPreferences().then((preferences) => {
      if (active) setSigning(preferences.defaultSignCommits ? "sign" : "default");
    }).catch(() => undefined);
    return () => { active = false; };
  }, [worktree.id]);

  const visibleChanges = useMemo(() => snapshot?.changes.filter((change) => !change.ignored) ?? [], [snapshot]);
  const visibleChangeIds = useMemo(() => visibleChanges.map((change) => change.id), [visibleChanges]);
  const selectedChange = visibleChanges.find((change) => change.id === changeSelection.activeId) ?? null;
  // Only the diff body is expensive to build, so it alone follows the selection
  // at transition priority. The list highlight, the header, and every action
  // target stay on the urgent path so they always agree with the selection.
  const deferredActiveId = useDeferredValue(changeSelection.activeId);
  const diffChange = visibleChanges.find((change) => change.id === deferredActiveId) ?? null;
  // Held as state rather than a ref so the diff pane can bind its virtualized
  // rows to the element as soon as it exists.
  const [diffScroller, setDiffScroller] = useState<HTMLDivElement | null>(null);
  const includedCount = includedChangeCount(visibleChanges, commitSelections);
  const gitStagedCount = visibleChanges.filter((change) => change.staged).length;
  const allChangesIncluded = visibleChanges.length > 0 && includedCount === visibleChanges.length;
  const syncKind: SyncKind = !snapshot?.upstream
    ? "publish"
    : snapshot.behind > 0
      ? "pull"
      : snapshot.ahead > 0
        ? "push"
        : "fetch";
  const syncLabel = syncKind === "publish"
    ? "Publish branch"
    : syncKind === "pull"
      ? `Pull ${snapshot?.behind ?? 0}`
      : syncKind === "push"
        ? `Push ${snapshot?.ahead ?? 0}`
        : "Fetch";

  useEffect(() => {
    const onSelectAll = (event: Event) => {
      event.preventDefault();
      changesListRef.current?.focus({ preventScroll: true });
      setChangeSelection((current) => selectAllChanges(visibleChangeIds, current));
    };
    document.addEventListener(SELECT_ALL_EVENT, onSelectAll);
    return () => document.removeEventListener(SELECT_ALL_EVENT, onSelectAll);
  }, [visibleChangeIds]);

  useEffect(() => {
    if (!snapshot) return;
    setCommitSelections((current) => reconcileCommitSelection(snapshot.changes, current));
  }, [snapshot]);

  const toggleSelectedCommitInclusion = () => {
    if (busyPath !== null || commitBusy) return;
    const selectedChanges = visibleChanges.filter((change) => (
      changeSelection.selectedIds.has(change.id) && !change.conflicted
    ));
    if (selectedChanges.length === 0) return;
    const include = selectedChanges.some((change) => (
      !isIncludedInCommit(commitSelectionFor(commitSelections, change.id))
    ));
    setCommitSelections((current) => setChangesIncluded(
      current,
      new Set(selectedChanges.map((change) => change.id)),
      include,
    ));
  };

  const submitCommit = async (event: FormEvent) => {
    event.preventDefault();
    setCommitBusy(true);
    setError(null);
    try {
      const hasAuthorOverride = authorName.trim() !== "" || authorEmail.trim() !== "";
      if (hasAuthorOverride && (authorName.trim() === "" || authorEmail.trim() === "")) {
        throw new Error("Enter both an author name and email address, or leave both blank to use Git configuration.");
      }
      const result = await commitWorkingCopy(machineId, {
        repositoryPath: repository.path,
        worktreePath: worktree.path,
        expectedHead: snapshot?.head ?? null,
        includedChanges: commitSelectionRequest(visibleChanges, commitSelections),
        summary,
        description,
        amend,
        author: hasAuthorOverride ? { name: authorName.trim(), email: authorEmail.trim() } : null,
        coAuthors: parseCommitPeople(coAuthors, "Each co-author"),
        trailers: parseCommitTrailers(trailers),
        signing,
      });
      setSnapshot(result.snapshot);
      setCommitSelections(createCommitSelection(result.snapshot.changes));
      setSummary("");
      setDescription("");
      setCoAuthors("");
      setTrailers("");
      setAmend(false);
      toast.add({
        type: "success",
        title: amend ? "Commit amended" : "Changes committed",
        description: `${shortSha(result.commit)}${result.hookOutput ? ` · ${result.hookOutput}` : ""}`,
      });
    } catch (cause) {
      setError(toMessage(cause));
    } finally {
      setCommitBusy(false);
    }
  };

  const synchronize = async (kind: SyncKind = syncKind) => {
    if (!snapshot) return;
    setSyncBusy(true);
    setError(null);
    try {
      const result = await synchronizeWorkingCopy(
        machineId,
        repository.path,
        worktree.path,
        kind,
        snapshot.head,
        snapshot.upstreamHead,
      );
      setSnapshot(result.snapshot);
      if (kind === "forcePush") setPendingForcePush(false);
      toast.add({
        type: "success",
        title: kind === "fetch" ? "Remote state fetched" : kind === "pull" ? "Changes pulled" : kind === "push" ? "Commits pushed" : kind === "forcePush" ? "Branch force-pushed safely" : "Branch published",
        description: result.output || undefined,
      });
    } catch (cause) {
      setError(toMessage(cause));
    } finally {
      setSyncBusy(false);
    }
  };

  const undoCommit = async () => {
    if (!snapshot?.head) return;
    setCommitBusy(true);
    setError(null);
    try {
      const result = await undoLatestCommit(machineId, repository.path, worktree.path, snapshot.head);
      setSnapshot(result.snapshot);
      setSummary(result.summary);
      setDescription(result.description);
      setAmend(false);
      setPendingUndo(false);
      toast.add({ type: "success", title: "Latest commit undone", description: "Its changes are staged and its message is ready to edit." });
    } catch (cause) {
      setError(toMessage(cause));
    } finally {
      setCommitBusy(false);
    }
  };

  const applyDiscard = async () => {
    if (!snapshot || !pendingDiscard) return;
    setBusyPath(pendingDiscard.change.id);
    setError(null);
    try {
      const next = await discardFile(
        machineId,
        repository.path,
        worktree.path,
        pendingDiscard.change,
        pendingDiscard.scope,
        snapshot.head,
      );
      setSnapshot(next);
      setPendingDiscard(null);
      toast.add({ type: "success", title: "Changes discarded", description: pendingDiscard.change.path.display });
    } catch (cause) {
      setError(toMessage(cause));
    } finally {
      setBusyPath(null);
    }
  };

  const applyDiscardAll = async () => {
    if (!snapshot) return;
    setCommitBusy(true);
    setError(null);
    try {
      const next = await discardAll(machineId, snapshot);
      setSnapshot(next);
      setPendingDiscardAll(false);
      toast.add({
        type: "success",
        title: "All changes discarded",
        description: "Tracked files were restored and reviewed untracked files were removed.",
      });
    } catch (cause) {
      setError(toMessage(cause));
    } finally {
      setCommitBusy(false);
    }
  };

  const runOperationAction = async (action: RepositoryOperationAction) => {
    if (!snapshot?.operation) return;
    setOperationBusy(true);
    setError(null);
    try {
      const result = await mutateRepositoryOperation(
        machineId,
        repository.path,
        worktree.path,
        action,
        snapshot.operation,
        snapshot.head,
      );
      setSnapshot(result.snapshot);
      setPendingOperationAction(null);
      toast.add({
        type: result.succeeded ? "success" : "warning",
        title: result.succeeded
          ? action === "abort" ? "Operation aborted" : action === "skip" ? "Step skipped" : "Operation continued"
          : "Git needs your attention",
        description: result.output || (result.snapshot.operation ? "Resolve the remaining conflicts, then continue." : undefined),
      });
    } catch (cause) {
      setError(toMessage(cause));
    } finally {
      setOperationBusy(false);
    }
  };

  if (!snapshot && !error) {
    return <div className="grid min-h-0 flex-1 place-items-center"><Spinner className="size-6" /></div>;
  }

  return (
    <div className="grid min-h-0 flex-1 grid-cols-[380px_minmax(0,1fr)]">
      <aside className="flex min-h-0 flex-col border-r bg-sidebar">
        <div className="flex h-12 shrink-0 items-center border-b px-4">
          <Checkbox
            checked={allChangesIncluded}
            indeterminate={includedCount > 0 && !allChangesIncluded}
            disabled={visibleChanges.length === 0 || commitBusy || busyPath !== null}
            onCheckedChange={(checked) => setCommitSelections((current) => setChangesIncluded(
              current,
              new Set(visibleChanges.filter((change) => !change.conflicted).map((change) => change.id)),
              checked === true,
            ))}
            aria-label={allChangesIncluded ? "Exclude all changes from commit" : "Include all changes in commit"}
          />
          <strong className="ml-3 text-sm">Changes</strong>
          <Badge variant="secondary" className="ml-2">{visibleChanges.length}</Badge>
          <TooltipButton
            variant="ghost"
            size="icon-sm"
            className="text-destructive"
            disabled={!snapshot || visibleChanges.length === 0 || commitBusy || busyPath !== null || snapshot.operation !== null}
            onClick={() => setPendingDiscardAll(true)}
            aria-label="Discard all changes"
            tooltip="Discard all changes"
          >
            <Trash2Icon aria-hidden="true" />
          </TooltipButton>
          <Button variant="outline" size="sm" className="ml-auto" disabled={!snapshot || commitBusy || busyPath !== null || snapshot.operation !== null} onClick={() => setStashOpen(true)}>
            <ArchiveIcon data-icon="inline-start" aria-hidden="true" />
            Stashes
          </Button>
          <Button variant="outline" size="sm" className="ml-2" disabled={syncBusy || !snapshot?.remote || snapshot.operation !== null} onClick={() => void synchronize()}>
            {syncBusy ? <Spinner data-icon="inline-start" /> : <RefreshCwIcon data-icon="inline-start" aria-hidden="true" />}
            {syncBusy ? "Working…" : syncLabel}
          </Button>
          {snapshot?.upstream && snapshot.ahead > 0 && snapshot.behind > 0 ? (
            <Button variant="destructive" size="sm" className="ml-2" disabled={syncBusy || snapshot.operation !== null} onClick={() => setPendingForcePush(true)}>Force…</Button>
          ) : null}
        </div>
        {snapshot?.operation ? (
          <div className="border-b bg-warning/10 px-4 py-3">
            <div className="flex items-start gap-3">
              <AlertTriangleIcon className="mt-0.5 size-4 shrink-0 text-warning-foreground" aria-hidden="true" />
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2">
                  <strong className="text-sm">{operationLabel(snapshot.operation)} in progress</strong>
                  {visibleChanges.some((change) => change.conflicted) ? <Badge variant="destructive">Conflicts remain</Badge> : <Badge variant="success">Ready</Badge>}
                </div>
                <p className="mt-1 text-xs leading-relaxed text-muted-foreground">{operationGuidance(snapshot.operation, visibleChanges.some((change) => change.conflicted))}</p>
                <div className="mt-3 flex flex-wrap gap-2">
                  {snapshot.operation !== "bisect" && snapshot.operation !== "sequencer" ? (
                    <Button size="sm" disabled={operationBusy || visibleChanges.some((change) => change.conflicted)} onClick={() => void runOperationAction("continue")}>
                      {operationBusy ? <Spinner data-icon="inline-start" /> : null}
                      Continue
                    </Button>
                  ) : null}
                  {operationSupportsSkip(snapshot.operation) ? (
                    <Button size="sm" variant="outline" disabled={operationBusy} onClick={() => setPendingOperationAction("skip")}>Skip step</Button>
                  ) : null}
                  {snapshot.operation !== "sequencer" ? (
                    <Button size="sm" variant="outline" disabled={operationBusy} onClick={() => setPendingOperationAction("abort")}>Abort</Button>
                  ) : null}
                </div>
              </div>
            </div>
          </div>
        ) : null}
        {error ? <ActionableGitError message={error} className="m-3" /> : null}
        <div
          ref={changesListRef}
          className="min-h-0 flex-1 overflow-y-auto focus:outline-none"
          tabIndex={-1}
          aria-keyshortcuts="Meta+A Control+A Space ArrowUp ArrowDown Home End"
          onKeyDown={(event) => {
            const arrowTarget = arrowKeyChangeTarget(visibleChangeIds, changeSelection, event);
            if (arrowTarget !== null) {
              event.preventDefault();
              setChangeSelection((current) => updateChangeSelection(visibleChangeIds, current, arrowTarget, { additive: false, range: event.shiftKey }));
              const row = event.currentTarget.querySelector<HTMLElement>(`[data-change-id="${CSS.escape(arrowTarget)}"]`);
              row?.focus({ preventScroll: true });
              row?.scrollIntoView({ block: "nearest" });
              return;
            }
            if (isSelectAllChangesShortcut(event)) {
              event.preventDefault();
              setChangeSelection((current) => selectAllChanges(visibleChangeIds, current));
              return;
            }
            if (!isToggleSelectedChangesShortcut(event)) return;
            if (event.target instanceof Element && event.target.closest('[role="checkbox"]')) return;
            event.preventDefault();
            toggleSelectedCommitInclusion();
          }}
        >
          {visibleChanges.map((change) => (
            <div key={change.id} className={cn("repola-windowed-row flex min-h-8 [--windowed-row-size:32px] items-center border-b", changeSelection.selectedIds.has(change.id) && "bg-accent")}>
              <span className="grid w-11 shrink-0 place-items-center">
                <Checkbox
                  checked={isIncludedInCommit(commitSelectionFor(commitSelections, change.id))}
                  indeterminate={commitSelectionFor(commitSelections, change.id).kind === "partial"}
                  aria-label={`${isIncludedInCommit(commitSelectionFor(commitSelections, change.id)) ? "Exclude" : "Include"} ${change.path.display} ${isIncludedInCommit(commitSelectionFor(commitSelections, change.id)) ? "from" : "in"} commit`}
                  disabled={busyPath !== null || commitBusy || change.conflicted}
                  onCheckedChange={(checked) => setCommitSelections((current) => setChangesIncluded(
                    current,
                    new Set([change.id]),
                    checked === true,
                  ))}
                />
              </span>
              <button
                type="button"
                className="flex min-w-0 flex-1 items-center gap-2 self-stretch pr-3 text-left"
                data-change-id={change.id}
                aria-pressed={changeSelection.selectedIds.has(change.id)}
                onClick={(event) => {
                  // WebKit on macOS does not consistently move keyboard focus to a
                  // button activated with the mouse. Keep the row as the active
                  // command target so Edit > Select All and Cmd+A reach this list.
                  event.currentTarget.focus({ preventScroll: true });
                  setChangeSelection((current) => updateChangeSelection(
                    visibleChangeIds,
                    current,
                    change.id,
                    { additive: event.metaKey || event.ctrlKey, range: event.shiftKey },
                  ));
                }}
              >
                <span className={cn("w-5 shrink-0 text-center font-mono text-xs font-medium", change.conflicted ? "text-destructive" : "text-brand")}>
                  {change.conflicted ? "!" : change.indexStatus !== "." ? change.indexStatus : change.worktreeStatus}
                </span>
                <span className="min-w-0 flex-1 truncate text-sm" title={change.path.display}><span className="text-muted-foreground">{change.path.display.slice(0, change.path.display.search(/[^\\/]*$/))}</span><span className="text-foreground">{change.path.display.split(/[\\/]/).pop()}</span></span>
                {change.submodule ? <Badge variant="outline">submodule</Badge> : null}
                {change.modeChange === "executableBit" ? <Badge variant="outline">executable bit</Badge> : null}
                {change.modeChange === "symlink" ? <Badge variant="outline">symlink</Badge> : null}
                {change.modeChange === "other" ? (
                  <Tooltip>
                    <TooltipTrigger render={<Badge variant="outline" tabIndex={0} />}>mode change</TooltipTrigger>
                    <TooltipContent>{[change.headMode, change.indexMode, change.worktreeMode].filter(Boolean).join(" → ")}</TooltipContent>
                  </Tooltip>
                ) : null}
              </button>
            </div>
          ))}
          {visibleChanges.length === 0 ? (
            <Empty className="h-full py-12">
              <EmptyHeader>
                <EmptyMedia variant="icon"><ShieldCheckIcon aria-hidden="true" /></EmptyMedia>
                <EmptyTitle>No local changes</EmptyTitle>
                <EmptyDescription>This working copy matches its current commit.</EmptyDescription>
              </EmptyHeader>
            </Empty>
          ) : null}
        </div>
        <form className="flex max-h-[58%] shrink-0 flex-col gap-2 overflow-y-auto border-t bg-card p-3" onSubmit={(event) => void submitCommit(event)}>
          <Input value={summary} onChange={(event) => setSummary(event.currentTarget.value)} placeholder="Summary (required)" maxLength={998} disabled={commitBusy} />
          <Textarea value={description} onChange={(event) => setDescription(event.currentTarget.value)} placeholder="Description" className="min-h-16 resize-none" disabled={commitBusy} />
          <Button type="button" variant="ghost" size="sm" className="justify-start" disabled={commitBusy} onClick={() => setCommitOptionsOpen((open) => !open)}>
            <Settings2Icon data-icon="inline-start" aria-hidden="true" />
            {commitOptionsOpen ? "Hide commit options" : "Author, co-authors, trailers, and signing…"}
          </Button>
          {commitOptionsOpen ? (
            <div className="flex flex-col gap-2 border bg-muted/30 p-2.5">
              <span className={sectionHeadingClass}>Author override</span>
              <div className="grid grid-cols-2 gap-2">
                <Input value={authorName} onChange={(event) => setAuthorName(event.currentTarget.value)} placeholder="Name (use Git config)" maxLength={200} disabled={commitBusy} aria-label="Commit author name" />
                <Input value={authorEmail} onChange={(event) => setAuthorEmail(event.currentTarget.value)} placeholder="Email (use Git config)" maxLength={320} disabled={commitBusy} aria-label="Commit author email" />
              </div>
              <FieldLabel htmlFor="commit-coauthors">Co-authors</FieldLabel>
              <Textarea id="commit-coauthors" value={coAuthors} onChange={(event) => setCoAuthors(event.currentTarget.value)} placeholder={"Name <email@example.com>\nOne co-author per line"} className="min-h-16 resize-y font-mono text-xs" disabled={commitBusy} />
              <FieldLabel htmlFor="commit-trailers">Additional trailers</FieldLabel>
              <Textarea id="commit-trailers" value={trailers} onChange={(event) => setTrailers(event.currentTarget.value)} placeholder={"Reviewed-by: Name\nIssue: 123"} className="min-h-16 resize-y font-mono text-xs" disabled={commitBusy} />
              <FieldLabel htmlFor="commit-signing">Signing</FieldLabel>
              <Select items={signingItems} value={signing} disabled={commitBusy} onValueChange={(value) => { if (value) setSigning(value as CommitSigning); }}>
                <SelectTrigger id="commit-signing" className="w-full"><UnlockKeyholeIcon className="size-3.5 text-muted-foreground" aria-hidden="true" /><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="default">Use Git configuration</SelectItem>
                  <SelectItem value="sign">Sign this commit</SelectItem>
                  <SelectItem value="doNotSign">Do not sign this commit</SelectItem>
                </SelectContent>
              </Select>
              <p className="text-xs leading-relaxed text-muted-foreground">Signing uses the key and signing program configured on {machineId === "local" ? "this machine" : "the remote machine"}. Repola never imports or stores private keys.</p>
            </div>
          ) : null}
          <label className="flex items-center gap-2 text-xs text-muted-foreground">
            <Checkbox checked={amend} disabled={commitBusy || snapshot?.head === null} onCheckedChange={(value) => setAmend(value === true)} />
            Amend latest commit
          </label>
          {snapshot?.head && snapshot.branch && !snapshot.operation && (!snapshot.upstream || snapshot.ahead > 0) ? (
            <Button type="button" variant="ghost" size="sm" disabled={commitBusy} onClick={() => setPendingUndo(true)}>Undo latest commit…</Button>
          ) : null}
          <Button type="submit" disabled={commitBusy || snapshot?.operation !== null || summary.trim() === "" || (!amend && includedCount === 0)}>
            {commitBusy ? <Spinner data-icon="inline-start" /> : <GitCommitIcon data-icon="inline-start" aria-hidden="true" />}
            {commitBusy ? "Committing…" : amend ? "Amend Commit" : `Commit ${includedCount} file${includedCount === 1 ? "" : "s"} to ${snapshot?.branch ?? "detached HEAD"}`}
          </Button>
        </form>
      </aside>
      <section className="flex min-h-0 flex-col bg-background">
        <div className="flex h-12 shrink-0 items-center border-b px-4">
          <div className="min-w-0">
            <strong className="block truncate text-sm">{selectedChange?.path.display ?? "Working copy"}</strong>
            {selectedChange?.previousPath ? <span className="block truncate text-xs text-muted-foreground">renamed from {selectedChange.previousPath.display}</span> : null}
          </div>
          {selectedChange && !selectedChange.conflicted ? (
            <Button variant="ghost" size="sm" className="ml-auto text-destructive" disabled={busyPath !== null} onClick={() => setPendingDiscard({ change: selectedChange, scope: selectedChange.unstaged || selectedChange.untracked ? "unstaged" : "all" })}>
              <Trash2Icon data-icon="inline-start" aria-hidden="true" />
              Discard…
            </Button>
          ) : null}
          <Button variant="outline" size="sm" className={selectedChange && !selectedChange.conflicted ? "ml-2" : "ml-auto"} disabled={visibleChanges.length === 0} onClick={() => setDiffOpen(true)}>
            <FileDiffIcon data-icon="inline-start" aria-hidden="true" />
            Review complete diff
          </Button>
        </div>
        {selectedChange?.conflicted ? (
          <div className="flex shrink-0 items-center gap-2 border-b bg-destructive/8 px-4 py-2">
            <AlertTriangleIcon className="size-4 text-destructive" aria-hidden="true" />
            <strong className="mr-auto text-xs">Resolve this conflict</strong>
            <Button variant="outline" size="xs" disabled={busyPath !== null} onClick={() => setPendingResolution({ kind: "ours", change: selectedChange })}>Use ours…</Button>
            <Button variant="outline" size="xs" disabled={busyPath !== null} onClick={() => setPendingResolution({ kind: "theirs", change: selectedChange })}>Use theirs…</Button>
            <Button variant="outline" size="xs" disabled={busyPath !== null} onClick={() => setPendingResolution({ kind: "both", change: selectedChange })}>Keep both…</Button>
            <Button variant="outline" size="xs" disabled={busyPath !== null} onClick={() => setPendingResolution({ kind: "manual", change: selectedChange })}>Edit manually…</Button>
            <Button variant="destructive" size="xs" disabled={busyPath !== null} onClick={() => setPendingResolution({ kind: "remove", change: selectedChange })}>Remove…</Button>
          </div>
        ) : null}
        <div ref={setDiffScroller} className="min-h-0 flex-1 overflow-auto">
          {diffChange ? (
            <Suspense fallback={<div className="grid h-full place-items-center"><Spinner className="size-6" /></div>}>
              <InlineFileDiff
                machineId={machineId}
                repositoryPath={repository.path}
                worktreePath={worktree.path}
                change={diffChange}
                cache={diffCache}
                scrollElement={diffScroller}
                selection={commitSelectionFor(commitSelections, diffChange.id)}
                onSelectionChange={(selection: FileCommitSelection) => setCommitSelections((current) => {
                  const next = new Map(current);
                  next.set(diffChange.id, selection);
                  return next;
                })}
              />
            </Suspense>
          ) : <div className="grid h-full place-items-center"><p className="text-sm text-muted-foreground">Select a changed file to review it.</p></div>}
        </div>
      </section>
      {diffOpen ? (
        <LazyDialog onClose={() => setDiffOpen(false)}>
          <DiffDialog machineId={machineId} worktree={worktree} onClose={() => setDiffOpen(false)} />
        </LazyDialog>
      ) : null}
      {stashOpen && snapshot ? (
        <StashDialog
          machineId={machineId}
          repository={repository}
          worktree={worktree}
          snapshot={snapshot}
          onSnapshot={setSnapshot}
          onClose={() => setStashOpen(false)}
        />
      ) : null}
      {pendingResolution && snapshot ? (
        <LazyDialog onClose={() => setPendingResolution(null)}>
          <ConflictResolutionDialog
            machineId={machineId}
            repository={repository}
            worktree={worktree}
            snapshot={snapshot}
            change={pendingResolution.change}
            initialKind={pendingResolution.kind}
            onClose={() => setPendingResolution(null)}
            onSnapshot={(next) => {
              setSnapshot(next);
              toast.add({ type: "success", title: "Conflict marked resolved", description: pendingResolution.change.path.display });
            }}
          />
        </LazyDialog>
      ) : null}
      {pendingDiscard ? (
        <Dialog open onOpenChange={(open) => { if (!open && busyPath === null) setPendingDiscard(null); }}>
          <DialogContent>
            <DialogHeader>
              <DialogTitle>Discard changes to this file?</DialogTitle>
              <DialogDescription>This cannot be undone by Repola. Git will revalidate the exact path and status immediately before changing it.</DialogDescription>
            </DialogHeader>
            <code className="rounded-md border bg-muted p-3 font-mono text-xs break-all">{pendingDiscard.change.path.display}</code>
            {pendingDiscard.change.staged && pendingDiscard.change.unstaged ? (
              <ToggleGroup value={[pendingDiscard.scope]} onValueChange={(value) => { if (value[0]) setPendingDiscard((current) => current ? { ...current, scope: value[0] as DiscardScope } : null); }} className="grid grid-cols-2">
                <ToggleGroupItem value="unstaged">Unstaged edits only</ToggleGroupItem>
                <ToggleGroupItem value="all">Staged and unstaged</ToggleGroupItem>
              </ToggleGroup>
            ) : null}
            <Alert variant="destructive">
              <AlertTriangleIcon aria-hidden="true" />
              <AlertDescription>
                {pendingDiscard.change.untracked
                  ? "This untracked file will be deleted from disk. It is not recoverable from Git."
                  : pendingDiscard.scope === "all"
                    ? "Both staged and unstaged changes for this file will be replaced by the current commit."
                    : "Only unstaged edits will be replaced by the staged version; staged changes are preserved."}
              </AlertDescription>
            </Alert>
            <DialogFooter>
              <Button variant="outline" disabled={busyPath !== null} onClick={() => setPendingDiscard(null)}>Cancel</Button>
              <Button variant="destructive" disabled={busyPath !== null} onClick={() => void applyDiscard()}>
                {busyPath !== null ? <Spinner data-icon="inline-start" /> : <Trash2Icon data-icon="inline-start" aria-hidden="true" />}
                {busyPath !== null ? "Discarding…" : "Discard Changes"}
              </Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      ) : null}
      {pendingDiscardAll && snapshot ? (
        <Dialog open onOpenChange={(open) => { if (!open && !commitBusy) setPendingDiscardAll(false); }}>
          <DialogContent>
            <DialogHeader>
              <DialogTitle>Discard all {visibleChanges.length} changed files?</DialogTitle>
              <DialogDescription>Repola will revalidate HEAD and the complete reviewed path/status set immediately before Git changes anything.</DialogDescription>
            </DialogHeader>
            <div className="grid grid-cols-3 border bg-muted/35">
              <div className="border-r p-3 text-center"><strong className="block font-mono text-lg">{gitStagedCount}</strong><span className="text-xs text-muted-foreground">staged</span></div>
              <div className="border-r p-3 text-center"><strong className="block font-mono text-lg">{visibleChanges.filter((change) => change.unstaged && !change.untracked).length}</strong><span className="text-xs text-muted-foreground">unstaged</span></div>
              <div className="p-3 text-center"><strong className="block font-mono text-lg">{visibleChanges.filter((change) => change.untracked).length}</strong><span className="text-xs text-muted-foreground">untracked</span></div>
            </div>
            <Alert variant="destructive">
              <AlertTriangleIcon aria-hidden="true" />
              <AlertDescription>Every tracked change will be restored to HEAD. Untracked files and nested untracked repositories in this working copy will be deleted from disk. Ignored files are preserved. Repola cannot undo this action.</AlertDescription>
            </Alert>
            <DialogFooter>
              <Button variant="outline" disabled={commitBusy} onClick={() => setPendingDiscardAll(false)}>Keep Changes</Button>
              <Button variant="destructive" disabled={commitBusy} onClick={() => void applyDiscardAll()}>
                {commitBusy ? <Spinner data-icon="inline-start" /> : <Trash2Icon data-icon="inline-start" aria-hidden="true" />}
                {commitBusy ? "Discarding…" : "Discard Everything"}
              </Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      ) : null}
      {pendingOperationAction && snapshot?.operation ? (
        <Dialog open onOpenChange={(open) => { if (!open && !operationBusy) setPendingOperationAction(null); }}>
          <DialogContent>
            <DialogHeader>
              <DialogTitle>{pendingOperationAction === "abort" ? `Abort ${operationLabel(snapshot.operation).toLowerCase()}?` : "Skip this step?"}</DialogTitle>
              <DialogDescription>
                {pendingOperationAction === "abort"
                  ? `Git will stop the ${operationLabel(snapshot.operation).toLowerCase()} and restore the working copy to its pre-operation state.`
                  : snapshot.operation === "bisect"
                    ? "Git will mark the current revision untestable and continue the bisect with another revision."
                    : "Git will omit the current commit from this operation and continue with the next step."}
              </DialogDescription>
            </DialogHeader>
            <Alert variant={pendingOperationAction === "abort" ? "destructive" : "default"}>
              <AlertTriangleIcon aria-hidden="true" />
              <AlertDescription>Uncommitted conflict-resolution edits made during this operation may be discarded.</AlertDescription>
            </Alert>
            <DialogFooter>
              <Button variant="outline" disabled={operationBusy} onClick={() => setPendingOperationAction(null)}>Cancel</Button>
              <Button variant={pendingOperationAction === "abort" ? "destructive" : "default"} disabled={operationBusy} onClick={() => void runOperationAction(pendingOperationAction)}>
                {operationBusy ? <Spinner data-icon="inline-start" /> : null}
                {operationBusy ? "Working…" : pendingOperationAction === "abort" ? "Abort Operation" : "Skip Step"}
              </Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      ) : null}
      {pendingForcePush && snapshot?.branch ? (
        <Dialog open onOpenChange={(open) => { if (!open && !syncBusy) setPendingForcePush(false); }}>
          <DialogContent>
            <DialogHeader>
              <DialogTitle>Force-push {snapshot.branch}?</DialogTitle>
              <DialogDescription>Replace the remote branch with this local history. Repola uses an exact force-with-lease, so Git will refuse if the remote changed since your last fetch.</DialogDescription>
            </DialogHeader>
            <Alert variant="destructive"><AlertTriangleIcon aria-hidden="true" /><AlertDescription>This rewrites published history and may disrupt anyone using the remote commits.</AlertDescription></Alert>
            <DialogFooter>
              <Button variant="outline" disabled={syncBusy} onClick={() => setPendingForcePush(false)}>Cancel</Button>
              <Button variant="destructive" disabled={syncBusy || !snapshot.upstreamHead} onClick={() => void synchronize("forcePush")}>
                {syncBusy ? <Spinner data-icon="inline-start" /> : null}
                {syncBusy ? "Pushing…" : "Force-push with Lease"}
              </Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      ) : null}
      {pendingUndo && snapshot?.head ? (
        <Dialog open onOpenChange={(open) => { if (!open && !commitBusy) setPendingUndo(false); }}>
          <DialogContent>
            <DialogHeader>
              <DialogTitle>Undo the latest commit?</DialogTitle>
              <DialogDescription>Move the branch back one commit while preserving the commit’s complete contents as staged changes. Repola refuses if any remote branch already contains this commit.</DialogDescription>
            </DialogHeader>
            <code className="rounded-md border bg-muted p-3 font-mono text-xs">{shortSha(snapshot.head)}</code>
            <DialogFooter>
              <Button variant="outline" disabled={commitBusy} onClick={() => setPendingUndo(false)}>Cancel</Button>
              <Button disabled={commitBusy} onClick={() => void undoCommit()}>{commitBusy ? <Spinner data-icon="inline-start" /> : null}{commitBusy ? "Undoing…" : "Undo Commit"}</Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      ) : null}
    </div>
  );
}
