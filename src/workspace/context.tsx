import { createContext, useContext, type ReactNode } from "react";
import type { MachineProfile, RepositorySummary, WorktreeRecord } from "../ipc/types";

/**
 * The machine → repository → worktree selection the workbench operates on, plus the
 * two callbacks every Git action needs: re-scan after Git state changed, and jump to
 * Changes when an operation needs conflict resolution.
 */
export interface RepositoryContextValue {
  machineId: string;
  machineKind: MachineProfile["kind"];
  repository: RepositorySummary | null;
  worktree: WorktreeRecord | null;
  refreshWorkspace: () => Promise<void>;
  showChanges: () => void;
}

/** The context narrowed to a selected repository and worktree. */
export interface WorkingCopyContextValue extends RepositoryContextValue {
  repository: RepositorySummary;
  worktree: WorktreeRecord;
}

const RepositoryContext = createContext<RepositoryContextValue | null>(null);

export function RepositoryProvider({ value, children }: { value: RepositoryContextValue; children: ReactNode }) {
  return <RepositoryContext.Provider value={value}>{children}</RepositoryContext.Provider>;
}

export function useRepositoryContext(): RepositoryContextValue {
  const value = useContext(RepositoryContext);
  if (!value) throw new Error("useRepositoryContext must be used inside a RepositoryProvider.");
  return value;
}

/** Use inside components that App only renders once a repository and worktree are selected. */
export function useWorkingCopy(): WorkingCopyContextValue {
  const value = useRepositoryContext();
  if (!value.repository || !value.worktree) {
    throw new Error("useWorkingCopy requires a selected repository and worktree.");
  }
  return value as WorkingCopyContextValue;
}
