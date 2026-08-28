import { useEffect, useState, type FormEvent } from "react";
import {
  ArchiveIcon,
  DatabaseIcon,
  FolderOpenIcon,
  FolderPlusIcon,
  GitBranchIcon,
  GitMergeIcon,
  HardDriveIcon,
  MoreHorizontalIcon,
  PencilIcon,
  PlusIcon,
  TagIcon,
  Trash2Icon,
  TriangleAlertIcon,
} from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { DropdownMenu, DropdownMenuContent, DropdownMenuGroup, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Field, FieldLabel } from "@/components/ui/field";
import { StashDialog } from "../dialogs/StashDialog";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectGroup, SelectItem, SelectLabel } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { toast } from "@/components/ui/toast";
import { toMessage } from "@/lib/errors";
import { ActionableGitError } from "../components/ActionableGitError";
import type { HistoryTarget } from "../dialogs/HistoryMutationDialog";
import { formatMeasuredBytes, shortSha } from "../domain/format";
import { loadBranches, mutateBranch, revealWorktree } from "../ipc/worktrees";
import type { BranchInfo, RepositorySummary, WorktreeRecord } from "../ipc/types";
import { ToolbarButton, ToolbarDivider, ToolbarSelectTrigger } from "./ContextHeader";
import { useRepositoryContext, useWorkingCopy } from "./context";
import { SyncControls } from "./SyncControls";
import { useOptionalWorkingCopyState } from "./working-copy-state";
import { historyMutationTitles } from "./labels";
import { LazyDialog } from "./LazyDialog";
import { HistoryMutationDialog, TagsDialog } from "./lazy";

