import type { ReactNode } from "react";
import { FileDiffIcon, GitCommitIcon, NetworkIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import type { WorkspaceView } from "../ipc/types";

export function WorkspaceNavigation({
  view,
  onViewChange,
}: {
  view: WorkspaceView;
  onViewChange: (view: WorkspaceView) => void;
}) {
  const destination = (target: WorkspaceView, label: string, icon: ReactNode) => (
    <Button
      variant="ghost"
      size="sm"
      className={cn(
        "h-10 w-full justify-start gap-3 px-3 font-normal",
        view === target && "bg-sidebar-accent text-sidebar-accent-foreground shadow-[inset_3px_0_0_var(--brand)]",
      )}
      aria-current={view === target ? "page" : undefined}
      aria-label={target === "worktrees" ? "Open Worktree Manager" : `Show ${label}`}
      onClick={() => onViewChange(target)}
    >
      {icon}
      <span className="flex-1 text-left">{label}</span>
    </Button>
  );

  return (
    <nav className="flex flex-col gap-1 px-2 py-1" aria-label="Primary workspace">
      {destination("changes", "Changes", <FileDiffIcon aria-hidden="true" />)}
      {destination("history", "History", <GitCommitIcon aria-hidden="true" />)}
      {destination("worktrees", "Worktrees", <NetworkIcon aria-hidden="true" />)}
    </nav>
  );
}
