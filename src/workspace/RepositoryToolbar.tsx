import { useEffect, useMemo, useState, type FormEvent } from "react";
import {
  ChevronRightIcon,
  DatabaseIcon,
  FolderOpenIcon,
  FolderPlusIcon,
  GitBranchIcon,
  GitMergeIcon,
  MoreHorizontalIcon,
  PencilIcon,
  PlusIcon,
  TagIcon,
  Trash2Icon,
} from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Combobox, ComboboxCollection, ComboboxContent, ComboboxEmpty, ComboboxGroup, ComboboxInput, ComboboxItem, ComboboxLabel, ComboboxList, ComboboxSeparator, ComboboxTrigger, ComboboxValue } from "@/components/ui/combobox";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { DropdownMenu, DropdownMenuContent, DropdownMenuGroup, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Field, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectGroup, SelectItem, SelectLabel, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { toast } from "@/components/ui/toast";
import { toMessage } from "@/lib/errors";
import { fileManagerName, machinePathSeparator } from "../domain/platform";
import { usePathSeparator } from "../app/environment";
import { ActionableGitError } from "../components/ActionableGitError";
import type { HistoryTarget } from "../dialogs/HistoryMutationDialog";
import { deletionNotice } from "../domain/branch-deletion-outcomes";
import { shortSha } from "../domain/format";
import { matchesRepositoryQuery } from "../domain/repository-picker";
import { groupWorktreesForPicker, matchesWorktreeQuery, worktreeBranchLabel, worktreeFolderName, type WorktreePickerGroup } from "../domain/worktree-picker";
import { loadBranches, mutateBranch, revealWorktree } from "../ipc/worktrees";
import type { BranchDeletionResult, BranchInfo, RepositorySummary, WorktreeRecord } from "../ipc/types";
import { useRepositoryContext, useWorkingCopy } from "./context";
import { historyMutationTitles } from "./labels";
import { LazyDialog } from "./LazyDialog";
import { DeleteBranchDialog, HistoryMutationDialog, TagsDialog } from "./lazy";

export function RepositoryToolbar({
  repositories,
  worktrees,
  onRepositoryChange,
  onWorktreeChange,
  onAddRepository,
  onCreateWorktree,
  onRemoveRepository,
}: {
  repositories: RepositorySummary[];
  worktrees: WorktreeRecord[];
  onRepositoryChange: (path: string) => void;
  onWorktreeChange: (path: string) => void;
  onAddRepository: () => void;
  onCreateWorktree: () => void;
  onRemoveRepository: (repositoryPath: string) => Promise<void>;
}) {
  const { machineKind, repository, worktree } = useRepositoryContext();
  const [pendingRepositoryRemoval, setPendingRepositoryRemoval] = useState(false);
  return (
    <section className="flex h-16 shrink-0 items-center gap-1 border-b bg-card px-3" aria-label="Working-copy context">
      <div className="flex min-w-0 flex-1 items-center gap-1">
        <span className="sr-only">Current repository and working copy</span>
          <RepositoryPicker
            repositories={repositories}
            onRepositoryChange={onRepositoryChange}
            onAddRepository={onAddRepository}
            onRemoveRepository={() => setPendingRepositoryRemoval(true)}
          />
          <ChevronRightIcon className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
          <WorktreePicker worktrees={worktrees} onWorktreeChange={onWorktreeChange} />
          <ChevronRightIcon className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
        {repository && worktree ? (
          <BranchControl key={`${repository.path}\0${worktree.id}\0${worktree.head ?? ""}`} />
        ) : (
          <span className="text-sm text-muted-foreground">No branch</span>
        )}
      </div>
      <Button variant="outline" size="sm" onClick={onCreateWorktree} disabled={!repository || !worktree}>
        <FolderPlusIcon data-icon="inline-start" aria-hidden="true" />
        New Worktree…
      </Button>
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

type RepositoryAction = { action: "add" | "reveal" | "remove"; label: string };
type RepositoryPickerItem = RepositorySummary | RepositoryAction;

const isRepositoryAction = (item: RepositoryPickerItem): item is RepositoryAction => "action" in item;

function RepositoryPicker({
  repositories,
  onRepositoryChange,
  onAddRepository,
  onRemoveRepository,
}: {
  repositories: RepositorySummary[];
  onRepositoryChange: (path: string) => void;
  onAddRepository: () => void;
  onRemoveRepository: () => void;
}) {
  const { machineKind, repository } = useRepositoryContext();
  const [query, setQuery] = useState("");
  const actions = useMemo<RepositoryAction[]>(() => [
    { action: "add", label: "Add Repository…" },
    ...(machineKind === "local" && repository ? [{ action: "reveal" as const, label: `Show in ${fileManagerName()}` }] : []),
    ...(repository ? [{ action: "remove" as const, label: "Remove from Repola…" }] : []),
  ], [machineKind, repository]);
  const matches = useMemo(() => repositories.filter((item) => matchesRepositoryQuery(item, query)), [repositories, query]);
  const selected = repositories.find((item) => item.path === repository?.path) ?? null;

  const runAction = ({ action }: RepositoryAction) => {
    if (action === "add") onAddRepository();
    else if (action === "remove") onRemoveRepository();
    else if (repository) {
      void revealWorktree(repository.path).catch((cause: unknown) => toast.add({ type: "error", title: `Could not show the repository in ${fileManagerName()}`, description: toMessage(cause) }));
    }
  };

  return (
    <Combobox<RepositoryPickerItem>
      items={[...repositories, ...actions]}
      filteredItems={[...matches, ...actions]}
      autoHighlight
      value={selected}
      inputValue={query}
      onInputValueChange={setQuery}
      onOpenChange={(open) => { if (!open) setQuery(""); }}
      itemToStringLabel={(item) => isRepositoryAction(item) ? item.label : item.name}
      itemToStringValue={(item) => isRepositoryAction(item) ? `action:${item.action}` : item.path}
      isItemEqualToValue={(item, value) => !isRepositoryAction(item) && !isRepositoryAction(value) && item.path === value.path}
      onValueChange={(value) => {
        if (!value) return;
        if (isRepositoryAction(value)) runAction(value);
        else if (value.path !== repository?.path) onRepositoryChange(value.path);
      }}
    >
      <ComboboxTrigger
        id="current-repository"
        className="w-56 font-medium"
        aria-label="Current repository"
      >
        <span className="flex min-w-0 items-center gap-1.5">
          <DatabaseIcon aria-hidden="true" />
          <span className="truncate">
            <ComboboxValue placeholder="Select a repository…" />
          </span>
        </span>
      </ComboboxTrigger>
      <ComboboxContent className="w-auto min-w-64 max-w-[min(28rem,var(--available-width))]">
        <ComboboxInput showTrigger={false} placeholder="Filter repositories" aria-label="Filter repositories" />
        <ComboboxList className="max-h-[min(40rem,calc(var(--available-height)---spacing(9)))]">
          {matches.length === 0 ? (
            <div className="py-2 text-center text-sm text-muted-foreground">
              {repositories.length === 0 ? "No repositories added." : "No repositories match."}
            </div>
          ) : matches.map((item) => (
            <ComboboxItem key={item.path} value={item}>
              <span className="min-w-0 flex-1 truncate">{item.name}</span>
            </ComboboxItem>
          ))}
          <ComboboxSeparator />
          {actions.map((item) => (
            <ComboboxItem
              key={item.action}
              value={item}
              data-variant={item.action === "remove" ? "destructive" : undefined}
              className="data-[variant=destructive]:text-destructive data-[variant=destructive]:data-highlighted:bg-destructive/10 data-[variant=destructive]:data-highlighted:text-destructive dark:data-[variant=destructive]:data-highlighted:bg-destructive/20 data-[variant=destructive]:*:[svg]:text-destructive"
            >
              {item.action === "add" ? <FolderPlusIcon aria-hidden="true" /> : item.action === "reveal" ? <FolderOpenIcon aria-hidden="true" /> : <Trash2Icon aria-hidden="true" />}
              {item.label}
            </ComboboxItem>
          ))}
        </ComboboxList>
      </ComboboxContent>
    </Combobox>
  );
}

function WorktreePicker({
  worktrees,
  onWorktreeChange,
}: {
  worktrees: WorktreeRecord[];
  onWorktreeChange: (path: string) => void;
}) {
  const { machineKind, machineOs, worktree } = useRepositoryContext();
  const separator = machinePathSeparator(machineKind, machineOs, usePathSeparator());
  const groups = useMemo(() => groupWorktreesForPicker(worktrees), [worktrees]);
  const selected = worktrees.find((item) => item.path === worktree?.path) ?? null;
  return (
    <Combobox
      items={groups}
      autoHighlight
      value={selected}
      itemToStringLabel={(item: WorktreeRecord) => worktreeFolderName(item.path, separator)}
      itemToStringValue={(item: WorktreeRecord) => item.path}
      isItemEqualToValue={(item: WorktreeRecord, value: WorktreeRecord) => item.path === value.path}
      filter={(item: WorktreeRecord, query: string) => matchesWorktreeQuery(item, query, separator)}
      onValueChange={(value: WorktreeRecord | null) => { if (value && value.path !== worktree?.path) onWorktreeChange(value.path); }}
    >
      <ComboboxTrigger
        id="current-worktree"
        className="w-52"
        aria-label="Current worktree"
      >
        <span className="flex min-w-0 items-center gap-1.5">
          <FolderOpenIcon aria-hidden="true" />
          <span className="truncate">
            <ComboboxValue placeholder="Select a worktree…" />
          </span>
        </span>
      </ComboboxTrigger>
      <ComboboxContent className="w-auto min-w-96 max-w-[min(36rem,var(--available-width))]">
        <ComboboxInput showTrigger={false} placeholder="Filter worktrees" aria-label="Filter worktrees" />
        <ComboboxEmpty>No worktrees match.</ComboboxEmpty>
        <ComboboxList className="max-h-[min(40rem,calc(var(--available-height)---spacing(9)))]">
          {(group: WorktreePickerGroup) => (
            <ComboboxGroup key={group.value} items={group.items}>
              <ComboboxLabel>{group.value}</ComboboxLabel>
              <ComboboxCollection>
                {(item: WorktreeRecord) => (
                  <ComboboxItem key={item.id} value={item}>
                    <FolderOpenIcon className="text-muted-foreground" aria-hidden="true" />
                    <span className="min-w-0 flex-1 truncate">{worktreeFolderName(item.path, separator)}</span>
                    <span className="max-w-60 shrink truncate text-xs text-muted-foreground">{worktreeBranchLabel(item)}</span>
                  </ComboboxItem>
                )}
              </ComboboxCollection>
            </ComboboxGroup>
          )}
        </ComboboxList>
      </ComboboxContent>
    </Combobox>
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
  const [deleteOpen, setDeleteOpen] = useState(false);

  useEffect(() => {
    const controller = new AbortController();
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

  const branchDeleted = async (result: BranchDeletionResult) => {
    setBranches(result.branches);
    await onChanged();
    toast.add(deletionNotice(result));
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
    <div className="flex min-w-0 flex-1 items-center gap-1">
        <Select items={items} value={current?.fullName ?? null} disabled={busy || branches === null} onValueChange={selectBranch}>
          <SelectTrigger id="current-branch" className="w-56" aria-label="Current branch">
            {busy || branches === null
              ? <Spinner />
              : <GitBranchIcon aria-hidden="true" />}
            <SelectValue placeholder="Select a branch…">
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
        <DropdownMenu>
          <DropdownMenuTrigger render={<Button variant="ghost" size="icon-sm" aria-label="Branch actions" disabled={busy || branches === null} />}>
            <MoreHorizontalIcon aria-hidden="true" />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="min-w-52">
            <DropdownMenuGroup>
              <DropdownMenuLabel>Branch actions</DropdownMenuLabel>
              <DropdownMenuItem onClick={() => openDialog("create")}><PlusIcon aria-hidden="true" />Create branch…</DropdownMenuItem>
              <DropdownMenuItem disabled={current === null} onClick={() => openDialog("rename")}><PencilIcon aria-hidden="true" />Rename current branch…</DropdownMenuItem>
              <DropdownMenuItem disabled={current === null || historyTargets.length === 0} onClick={() => setHistoryActionsOpen(true)}><GitMergeIcon aria-hidden="true" />Merge or rebase…</DropdownMenuItem>
              <DropdownMenuItem disabled={worktree.head === null} onClick={() => setTagsOpen(true)}><TagIcon aria-hidden="true" />Tags…</DropdownMenuItem>
            </DropdownMenuGroup>
            <DropdownMenuSeparator />
            <DropdownMenuGroup>
              <DropdownMenuItem variant="destructive" disabled={branches === null || branches.length === 0} onClick={() => setDeleteOpen(true)}><Trash2Icon aria-hidden="true" />Delete branch…</DropdownMenuItem>
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
      {deleteOpen && branches ? (
        <LazyDialog onClose={() => setDeleteOpen(false)}>
          <DeleteBranchDialog
            machineId={machineId}
            repositoryPath={repository.path}
            worktreePath={worktree.path}
            branches={branches}
            onClose={() => setDeleteOpen(false)}
            onDeleted={branchDeleted}
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
