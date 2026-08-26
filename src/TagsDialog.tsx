import { useEffect, useMemo, useState } from "react";
import { AlertTriangleIcon, CloudUploadIcon, PlusIcon, SearchIcon, TagIcon, Trash2Icon } from "lucide-react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Field, FieldDescription, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/ui/input-group";
import { Spinner } from "@/components/ui/spinner";
import { Textarea } from "@/components/ui/textarea";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { TooltipButton } from "@/components/tooltip-button";
import { ActionableGitError } from "./ActionableGitError";
import type { RepositorySummary, TagInfo, TagMutationKind, WorkingCopySnapshot, WorktreeRecord } from "./types";
import { shortSha } from "./format";
import { fetchWorkingCopy, loadTags, mutateTag } from "./worktrees";
import { toMessage } from "@/lib/errors";

interface TagsDialogProps {
  machineId: string;
  repository: RepositorySummary;
  worktree: WorktreeRecord;
  initialTarget?: string;
  onClose: () => void;
  onChanged: (snapshot: WorkingCopySnapshot) => void | Promise<void>;
}

type PendingTagAction = { kind: "push" | "delete"; tag: TagInfo };

export default function TagsDialog({ machineId, repository, worktree, initialTarget, onClose, onChanged }: TagsDialogProps) {
  const [snapshot, setSnapshot] = useState<WorkingCopySnapshot | null>(null);
  const [tags, setTags] = useState<TagInfo[] | null>(null);
  const [name, setName] = useState("");
  const [target, setTarget] = useState(initialTarget ?? "HEAD");
  const [creationKind, setCreationKind] = useState<"lightweight" | "annotated">("annotated");
  const [message, setMessage] = useState("");
  const [query, setQuery] = useState("");
  const [pending, setPending] = useState<PendingTagAction | null>(null);
  const [confirmation, setConfirmation] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const controller = new AbortController();
    void Promise.all([
      fetchWorkingCopy(machineId, repository.path, worktree.path, controller.signal),
      loadTags(machineId, repository.path, worktree.path, controller.signal),
    ])
      .then(([nextSnapshot, nextTags]) => {
        setSnapshot(nextSnapshot);
        setTags(nextTags);
        if (!initialTarget) setTarget(nextSnapshot.head ?? "HEAD");
      })
      .catch((cause: unknown) => {
        if (!controller.signal.aborted) setError(toMessage(cause));
      });
    return () => controller.abort();
  }, [initialTarget, machineId, repository.path, worktree.path]);

  const filteredTags = useMemo(() => {
    const needle = query.trim().toLowerCase();
    return (tags ?? []).filter((tag) => !needle || tag.name.toLowerCase().includes(needle) || tag.subject.toLowerCase().includes(needle));
  }, [query, tags]);

  const create = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!snapshot || !name.trim() || !target.trim() || (creationKind === "annotated" && !message.trim())) return;
    setBusy("create");
    setError(null);
    try {
      const result = await mutateTag(
        machineId,
        snapshot,
        creationKind === "annotated" ? "createAnnotated" : "createLightweight",
        name.trim(),
        target.trim(),
        creationKind === "annotated" ? message.trim() : null,
        null,
      );
      setTags(result.tags);
      setSnapshot(result.snapshot);
      setName("");
      setMessage("");
      await onChanged(result.snapshot);
    } catch (cause) {
      setError(toMessage(cause));
    } finally {
      setBusy(null);
    }
  };

  const executePending = async () => {
    if (!pending || !snapshot) return;
    setBusy(`${pending.kind}:${pending.tag.name}`);
    setError(null);
    try {
      const result = await mutateTag(
        machineId,
        snapshot,
        pending.kind as TagMutationKind,
        pending.tag.name,
        null,
        null,
        pending.tag.target,
      );
      setTags(result.tags);
      setSnapshot(result.snapshot);
      setPending(null);
      setConfirmation("");
      await onChanged(result.snapshot);
    } catch (cause) {
      setError(toMessage(cause));
    } finally {
      setBusy(null);
    }
  };

  const deleteConfirmation = pending?.kind === "delete" ? `DELETE ${pending.tag.name}` : "";

  return (
    <Dialog open onOpenChange={(open) => { if (!open && busy === null) onClose(); }}>
      <DialogContent className="flex max-h-[88vh] flex-col sm:max-w-3xl" showCloseButton={busy === null}>
        <DialogHeader>
          <DialogTitle>Tags</DialogTitle>
          <DialogDescription>Create named release points at exact commits, inspect local tags, or publish one tag at a time without force.</DialogDescription>
        </DialogHeader>

        <form className="grid shrink-0 grid-cols-2 gap-3 border bg-card p-3" onSubmit={(event) => void create(event)}>
          <Field>
            <FieldLabel htmlFor="tag-name">Tag name</FieldLabel>
            <Input id="tag-name" value={name} onChange={(event) => setName(event.currentTarget.value)} placeholder="v1.0.0" autoComplete="off" spellCheck={false} disabled={busy !== null} />
          </Field>
          <Field>
            <FieldLabel htmlFor="tag-target">Commit or reference</FieldLabel>
            <Input id="tag-target" value={target} onChange={(event) => setTarget(event.currentTarget.value)} placeholder="HEAD" autoComplete="off" spellCheck={false} disabled={busy !== null} />
            <FieldDescription>Resolved to an exact commit immediately before creation.</FieldDescription>
          </Field>
          <div className="col-span-2 flex items-start gap-3">
            <Field className="w-56 shrink-0">
              <FieldLabel>Tag type</FieldLabel>
              <ToggleGroup variant="outline" value={[creationKind]} onValueChange={(value) => { const next = value[0]; if (next === "lightweight" || next === "annotated") setCreationKind(next); }} disabled={busy !== null}>
                <ToggleGroupItem value="annotated">Annotated</ToggleGroupItem>
                <ToggleGroupItem value="lightweight">Lightweight</ToggleGroupItem>
              </ToggleGroup>
            </Field>
            {creationKind === "annotated" ? (
              <Field className="min-w-0 flex-1">
                <FieldLabel htmlFor="tag-message">Annotation</FieldLabel>
                <Textarea id="tag-message" value={message} onChange={(event) => setMessage(event.currentTarget.value)} placeholder="Release summary" rows={2} maxLength={16 * 1024} disabled={busy !== null} />
              </Field>
            ) : <p className="pt-7 text-xs leading-relaxed text-muted-foreground">A lightweight tag is only a name pointing at the selected commit. Annotated tags also preserve a tagger, timestamp, and message.</p>}
          </div>
          <div className="col-span-2 flex justify-end">
            <Button type="submit" disabled={!snapshot || busy !== null || !name.trim() || !target.trim() || (creationKind === "annotated" && !message.trim())}>
              {busy === "create" ? <Spinner data-icon="inline-start" /> : <PlusIcon data-icon="inline-start" aria-hidden="true" />}
              {busy === "create" ? "Creating…" : "Create Tag"}
            </Button>
          </div>
        </form>

        <InputGroup className="shrink-0">
          <InputGroupAddon><SearchIcon aria-hidden="true" /></InputGroupAddon>
          <InputGroupInput value={query} onChange={(event) => setQuery(event.currentTarget.value)} placeholder="Filter tags…" aria-label="Filter tags" />
        </InputGroup>

        <section className="min-h-0 flex-1 overflow-y-auto border" aria-label="Repository tags">
          {tags === null && !error ? <div className="grid h-28 place-items-center"><Spinner className="size-5" /></div> : null}
          {tags?.length === 0 ? <div className="grid h-28 place-items-center text-sm text-muted-foreground">No tags yet.</div> : null}
          {tags !== null && tags.length > 0 && filteredTags.length === 0 ? <div className="grid h-28 place-items-center text-sm text-muted-foreground">No matching tags.</div> : null}
          {filteredTags.map((tag) => (
            <div key={tag.name} className="flex min-h-16 items-center gap-3 border-b px-3 py-2 last:border-b-0">
              <div className="grid size-8 shrink-0 place-items-center bg-muted text-muted-foreground"><TagIcon className="size-4" aria-hidden="true" /></div>
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2"><strong className="truncate text-sm font-medium">{tag.name}</strong><Badge variant="outline">{tag.annotated ? "annotated" : "lightweight"}</Badge></div>
                <div className="mt-1 flex min-w-0 items-center gap-2 text-xs text-muted-foreground"><code className="font-mono">{shortSha(tag.target)}</code>{tag.subject ? <span className="truncate">{tag.subject}</span> : null}</div>
              </div>
              <TooltipButton
                variant="outline"
                size="sm"
                disabled={busy !== null || snapshot?.remote === null}
                tooltip={snapshot?.remote ? `Publish to ${snapshot.remote}` : "Configure a remote first"}
                onClick={() => { setPending({ kind: "push", tag }); setConfirmation(""); setError(null); }}
              >
                <CloudUploadIcon data-icon="inline-start" aria-hidden="true" />
                Publish
              </TooltipButton>
              <TooltipButton variant="ghost" size="icon-sm" className="text-destructive" disabled={busy !== null} aria-label={`Delete ${tag.name}`} tooltip="Delete tag" onClick={() => { setPending({ kind: "delete", tag }); setConfirmation(""); setError(null); }}><Trash2Icon aria-hidden="true" /></TooltipButton>
            </div>
          ))}
        </section>

        {pending ? (
          <Alert variant={pending.kind === "delete" ? "destructive" : "default"}>
            {pending.kind === "delete" ? <AlertTriangleIcon aria-hidden="true" /> : <CloudUploadIcon aria-hidden="true" />}
            <AlertTitle>{pending.kind === "delete" ? `Delete local tag ${pending.tag.name}?` : `Publish ${pending.tag.name} to ${snapshot?.remote}?`}</AlertTitle>
            <AlertDescription>
              {pending.kind === "delete" ? "This removes only the local tag. A previously published remote tag is intentionally left untouched." : "Repola pushes only this exact tag without force. Git refuses if the remote already has a conflicting tag."}
            </AlertDescription>
            {pending.kind === "delete" ? <Input className="mt-3" value={confirmation} onChange={(event) => setConfirmation(event.currentTarget.value)} placeholder={deleteConfirmation} aria-label={`Type ${deleteConfirmation} to confirm`} disabled={busy !== null} /> : null}
            <div className="mt-3 flex justify-end gap-2">
              <Button variant="outline" size="sm" disabled={busy !== null} onClick={() => { setPending(null); setConfirmation(""); }}>Cancel</Button>
              <Button variant={pending.kind === "delete" ? "destructive" : "default"} size="sm" disabled={busy !== null || (pending.kind === "delete" && confirmation !== deleteConfirmation)} onClick={() => void executePending()}>
                {busy ? <Spinner data-icon="inline-start" /> : pending.kind === "delete" ? <Trash2Icon data-icon="inline-start" aria-hidden="true" /> : <CloudUploadIcon data-icon="inline-start" aria-hidden="true" />}
                {busy ? "Revalidating…" : pending.kind === "delete" ? "Delete Local Tag" : "Publish Tag"}
              </Button>
            </div>
          </Alert>
        ) : null}

        {error ? <ActionableGitError message={error} /> : null}
        <DialogFooter><Button variant="outline" disabled={busy !== null} onClick={onClose}>Close</Button></DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

