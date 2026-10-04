import type { LucideIcon } from "lucide-react";
import {
  AlertTriangleIcon,
  EyeOffIcon,
  FileMinusIcon,
  FilePenLineIcon,
  FilePlusIcon,
  FileQuestionMarkIcon,
  FilesIcon,
  FileSymlinkIcon,
  FileTypeIcon,
} from "lucide-react";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import {
  changeKindFilterKind,
  isChangeKindFilterKind,
  type ChangeKindCount,
  type ChangeKindFilterKind,
} from "../domain/change-kind-filter";
import type { FileChangeKind } from "../ipc/types";
import { changeKindLabels } from "./labels";

const changeKindIcons: Record<ChangeKindFilterKind, { icon: LucideIcon; className: string }> = {
  added: { icon: FilePlusIcon, className: "text-success" },
  modified: { icon: FilePenLineIcon, className: "text-warning" },
  deleted: { icon: FileMinusIcon, className: "text-destructive" },
  renamed: { icon: FileSymlinkIcon, className: "text-brand" },
  copied: { icon: FilesIcon, className: "text-brand" },
  typeChanged: { icon: FileTypeIcon, className: "text-warning" },
  unmerged: { icon: AlertTriangleIcon, className: "text-destructive" },
  unknown: { icon: FileQuestionMarkIcon, className: "text-muted-foreground" },
  ignored: { icon: EyeOffIcon, className: "text-muted-foreground" },
};

export function ChangeKindIcon({ kind, className }: { kind: FileChangeKind; className?: string }) {
  const filterKind = changeKindFilterKind(kind);
  const { icon: Icon, className: tone } = changeKindIcons[filterKind];
  return <Icon className={cn("size-3.5 shrink-0", tone, className)} role="img" aria-label={changeKindLabels[filterKind]} />;
}

interface ChangeKindFilterProps {
  counts: readonly ChangeKindCount[];
  value: readonly ChangeKindFilterKind[];
  onValueChange: (value: ChangeKindFilterKind[]) => void;
  className?: string;
}

/** Toggles that narrow a changed-file list to the pressed kinds; none pressed shows every kind. */
export function ChangeKindFilter({ counts, value, onValueChange, className }: ChangeKindFilterProps) {
  return (
    <ToggleGroup
      multiple
      variant="outline"
      size="sm"
      spacing={0}
      aria-label="Filter by change type"
      value={[...value]}
      onValueChange={(next) => onValueChange(next.filter(isChangeKindFilterKind))}
      className={className}
    >
      {counts.map(({ kind, count }) => (
        <Tooltip key={kind}>
          <TooltipTrigger render={<ToggleGroupItem value={kind} aria-label={`${changeKindLabels[kind]} (${count})`} />}>
            <ChangeKindIcon kind={kind} />
            <span className="tabular-nums">{count}</span>
          </TooltipTrigger>
          <TooltipContent>{changeKindLabels[kind]}</TooltipContent>
        </Tooltip>
      ))}
    </ToggleGroup>
  );
}
