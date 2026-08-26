import { useRef, useState } from "react";
import { AlertTriangleIcon, CheckIcon } from "lucide-react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Field, FieldDescription, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Spinner } from "@/components/ui/spinner";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type { ActionPlan } from "./types";
import { useShortPath } from "./environment";

interface ActionDialogProps {
  busy: boolean;
  error: string | null;
  plan: ActionPlan;
  onCancel: () => void;
  onConfirm: () => void;
}

export function ActionDialog({ busy, error, plan, onCancel, onConfirm }: ActionDialogProps) {
  const shortPath = useShortPath();
  const [confirmation, setConfirmation] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);
  const confirmed = confirmation === plan.confirmationText;
  const affectedLabel = `${plan.affectedPaths.length} ${plan.affectedPaths.length === 1 ? "path" : "paths"}`;

  return (
    <Dialog open onOpenChange={(open) => { if (!open && !busy) onCancel(); }}>
      <DialogContent className="sm:max-w-lg" initialFocus={inputRef}>
        <DialogHeader>
          <div className="flex items-center gap-3">
            <div className={`flex size-9 shrink-0 items-center justify-center ${plan.destructive ? "bg-destructive/10 text-destructive" : "bg-success/10 text-success"}`}>
              {plan.destructive ? <AlertTriangleIcon className="size-4" aria-hidden="true" /> : <CheckIcon className="size-4" aria-hidden="true" />}
            </div>
            <div className="flex flex-col gap-1">
              <span className="text-xs font-medium tracking-widest text-muted-foreground uppercase">Preflight complete</span>
              <DialogTitle>{plan.title}</DialogTitle>
            </div>
          </div>
          <DialogDescription>{plan.summary}</DialogDescription>
        </DialogHeader>

        <div className="grid grid-cols-3 gap-px border bg-border">
          <DialogFact label="Repository" value={shortPath(plan.repositoryPath)} />
          {plan.branch && <DialogFact label="Branch" value={plan.branch} />}
          <DialogFact label="Affected" value={affectedLabel} />
        </div>

        <ScrollArea className="max-h-28 border bg-card">
          <div className="flex flex-col gap-1 p-2">
            {plan.affectedPaths.map((path) => (
              <code key={path} className="font-mono text-xs break-all text-muted-foreground">{shortPath(path)}</code>
            ))}
          </div>
        </ScrollArea>

        {plan.warnings.map((warning) => (
          <Alert key={warning} variant="warning">
            <AlertTriangleIcon aria-hidden="true" />
            <AlertDescription>{warning}</AlertDescription>
          </Alert>
        ))}

        <div className="flex flex-col gap-1.5">
          <span className="text-xs font-medium tracking-widest text-muted-foreground uppercase">Exact command</span>
          <code className="overflow-x-auto border-l-2 border-foreground bg-muted p-2 font-mono text-xs whitespace-nowrap">{plan.commandDisplay}</code>
        </div>

        <Field>
          <FieldLabel htmlFor="action-confirmation">Type {plan.confirmationText} to confirm</FieldLabel>
          <Input
            id="action-confirmation"
            ref={inputRef}
            value={confirmation}
            disabled={busy}
            autoCapitalize="characters"
            autoComplete="off"
            spellCheck={false}
            onChange={(event) => setConfirmation(event.currentTarget.value.toUpperCase())}
          />
          <FieldDescription>The action runs a fresh revalidation before executing.</FieldDescription>
        </Field>

        {error && (
          <Alert variant="destructive" role="alert">
            <AlertTriangleIcon aria-hidden="true" />
            <AlertTitle>The action was not executed</AlertTitle>
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        )}

        <DialogFooter>
          <Button variant="outline" disabled={busy} onClick={onCancel}>Cancel</Button>
          <Button variant={plan.destructive ? "destructive" : "default"} disabled={!confirmed || busy} onClick={onConfirm}>
            {busy && <Spinner data-icon="inline-start" />}
            {busy ? "Revalidating…" : plan.confirmationText}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function DialogFact({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex min-w-0 flex-col gap-1 bg-card p-2.5">
      <span className="text-xs text-muted-foreground">{label}</span>
      <Tooltip>
        <TooltipTrigger render={<span className="truncate text-xs font-medium" tabIndex={0} />}>
          {value}
        </TooltipTrigger>
        <TooltipContent>{value}</TooltipContent>
      </Tooltip>
    </div>
  );
}
