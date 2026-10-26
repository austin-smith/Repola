import { useId, type Ref } from "react";
import { SlidersHorizontalIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";

type WhitespaceChoice = "show" | "hide";

/**
 * Presentation choices for the text diff on screen. Rendered only for diffs
 * the options apply to; binary, image, and submodule fallbacks never show it.
 */
export function DiffOptionsMenu({
  hideWhitespace,
  onHideWhitespaceChange,
  whitespaceUnavailable = null,
  triggerRef,
}: {
  hideWhitespace: boolean;
  onHideWhitespaceChange: (hide: boolean) => void;
  /** Why the whitespace choice cannot change for this diff, if it cannot. */
  whitespaceUnavailable?: string | null;
  triggerRef?: Ref<HTMLButtonElement>;
}) {
  // Showing whitespace is always allowed, so a filtered diff can never trap
  // the reviewer in the filtered view.
  const hideUnavailable = whitespaceUnavailable !== null && !hideWhitespace;
  const labelId = useId();
  const reasonId = useId();
  return (
    <DropdownMenu>
      <DropdownMenuTrigger render={<Button ref={triggerRef} variant="ghost" size="sm" />}>
        <SlidersHorizontalIcon data-icon="inline-start" aria-hidden="true" />
        Diff options
        {hideWhitespace ? <span className="text-muted-foreground">(whitespace hidden)</span> : null}
      </DropdownMenuTrigger>
      <DropdownMenuContent className="min-w-56" align="end">
        <DropdownMenuGroup>
          <DropdownMenuLabel id={labelId}>Whitespace changes</DropdownMenuLabel>
          <DropdownMenuRadioGroup
            aria-labelledby={labelId}
            value={hideWhitespace ? "hide" : "show"}
            onValueChange={(value: WhitespaceChoice) => onHideWhitespaceChange(value === "hide")}
          >
            <DropdownMenuRadioItem value="show">Show</DropdownMenuRadioItem>
            <DropdownMenuRadioItem
              value="hide"
              disabled={hideUnavailable}
              aria-describedby={hideUnavailable ? reasonId : undefined}
            >
              Hide
            </DropdownMenuRadioItem>
          </DropdownMenuRadioGroup>
          {hideUnavailable ? (
            <p id={reasonId} className="max-w-56 px-1.5 pb-1 text-xs text-muted-foreground">{whitespaceUnavailable}</p>
          ) : null}
        </DropdownMenuGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
