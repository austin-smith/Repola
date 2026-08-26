import { useRef, useState } from "react";
import { CopyIcon, FolderOpenIcon, FolderPlusIcon, PlusIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Field, FieldDescription, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Spinner } from "@/components/ui/spinner";
import { ActionableGitError } from "./ActionableGitError";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import type { MachineProfile } from "./types";
import { cloneRepository, createRepository } from "./worktrees";

type RepositoryMode = "add" | "clone" | "create";

export function RepositoryDialog({
  machine,
  onAddExisting,
  onCompleted,
  onClose,
}: {
  machine: MachineProfile;
  onAddExisting: (path?: string) => Promise<boolean>;
  onCompleted: (repositoryPath: string) => Promise<boolean>;
  onClose: () => void;
}) {
  const [mode, setMode] = useState<RepositoryMode>("clone");
  const [source, setSource] = useState("");
  const [destination, setDestination] = useState("");
  const [existingPath, setExistingPath] = useState("");
  const [initialBranch, setInitialBranch] = useState("main");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const controller = useRef<AbortController | null>(null);

  const close = () => {
    if (busy) controller.current?.abort();
    else onClose();
  };

  const addExisting = async () => {
    setBusy(true);
    setError(null);
    try {
      const added = await onAddExisting(machine.kind === "ssh" ? existingPath.trim() : undefined);
      if (added) onClose();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  };

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (mode === "add") {
      await addExisting();
      return;
    }
    const operation = new AbortController();
    controller.current = operation;
    setBusy(true);
    setError(null);
    try {
      const result = mode === "clone"
        ? await cloneRepository(machine.id, source.trim(), destination.trim(), operation.signal)
        : await createRepository(machine.id, destination.trim(), initialBranch.trim(), operation.signal);
      if (await onCompleted(result.repositoryPath)) onClose();
    } catch (cause) {
      if (!operation.signal.aborted) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (controller.current === operation) controller.current = null;
      setBusy(false);
    }
  };

  const submitDisabled = busy || (mode === "add"
    ? machine.kind === "ssh" && existingPath.trim() === ""
    : destination.trim() === "" || (mode === "clone" ? source.trim() === "" : initialBranch.trim() === ""));

  return (
    <Dialog open onOpenChange={(open) => { if (!open) close(); }}>
      <DialogContent className="sm:max-w-lg" showCloseButton={!busy}>
        <form className="contents" onSubmit={(event) => void submit(event)}>
          <DialogHeader>
            <DialogTitle>Add a repository on {machine.name}</DialogTitle>
            <DialogDescription>
              Repository commands run where the working copy lives. Git credentials remain owned by that machine.
            </DialogDescription>
          </DialogHeader>
          <ToggleGroup className="w-full" value={[mode]} onValueChange={(value) => { const next = value[0] as RepositoryMode | undefined; if (next && !busy) { setMode(next); setError(null); } }}>
            <ToggleGroupItem value="add" className="flex-1"><FolderOpenIcon aria-hidden="true" />Add Existing</ToggleGroupItem>
            <ToggleGroupItem value="clone" className="flex-1"><CopyIcon aria-hidden="true" />Clone</ToggleGroupItem>
            <ToggleGroupItem value="create" className="flex-1"><PlusIcon aria-hidden="true" />Create</ToggleGroupItem>
          </ToggleGroup>

          {mode === "add" ? (
            machine.kind === "ssh" ? (
              <Field>
                <FieldLabel htmlFor="existing-repository-path">Repository path</FieldLabel>
                <Input id="existing-repository-path" autoFocus className="font-mono" value={existingPath} onChange={(event) => setExistingPath(event.currentTarget.value)} placeholder="/srv/projects/example" spellCheck={false} disabled={busy} />
                <FieldDescription>Enter the full path as it exists on {machine.name}.</FieldDescription>
              </Field>
            ) : (
              <div className="flex flex-col items-center gap-3 border bg-muted/35 p-6 text-center">
                <FolderOpenIcon className="size-6 text-muted-foreground" aria-hidden="true" />
                <div><strong className="text-sm">Choose an existing working copy</strong><p className="mt-1 text-sm text-muted-foreground">Repola will inspect the folder without changing the repository.</p></div>
              </div>
            )
          ) : (
            <>
              {mode === "clone" ? (
                <Field>
                  <FieldLabel htmlFor="clone-source">Repository URL</FieldLabel>
                  <Input id="clone-source" autoFocus value={source} onChange={(event) => setSource(event.currentTarget.value)} placeholder="git@github.com:owner/project.git" spellCheck={false} disabled={busy} />
                  <FieldDescription>HTTPS, SSH, and local Git sources are supported.</FieldDescription>
                </Field>
              ) : (
                <Field>
                  <FieldLabel htmlFor="initial-branch">Initial branch</FieldLabel>
                  <Input id="initial-branch" autoFocus value={initialBranch} onChange={(event) => setInitialBranch(event.currentTarget.value)} placeholder="main" spellCheck={false} disabled={busy} />
                </Field>
              )}
              <Field>
                <FieldLabel htmlFor="repository-destination">Destination path</FieldLabel>
                <Input id="repository-destination" className="font-mono" value={destination} onChange={(event) => setDestination(event.currentTarget.value)} placeholder={machine.kind === "ssh" ? "/srv/projects/example" : "/Users/you/Developer/example"} spellCheck={false} disabled={busy} />
                <FieldDescription>The full destination path must not exist or must be empty.</FieldDescription>
              </Field>
            </>
          )}
          {error ? <ActionableGitError message={error} /> : null}
          <DialogFooter>
            <Button type="button" variant="outline" onClick={close}>{busy ? "Cancel operation" : "Cancel"}</Button>
            <Button type="submit" disabled={submitDisabled}>
              {busy ? <Spinner data-icon="inline-start" /> : mode === "clone" ? <CopyIcon data-icon="inline-start" aria-hidden="true" /> : mode === "create" ? <PlusIcon data-icon="inline-start" aria-hidden="true" /> : <FolderPlusIcon data-icon="inline-start" aria-hidden="true" />}
              {busy ? "Working…" : mode === "clone" ? "Clone Repository" : mode === "create" ? "Create Repository" : machine.kind === "ssh" ? "Add Repository" : "Choose Repository…"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
