import { useEffect, useMemo, useRef, useState } from "react";
import { GitBranchIcon, PlusIcon } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Field, FieldDescription, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { ActionableGitError } from "./ActionableGitError";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import type { BranchInfo, CreateWorktreeResult, RepositorySummary, WorkingCopySnapshot, WorktreeRecord } from "./types";
import { createLinkedWorktree, fetchWorkingCopy, loadBranches } from "./worktrees";

type Mode = "new" | "existing";

export function CreateWorktreeDialog({
  machineId,
  repository,
  sourceWorktree,
  onCreated,
  onClose,
}: {
  machineId: string;
  repository: RepositorySummary;
  sourceWorktree: WorktreeRecord;
  onCreated: (result: CreateWorktreeResult) => Promise<void> | void;
  onClose: () => void;
}) {
  const [mode, setMode] = useState<Mode>("new");
  const [branches, setBranches] = useState<BranchInfo[] | null>(null);
  const [snapshot, setSnapshot] = useState<WorkingCopySnapshot | null>(null);
  const [branch, setBranch] = useState("");
  const [startPoint, setStartPoint] = useState(sourceWorktree.branch ?? sourceWorktree.head ?? "HEAD");
  const [destination, setDestination] = useState("");
  const [destinationEdited, setDestinationEdited] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const operation = useRef<AbortController | null>(null);

  useEffect(() => {
    const controller = new AbortController();
    void Promise.all([
      loadBranches(machineId, repository.path, sourceWorktree.path, controller.signal),
      fetchWorkingCopy(machineId, repository.path, sourceWorktree.path, controller.signal),
    ]).then(([nextBranches, nextSnapshot]) => {
      setBranches(nextBranches);
      setSnapshot(nextSnapshot);
    }).catch((cause: unknown) => {
      if (!controller.signal.aborted) setError(cause instanceof Error ? cause.message : String(cause));
    });
    return () => controller.abort();
  }, [machineId, repository.path, sourceWorktree.path]);

  const availableLocalBranches = useMemo(
    () => branches?.filter((item) => !item.remote && item.occupiedWorktreePath === null) ?? [],
    [branches],
  );
  const startPoints = useMemo(() => branches ?? [], [branches]);

  const changeBranch = (value: string) => {
    setBranch(value);
    if (!destinationEdited) setDestination(suggestDestination(sourceWorktree.path, value));
  };

  const changeMode = (next: Mode) => {
    setMode(next);
    setBranch("");
    setDestination("");
    setDestinationEdited(false);
    setError(null);
  };

  const close = () => {
    if (operation.current) {
      operation.current.abort();
      return;
    }
    onClose();
  };

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!snapshot) return;
    const controller = new AbortController();
    operation.current = controller;
    setBusy(true);
    setError(null);
    try {
      const result = await createLinkedWorktree(machineId, {
        repositoryPath: repository.path,
        sourceWorktreePath: sourceWorktree.path,
        destinationPath: destination.trim(),
        branch: branch.trim(),
        createBranch: mode === "new",
        startPoint: mode === "new" ? startPoint : null,
        expectedHead: snapshot.head,
      }, controller.signal);
      await onCreated(result);
      onClose();
    } catch (cause) {
      if (!controller.signal.aborted) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (operation.current === controller) operation.current = null;
      setBusy(false);
    }
  };

  return (
    <Dialog open onOpenChange={(open) => { if (!open) close(); }}>
      <DialogContent className="sm:max-w-xl">
        <form onSubmit={(event) => void submit(event)}>
          <DialogHeader>
            <DialogTitle>Create linked worktree</DialogTitle>
            <DialogDescription>
              Give a branch its own working folder on this machine. It remains part of <strong>{repository.name}</strong> and appears everywhere in Repola.
            </DialogDescription>
          </DialogHeader>

          <div className="my-5 flex flex-col gap-5">
            <ToggleGroup value={[mode]} onValueChange={(value) => { if (value[0]) changeMode(value[0] as Mode); }} className="grid grid-cols-2">
              <ToggleGroupItem value="new"><PlusIcon aria-hidden="true" /> New branch</ToggleGroupItem>
              <ToggleGroupItem value="existing"><GitBranchIcon aria-hidden="true" /> Existing branch</ToggleGroupItem>
            </ToggleGroup>

            {mode === "new" ? (
              <>
                <Field>
                  <FieldLabel htmlFor="worktree-new-branch">New branch name</FieldLabel>
                  <Input id="worktree-new-branch" value={branch} onChange={(event) => changeBranch(event.currentTarget.value)} placeholder="feature/faster-search" autoFocus spellCheck={false} />
                </Field>
                <Field>
                  <FieldLabel htmlFor="worktree-start-point">Start from</FieldLabel>
                  <Select items={Object.fromEntries(startPoints.map((item) => [item.name, item.name]))} value={startPoint} onValueChange={(value) => { if (value) setStartPoint(value); }}>
                    <SelectTrigger id="worktree-start-point" className="w-full"><SelectValue /></SelectTrigger>
                    <SelectContent><SelectGroup>{startPoints.map((item) => <SelectItem key={item.fullName} value={item.name}>{item.name}{item.current ? " · current" : item.remote ? " · remote" : ""}</SelectItem>)}</SelectGroup></SelectContent>
                  </Select>
                </Field>
              </>
            ) : (
              <Field>
                <FieldLabel htmlFor="worktree-existing-branch">Available local branch</FieldLabel>
                <Select items={Object.fromEntries(availableLocalBranches.map((item) => [item.name, item.name]))} value={branch || null} onValueChange={(value) => { if (value) changeBranch(value); }}>
                  <SelectTrigger id="worktree-existing-branch" className="w-full"><SelectValue placeholder="Select a branch" /></SelectTrigger>
                  <SelectContent><SelectGroup>{availableLocalBranches.map((item) => <SelectItem key={item.fullName} value={item.name}>{item.name}</SelectItem>)}</SelectGroup></SelectContent>
                </Select>
                <FieldDescription>Branches already open in another worktree are intentionally unavailable.</FieldDescription>
              </Field>
            )}

            <Field>
              <FieldLabel htmlFor="worktree-destination">Working folder</FieldLabel>
              <Input id="worktree-destination" value={destination} onChange={(event) => { setDestination(event.currentTarget.value); setDestinationEdited(true); }} placeholder="Full path to a new folder" spellCheck={false} />
              <FieldDescription>The parent folder must exist; the final folder must not. This path is interpreted on the selected machine.</FieldDescription>
            </Field>

            <div className="flex items-center gap-2 rounded-lg border bg-muted/40 px-3 py-2 text-xs text-muted-foreground">
              <Badge variant="outline">Source</Badge>
              <Tooltip>
                <TooltipTrigger render={<code className="truncate font-mono" tabIndex={0} />}>
                  {sourceWorktree.path}
                </TooltipTrigger>
                <TooltipContent>{sourceWorktree.path}</TooltipContent>
              </Tooltip>
            </div>

            {error ? <ActionableGitError message={error} /> : null}
          </div>

          <DialogFooter>
            <Button type="button" variant="outline" onClick={close}>{busy ? "Cancel operation" : "Cancel"}</Button>
            <Button type="submit" disabled={busy || !snapshot || branch.trim() === "" || destination.trim() === "" || (mode === "new" && startPoint === "")}>
              {busy || branches === null ? <Spinner data-icon="inline-start" /> : <PlusIcon data-icon="inline-start" aria-hidden="true" />}
              {busy ? "Creating…" : "Create Worktree"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function suggestDestination(source: string, branch: string): string {
  if (!branch.trim()) return "";
  const safeBranch = branch.trim().replace(/[\\/:*?"<>|]+/g, "-");
  return `${source}-${safeBranch}`;
}