export function RepositoryToolbar({
  repositories,
  worktrees,
  onRepositoryChange,
  onWorktreeChange,
  onCreateWorktree,
  onAddRepository,
  onRemoveRepository,
}: {
  repositories: RepositorySummary[];
  worktrees: WorktreeRecord[];
  onRepositoryChange: (path: string) => void;
  onWorktreeChange: (path: string) => void;
  onCreateWorktree: () => void;
  onAddRepository: () => void;
  onRemoveRepository: (repositoryPath: string) => Promise<void>;
}) {
  const { machineId, machineKind, repository, worktree, view, showView } = useRepositoryContext();
  const workingCopy = useOptionalWorkingCopyState();
  const [pendingRepositoryRemoval, setPendingRepositoryRemoval] = useState(false);
  const [stashOpen, setStashOpen] = useState(false);
  const repositoryItems = Object.fromEntries(repositories.map((item) => [item.path, item.name]));
  const worktreeLabel = (item: WorktreeRecord) => item.branch ?? `Detached at ${shortSha(item.head)}`;
  const attention = repository ? repository.conflictedCount + repository.attentionCount : 0;
  return (
    <>
      <div className="flex min-w-0 items-center gap-0.5" role="group" aria-label="Repository context">
        <Select items={repositoryItems} value={repository?.path ?? null} onValueChange={(value) => { if (value) onRepositoryChange(value); }}>
          <ToolbarSelectTrigger id="current-repository" caption="Repository" icon={<DatabaseIcon />} placeholder="Select a repository" aria-label="Current repository" className="w-60 flex-none" />
          <SelectContent className="min-w-80" align="start" alignItemWithTrigger={false}>
            <SelectGroup>
              {repositories.map((item) => (
                <SelectItem key={item.path} value={item.path} label={item.name}>
                  <span className="flex min-w-64 items-center gap-2">
                    <span className="min-w-0 flex-1"><span className="block truncate">{item.name}</span><span className="block text-xs text-muted-foreground">{item.worktreeCount} worktree{item.worktreeCount === 1 ? "" : "s"} · {formatMeasuredBytes(item.allocatedBytes, item.allocationIncomplete)}</span></span>
                    {item.conflictedCount > 0 ? <Badge variant="destructive">{item.conflictedCount} conflicted</Badge> : item.attentionCount > 0 ? <Badge variant="warning">{item.attentionCount} attention</Badge> : <Badge variant="success">healthy</Badge>}
                  </span>
                </SelectItem>
              ))}
            </SelectGroup>
          </SelectContent>
        </Select>
        <DropdownMenu>
          <DropdownMenuTrigger render={<Button variant="ghost" size="icon-sm" aria-label="Repository actions" />}>
            <MoreHorizontalIcon aria-hidden="true" />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start" className="min-w-56">
            <DropdownMenuGroup>
              <DropdownMenuItem onClick={onAddRepository}><PlusIcon aria-hidden="true" />Add repository…</DropdownMenuItem>
            </DropdownMenuGroup>
            {repository ? (
              <>
                <DropdownMenuSeparator />
                <DropdownMenuGroup>
                  <DropdownMenuLabel>{repository.name}</DropdownMenuLabel>
                  <DropdownMenuItem disabled={!worktree} onClick={onCreateWorktree}><FolderPlusIcon aria-hidden="true" />Create linked worktree…</DropdownMenuItem>
                  <DropdownMenuItem disabled={!workingCopy?.snapshot || workingCopy.snapshot.operation !== null} onClick={() => setStashOpen(true)}><ArchiveIcon aria-hidden="true" />Stashes…</DropdownMenuItem>
                  <DropdownMenuItem disabled={machineKind !== "local"} onClick={() => { void revealWorktree(repository.path).catch((cause: unknown) => toast.add({ type: "error", title: "Could not reveal repository", description: toMessage(cause) })); }}><FolderOpenIcon aria-hidden="true" />Reveal in file manager</DropdownMenuItem>
                </DropdownMenuGroup>
                <DropdownMenuSeparator />
                <DropdownMenuGroup>
                  <DropdownMenuItem variant="destructive" onClick={() => setPendingRepositoryRemoval(true)}><Trash2Icon aria-hidden="true" />Remove from Repola…</DropdownMenuItem>
                </DropdownMenuGroup>
              </>
            ) : null}
          </DropdownMenuContent>
        </DropdownMenu>
        <ToolbarDivider />
        <DropdownMenu>
          <DropdownMenuTrigger
            disabled={!repository}
            render={<ToolbarButton caption="Worktree" icon={<FolderOpenIcon />} className="w-56 flex-none aria-expanded:bg-muted" aria-label="Current worktree" />}
          >
            {worktree ? worktreeLabel(worktree) : "Select a worktree"}
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start" className="min-w-80">
            <DropdownMenuGroup>
              <DropdownMenuLabel>{repository?.name ?? "Worktrees"} · {worktrees.length} worktree{worktrees.length === 1 ? "" : "s"}</DropdownMenuLabel>
              {worktrees.map((item) => (
                <DropdownMenuItem key={item.id} onClick={() => onWorktreeChange(item.path)} aria-current={item.path === worktree?.path ? "true" : undefined} className="items-start">
                  <span className={item.path === worktree?.path ? "mt-0.5 size-4 shrink-0 text-brand" : "mt-0.5 size-4 shrink-0 text-muted-foreground"} aria-hidden="true"><FolderOpenIcon className="size-4" /></span>
                  <span className="min-w-0 flex-1">
                    <span className="flex items-center gap-2">
                      <span className="truncate">{worktreeLabel(item)}</span>
                      {item.isPrimary ? <Badge variant="outline">primary</Badge> : null}
                      {item.safety.level === "review" || item.safety.level === "repair" ? <Badge variant="warning" className="ml-auto">{item.safety.label}</Badge> : null}
                    </span>
                    <span className="block truncate text-xs text-muted-foreground">
                      {item.status.available ? `${item.status.total} changed` : "status unavailable"}
                      {item.status.conflicted > 0 ? ` · ${item.status.conflicted} conflicted` : ""}
                      {item.sizeBytes !== null ? ` · ${formatMeasuredBytes(item.sizeBytes, item.sizeIncomplete)}` : ""}
                    </span>
                  </span>
                </DropdownMenuItem>
              ))}
            </DropdownMenuGroup>
            <DropdownMenuSeparator />
            <DropdownMenuGroup>
              <DropdownMenuItem disabled={!worktree} onClick={onCreateWorktree}><FolderPlusIcon aria-hidden="true" />Create linked worktree…</DropdownMenuItem>
              <DropdownMenuItem onClick={() => showView("worktrees")}><HardDriveIcon aria-hidden="true" />Manage worktrees</DropdownMenuItem>
            </DropdownMenuGroup>
          </DropdownMenuContent>
        </DropdownMenu>
        <ToolbarDivider />
        {repository && worktree ? (
          <BranchControl />
        ) : (
          <span className="w-60 px-2 text-sm text-muted-foreground">No branch</span>
        )}
        <ToolbarDivider />
        <SyncControls />
        <ToolbarDivider />
        <ToolbarButton
          caption="Worktrees"
          icon={<HardDriveIcon />}
          active={view === "worktrees"}
          className="w-44 flex-none"
          onClick={() => showView("worktrees")}
          trailing={attention > 0 ? <TriangleAlertIcon className="size-4 shrink-0 text-warning" aria-hidden="true" /> : null}
        >
          {repository
            ? `${repository.worktreeCount} on disk${attention > 0 ? ` · ${attention} need${attention === 1 ? "s" : ""} attention` : ""}`
            : "No repository"}
        </ToolbarButton>
      </div>
      {stashOpen && workingCopy?.snapshot && repository && worktree ? (
        <StashDialog
          machineId={machineId}
          repository={repository}
          worktree={worktree}
          snapshot={workingCopy.snapshot}
          onSnapshot={workingCopy.setSnapshot}
          onClose={() => setStashOpen(false)}
        />
      ) : null}
      <Dialog open={pendingRepositoryRemoval} onOpenChange={setPendingRepositoryRemoval}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Remove {repository?.name ?? "this repository"} from Repola?</DialogTitle>
            <DialogDescription>This only hides the repository on {machineKind === "local" ? "this computer" : "the selected remote machine"}. No files, commits, branches, worktrees, or Git configuration will be changed.</DialogDescription>
          </DialogHeader>
          <code className="border bg-muted p-3 font-mono text-xs break-all">{repository?.path}</code>
          <DialogFooter>
            <Button variant="outline" onClick={() => setPendingRepositoryRemoval(false)}>Cancel</Button>
            <Button variant="destructive" disabled={!repository} onClick={() => { if (repository) void onRemoveRepository(repository.path).then(() => setPendingRepositoryRemoval(false)); }}>Remove from Repola</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}

function BranchControl() {
  const { machineId, repository, worktree, refreshWorkspace: onChanged, showChanges: onNeedsResolution } = useWorkingCopy();
  const [branches, setBranches] = useState<BranchInfo[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [dialogKind, setDialogKind] = useState<"create" | "rename" | null>(null);
  const [branchName, setBranchName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [historyActionsOpen, setHistoryActionsOpen] = useState(false);
  const [tagsOpen, setTagsOpen] = useState(false);

  useEffect(() => {
    const controller = new AbortController();
    setBranches(null);
    setError(null);
    void loadBranches(machineId, repository.path, worktree.path, controller.signal)
      .then(setBranches)
      .catch((cause: unknown) => {
        if (!controller.signal.aborted) setError(toMessage(cause));
      });
    return () => controller.abort();
  }, [machineId, repository.path, worktree.id, worktree.path, worktree.head]);

  const current = branches?.find((branch) => branch.current && !branch.remote) ?? null;
  const localBranches = branches?.filter((branch) => !branch.remote) ?? [];
  const remoteBranches = branches?.filter((branch) => branch.remote) ?? [];
  const items = Object.fromEntries((branches ?? []).map((branch) => [branch.fullName, branch.name]));
  const historyTargets: HistoryTarget[] = (branches ?? [])
    .filter((branch) => !branch.current)
    .map((branch) => ({
      id: branch.fullName,
      oid: branch.head,
      label: branch.name,
      detail: branch.remote ? "Remote branch" : "Local branch",
    }));

  const runMutation = async (
    kind: "create" | "checkout" | "rename",
    name: string,
    startPoint: string | null = null,
  ) => {
    setBusy(true);
    setError(null);
    try {
      const result = await mutateBranch(
        machineId,
        repository.path,
        worktree.path,
        kind,
        name,
        startPoint,
        worktree.head,
      );
      setBranches(result.branches);
      setDialogKind(null);
      setBranchName("");
      await onChanged();
      toast.add({
        type: "success",
        title: kind === "create" ? "Branch created" : kind === "rename" ? "Branch renamed" : "Branch checked out",
        description: result.snapshot.branch ?? `Detached at ${shortSha(result.snapshot.head)}`,
      });
    } catch (cause) {
      const message = toMessage(cause);
      setError(message);
      toast.add({ type: "error", title: "Branch action failed", description: message });
    } finally {
      setBusy(false);
    }
  };

  const selectBranch = (fullName: string | null) => {
    if (!fullName || busy || fullName === current?.fullName) return;
    const branch = branches?.find((item) => item.fullName === fullName);
    if (branch) void runMutation("checkout", branch.name);
  };

  const openDialog = (kind: "create" | "rename") => {
    setError(null);
    setBranchName(kind === "rename" ? current?.name ?? "" : "");
    setDialogKind(kind);
  };

  const submitDialog = (event: FormEvent) => {
    event.preventDefault();
    const name = branchName.trim();
    if (!name || !dialogKind) return;
    void runMutation(dialogKind, name);
  };

  return (
    <>
      <Select items={items} value={current?.fullName ?? null} disabled={busy || branches === null} onValueChange={selectBranch}>
        <ToolbarSelectTrigger id="current-branch" caption="Branch" icon={busy || branches === null ? <Spinner /> : <GitBranchIcon />} placeholder="Select a branch" aria-label="Current branch" className="w-60 flex-none">
          {current?.name ?? (worktree.branch ? worktree.branch : `Detached at ${shortSha(worktree.head)}`)}
        </ToolbarSelectTrigger>
        <SelectContent className="min-w-72" align="start" alignItemWithTrigger={false}>
          <SelectGroup>
            <SelectLabel>Local branches</SelectLabel>
            {localBranches.map((branch) => {
              const occupiedElsewhere = branch.occupiedWorktreePath !== null
                && branch.occupiedWorktreePath !== worktree.path;
              return (
                <SelectItem key={branch.fullName} value={branch.fullName} disabled={occupiedElsewhere}>
                  <span className="flex min-w-0 flex-1 items-center gap-2">
                    <span className="truncate">{branch.name}</span>
                    {branch.ahead > 0 ? <Badge variant="outline">↑{branch.ahead}</Badge> : null}
                    {branch.behind > 0 ? <Badge variant="outline">↓{branch.behind}</Badge> : null}
                    {occupiedElsewhere ? <span className="ml-auto max-w-36 truncate text-xs text-muted-foreground">in {branch.occupiedWorktreePath}</span> : null}
                  </span>
                </SelectItem>
              );
            })}
          </SelectGroup>
          {remoteBranches.length > 0 ? (
            <SelectGroup>
              <SelectLabel>Remote branches</SelectLabel>
              {remoteBranches.map((branch) => (
                <SelectItem key={branch.fullName} value={branch.fullName}>
                  <span className="truncate text-muted-foreground">{branch.name}</span>
                </SelectItem>
              ))}
            </SelectGroup>
          ) : null}
        </SelectContent>
      </Select>
      <DropdownMenu>
        <DropdownMenuTrigger render={<Button variant="ghost" size="icon-sm" disabled={busy || branches === null} aria-label="Branch actions" />}>
          <MoreHorizontalIcon aria-hidden="true" />
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="min-w-56">
          <DropdownMenuGroup>
            <DropdownMenuLabel>{current?.name ?? "Detached HEAD"}</DropdownMenuLabel>
            <DropdownMenuItem onClick={() => openDialog("create")}><PlusIcon aria-hidden="true" />Create branch…</DropdownMenuItem>
            <DropdownMenuItem disabled={current === null} onClick={() => openDialog("rename")}><PencilIcon aria-hidden="true" />Rename branch…</DropdownMenuItem>
          </DropdownMenuGroup>
          <DropdownMenuSeparator />
          <DropdownMenuGroup>
            <DropdownMenuItem disabled={current === null || historyTargets.length === 0} onClick={() => setHistoryActionsOpen(true)}><GitMergeIcon aria-hidden="true" />Merge or rebase…</DropdownMenuItem>
            <DropdownMenuItem disabled={worktree.head === null} onClick={() => setTagsOpen(true)}><TagIcon aria-hidden="true" />Tags…</DropdownMenuItem>
          </DropdownMenuGroup>
        </DropdownMenuContent>
      </DropdownMenu>
      <Dialog open={dialogKind !== null} onOpenChange={(open) => { if (!open && !busy) setDialogKind(null); }}>
        <DialogContent>
          <form className="contents" onSubmit={submitDialog}>
            <DialogHeader>
              <DialogTitle>{dialogKind === "rename" ? "Rename branch" : "Create a branch"}</DialogTitle>
              <DialogDescription>
                {dialogKind === "rename"
                  ? `Rename ${current?.name ?? "the current branch"}. Git will preserve its commits and upstream configuration.`
                  : `Create a branch from ${current?.name ?? shortSha(worktree.head)} and check it out in this worktree.`}
              </DialogDescription>
            </DialogHeader>
            <Field>
              <FieldLabel htmlFor="branch-name">Branch name</FieldLabel>
              <Input
                id="branch-name"
                autoFocus
                autoComplete="off"
                spellCheck={false}
                value={branchName}
                disabled={busy}
                onChange={(event) => setBranchName(event.currentTarget.value)}
              />
            </Field>
            {error ? <ActionableGitError message={error} /> : null}
            <DialogFooter>
              <Button type="button" variant="outline" disabled={busy} onClick={() => setDialogKind(null)}>Cancel</Button>
              <Button type="submit" disabled={busy || branchName.trim() === "" || (dialogKind === "rename" && branchName.trim() === current?.name)}>
                {busy ? <Spinner data-icon="inline-start" /> : dialogKind === "rename" ? <PencilIcon data-icon="inline-start" aria-hidden="true" /> : <PlusIcon data-icon="inline-start" aria-hidden="true" />}
                {busy ? "Working…" : dialogKind === "rename" ? "Rename Branch" : "Create Branch"}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
      {historyActionsOpen ? (
        <LazyDialog onClose={() => setHistoryActionsOpen(false)}>
          <HistoryMutationDialog
            machineId={machineId}
            repository={repository}
            worktree={worktree}
            kinds={["merge", "squashMerge", "rebase"]}
            targets={historyTargets}
            initialKind="merge"
            title={`Update ${current?.name ?? "current branch"}`}
            onClose={() => setHistoryActionsOpen(false)}
            onCompleted={async (result, kind, target) => {
              await onChanged();
              setBranches(await loadBranches(machineId, repository.path, worktree.path));
              if (result.conflicted || result.snapshot.operation) {
                toast.add({ type: "warning", title: "Conflict resolution required", description: `${target.label} could not be applied cleanly. Resolve the conflicted files, then continue or abort the operation.` });
                onNeedsResolution();
                return;
              }
              toast.add({ type: "success", title: historyMutationTitles[kind], description: kind === "squashMerge" ? `Review and commit the staged changes from ${target.label}.` : target.label });
            }}
          />
        </LazyDialog>
      ) : null}
      {tagsOpen ? (
        <LazyDialog onClose={() => setTagsOpen(false)}>
          <TagsDialog machineId={machineId} repository={repository} worktree={worktree} onClose={() => setTagsOpen(false)} onChanged={async () => { await onChanged(); }} />
        </LazyDialog>
      ) : null}
    </>
  );
}
