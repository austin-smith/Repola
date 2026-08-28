import { CopyIcon, FileInputIcon, FileMinusIcon, FilePenIcon, FilePlusIcon, FileQuestionIcon, FileSymlinkIcon, TriangleAlertIcon } from "lucide-react";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import type { FileChangeKind } from "../ipc/types";

const glyphs: Record<FileChangeKind, { icon: typeof FilePlusIcon; label: string; tone: string }> = {
  added: { icon: FilePlusIcon, label: "Added", tone: "text-success" },
  untracked: { icon: FilePlusIcon, label: "Untracked", tone: "text-success" },
  modified: { icon: FilePenIcon, label: "Modified", tone: "text-warning" },
  deleted: { icon: FileMinusIcon, label: "Deleted", tone: "text-destructive" },
  renamed: { icon: FileInputIcon, label: "Renamed", tone: "text-muted-foreground" },
  copied: { icon: CopyIcon, label: "Copied", tone: "text-muted-foreground" },
  typeChanged: { icon: FileSymlinkIcon, label: "Type changed", tone: "text-muted-foreground" },
  unmerged: { icon: TriangleAlertIcon, label: "Conflicted", tone: "text-destructive" },
  ignored: { icon: FileQuestionIcon, label: "Ignored", tone: "text-muted-foreground" },
  unknown: { icon: FileQuestionIcon, label: "Unknown", tone: "text-muted-foreground" },
};

/**
 * A small, low-contrast outline glyph for a file's change kind, so the
 * filename stays the most prominent thing in the row.
 */
export function ChangeStatusIcon({ kind, conflicted = false, className }: { kind: FileChangeKind; conflicted?: boolean; className?: string }) {
  const glyph = conflicted ? glyphs.unmerged : glyphs[kind];
  const Icon = glyph.icon;
  return (
    <Tooltip>
      <TooltipTrigger render={<span className={cn("grid size-5 shrink-0 place-items-center", glyph.tone, className)} tabIndex={-1} />}>
        <Icon className="size-3.5" aria-hidden="true" strokeWidth={1.75} />
        <span className="sr-only">{glyph.label}</span>
      </TooltipTrigger>
      <TooltipContent>{glyph.label}</TooltipContent>
    </Tooltip>
  );
}
