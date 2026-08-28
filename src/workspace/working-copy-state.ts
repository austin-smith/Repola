import { createContext, useContext, useEffect } from "react";
import type { FileDiff, SyncKind, WorkingCopySnapshot } from "../ipc/types";

/**
 * The live working-copy snapshot for the selected worktree, shared by the
 * toolbar (sync, stashes) and the Changes view (file list, commit form).
 */
export interface WorkingCopyState {
  snapshot: WorkingCopySnapshot | null;
  /**
   * File diffs already loaded for `snapshot`, keyed by path token. Replaced
   * together with the snapshot so an entry is never older than the file list.
   */
  diffCache: Map<string, FileDiff>;
  error: string | null;
  /** Replace the snapshot with the result of a mutation. */
  setSnapshot: (next: WorkingCopySnapshot) => void;
  setError: (message: string | null) => void;
  /** Re-read the working copy from Git unless a mutation is in progress. */
  reloadSnapshot: () => void;
  /**
   * Mark a mutation as in progress. Disk-triggered reloads are suppressed
   * until the returned release function is called, because the mutation will
   * replace the snapshot with its own result and a reload would only race it.
   */
  retainMutation: () => () => void;
  syncKind: SyncKind;
  syncLabel: string;
  syncBusy: boolean;
  synchronize: (kind?: SyncKind) => Promise<boolean>;
}

export const WorkingCopyStateContext = createContext<WorkingCopyState | null>(null);

export function syncKindFor(snapshot: WorkingCopySnapshot | null): SyncKind {
  if (!snapshot?.upstream) return "publish";
  if (snapshot.behind > 0) return "pull";
  if (snapshot.ahead > 0) return "push";
  return "fetch";
}

export function syncLabelFor(snapshot: WorkingCopySnapshot | null): string {
  const labels: Record<SyncKind, string> = {
    publish: "Publish branch",
    pull: `Pull ${snapshot?.behind ?? 0}`,
    push: `Push ${snapshot?.ahead ?? 0}`,
    fetch: "Fetch",
    forcePush: "Force-push",
  };
  return labels[syncKindFor(snapshot)];
}

export function useWorkingCopyState(): WorkingCopyState {
  const value = useContext(WorkingCopyStateContext);
  if (!value) throw new Error("useWorkingCopyState must be used inside a WorkingCopyProvider.");
  return value;
}

/** For controls that render whether or not a worktree is selected. */
export function useOptionalWorkingCopyState(): WorkingCopyState | null {
  return useContext(WorkingCopyStateContext);
}

/** Suppress disk-triggered snapshot reloads while `active` is true. */
export function useMutationGuard(active: boolean): void {
  const { retainMutation } = useWorkingCopyState();
  useEffect(() => (active ? retainMutation() : undefined), [active, retainMutation]);
}
