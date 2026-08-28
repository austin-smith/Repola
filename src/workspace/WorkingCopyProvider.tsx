import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { toMessage } from "@/lib/errors";
import { toast } from "@/components/ui/toast";
import type { FileDiff, SyncKind, WorkingCopySnapshot } from "../ipc/types";
import { fetchWorkingCopy, onWorktreeChanged, synchronizeWorkingCopy, unwatchWorktree, watchWorktree } from "../ipc/worktrees";
import { useWorkingCopy } from "./context";
import { syncKindFor, syncLabelFor, WorkingCopyStateContext, type WorkingCopyState } from "./working-copy-state";

/** Mount once per selected worktree (key it by the worktree id). */
export function WorkingCopyProvider({ children }: { children: ReactNode }) {
  const { machineId, repository, worktree } = useWorkingCopy();
  const [workingCopy, setWorkingCopy] = useState<{ snapshot: WorkingCopySnapshot | null; diffCache: Map<string, FileDiff> }>(() => ({ snapshot: null, diffCache: new Map() }));
  const [error, setError] = useState<string | null>(null);
  const [syncBusy, setSyncBusy] = useState(false);
  const setSnapshot = useCallback((next: WorkingCopySnapshot | null) => setWorkingCopy({ snapshot: next, diffCache: new Map() }), []);

  const mutations = useRef(0);
  const retainMutation = useCallback(() => {
    mutations.current += 1;
    let released = false;
    return () => {
      if (released) return;
      released = true;
      mutations.current -= 1;
    };
  }, []);

  useEffect(() => {
    const controller = new AbortController();
    setSnapshot(null);
    setError(null);
    void fetchWorkingCopy(machineId, repository.path, worktree.path, controller.signal)
      .then(setSnapshot)
      .catch((cause: unknown) => {
        if (!controller.signal.aborted) setError(toMessage(cause));
      });
    return () => controller.abort();
  }, [machineId, repository.path, setSnapshot, worktree.id, worktree.path]);

  const reloadController = useRef<AbortController | null>(null);
  const reloadSnapshot = useCallback(() => {
    if (mutations.current > 0) return;
    reloadController.current?.abort();
    const controller = new AbortController();
    reloadController.current = controller;
    void fetchWorkingCopy(machineId, repository.path, worktree.path, controller.signal)
      .then((next) => {
        if (controller.signal.aborted || mutations.current > 0) return;
        setSnapshot(next);
      })
      .catch((cause: unknown) => {
        if (!controller.signal.aborted) setError(toMessage(cause));
      })
      .finally(() => {
        if (reloadController.current === controller) reloadController.current = null;
      });
  }, [machineId, repository.path, setSnapshot, worktree.path]);
  useEffect(() => () => reloadController.current?.abort(), []);

  // Refresh when files change on disk (local machines) and whenever the window
  // regains focus, so edits and Git commands made outside Repola show up
  // without a manual refresh.
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    void watchWorktree(machineId, worktree.path).catch(() => undefined);
    void onWorktreeChanged((event) => {
      if (!disposed && event.machineId === machineId) reloadSnapshot();
    }).then((dispose) => {
      if (disposed) dispose();
      else unlisten = dispose;
    });
    const onFocus = () => reloadSnapshot();
    window.addEventListener("focus", onFocus);
    return () => {
      disposed = true;
      unlisten?.();
      window.removeEventListener("focus", onFocus);
      void unwatchWorktree().catch(() => undefined);
    };
  }, [machineId, reloadSnapshot, worktree.path]);

  const { snapshot, diffCache } = workingCopy;
  const synchronize = useCallback(async (kind: SyncKind = syncKindFor(snapshot)) => {
    if (!snapshot) return false;
    const release = retainMutation();
    setSyncBusy(true);
    setError(null);
    try {
      const result = await synchronizeWorkingCopy(machineId, repository.path, worktree.path, kind, snapshot.head, snapshot.upstreamHead);
      setSnapshot(result.snapshot);
      toast.add({
        type: "success",
        title: kind === "fetch" ? "Remote state fetched" : kind === "pull" ? "Changes pulled" : kind === "push" ? "Commits pushed" : kind === "forcePush" ? "Branch force-pushed safely" : "Branch published",
        description: result.output || undefined,
      });
      return true;
    } catch (cause) {
      setError(toMessage(cause));
      return false;
    } finally {
      setSyncBusy(false);
      release();
    }
  }, [machineId, repository.path, retainMutation, setSnapshot, snapshot, worktree.path]);

  const value = useMemo<WorkingCopyState>(() => ({
    snapshot,
    diffCache,
    error,
    setSnapshot,
    setError,
    reloadSnapshot,
    retainMutation,
    syncKind: syncKindFor(snapshot),
    syncLabel: syncLabelFor(snapshot),
    syncBusy,
    synchronize,
  }), [snapshot, diffCache, error, setSnapshot, reloadSnapshot, retainMutation, syncBusy, synchronize]);

  return <WorkingCopyStateContext.Provider value={value}>{children}</WorkingCopyStateContext.Provider>;
}

