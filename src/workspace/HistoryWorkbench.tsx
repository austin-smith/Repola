import { Suspense, useEffect, useRef, useState } from "react";
import {
  AlertTriangleIcon,
  FileClockIcon,
  GitBranchIcon,
  GitCommitIcon,
  MoreHorizontalIcon,
  RotateCcwIcon,
  SearchIcon,
  SearchXIcon,
  TagIcon,
  XIcon,
} from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuGroup, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { InputGroup, InputGroupAddon, InputGroupButton, InputGroupInput } from "@/components/ui/input-group";
import { Select, SelectContent, SelectGroup, SelectItem, SelectLabel, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { toast } from "@/components/ui/toast";
import { cn } from "@/lib/utils";
import { toMessage } from "@/lib/errors";
import { ActionableGitError } from "../components/ActionableGitError";
import { activeChangeKinds, countChangeKinds, filterByChangeKind, type ChangeKindFilterKind } from "../domain/change-kind-filter";
import { shortSha } from "../domain/format";
import { loadBranches, loadCommitFiles, loadHistory } from "../ipc/worktrees";
import type { BranchInfo, CommitChangedFile, CommitSummary } from "../ipc/types";
import { ChangeKindFilter, ChangeKindIcon } from "./ChangeKindFilter";
import { useWorkingCopy } from "./context";
import { ActivityFact } from "./facts";
import { historyMutationTitles, sectionHeadingClass } from "./labels";
import { LazyDialog } from "./LazyDialog";
import { CommitFileDiffView, HistoryMutationDialog, ReflogDialog, TagsDialog } from "./lazy";

export function HistoryWorkbench() {
  const { machineId, repository, worktree, gitRevision, registerRefreshConsumer, refreshWorkspace: onWorkingCopyChanged, showChanges: onNeedsResolution } = useWorkingCopy();
  const [commits, setCommits] = useState<CommitSummary[] | null>(null);
  const [nextCursor, setNextCursor] = useState<string | null>(null);
  const [selectedOid, setSelectedOid] = useState<string | null>(null);
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [files, setFiles] = useState<CommitChangedFile[] | null>(null);
  const [selectedFileId, setSelectedFileId] = useState<string | null>(null);
  const [kindFilter, setKindFilter] = useState<ChangeKindFilterKind[]>([]);
  const [historyQuery, setHistoryQuery] = useState("");
  const [debouncedQuery, setDebouncedQuery] = useState("");
  const [comparisonBase, setComparisonBase] = useState<string | null>(null);
  const [historyBranches, setHistoryBranches] = useState<BranchInfo[] | null>(null);
  const [pendingHistoryAction, setPendingHistoryAction] = useState<{ kind: "cherryPick" | "revert" | "reset"; commit: CommitSummary; branch: string | null } | null>(null);
  const [tagTarget, setTagTarget] = useState<string | null>(null);
  const [reflogOpen, setReflogOpen] = useState(false);
  const historyGeneration = useRef(0);
  const historyReloading = useRef(false);
  const moreController = useRef<AbortController | null>(null);
  const loadedCount = useRef(0);
  const historyScope = useRef("");
  const pendingHistory = useRef<Promise<void> | null>(null);
  const pendingBranches = useRef<Promise<void> | null>(null);
  useEffect(() => registerRefreshConsumer?.(async () => { await Promise.all([pendingHistory.current, pendingBranches.current]); }), [registerRefreshConsumer]);

  useEffect(() => {
    const timeout = setTimeout(() => setDebouncedQuery(historyQuery.trim()), 250);
    return () => clearTimeout(timeout);
  }, [historyQuery]);

  useEffect(() => {
    const controller = new AbortController();
    setHistoryBranches(null);
    pendingBranches.current = loadBranches(machineId, repository.path, worktree.path, controller.signal)
      .then((next) => { if (!controller.signal.aborted) setHistoryBranches(next); })
      .catch((cause: unknown) => {
        if (!controller.signal.aborted) setError(toMessage(cause));
      });
    return () => controller.abort();
  }, [gitRevision, machineId, repository.path, worktree.id, worktree.path]);

  useEffect(() => {
    const controller = new AbortController();
    const generation = ++historyGeneration.current;
    historyReloading.current = true;
    moreController.current?.abort();
    moreController.current = null;
    setLoadingMore(false);
    setNextCursor(null);
    const scope = JSON.stringify([machineId, repository.path, worktree.path, debouncedQuery, comparisonBase]);
    const sameScope = historyScope.current === scope;
    historyScope.current = scope;
    const targetCount = sameScope ? loadedCount.current : 0;
    if (!sameScope) {
      setCommits(null);
      setNextCursor(null);
      loadedCount.current = 0;
    }
    setError(null);
    pendingHistory.current = (async () => {
      let page = await loadHistory(machineId, repository.path, worktree.path, null, debouncedQuery || null, comparisonBase, controller.signal);
      const refreshed = [...page.commits];
      const cursors = new Set<string>();
      while (refreshed.length < targetCount && page.nextCursor && !controller.signal.aborted) {
        if (cursors.has(page.nextCursor)) break;
        cursors.add(page.nextCursor);
        page = await loadHistory(machineId, repository.path, worktree.path, page.nextCursor, debouncedQuery || null, comparisonBase, controller.signal);
        refreshed.push(...page.commits);
      }
      return { commits: refreshed, nextCursor: page.nextCursor };
    })()
      .then((page) => {
        if (controller.signal.aborted || generation !== historyGeneration.current) return;
        loadedCount.current = page.commits.length;
        setCommits(page.commits);
        setNextCursor(page.nextCursor);
        setSelectedOid((current) => sameScope && page.commits.some((commit) => commit.oid === current) ? current : page.commits[0]?.oid ?? null);
      })
      .catch((cause: unknown) => {
        if (!controller.signal.aborted) setError(toMessage(cause));
      })
      .finally(() => { if (generation === historyGeneration.current) historyReloading.current = false; });
    return () => { controller.abort(); moreController.current?.abort(); };
  }, [comparisonBase, debouncedQuery, gitRevision, machineId, repository.path, worktree.id, worktree.path]);

  const loadMore = async () => {
    if (!nextCursor || loadingMore || historyReloading.current || moreController.current) return;
    const generation = historyGeneration.current;
    const controller = new AbortController();
    moreController.current = controller;
    setLoadingMore(true);
    try {
      const page = await loadHistory(
        machineId,
        repository.path,
        worktree.path,
        nextCursor,
        debouncedQuery || null,
        comparisonBase,
        controller.signal,
      );
      if (controller.signal.aborted || generation !== historyGeneration.current) return;
      loadedCount.current += page.commits.length;
      setCommits((current) => [...(current ?? []), ...page.commits]);
      setNextCursor(page.nextCursor);
    } catch (cause) {
      if (!controller.signal.aborted && generation === historyGeneration.current) setError(toMessage(cause));
    } finally {
      if (moreController.current === controller) { moreController.current = null; setLoadingMore(false); }
    }
  };

  const selected = commits?.find((commit) => commit.oid === selectedOid) ?? commits?.[0] ?? null;
  useEffect(() => {
    if (!selected?.oid) {
      setFiles(null);
      setSelectedFileId(null);
      return;
    }
    const controller = new AbortController();
    setFiles(null);
    setSelectedFileId(null);
    void loadCommitFiles(machineId, repository.path, worktree.path, selected.oid, controller.signal)
      .then((next) => {
        if (controller.signal.aborted) return;
        setFiles(next);
        setSelectedFileId((current) => next.some((file) => file.id === current) ? current : next[0]?.id ?? null);
      })
      .catch((cause: unknown) => { if (!controller.signal.aborted) setError(toMessage(cause)); });
    return () => controller.abort();
  }, [machineId, repository.path, selected?.oid, worktree.path]);
  const kindCounts = countChangeKinds(files ?? []);
  const activeKinds = activeChangeKinds(kindFilter, kindCounts);
  const listedFiles = filterByChangeKind(files ?? [], activeKinds);
  const selectedFile = listedFiles.find((file) => file.id === selectedFileId) ?? listedFiles[0] ?? null;
  const comparisonBranches = historyBranches?.filter((branch) => !branch.current) ?? [];
  const comparisonItems = Object.fromEntries([
    ["none", "Current branch"],
    ...comparisonBranches.map((branch) => [branch.fullName, branch.name]),
  ]);
  const comparisonLabel = comparisonBase
    ? historyBranches?.find((branch) => branch.fullName === comparisonBase)?.name ?? comparisonBase
    : null;
  if (!commits && !error) {
    return <div className="grid min-h-0 flex-1 place-items-center"><Spinner className="size-6" /></div>;
  }
  return (
    <div className="grid min-h-0 flex-1 grid-cols-[minmax(280px,0.8fr)_minmax(240px,0.7fr)_minmax(360px,1.5fr)]">
      <section className="flex min-h-0 flex-col border-r" aria-label="Commit history">
        <div className="flex shrink-0 flex-col gap-2 border-b bg-card p-3">
          <div className="flex items-center">
            <strong className="text-sm">{comparisonLabel ? "Branch comparison" : "History"}</strong>
            <span className="ml-2 text-xs text-muted-foreground">
              {comparisonLabel ? "commits not in " + comparisonLabel : "newest first"}
            </span>
            <Button variant="ghost" size="sm" className="ml-auto" onClick={() => setReflogOpen(true)}><FileClockIcon data-icon="inline-start" aria-hidden="true" />Undo points</Button>
          </div>
          <InputGroup>
            <InputGroupAddon><SearchIcon aria-hidden="true" /></InputGroupAddon>
            <InputGroupInput
              value={historyQuery}
              onChange={(event) => setHistoryQuery(event.currentTarget.value)}
              placeholder="Search commit messages…"
              aria-label="Search commit messages"
              autoComplete="off"
              spellCheck={false}
            />
            {historyQuery ? (
              <InputGroupAddon align="inline-end">
                <Tooltip>
                  <TooltipTrigger render={<InputGroupButton size="icon-xs" aria-label="Clear history search" onClick={() => setHistoryQuery("")} />}>
                    <XIcon aria-hidden="true" />
                  </TooltipTrigger>
                  <TooltipContent>Clear history search</TooltipContent>
                </Tooltip>
              </InputGroupAddon>
            ) : null}
          </InputGroup>
          <Select
            items={comparisonItems}
            value={comparisonBase ?? "none"}
            disabled={historyBranches === null}
            onValueChange={(value) => setComparisonBase(value && value !== "none" ? value : null)}
          >
            <SelectTrigger className="w-full" aria-label="Compare current branch">
              <GitBranchIcon className="size-3.5 text-muted-foreground" aria-hidden="true" />
              <SelectValue>{comparisonLabel ? "Compared with " + comparisonLabel : "Compare current branch…"}</SelectValue>
            </SelectTrigger>
            <SelectContent>
              <SelectGroup>
                <SelectItem value="none">No comparison</SelectItem>
              </SelectGroup>
              {comparisonBranches.some((branch) => !branch.remote) ? (
                <SelectGroup>
                  <SelectLabel>Local branches</SelectLabel>
                  {comparisonBranches.filter((branch) => !branch.remote).map((branch) => (
                    <SelectItem key={branch.fullName} value={branch.fullName}>{branch.name}</SelectItem>
                  ))}
                </SelectGroup>
              ) : null}
              {comparisonBranches.some((branch) => branch.remote) ? (
                <SelectGroup>
                  <SelectLabel>Remote branches</SelectLabel>
                  {comparisonBranches.filter((branch) => branch.remote).map((branch) => (
                    <SelectItem key={branch.fullName} value={branch.fullName}>{branch.name}</SelectItem>
                  ))}
                </SelectGroup>
              ) : null}
            </SelectContent>
          </Select>
        </div>
        {error ? <ActionableGitError message={error} className="m-3" /> : null}
        <div className="min-h-0 flex-1 overflow-y-auto">
          {(commits ?? []).map((commit) => (
            <button
              key={commit.oid}
              type="button"
              className={cn("repola-windowed-row flex w-full items-start gap-3 border-b px-4 py-3 text-left", selected?.oid === commit.oid && "bg-accent")}
              onClick={() => setSelectedOid(commit.oid)}
            >
              <span className="mt-1 size-2 shrink-0 rounded-full border-2 border-brand bg-background" aria-hidden="true" />
              <span className="min-w-0 flex-1">
                <strong className="block truncate text-sm font-medium">{commit.subject || "(no subject)"}</strong>
                <span className="mt-1 block truncate text-xs text-muted-foreground">
                  {commit.authorName} · {new Date(commit.committedAt).toLocaleString()}
                </span>
              </span>
              <code className="font-mono text-xs text-muted-foreground">{shortSha(commit.oid)}</code>
            </button>
          ))}
          {commits?.length === 0 ? (
            <Empty className="py-12">
              <EmptyHeader>
                <EmptyMedia variant="icon">{debouncedQuery ? <SearchXIcon aria-hidden="true" /> : <GitCommitIcon aria-hidden="true" />}</EmptyMedia>
                <EmptyTitle>{debouncedQuery ? "No matching commits" : comparisonBase ? "Branches are even" : "No commits yet"}</EmptyTitle>
                <EmptyDescription>
                  {debouncedQuery
                    ? "Try a different word or phrase from the commit message."
                    : comparisonBase
                      ? "The current branch has no commits that are absent from the comparison branch."
                      : "Create the first commit from Changes."}
                </EmptyDescription>
              </EmptyHeader>
            </Empty>
          ) : null}
          {nextCursor ? (
            <div className="grid place-items-center p-4">
              <Button variant="outline" size="sm" disabled={loadingMore} onClick={() => void loadMore()}>
                {loadingMore ? <Spinner data-icon="inline-start" /> : null}
                {loadingMore ? "Loading…" : "Load older commits"}
              </Button>
            </div>
          ) : null}
        </div>
      </section>
      <aside className="flex min-h-0 flex-col border-r bg-sidebar">
        {selected ? (
          <>
            <div className="max-h-[45%] shrink-0 overflow-y-auto border-b p-4">
              <span className={sectionHeadingClass}>Commit</span>
              <h2 className="mt-2 text-base leading-tight font-medium">{selected.subject || "(no subject)"}</h2>
              {selected.body ? <pre className="mt-2 font-sans text-xs whitespace-pre-wrap text-muted-foreground">{selected.body}</pre> : null}
              <dl className="mt-4 flex flex-col border bg-card px-3">
                <ActivityFact label="Author" value={selected.authorName} />
                <ActivityFact label="Committed" value={new Date(selected.committedAt).toLocaleString()} />
                <ActivityFact label="Signature" value={selected.signature.replace(/([a-z])([A-Z])/g, "$1 $2").toLowerCase()} />
              </dl>
              <Tooltip>
                <TooltipTrigger render={<code className="mt-3 block truncate font-mono text-xs text-muted-foreground" tabIndex={0} />}>
                  {selected.oid}
                </TooltipTrigger>
                <TooltipContent>{selected.oid}</TooltipContent>
              </Tooltip>
              <p className="mt-1 text-xs text-muted-foreground">{selected.parents.length === 0 ? "Root commit" : `${selected.parents.length} parent${selected.parents.length === 1 ? "" : "s"}`}</p>
              <DropdownMenu>
                <DropdownMenuTrigger render={<Button variant="outline" size="sm" className="mt-4 w-full justify-start" />}>
                  <MoreHorizontalIcon data-icon="inline-start" aria-hidden="true" />
                  Commit actions
                </DropdownMenuTrigger>
                <DropdownMenuContent className="min-w-56" align="start">
                  <DropdownMenuGroup>
                    <DropdownMenuLabel>Apply commit</DropdownMenuLabel>
                    <DropdownMenuItem onClick={() => setPendingHistoryAction({ kind: "cherryPick", commit: selected, branch: worktree.branch })}><GitCommitIcon aria-hidden="true" />Cherry-pick commit…</DropdownMenuItem>
                    <DropdownMenuItem onClick={() => setPendingHistoryAction({ kind: "revert", commit: selected, branch: worktree.branch })}><RotateCcwIcon aria-hidden="true" />Revert commit…</DropdownMenuItem>
                    <DropdownMenuItem onClick={() => setTagTarget(selected.oid)}><TagIcon aria-hidden="true" />Create tag here…</DropdownMenuItem>
                  </DropdownMenuGroup>
                  <DropdownMenuSeparator />
                  <DropdownMenuGroup>
                    <DropdownMenuItem variant="destructive" onClick={() => setPendingHistoryAction({ kind: "reset", commit: selected, branch: worktree.branch })}><AlertTriangleIcon aria-hidden="true" />Reset branch to commit…</DropdownMenuItem>
                  </DropdownMenuGroup>
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
            <div className="flex h-11 shrink-0 items-center border-b px-4">
              <strong className="text-sm">Changed files</strong>
              {files ? <Badge variant="secondary" className="ml-2">{files.length}</Badge> : <Spinner className="ml-auto size-4" />}
              {kindCounts.length > 1 ? (
                <div className="ml-auto text-muted-foreground">
                  <ChangeKindFilter counts={kindCounts} value={activeKinds} onValueChange={setKindFilter} />
                </div>
              ) : null}
            </div>
            <div className="min-h-0 flex-1 overflow-y-auto">
              {listedFiles.map((file) => (
                <button key={file.id} type="button" className={cn("flex w-full items-center gap-2 border-b px-3 py-1 text-left", selectedFile?.id === file.id && "bg-accent")} onClick={() => setSelectedFileId(file.id)}>
                  <span className="grid w-6 shrink-0 place-items-center"><ChangeKindIcon kind={file.kind} /></span>
                  <span className="min-w-0 flex-1 truncate text-sm" title={file.path.display}><span className="text-muted-foreground">{file.path.display.slice(0, file.path.display.search(/[^\\/]*$/))}</span><span className="text-foreground">{file.path.display.split(/[\\/]/).pop()}</span></span>
                </button>
              ))}
              {files?.length === 0 ? <p className="p-4 text-sm text-muted-foreground">No first-parent file changes.</p> : null}
            </div>
          </>
        ) : (
          <Empty className="h-full">
            <EmptyHeader><EmptyTitle>Select a commit</EmptyTitle></EmptyHeader>
          </Empty>
        )}
      </aside>
      <section className="flex min-h-0 flex-col bg-background" aria-label="Commit file diff">
        <div className="flex h-12 shrink-0 items-center border-b px-4">
          <div className="min-w-0"><strong className="block truncate text-sm">{selectedFile?.path.display ?? "Commit diff"}</strong>{selectedFile?.previousPath ? <span className="block truncate text-xs text-muted-foreground">renamed from {selectedFile.previousPath.display}</span> : null}</div>
        </div>
        <div className="min-h-0 flex-1 overflow-auto">
          {selected && selectedFile ? (
            <Suspense fallback={<div className="grid h-full place-items-center"><Spinner className="size-6" /></div>}>
              <CommitFileDiffView machineId={machineId} repositoryPath={repository.path} worktreePath={worktree.path} commit={selected.oid} file={selectedFile} />
            </Suspense>
          ) : <div className="grid h-full place-items-center text-sm text-muted-foreground">Select a changed file.</div>}
        </div>
      </section>
      {pendingHistoryAction ? (
        <LazyDialog onClose={() => setPendingHistoryAction(null)}>
          <HistoryMutationDialog
            machineId={machineId}
            repository={repository}
            worktree={worktree}
            kinds={pendingHistoryAction.kind === "reset" ? ["resetSoft", "resetMixed", "resetHard"] : [pendingHistoryAction.kind]}
            targets={[{ id: pendingHistoryAction.commit.oid, oid: pendingHistoryAction.commit.oid, label: pendingHistoryAction.commit.subject || shortSha(pendingHistoryAction.commit.oid), detail: pendingHistoryAction.commit.oid }]}
            initialKind={pendingHistoryAction.kind === "reset" ? "resetMixed" : pendingHistoryAction.kind}
            title={pendingHistoryAction.kind === "reset" ? `Reset ${pendingHistoryAction.branch ?? "current branch"}` : `${pendingHistoryAction.kind === "revert" ? "Revert" : "Cherry-pick"} ${shortSha(pendingHistoryAction.commit.oid)}`}
            onClose={() => setPendingHistoryAction(null)}
            onCompleted={async (result, kind) => {
              await onWorkingCopyChanged();
              if (result.conflicted || result.snapshot.operation) {
                toast.add({ type: "warning", title: "Conflict resolution required", description: "Resolve every conflicted file in Changes, then continue or abort the operation." });
                onNeedsResolution();
                return;
              }
              toast.add({ type: "success", title: historyMutationTitles[kind], description: result.output || shortSha(result.snapshot.head) });
            }}
          />
        </LazyDialog>
      ) : null}
      {tagTarget ? (
        <LazyDialog onClose={() => setTagTarget(null)}>
          <TagsDialog machineId={machineId} repository={repository} worktree={worktree} initialTarget={tagTarget} onClose={() => setTagTarget(null)} onChanged={async () => { await onWorkingCopyChanged(); }} />
        </LazyDialog>
      ) : null}
      {reflogOpen ? (
        <LazyDialog onClose={() => setReflogOpen(false)}>
          <ReflogDialog machineId={machineId} repository={repository} worktree={worktree} onClose={() => setReflogOpen(false)} onChanged={async () => { await onWorkingCopyChanged(); }} onNeedsResolution={onNeedsResolution} />
        </LazyDialog>
      ) : null}
    </div>
  );
}
