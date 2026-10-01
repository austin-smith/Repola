import type { Ref } from "react";
import { FolderPlusIcon } from "lucide-react";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "./ui/empty";
import { cn } from "../lib/utils";

export function RepositoryDropZone({ ref, active, disabled, className }: {
  ref: Ref<HTMLDivElement>;
  active: boolean;
  disabled: boolean;
  className?: string;
}) {
  return (
    <Empty
      ref={ref}
      role="region"
      aria-label="Drop repositories"
      aria-disabled={disabled}
      className={cn("flex-none border bg-muted/35", active && "border-brand bg-brand/10", disabled && "opacity-50", className)}
    >
      <EmptyHeader>
        <EmptyMedia variant="icon"><FolderPlusIcon aria-hidden="true" /></EmptyMedia>
        <EmptyTitle>{active ? "Drop to add repositories" : "Drop repositories here"}</EmptyTitle>
        <EmptyDescription>Drop a Git working copy folder, or a file inside one. Repola will inspect it without changing the repository.</EmptyDescription>
      </EmptyHeader>
    </Empty>
  );
}
