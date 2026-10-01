import { useEffect, useRef, useState } from "react";
import { CopyIcon, FolderOpenIcon, FolderPlusIcon, PlusIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Field, FieldDescription, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Spinner } from "@/components/ui/spinner";
import { ActionableGitError } from "../components/ActionableGitError";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import type { MachineProfile } from "../ipc/types";
import { cloneRepository, createRepository } from "../ipc/worktrees";
import { RepositoryDropZone } from "../components/RepositoryDropZone";
import { useRepositoryDrop } from "../app/use-repository-drop";

type RepositoryMode = "add" | "clone" | "create";

export function RepositoryDialog({
  machine,
  onAddExisting,
  onCompleted,
  onDropRepositories,
  dropDisabled,
  onClose,
}: {
  machine: MachineProfile;
  onAddExisting: (path?: string) => Promise<boolean>;
  onCompleted: (repositoryPath: string) => Promise<boolean>;
  onDropRepositories: (paths: string[]) => Promise<boolean>;
  dropDisabled: boolean;
  onClose: () => void;
}) {
  const [mode, setMode] = useState<RepositoryMode>("add");
  const [source, setSource] = useState("");
  const [destination, setDestination] = useState("");
  const [existingPath, setExistingPath] = useState("");
  const [initialBranch, setInitialBranch] = useState("main");
  const [busy, setBusy] = useState(false);
  const [dropping, setDropping] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const controller = useRef<AbortController | null>(null);
  const dropLifetime = useRef<AbortController | null>(null);
  useEffect(() => () => { dropLifetime.current?.abort(); }, []);
  const dropEnabled = machine.kind === "local" && mode === "add" && !busy && !dropDisabled;
  const { ref: dropRef, active: dropActive } = useRepositoryDrop(dropEnabled, async (paths) => {
    // This signal only guards dialog completion; App owns the accepted addition.
    const lifetime = new AbortController();
    dropLifetime.current = lifetime;
    setBusy(true);
    setDropping(true);
    setError(null);
    try {
      if (await onDropRepositories(paths) && !lifetime.signal.aborted) onClose();
    } finally {
      if (dropLifetime.current === lifetime) dropLifetime.current = null;
      if (!lifetime.signal.aborted) {
        setBusy(false);
        setDropping(false);
      }
    }
  });

  const close = () => {
    if (dropping) {
      dropLifetime.current?.abort();
      onClose();
    } else if (busy) controller.current?.abort();
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
      <DialogContent className="sm:max-w-lg" showCloseButton={!busy || dropping}>
        <form className="contents" onSubmit={(event) => void submit(event)}>
          <DialogHeader>
            <DialogTitle>{machine.kind === "local" ? "Add Repository" : `Add Repository on ${machine.name}`}</DialogTitle>
            <DialogDescription>
              Open an existing repository, clone one, or create a new one.
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
                <Input id="existing-repository-path" name="existing-repository-path" autoComplete="off" autoFocus className="font-mono" value={existingPath} onChange={(event) => setExistingPath(event.currentTarget.value)} spellCheck={false} disabled={busy} />
                <FieldDescription>Enter the full path as it exists on {machine.name}.</FieldDescription>
              </Field>
            ) : (
              <RepositoryDropZone ref={dropRef} active={dropActive} disabled={!dropEnabled} />
            )
          ) : (
            <>
              {mode === "clone" ? (
                <Field>
                  <FieldLabel htmlFor="clone-source">Repository URL</FieldLabel>
                  <Input id="clone-source" name="clone-source" autoComplete="off" autoFocus value={source} onChange={(event) => setSource(event.currentTarget.value)} placeholder="git@github.com:owner/project.git" spellCheck={false} disabled={busy} />
                  <FieldDescription>HTTPS, SSH, and local Git sources are supported.</FieldDescription>
                </Field>
              ) : (
                <Field>
                  <FieldLabel htmlFor="initial-branch">Initial branch</FieldLabel>
                  <Input id="initial-branch" name="initial-branch" autoComplete="off" autoFocus value={initialBranch} onChange={(event) => setInitialBranch(event.currentTarget.value)} spellCheck={false} disabled={busy} />
                </Field>
              )}
              <Field>
                <FieldLabel htmlFor="repository-destination">Destination path</FieldLabel>
                <Input id="repository-destination" name="repository-destination" autoComplete="off" className="font-mono" value={destination} onChange={(event) => setDestination(event.currentTarget.value)} spellCheck={false} disabled={busy} />
                <FieldDescription>The full destination path must not exist or must be empty.</FieldDescription>
              </Field>
            </>
          )}
          {error ? <ActionableGitError message={error} /> : null}
          {dropping ? <p role="status" className="text-sm text-muted-foreground">Repository addition continues in the workspace after you close this dialog.</p> : null}
          <DialogFooter>
            <Button type="button" variant="outline" onClick={close}>{dropping ? "Close dialog" : busy ? "Cancel operation" : "Cancel"}</Button>
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
