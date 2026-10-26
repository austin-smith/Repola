export interface WorkspaceMutationScope {
  machineId: string;
  repositoryPath?: string;
  worktreePath?: string;
  impact?: "files" | "git" | "inventory";
}

export interface WorkspaceMutationEvent extends WorkspaceMutationScope {
  phase: "start" | "settled";
}

const listeners = new Set<(event: WorkspaceMutationEvent) => void>();

export function onWorkspaceMutation(listener: (event: WorkspaceMutationEvent) => void): () => void {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

function emit(scope: WorkspaceMutationScope, phase: WorkspaceMutationEvent["phase"]): void {
  for (const listener of [...listeners]) {
    try {
      listener({ ...scope, phase });
    } catch {
      // Refresh subscribers must not prevent a mutation or hide its result.
    }
  }
}

/** Capture identity before invoking: selection may change while Git is busy. */
export async function runWorkspaceMutation<T>(
  scope: WorkspaceMutationScope,
  operation: () => Promise<T>,
): Promise<T> {
  const captured = { ...scope };
  emit(captured, "start");
  try {
    return await operation();
  } finally {
    // A failed operation can still have changed refs, files, or registration.
    emit(captured, "settled");
  }
}
