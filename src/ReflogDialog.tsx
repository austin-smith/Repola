import { useEffect, useMemo, useState } from "react";
import { Clock3Icon, RotateCcwIcon, SearchIcon } from "lucide-react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/ui/input-group";
import { Spinner } from "@/components/ui/spinner";
import { ActionableGitError } from "./ActionableGitError";
import type { HistoryMutationResult, ReflogEntry, RepositorySummary, WorkingCopySnapshot, WorktreeRecord } from "./types";
import { shortSha } from "./format";
import HistoryMutationDialog from "./HistoryMutationDialog";
import { loadReflog } from "./worktrees";
import { toMessage } from "@/lib/errors";

interface ReflogDialogProps {
  machineId: string;
  repository: RepositorySummary;
  worktree: WorktreeRecord;
  onClose: () => void;
  onChanged: (snapshot: WorkingCopySnapshot) => void | Promise<void>;
  onNeedsResolution: () => void;
}

export default function ReflogDialog({ machineId, repository, worktree, onClose, onChanged, onNeedsResolution }: ReflogDialogProps) {
  const [entries, setEntries] = useState<ReflogEntry[] | null>(null);
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<ReflogEntry | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const controller = new AbortController();
    void loadReflog(machineId, repository.path, worktree.path, controller.signal)
      .then(setEntries)
      .catch((cause: unknown) => {
        if (!controller.signal.aborted) setError(toMessage(cause));
      });
    return () => controller.abort();
  }, [machineId, repository.path, worktree.path]);

  const filtered = useMemo(() => {
    const needle = query.trim().toLowerCase();
    return (entries ?? []).filter((entry) => !needle
      || entry.subject.toLowerCase().includes(needle)
      || entry.selector.toLowerCase().includes(needle)
      || entry.oid.toLowerCase().includes(needle));
  }, [entries, query]);

  const complete = async (result: HistoryMutationResult) => {
    await onChanged(result.snapshot);
    if (result.conflicted || result.snapshot.operation) onNeedsResolution();
    onClose();
  };

  if (selected) {
    return (
      <HistoryMutationDialog
        machineId={machineId}
        repository={repository}
        worktree={worktree}
        kinds={["resetSoft", "resetMixed", "resetHard"]}
        targets={[{ id: selected.selector, oid: selected.oid, label: selected.selector, detail: selected.subject }]}
        initialKind="resetMixed"
        title={`Restore ${selected.selector}`}
        onClose={() => setSelected(null)}
        onCompleted={complete}
      />
    );
  }

  return (
    <Dialog open onOpenChange={(open) => { if (!open) onClose(); }}>
      <DialogContent className="flex max-h-[86vh] flex-col sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>Undo points</DialogTitle>
          <DialogDescription>The reflog records where this worktree’s HEAD has been, including commits made unreachable by reset, rebase, amend, or branch movement.</DialogDescription>
        </DialogHeader>
        <Alert>
          <Clock3Icon aria-hidden="true" />
          <AlertDescription>Reflog entries are local to this machine and Git may eventually expire them. Restore important work before routine Git maintenance removes an old entry.</AlertDescription>
        </Alert>
        <InputGroup>
          <InputGroupAddon><SearchIcon aria-hidden="true" /></InputGroupAddon>
          <InputGroupInput value={query} onChange={(event) => setQuery(event.currentTarget.value)} placeholder="Search actions, selectors, or commit IDs…" aria-label="Search undo points" autoComplete="off" spellCheck={false} />
        </InputGroup>
        {error ? <ActionableGitError message={error} /> : null}
        <section className="min-h-0 flex-1 overflow-y-auto border" aria-label="Reflog undo points">
          {entries === null && !error ? <div className="grid h-32 place-items-center"><Spinner className="size-5" /></div> : null}
          {entries?.length === 0 ? <div className="grid h-32 place-items-center text-sm text-muted-foreground">No undo points are available yet.</div> : null}
          {entries !== null && entries.length > 0 && filtered.length === 0 ? <div className="grid h-32 place-items-center text-sm text-muted-foreground">No matching undo points.</div> : null}
          {filtered.map((entry, index) => {
            const current = entry.oid === worktree.head && index === 0;
            return (
              <div key={`${entry.selector}-${entry.oid}`} className="flex min-h-16 items-center gap-3 border-b px-3 py-2 last:border-b-0">
                <div className="grid size-8 shrink-0 place-items-center bg-muted text-muted-foreground"><Clock3Icon className="size-4" aria-hidden="true" /></div>
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2"><strong className="font-mono text-xs font-medium">{entry.selector}</strong><code className="font-mono text-xs text-muted-foreground">{shortSha(entry.oid)}</code>{current ? <Badge variant="secondary">current</Badge> : null}</div>
                  <p className="mt-1 truncate text-sm">{entry.subject}</p>
                  <time className="mt-0.5 block text-xs text-muted-foreground" dateTime={entry.committedAt}>{new Date(entry.committedAt).toLocaleString()}</time>
                </div>
                <Button variant="outline" size="sm" disabled={current} onClick={() => setSelected(entry)}><RotateCcwIcon data-icon="inline-start" aria-hidden="true" />Restore…</Button>
              </div>
            );
          })}
        </section>
        <DialogFooter><Button variant="outline" onClick={onClose}>Close</Button></DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

