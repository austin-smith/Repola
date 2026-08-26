import { useEffect, useState, type FormEvent, type ReactNode } from "react";
import {
  DatabaseIcon,
  FileDiffIcon,
  FolderOpenIcon,
  FolderPlusIcon,
  GitBranchIcon,
  GitCommitIcon,
  GitMergeIcon,
  HardDriveIcon,
  MoreHorizontalIcon,
  PencilIcon,
  PlusIcon,
  TagIcon,
  Trash2Icon,
} from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { DropdownMenu, DropdownMenuContent, DropdownMenuGroup, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Field, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectGroup, SelectItem, SelectLabel, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { toast } from "@/components/ui/toast";
import { TooltipButton } from "@/components/tooltip-button";
import { toMessage } from "@/lib/errors";
import { ActionableGitError } from "../ActionableGitError";
import type { HistoryTarget } from "../HistoryMutationDialog";
import { formatMeasuredBytes, shortSha } from "../format";
import { loadBranches, mutateBranch, revealWorktree } from "../worktrees";
import type { BranchInfo, RepositorySummary, WorkspaceView, WorktreeRecord } from "../types";
import { useRepositoryContext, useWorkingCopy } from "./context";
import { historyMutationTitles } from "./labels";
import { LazyDialog } from "./LazyDialog";
import { HistoryMutationDialog, TagsDialog } from "./lazy";

function ContextToolbarField({
  id,
  label,
  children,
}: {
  id: string;
  label: string;
  children: ReactNode;
}) {
  return (
    <Field className="min-w-0">
      <FieldLabel htmlFor={id}>{label}</FieldLabel>
      <div className="flex min-w-0 items-center gap-1">
        {children}
      </div>
    </Field>
  );
}

export function RepositoryToolbar({
  repositories,
  worktrees,
  view,
  onRepositoryChange,
  onWorktreeChange,
  onViewChange,
  onCreateWorktree,
  onRemoveRepository,
}: {
  repositories: RepositorySummary[];
  worktrees: WorktreeRecord[];
  view: WorkspaceView;
  onRepositoryChange: (path: string) => void;
  onWorktreeChange: (path: string) => void;
  onViewChange: (view: WorkspaceView) => void;
  onCreateWorktree: () => void;
  onRemoveRepository: (repositoryPath: string) => Promise<void>;
}) {
  const { machineKind, repository, worktree } = useRepositoryContext();
  const [pendingRepositoryRemoval, setPendingRepositoryRemoval] = useState(false);
  const repositoryItems = Object.fromEntries(repositories.map((item) => [item.path, item.name]));
  const worktreeItems = Object.fromEntries(worktrees.map((item) => [
    item.path,
    item.branch ?? `Detached at ${shortSha(item.head)}`,
  ]));
  return (
    <section className="flex shrink-0 items-stretch border-b bg-card" aria-label="Repository context">
      <div className="flex min-w-0 flex-1 border-r px-4 py-3">
        <ContextToolbarField id="current-repository" label="Repository">
          <Select items={repositoryItems} value={repository?.path ?? null} onValueChange={(value) => { if (value) onRepositoryChange(value); }}>
            <SelectTrigger id="current-repository" className="min-w-0 flex-1" aria-label="Current repository">
              <DatabaseIcon aria-hidden="true" />
              <SelectValue placeholder="Select a repository" />
            </SelectTrigger>
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
          <TooltipButton variant="ghost" size="icon-sm" onClick={onCreateWorktree} disabled={!repository || !worktree} aria-label="Create linked worktree" tooltip="Create linked worktree">
            <FolderPlusIcon aria-hidden="true" />
          </TooltipButton>
          <Tooltip>
            <TooltipTrigger render={<span className="inline-flex w-fit" tabIndex={repository ? undefined : 0} />}>
              <DropdownMenu>
                <DropdownMenuTrigger render={<Button variant="ghost" size="icon-sm" disabled={!repository} aria-label="Repository actions" />}>
                  <MoreHorizontalIcon aria-hidden="true" />
                </DropdownMenuTrigger>
                <DropdownMenuContent align="start" className="min-w-56">
                  <DropdownMenuGroup>
                    <DropdownMenuLabel>{repository?.name ?? "Repository"}</DropdownMenuLabel>
                    <DropdownMenuItem disabled={machineKind !== "local" || !repository} onClick={() => { if (repository) void revealWorktree(repository.path).catch((cause: unknown) => toast.add({ type: "error", title: "Could not reveal repository", description: toMessage(cause) })); }}><FolderOpenIcon aria-hidden="true" />Reveal in file manager</DropdownMenuItem>
                  </DropdownMenuGroup>
                  <DropdownMenuSeparator />
                  <DropdownMenuGroup>
                    <DropdownMenuItem variant="destructive" disabled={!repository} onClick={() => setPendingRepositoryRemoval(true)}><Trash2Icon aria-hidden="true" />Remove from Repola…</DropdownMenuItem>
                  </DropdownMenuGroup>
                </DropdownMenuContent>
              </DropdownMenu>
            </TooltipTrigger>
            <TooltipContent>Repository actions</TooltipContent>
          </Tooltip>
        </ContextToolbarField>
      </div>
      <div className="flex min-w-0 flex-1 border-r px-4 py-3">
        <ContextToolbarField id="current-worktree" label="Worktree">
          <Select items={worktreeItems} value={worktree?.path ?? null} onValueChange={(value) => { if (value) onWorktreeChange(value); }}>
            <SelectTrigger id="current-worktree" className="min-w-0 flex-1" aria-label="Current worktree">
              <FolderOpenIcon aria-hidden="true" />
              <SelectValue placeholder="Select a worktree" />
            </SelectTrigger>
            <SelectContent>
              <SelectGroup>
                {worktrees.map((item) => (
                  <SelectItem key={item.id} value={item.path}>
                    {item.branch ?? `Detached at ${shortSha(item.head)}`}{item.isPrimary ? " · primary" : ""}
                  </SelectItem>
                ))}
              </SelectGroup>
            </SelectContent>
          </Select>
        </ContextToolbarField>
      </div>
      <div className="flex min-w-64 flex-1 items-center border-r px-4 py-3">
        {repository && worktree ? (
          <BranchControl />
        ) : (
          <span className="text-sm text-muted-foreground">No branch</span>
        )}
      </div>
      <div className="ml-auto flex items-center gap-1 px-4">
        <Button variant={view === "changes" ? "secondary" : "ghost"} size="sm" onClick={() => onViewChange("changes")}>
          <FileDiffIcon data-icon="inline-start" aria-hidden="true" />
          Changes
        </Button>
        <Button variant={view === "history" ? "secondary" : "ghost"} size="sm" onClick={() => onViewChange("history")}>
          <GitCommitIcon data-icon="inline-start" aria-hidden="true" />
          History
        </Button>
        <Button variant={view === "worktrees" ? "secondary" : "ghost"} size="sm" onClick={() => onViewChange("worktrees")}>
          <HardDriveIcon data-icon="inline-start" aria-hidden="true" />
          Worktrees
        </Button>
      </div>
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
    </section>
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
    <div className="min-w-0 flex-1">
      <ContextToolbarField id="current-branch" label="Branch">
        <Select items={items} value={current?.fullName ?? null} disabled={busy || branches === null} onValueChange={selectBranch}>
          <SelectTrigger id="current-branch" className="min-w-0 flex-1" aria-label="Current branch">
            {busy || branches === null
              ? <Spinner />
              : <GitBranchIcon aria-hidden="true" />}
            <SelectValue placeholder="Select a branch">
              {current?.name ?? (worktree.branch ? worktree.branch : `Detached at ${shortSha(worktree.head)}`)}
            </SelectValue>
          </SelectTrigger>
          <SelectContent className="min-w-72" align="start">
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
        <TooltipButton variant="ghost" size="icon-sm" disabled={busy || branches === null} tooltip="Create branch" aria-label="Create branch" onClick={() => openDialog("create")}>
          <PlusIcon aria-hidden="true" />
        </TooltipButton>
        <TooltipButton variant="ghost" size="icon-sm" disabled={busy || current === null} tooltip="Rename current branch" aria-label="Rename current branch" onClick={() => openDialog("rename")}>
          <PencilIcon aria-hidden="true" />
        </TooltipButton>
        <TooltipButton variant="ghost" size="icon-sm" disabled={busy || current === null || historyTargets.length === 0} tooltip="Merge or rebase" aria-label="Merge or rebase" onClick={() => setHistoryActionsOpen(true)}>
          <GitMergeIcon aria-hidden="true" />
        </TooltipButton>
        <TooltipButton variant="ghost" size="icon-sm" disabled={busy || worktree.head === null} tooltip="Tags" aria-label="Tags" onClick={() => setTagsOpen(true)}>
          <TagIcon aria-hidden="true" />
        </TooltipButton>
      </ContextToolbarField>
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
    </div>
  );
}
