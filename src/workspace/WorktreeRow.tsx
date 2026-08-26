import { memo } from "react";
import { AlertTriangleIcon, ChevronRightIcon, GitBranchIcon } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Checkbox } from "@/components/ui/checkbox";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import type { SafetyLevel, WorktreeRecord } from "../ipc/types";
import { useShortPath } from "../app/environment";
import { formatAge, formatMeasuredBytes } from "../domain/format";
import { isRemovable } from "../domain/inventory";

export const inventoryGridClass =
  "grid grid-cols-[24px_minmax(220px,2.4fr)_82px_108px_minmax(108px,1fr)_72px_18px] items-center gap-x-2.5";

export const safetyBadgeVariant: Record<SafetyLevel, "destructive" | "success" | "warning" | "secondary"> = {
  protected: "destructive",
  review: "success",
  repair: "warning",
  metadataOnly: "secondary",
};

interface WorktreeRowProps {
  now: number;
  selected: boolean;
  checked: boolean;
  worktree: WorktreeRecord;
  onSelect: (id: string) => void;
  onToggleChecked: (id: string) => void;
  onContextMenu: (event: React.MouseEvent, worktree: WorktreeRecord) => void;
}

export const WorktreeRow = memo(function WorktreeRow({ now, selected, checked, worktree, onSelect, onToggleChecked, onContextMenu }: WorktreeRowProps) {
  const shortPath = useShortPath();
  const ageTimestamp = worktree.lastActivityAtMs;
  const dirty = worktree.status.available && worktree.status.total > 0;
  const removable = isRemovable(worktree);
  const stateLabel = worktree.isPrimary
    ? "Primary"
    : worktree.registration.kind === "prunable"
      ? "Prunable"
      : worktree.registration.kind === "locked"
        ? "Locked"
        : worktree.registration.kind === "brokenLink"
          ? "Broken link"
          : worktree.registration.kind === "missing"
            ? "Missing"
            : dirty
              ? `${worktree.status.total} changed`
              : worktree.status.available ? "Clean" : "Inspect";

  return (
    <div
      className={cn(
        inventoryGridClass,
        "repola-windowed-row min-h-16 w-full cursor-pointer border-b px-3 py-1.5 pl-4 text-left outline-none hover:bg-muted/60 focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring",
        selected && "bg-card shadow-[inset_3px_0_0_var(--brand)]",
      )}
      role="row"
      tabIndex={0}
      aria-selected={selected}
      aria-label={`${worktree.repositoryName}, ${worktree.branch ?? "detached HEAD"}, ${stateLabel}, activity age ${formatAge(ageTimestamp, now)}`}
      onClick={() => onSelect(worktree.id)}
      onContextMenu={(event) => onContextMenu(event, worktree)}
      onKeyDown={(event) => {
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          onSelect(worktree.id);
        }
      }}
    >
      <span className="flex items-center justify-center" role="cell">
        {removable && (
          <Checkbox
            checked={checked}
            aria-label={`Select ${worktree.branch ?? worktree.path} for bulk removal review`}
            onClick={(event) => event.stopPropagation()}
            onKeyDown={(event) => event.stopPropagation()}
            onCheckedChange={() => onToggleChecked(worktree.id)}
          />
        )}
      </span>
      <span className="flex min-w-0 flex-col gap-0.5" role="cell">
        <span className="flex min-w-0 items-center gap-1.5">
          <GitBranchIcon className="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
          <span className="truncate text-sm font-medium">{worktree.branch ?? "detached HEAD"}</span>
          <Badge variant={worktree.origin.kind === "agent" ? "brand" : "outline"} data-origin-id={worktree.origin.id}>
            {worktree.origin.label}
          </Badge>
        </span>
        <span className="truncate text-xs font-medium text-foreground/70">{worktree.repositoryName}</span>
        <Tooltip>
          <TooltipTrigger
            render={<span className="truncate font-mono text-xs text-muted-foreground" tabIndex={0} aria-label={worktree.path} />}
          >
            {shortPath(worktree.path)}
          </TooltipTrigger>
          <TooltipContent>{worktree.path}</TooltipContent>
        </Tooltip>
      </span>
      <span className="flex flex-col gap-0.5" role="cell">
        <span className="font-mono text-sm tabular-nums">{formatAge(ageTimestamp, now)}</span>
        <span className="text-xs text-muted-foreground">last activity</span>
      </span>
      <span role="cell">
        <Badge variant={safetyBadgeVariant[worktree.safety.level]}>
          {(dirty || worktree.safety.level === "repair") && <AlertTriangleIcon aria-hidden="true" />}
          {stateLabel}
        </Badge>
      </span>
      <span className="text-xs text-muted-foreground" role="cell">
        {worktree.integration.kind === "headContained" ? "HEAD contained" :
          worktree.integration.kind === "headNotContained" ? "Not contained" :
            worktree.integration.kind === "notApplicable" ? "—" : "Unknown"}
      </span>
      {worktree.sizeIncomplete ? (
        <Tooltip>
          <TooltipTrigger render={<span className="text-right font-mono text-sm tabular-nums" role="cell" tabIndex={0} />}>
            {formatMeasuredBytes(worktree.sizeBytes, true)}
          </TooltipTrigger>
          <TooltipContent>Some files could not be measured; this is a lower bound.</TooltipContent>
        </Tooltip>
      ) : (
        <span className="text-right font-mono text-sm tabular-nums" role="cell">
          {formatMeasuredBytes(worktree.sizeBytes, false)}
        </span>
      )}
      <ChevronRightIcon className="size-4 text-border" aria-hidden="true" />
    </div>
  );
});
