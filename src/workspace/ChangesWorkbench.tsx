import { Suspense, useDeferredValue, useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import {
  AlertTriangleIcon,
  CopyIcon,
  FileDiffIcon,
  FolderOpenIcon,
  GitCommitIcon,
  KeyRoundIcon,
  MoreHorizontalIcon,
  SearchIcon,
  SearchXIcon,
  ShieldCheckIcon,
  Trash2Icon,
  UnlockKeyholeIcon,
  UserPenIcon,
  UsersIcon,
  XIcon,
} from "lucide-react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { ContextMenu, ContextMenuContent, ContextMenuGroup, ContextMenuItem, ContextMenuSeparator, ContextMenuTrigger } from "@/components/ui/context-menu";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { DropdownMenu, DropdownMenuContent, DropdownMenuGroup, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Textarea } from "@/components/ui/textarea";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { toast } from "@/components/ui/toast";
import { TooltipButton } from "@/components/tooltip-button";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { cn } from "@/lib/utils";
import { toMessage } from "@/lib/errors";
import { ActionableGitError } from "../components/ActionableGitError";
import { usePathSeparator } from "../app/environment";
import { ChangeStatusIcon } from "./ChangeStatusIcon";
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
import { filterChanges } from "../domain/change-filter";
import { shortSha } from "../domain/format";
import { SELECT_ALL_EVENT } from "../domain/select-all";
import { loadAppPreferences } from "../ipc/app-preferences";
import {
  commitWorkingCopy,
  discardAll,
  discardFile,
  mutateRepositoryOperation,
  revealWorktree,
  undoLatestCommit,
} from "../ipc/worktrees";
import type { CommitSigning, ConflictResolutionKind, DiscardScope, RepositoryOperationAction, WorkingCopySnapshot } from "../ipc/types";
import { parseCommitPeople, parseCommitTrailers } from "./commit-form";
import { useWorkingCopy } from "./context";
import { sectionHeadingClass, signingItems } from "./labels";
import { LazyDialog } from "./LazyDialog";
import { ConflictResolutionDialog, DiffDialog, InlineFileDiff } from "./lazy";
import { operationGuidance, operationLabel, operationSupportsSkip } from "./operations";
import { useMutationGuard, useWorkingCopyState } from "./working-copy-state";

type CommitOptionSection = "author" | "trailers" | "signing";

const commitOptionSections: { id: CommitOptionSection; label: string; icon: typeof UserPenIcon }[] = [
  { id: "author", label: "Author override", icon: UserPenIcon },
  { id: "trailers", label: "Co-authors and trailers", icon: UsersIcon },
  { id: "signing", label: "Commit signing", icon: KeyRoundIcon },
];

export function ChangesWorkbench() {
  const { machineId, machineKind, repository, worktree } = useWorkingCopy();
  const { snapshot, diffCache, error, setSnapshot, setError } = useWorkingCopyState();
  const separator = usePathSeparator();
  const [rawChangeSelection, setChangeSelection] = useState(emptyChangeSelection);
  const [filter, setFilter] = useState("");
  const [commitSelections, setCommitSelections] = useState<CommitSelectionMap>(() => new Map());
  const changesListRef = useRef<HTMLDivElement>(null);
  const [busyPath, setBusyPath] = useState<string | null>(null);
  const [commitBusy, setCommitBusy] = useState(false);
  const [operationBusy, setOperationBusy] = useState(false);
  const [pendingUndo, setPendingUndo] = useState(false);
  const [pendingDiscardAll, setPendingDiscardAll] = useState(false);
  const [summary, setSummary] = useState("");
  const [description, setDescription] = useState("");
  const [amend, setAmend] = useState(false);
  const [commitOptions, setCommitOptions] = useState<Set<CommitOptionSection>>(() => new Set());
  const [authorName, setAuthorName] = useState("");
  const [authorEmail, setAuthorEmail] = useState("");
  const [coAuthors, setCoAuthors] = useState("");
  const [trailers, setTrailers] = useState("");
  const [signing, setSigning] = useState<CommitSigning>("default");
  const [diffOpen, setDiffOpen] = useState(false);
  const [pendingResolution, setPendingResolution] = useState<{
    kind: ConflictResolutionKind;
    change: WorkingCopySnapshot["changes"][number];
  } | null>(null);
  const [pendingDiscard, setPendingDiscard] = useState<{
    change: WorkingCopySnapshot["changes"][number];
    scope: DiscardScope;
  } | null>(null);
  const [pendingOperationAction, setPendingOperationAction] = useState<RepositoryOperationAction | null>(null);

  // Mutations replace the snapshot with their own result, so a disk-triggered
  // reload while one is running would only race it.
  useMutationGuard(busyPath !== null || commitBusy || operationBusy);

  useEffect(() => {
    let active = true;
    void loadAppPreferences().then((preferences) => {
      if (active) setSigning(preferences.defaultSignCommits ? "sign" : "default");
    }).catch(() => undefined);
    return () => { active = false; };
  }, [worktree.id]);

  // `changes` is everything the commit form operates on; `visibleChanges` is
  // the subset the list shows after the filter box is applied.
  const changes = useMemo(() => snapshot?.changes.filter((change) => !change.ignored) ?? [], [snapshot]);
  const visibleChanges = useMemo(() => filterChanges(changes, filter), [changes, filter]);
  const visibleChangeIds = useMemo(() => visibleChanges.map((change) => change.id), [visibleChanges]);
  // When the active file disappears (snapshot refresh, filter change), fall
  // back to the first visible file rather than showing an empty diff pane.
  const changeSelection = useMemo(
    () => rawChangeSelection.activeId !== null && visibleChangeIds.includes(rawChangeSelection.activeId)
      ? rawChangeSelection
      : singleChangeSelection(visibleChangeIds[0] ?? null),
    [rawChangeSelection, visibleChangeIds],
  );
  const selectedChange = visibleChanges.find((change) => change.id === changeSelection.activeId) ?? null;
  // Only the diff body is expensive to build, so it alone follows the selection
  // at transition priority. The list highlight, the header, and every action
  // target stay on the urgent path so they always agree with the selection.
  const deferredActiveId = useDeferredValue(changeSelection.activeId);
  const diffChange = visibleChanges.find((change) => change.id === deferredActiveId) ?? null;
  // Held as state rather than a ref so the diff pane can bind its virtualized
  // rows to the element as soon as it exists.
  const [diffScroller, setDiffScroller] = useState<HTMLDivElement | null>(null);
  const includedCount = includedChangeCount(changes, commitSelections);
  const visibleIncludedCount = includedChangeCount(visibleChanges, commitSelections);
  const gitStagedCount = changes.filter((change) => change.staged).length;
  const allVisibleIncluded = visibleChanges.length > 0 && visibleIncludedCount === visibleChanges.length;

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

  type Change = WorkingCopySnapshot["changes"][number];
  const requestDiscard = (change: Change) => {
    if (busyPath !== null || change.conflicted) return;
    setPendingDiscard({ change, scope: change.unstaged || change.untracked ? "unstaged" : "all" });
  };
  const absolutePath = (change: Change) => worktree.path + separator + change.path.display.split("/").join(separator);
  const copyPath = async (change: Change) => {
    try {
      await navigator.clipboard.writeText(change.path.display);
      toast.add({ type: "success", title: "Path copied", description: change.path.display });
    } catch (cause) {
      toast.add({ type: "error", title: "Could not copy the path", description: toMessage(cause) });
    }
  };
  const revealChange = (change: Change) => {
    void revealWorktree(absolutePath(change)).catch((cause: unknown) => toast.add({ type: "error", title: "Could not reveal the file", description: toMessage(cause) }));
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
        includedChanges: commitSelectionRequest(changes, commitSelections),
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
        <div className="flex h-12 shrink-0 items-center gap-2 border-b px-3">
          <Checkbox
            checked={allVisibleIncluded}
            indeterminate={visibleIncludedCount > 0 && !allVisibleIncluded}
            disabled={visibleChanges.length === 0 || commitBusy || busyPath !== null}
            onCheckedChange={(checked) => setCommitSelections((current) => setChangesIncluded(
              current,
              new Set(visibleChanges.filter((change) => !change.conflicted).map((change) => change.id)),
              checked === true,
            ))}
            aria-label={allVisibleIncluded ? "Exclude all listed changes from commit" : "Include all listed changes in commit"}
          />
          <strong className="text-sm">Changes</strong>
          <Badge variant="secondary">{changes.length}</Badge>
          <div className="relative ml-auto min-w-0 flex-1 max-w-52">
            <SearchIcon className="pointer-events-none absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-muted-foreground" aria-hidden="true" />
            <Input
              value={filter}
              onChange={(event) => setFilter(event.currentTarget.value)}
              placeholder="Filter"
              aria-label="Filter changed files"
              className="h-7 pr-7 pl-7 text-xs"
              spellCheck={false}
              autoComplete="off"
            />
            {filter ? (
              <button type="button" className="absolute top-1/2 right-1 grid size-5 -translate-y-1/2 place-items-center rounded-sm text-muted-foreground hover:text-foreground" onClick={() => setFilter("")} aria-label="Clear filter">
                <XIcon className="size-3.5" aria-hidden="true" />
              </button>
            ) : null}
          </div>
          <DropdownMenu>
            <DropdownMenuTrigger render={<Button variant="ghost" size="icon-sm" aria-label="More change actions" />}>
              <MoreHorizontalIcon aria-hidden="true" />
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="min-w-56">
              <DropdownMenuGroup>
                <DropdownMenuItem disabled={changes.length === 0} onClick={() => setDiffOpen(true)}><FileDiffIcon aria-hidden="true" />Review complete diff…</DropdownMenuItem>
              </DropdownMenuGroup>
              <DropdownMenuGroup>
                <DropdownMenuItem variant="destructive" disabled={!snapshot || changes.length === 0 || commitBusy || busyPath !== null || snapshot.operation !== null} onClick={() => setPendingDiscardAll(true)}><Trash2Icon aria-hidden="true" />Discard all changes…</DropdownMenuItem>
              </DropdownMenuGroup>
            </DropdownMenuContent>
          </DropdownMenu>
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
          aria-keyshortcuts="Meta+A Control+A Space ArrowUp ArrowDown Home End Delete"
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
            if ((event.key === "Delete" || event.key === "Backspace") && !event.metaKey && !event.ctrlKey && !event.altKey) {
              if (event.target instanceof Element && event.target.closest('input, textarea, [role="checkbox"]')) return;
              event.preventDefault();
              if (selectedChange) requestDiscard(selectedChange);
              return;
            }
            if (!isToggleSelectedChangesShortcut(event)) return;
            if (event.target instanceof Element && event.target.closest('[role="checkbox"]')) return;
            event.preventDefault();
            toggleSelectedCommitInclusion();
          }}
        >
          {visibleChanges.map((change) => (
            <ContextMenu key={change.id}>
              <ContextMenuTrigger
                render={<div className={cn("repola-windowed-row flex min-h-8 [--windowed-row-size:32px] items-center border-b", changeSelection.selectedIds.has(change.id) && "bg-accent")} />}
                onContextMenu={() => {
                  // Right-clicking an unselected row targets that row alone, the
                  // same as a plain click, so the menu never acts on a hidden selection.
                  if (!changeSelection.selectedIds.has(change.id)) setChangeSelection(singleChangeSelection(change.id));
                }}
              >
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
                <ChangeStatusIcon kind={change.kind} conflicted={change.conflicted} />
              </button>
              </ContextMenuTrigger>
              <ContextMenuContent className="min-w-56">
                <ContextMenuGroup>
                  <ContextMenuItem onClick={() => void copyPath(change)}><CopyIcon aria-hidden="true" />Copy path</ContextMenuItem>
                  <ContextMenuItem disabled={machineKind !== "local" || change.kind === "deleted"} onClick={() => revealChange(change)}><FolderOpenIcon aria-hidden="true" />Reveal in file manager</ContextMenuItem>
                </ContextMenuGroup>
                <ContextMenuSeparator />
                <ContextMenuGroup>
                  <ContextMenuItem variant="destructive" disabled={busyPath !== null || change.conflicted || snapshot?.operation !== null} onClick={() => requestDiscard(change)}><Trash2Icon aria-hidden="true" />Discard changes…</ContextMenuItem>
                </ContextMenuGroup>
              </ContextMenuContent>
            </ContextMenu>
          ))}
          {visibleChanges.length === 0 ? (
            changes.length === 0 ? (
              <Empty className="h-full py-12">
                <EmptyHeader>
                  <EmptyMedia variant="icon"><ShieldCheckIcon aria-hidden="true" /></EmptyMedia>
                  <EmptyTitle>No local changes</EmptyTitle>
                  <EmptyDescription>This working copy matches its current commit.</EmptyDescription>
                </EmptyHeader>
              </Empty>
            ) : (
              <Empty className="h-full py-12">
                <EmptyHeader>
                  <EmptyMedia variant="icon"><SearchXIcon aria-hidden="true" /></EmptyMedia>
                  <EmptyTitle>No matching files</EmptyTitle>
                  <EmptyDescription>None of the {changes.length} changed files match “{filter.trim()}”.</EmptyDescription>
                </EmptyHeader>
                <Button variant="outline" size="sm" onClick={() => setFilter("")}>Clear filter</Button>
              </Empty>
            )
          ) : null}
        </div>
        <form className="flex max-h-[58%] shrink-0 flex-col gap-2 overflow-y-auto border-t bg-card p-3" onSubmit={(event) => void submitCommit(event)}>
          <Input value={summary} onChange={(event) => setSummary(event.currentTarget.value)} placeholder="Summary (required)" maxLength={998} disabled={commitBusy} />
          <Textarea value={description} onChange={(event) => setDescription(event.currentTarget.value)} placeholder="Description" className="min-h-16 resize-none" disabled={commitBusy} />
          <div className="flex items-center gap-0.5" role="group" aria-label="Commit options">
            {commitOptionSections.map(({ id, label, icon: Icon }) => {
              const open = commitOptions.has(id);
              const filled = id === "author"
                ? authorName.trim() !== "" || authorEmail.trim() !== ""
                : id === "trailers"
                  ? coAuthors.trim() !== "" || trailers.trim() !== ""
                  : signing !== "default";
              return (
                <TooltipButton
                  key={id}
                  type="button"
                  variant={open ? "secondary" : "ghost"}
                  size="icon-sm"
                  className={cn(!open && filled && "text-brand")}
                  aria-pressed={open}
                  aria-label={label}
                  tooltip={label}
                  disabled={commitBusy}
                  onClick={() => setCommitOptions((current) => {
                    const next = new Set(current);
                    if (next.has(id)) next.delete(id);
                    else next.add(id);
                    return next;
                  })}
                >
                  <Icon aria-hidden="true" />
                </TooltipButton>
              );
            })}
            <label className="ml-auto flex items-center gap-2 text-xs text-muted-foreground">
              <Checkbox checked={amend} disabled={commitBusy || snapshot?.head === null} onCheckedChange={(value) => setAmend(value === true)} />
              Amend latest commit
            </label>
          </div>
          {commitOptions.has("author") ? (
            <div className="flex flex-col gap-2 border bg-muted/30 p-2.5">
              <span className={sectionHeadingClass}>Author override</span>
              <div className="grid grid-cols-2 gap-2">
                <Input value={authorName} onChange={(event) => setAuthorName(event.currentTarget.value)} placeholder="Name (use Git config)" maxLength={200} disabled={commitBusy} aria-label="Commit author name" />
                <Input value={authorEmail} onChange={(event) => setAuthorEmail(event.currentTarget.value)} placeholder="Email (use Git config)" maxLength={320} disabled={commitBusy} aria-label="Commit author email" />
              </div>
            </div>
          ) : null}
          {commitOptions.has("trailers") ? (
            <div className="flex flex-col gap-2 border bg-muted/30 p-2.5">
              <FieldLabel htmlFor="commit-coauthors">Co-authors</FieldLabel>
              <Textarea id="commit-coauthors" value={coAuthors} onChange={(event) => setCoAuthors(event.currentTarget.value)} placeholder={"Name <email@example.com>\nOne co-author per line"} className="min-h-16 resize-y font-mono text-xs" disabled={commitBusy} />
              <FieldLabel htmlFor="commit-trailers">Additional trailers</FieldLabel>
              <Textarea id="commit-trailers" value={trailers} onChange={(event) => setTrailers(event.currentTarget.value)} placeholder={"Reviewed-by: Name\nIssue: 123"} className="min-h-16 resize-y font-mono text-xs" disabled={commitBusy} />
            </div>
          ) : null}
          {commitOptions.has("signing") ? (
            <div className="flex flex-col gap-2 border bg-muted/30 p-2.5">
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
              <DialogTitle>Discard all {changes.length} changed files?</DialogTitle>
              <DialogDescription>Repola will revalidate HEAD and the complete reviewed path/status set immediately before Git changes anything.</DialogDescription>
            </DialogHeader>
            <div className="grid grid-cols-3 border bg-muted/35">
              <div className="border-r p-3 text-center"><strong className="block font-mono text-lg">{gitStagedCount}</strong><span className="text-xs text-muted-foreground">staged</span></div>
              <div className="border-r p-3 text-center"><strong className="block font-mono text-lg">{changes.filter((change) => change.unstaged && !change.untracked).length}</strong><span className="text-xs text-muted-foreground">unstaged</span></div>
              <div className="p-3 text-center"><strong className="block font-mono text-lg">{changes.filter((change) => change.untracked).length}</strong><span className="text-xs text-muted-foreground">untracked</span></div>
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
