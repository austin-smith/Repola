import type { LucideIcon } from "lucide-react";
import type { ReactNode } from "react";
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
  ListFilterIcon,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuCheckboxItem, DropdownMenuContent, DropdownMenuGroup, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { InputGroupButton } from "@/components/ui/input-group";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import {
  changeKindFilterKind,
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
  children?: ReactNode;
}

export function ChangeKindFilterTrigger({ value, inInput = false }: { value: readonly ChangeKindFilterKind[]; inInput?: boolean }) {
  const label = value.length === 0 ? "Filter by change type" : `Filter by change type: ${value.map((kind) => changeKindLabels[kind]).join(", ")}`;
  const trigger = inInput
    ? <InputGroupButton size="icon-xs" className="relative" aria-label={label} />
    : <Button variant="ghost" size="icon-sm" className="relative" aria-label={label} />;
  return (
    <Tooltip>
      <TooltipTrigger render={<DropdownMenuTrigger render={trigger} />}>
        <ListFilterIcon aria-hidden="true" />
        {value.length > 0 ? <span aria-hidden="true" className="absolute right-1 top-1 size-1 rounded-full bg-brand" /> : null}
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
}

/** A quiet menu for narrowing a file list; no selected kinds shows every kind. */
export function ChangeKindFilter({ counts, value, onValueChange, children }: ChangeKindFilterProps) {
  return (
    <DropdownMenu>
      {children ?? <ChangeKindFilterTrigger value={value} />}
      <DropdownMenuContent align="end" className="w-48">
        <DropdownMenuGroup>
          <DropdownMenuLabel>Change type</DropdownMenuLabel>
          <DropdownMenuCheckboxItem checked={value.length === 0} onCheckedChange={() => onValueChange([])} closeOnClick={false}>
            All change types
          </DropdownMenuCheckboxItem>
        </DropdownMenuGroup>
        <DropdownMenuSeparator />
        <DropdownMenuGroup>
          {counts.map(({ kind, count }) => (
            <DropdownMenuCheckboxItem
              key={kind}
              checked={value.includes(kind)}
              closeOnClick={false}
              aria-label={`${changeKindLabels[kind]} (${count})`}
              onCheckedChange={(checked) => onValueChange(checked ? [...value, kind] : value.filter((entry) => entry !== kind))}
            >
              {changeKindLabels[kind]}
              <span className="ml-auto tabular-nums text-muted-foreground">{count}</span>
            </DropdownMenuCheckboxItem>
          ))}
        </DropdownMenuGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
