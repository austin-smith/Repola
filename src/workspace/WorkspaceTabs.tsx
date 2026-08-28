import { FileDiffIcon, GitCommitIcon } from "lucide-react";
import { cn } from "@/lib/utils";
import { useRepositoryContext } from "./context";

/**
 * Changes | History at the top of the sidebar. Both are views of the selected
 * worktree, which is why they live here rather than in the repository toolbar.
 */
export function WorkspaceTabs() {
  const { view, showView } = useRepositoryContext();
  const tabs = [
    { id: "changes", label: "Changes", icon: FileDiffIcon },
    { id: "history", label: "History", icon: GitCommitIcon },
  ] as const;
  return (
    <div className="grid h-10 shrink-0 grid-cols-2 border-b bg-card" role="tablist" aria-label="Worktree views">
      {tabs.map(({ id, label, icon: Icon }) => {
        const selected = view === id;
        return (
          <button
            key={id}
            type="button"
            role="tab"
            aria-selected={selected}
            className={cn(
              "flex items-center justify-center gap-1.5 text-sm outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:ring-inset",
              selected ? "-mb-px border-b-2 border-brand font-medium text-foreground" : "text-muted-foreground hover:bg-muted/60 hover:text-foreground",
            )}
            onClick={() => showView(id)}
          >
            <Icon className="size-4" aria-hidden="true" />
            {label}
          </button>
        );
      })}
    </div>
  );
}
