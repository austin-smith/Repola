import { PilcrowIcon } from "lucide-react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";

export const truncatedDiffWhitespaceReason = "Unavailable because this diff was too large and was truncated.";

const hiddenSelectionText = "Some lines selected for the commit are hidden here, but they are still included.";

/**
 * Explains, next to a whitespace-filtered working-copy diff, why its lines
 * cannot be selected, and offers the way back to the exact diff.
 */
export function HiddenWhitespaceNotice({ hasHiddenSelection, onShowWhitespace }: {
  /** The file has a line selection the filtered diff cannot show. */
  hasHiddenSelection: boolean;
  onShowWhitespace: () => void;
}) {
  return (
    <Alert role="status" className="m-3 w-auto">
      <PilcrowIcon aria-hidden="true" />
      <AlertTitle>Whitespace changes are hidden</AlertTitle>
      <AlertDescription>
        <p>Line and hunk selection is off, because a patch built from a whitespace-filtered diff can’t be applied safely.</p>
        {hasHiddenSelection ? <p className="font-medium text-foreground">{hiddenSelectionText}</p> : null}
        <Button type="button" variant="outline" size="xs" className="mt-2" onClick={onShowWhitespace}>
          Show whitespace
        </Button>
      </AlertDescription>
    </Alert>
  );
}

/** A file whose every change is whitespace, viewed with whitespace hidden. */
export function OnlyWhitespaceChanged({ hasHiddenSelection = false, onShowWhitespace }: {
  hasHiddenSelection?: boolean;
  onShowWhitespace: () => void;
}) {
  return (
    <div className="grid min-h-full place-items-center p-8 text-center">
      <div className="flex max-w-md flex-col items-center gap-1">
        <strong className="text-sm">Only whitespace changes found</strong>
        <p className="text-sm text-muted-foreground">This file’s changes are hidden because whitespace changes are hidden.</p>
        {hasHiddenSelection ? <p className="text-sm font-medium">{hiddenSelectionText}</p> : null}
        <Button type="button" variant="outline" size="sm" className="mt-2" onClick={onShowWhitespace}>
          Show whitespace
        </Button>
      </div>
    </div>
  );
}
