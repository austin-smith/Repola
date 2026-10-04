import { useRef, useState } from "react";
import { AlertTriangleIcon, Trash2Icon } from "lucide-react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Field, FieldDescription, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Spinner } from "@/components/ui/spinner";
import { deletionNotice } from "../domain/branch-deletion";
import type { BranchDeletionResult } from "../ipc/types";

export interface BulkItem {
  key: string;
  title: string;
  subtitle: string;
  sizeLabel?: string;
  /** The reviewed command, or null when the item could not be reviewed. */
  command: string | null;
  warnings: string[];
  error: string | null;
  /** The action was interrupted after it started, so it may have completed anyway. */
  unconfirmed?: boolean;
  /** A finished branch deletion, reported as the single-deletion dialog reports it. */
  deletion?: BranchDeletionResult;
  done: boolean;
}

export type BulkStage = "review" | "running" | "done";

interface BulkActionDialogProps {
  title: string;
  summary: string;
  confirmationText: string;
  stage: BulkStage;
  items: BulkItem[];
  followUpLabel: string | null;
  onCancel: () => void;
  onConfirm: () => void;
  onFollowUp: () => void;
}

function itemBadge(item: BulkItem, stage: BulkStage) {
  if (item.done && item.deletion && deletionNotice(item.deletion).type === "warning") {
    return <Badge variant="warning">Done with warnings</Badge>;
  }
  if (item.done) return <Badge variant="success">Done</Badge>;
  if (item.unconfirmed) return <Badge variant="warning">Unconfirmed</Badge>;
  if (item.error) return <Badge variant="destructive">{stage === "review" ? "Blocked" : "Failed"}</Badge>;
  if (stage === "review") return <Badge variant="secondary">Ready</Badge>;
  return <Badge variant="ghost">Pending…</Badge>;
}

export function BulkActionDialog({
  title,
  summary,
  confirmationText,
  stage,
  items,
  followUpLabel,
  onCancel,
  onConfirm,
  onFollowUp,
}: BulkActionDialogProps) {
  const [confirmation, setConfirmation] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);
  const busy = stage === "running";

  const readyCount = items.filter((item) => item.command !== null && !item.error && !item.done).length;
  const doneCount = items.filter((item) => item.done).length;
  const failedCount = items.filter((item) => item.error).length;
  const unconfirmedCount = items.filter((item) => item.unconfirmed).length;
  const warnings = Array.from(new Set(items.flatMap((item) => item.warnings)));
  const confirmed = confirmation === confirmationText;

  return (
    <Dialog open onOpenChange={(open) => { if (!open && !busy) onCancel(); }}>
      <DialogContent className="max-h-[calc(100dvh-2rem)] grid-cols-1 overflow-y-auto sm:max-w-xl" initialFocus={stage === "review" ? inputRef : undefined}>
        <DialogHeader>
          <div className="flex items-center gap-3 pr-6">
            <div className="flex size-9 shrink-0 items-center justify-center bg-destructive/10 text-destructive">
              <Trash2Icon className="size-4" aria-hidden="true" />
            </div>
            <div className="flex min-w-0 flex-col gap-1 wrap-anywhere">
              <span className="text-xs font-medium tracking-widest text-muted-foreground uppercase">
                {stage === "done"
                  ? [`${doneCount} completed`, `${failedCount - unconfirmedCount} not completed`, unconfirmedCount > 0 && `${unconfirmedCount} unconfirmed`].filter(Boolean).join(" · ")
                  : "Batch preflight complete"}
              </span>
              <DialogTitle>{title}</DialogTitle>
            </div>
          </div>
          <DialogDescription>{summary}</DialogDescription>
        </DialogHeader>

        <ScrollArea className="min-w-0 border bg-card [&_[data-slot=scroll-area-viewport]]:max-h-[44dvh]">
          <div className="flex flex-col gap-2 p-2" role="list">
            {items.map((item) => (
              <div key={item.key} role="listitem" className="flex flex-col gap-1 border bg-background p-2.5">
                <div className="flex items-center gap-2">
                  <span className="truncate text-sm font-medium">{item.title}</span>
                  {item.sizeLabel && <span className="shrink-0 font-mono text-xs text-muted-foreground">{item.sizeLabel}</span>}
                  <div className="ml-auto shrink-0">{itemBadge(item, stage)}</div>
                </div>
                <code className="font-mono text-xs break-all text-muted-foreground">{item.subtitle}</code>
                {item.error && <p className="text-xs wrap-anywhere text-destructive">{item.error}</p>}
                {item.done && item.deletion ? <DeletionOutcome deletion={item.deletion} /> : null}
                {!item.error && item.command !== null && (
                  <code className="border-l-2 border-foreground bg-muted px-2 py-1 font-mono text-xs whitespace-pre-wrap break-all">
                    {item.command}
                  </code>
                )}
              </div>
            ))}
          </div>
        </ScrollArea>

        {warnings.map((warning) => (
          <Alert key={warning} variant="warning">
            <AlertTriangleIcon aria-hidden="true" />
            <AlertDescription>{warning}</AlertDescription>
          </Alert>
        ))}

        {stage !== "done" && (
          <Field>
            <FieldLabel htmlFor="bulk-confirmation">Type {confirmationText} to confirm</FieldLabel>
            <Input
              id="bulk-confirmation"
              ref={inputRef}
              value={confirmation}
              disabled={busy}
              autoCapitalize="characters"
              autoComplete="off"
              spellCheck={false}
              onChange={(event) => setConfirmation(event.currentTarget.value.toUpperCase())}
            />
            <FieldDescription>
              Runs the {readyCount} ready action{readyCount === 1 ? "" : "s"} one at a time; blocked entries are skipped.
            </FieldDescription>
          </Field>
        )}

        <DialogFooter>
          <Button variant="outline" disabled={busy} onClick={onCancel}>{stage === "done" ? "Close" : "Cancel"}</Button>
          {stage === "done" && followUpLabel && <Button onClick={onFollowUp}>{followUpLabel}</Button>}
          {stage !== "done" && (
            <Button variant="destructive" disabled={!confirmed || busy || readyCount === 0} onClick={onConfirm}>
              {busy && <Spinner data-icon="inline-start" />}
              {busy ? `Running ${Math.min(doneCount + failedCount + 1, items.length)} of ${items.length}…` : `${confirmationText} (${readyCount})`}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/** What a finished branch deletion left to do and how to restore it, as the single deletion reports it. */
function DeletionOutcome({ deletion }: { deletion: BranchDeletionResult }) {
  const { type, description } = deletionNotice(deletion);
  if (!description) return null;
  return <p className={`text-xs wrap-anywhere ${type === "warning" ? "text-warning" : "text-muted-foreground"}`}>{description}</p>;
}
