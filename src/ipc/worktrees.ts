import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
import type {
  ActionKind,
  ActionPlan,
  ActionResult,
  CommitRequest,
  TextGenerationStatus,
  TextGenerationProvider,
  CommitChangedFile,
  CommitResult,
  GeneratedCommitMessage,
  GenerateCommitMessageRequest,
  CreateWorktreeResult,
  ConflictResolutionKind,
  ConflictFile,
  DiscardPlan,
  DiscardResult,
  DiscardTarget,
  BranchInfo,
  BranchMutationKind,
  BranchMutationResult,
  FileDiff,
  GitPath,
  PatchHunk,
  PatchHunkAction,
  HistoryPage,
  HistoryMutationKind,
  HistoryMutationResult,
  SyncKind,
  SyncResult,
  TagInfo,
  TagMutationKind,
  TagMutationResult,
  UndoCommitResult,
  PullRequestEvidence,
  PullRequestMutationKind,
  PullRequestMutationResult,
  PullRequestSummary,
  RecoveryFileDiff,
  RecoveryPointList,
  RecoveryPointReference,
  RecoveryRestorePlan,
  RecoveryRestoreResult,
  RepositoryOperationResult,
  RepositoryRegistrationResult,
  ReflogEntry,
  RepositoryOperation,
  RepositoryOperationAction,
  RepositoryOperationMutationResult,
  ScanEvent,
  ScanResult,
  StashEntry,
  StashMutationKind,
  StashMutationResult,
  WorktreeChangedEvent,
  WorktreeChanges,
  WorktreeWatchResult,
  WorkingCopySnapshot,
} from "./types";
import { invokeOperation } from "./operations";

/** Read the repositories the user explicitly added, cloned, or created on one machine. */
export function loadRegisteredRepositories(machineId: string): Promise<string[]> {
  return invoke<string[]>("load_registered_repositories", { machineId });
}

/** Validate an exact repository path on its owning machine, then persist the resolved working copy. */
export function registerRepository(machineId: string, path: string): Promise<RepositoryRegistrationResult> {
  return invokeOperation<RepositoryRegistrationResult>("register_repository", { machineId, path });
}

/** Remove one exact repository from Repola without changing files or Git data. */
export function unregisterRepository(machineId: string, repositoryPath: string): Promise<string[]> {
  return invoke<string[]>("unregister_repository", { machineId, repositoryPath });
}

export function scanWorktrees(
  machineId: string,
  repositoryPaths: string[],
  onEvent: (event: ScanEvent) => void,
  signal?: AbortSignal,
): Promise<ScanResult> {
  const channel = new Channel<ScanEvent>();
  channel.onmessage = onEvent;
  return invokeOperation<ScanResult>("scan_worktrees", {
    request: { repositoryPaths },
    machineId,
    onEvent: channel,
  }, { signal });
}

export function resolveDroppedRepository(path: string): Promise<string> {
  return invoke<string>("resolve_dropped_repository", { path });
}

export function fetchPullRequests(
  machineId: string,
  repositoryPath: string,
  branch: string,
): Promise<PullRequestEvidence> {
  return invokeOperation<PullRequestEvidence>("fetch_pull_requests", { machineId, repositoryPath, branch });
}

export function mutatePullRequest(
  machineId: string,
  repositoryPath: string,
  worktreePath: string,
  kind: PullRequestMutationKind,
  branch: string,
  expectedHead: string | null,
  options: {
    expectedPull?: PullRequestSummary | null;
    title?: string | null;
    body?: string | null;
    baseBranch?: string | null;
    draft?: boolean;
  } = {},
): Promise<PullRequestMutationResult> {
  return invokeOperation<PullRequestMutationResult>("mutate_pull_request", {
    machineId,
    request: {
      repositoryPath,
      worktreePath,
      kind,
      branch,
      expectedHead,
      expectedPull: options.expectedPull ?? null,
      title: options.title ?? null,
      body: options.body ?? null,
      baseBranch: options.baseBranch ?? null,
      draft: options.draft ?? false,
    },
  });
}

export function fetchWorktreeChanges(
  machineId: string,
  repositoryPath: string,
  worktreePath: string,
  signal?: AbortSignal,
): Promise<WorktreeChanges> {
  return invokeOperation<WorktreeChanges>("worktree_changes", { machineId, repositoryPath, worktreePath }, { signal });
}

export function fetchFileDiff(
  machineId: string,
  repositoryPath: string,
  worktreePath: string,
  path: GitPath,
  signal?: AbortSignal,
): Promise<FileDiff> {
  return invokeOperation<FileDiff>("file_diff", {
    machineId,
    request: { repositoryPath, worktreePath, path },
  }, { signal });
}

export function applyPatchHunk(
  machineId: string,
  repositoryPath: string,
  worktreePath: string,
  path: GitPath,
  action: PatchHunkAction,
  hunk: PatchHunk,
  expectedHead: string | null,
  selectedLineIndices: number[] = [],
): Promise<WorkingCopySnapshot> {
  return invokeOperation<WorkingCopySnapshot>("apply_patch_hunk", {
    machineId,
    request: {
      repositoryPath,
      worktreePath,
      path,
      action,
      hunkIndex: hunk.index,
      expectedPatch: hunk.patch,
      expectedHead,
      selectedLineIndices,
    },
  });
}

export function fetchWorkingCopy(
  machineId: string,
  repositoryPath: string,
  worktreePath: string,
  signal?: AbortSignal,
): Promise<WorkingCopySnapshot> {
  return invokeOperation<WorkingCopySnapshot>("working_copy_snapshot", {
    machineId,
    request: { repositoryPath, worktreePath },
  }, { signal });
}

export function setFileStaging(
  machineId: string,
  repositoryPath: string,
  worktreePath: string,
  paths: GitPath[],
  staged: boolean,
): Promise<WorkingCopySnapshot> {
  return invokeOperation<WorkingCopySnapshot>("set_file_staging", {
    machineId,
    request: { repositoryPath, worktreePath, paths, staged },
  });
}

export function resolveConflict(
  machineId: string,
  repositoryPath: string,
  worktreePath: string,
  path: GitPath,
  kind: ConflictResolutionKind,
  expectedHead: string | null,
  expectedContent: string | null = null,
  manualContent: string | null = null,
): Promise<WorkingCopySnapshot> {
  return invokeOperation<WorkingCopySnapshot>("resolve_conflict", {
    machineId,
    request: { repositoryPath, worktreePath, path, kind, expectedHead, expectedContent, manualContent },
  });
}

export function loadConflictFile(
  machineId: string,
  repositoryPath: string,
  worktreePath: string,
  path: GitPath,
  signal?: AbortSignal,
): Promise<ConflictFile> {
  return invokeOperation<ConflictFile>("load_conflict_file", {
    machineId,
    request: { repositoryPath, worktreePath, path },
  }, { signal });
}

export function planDiscard(
  machineId: string,
  repositoryPath: string,
  worktreePath: string,
  target: DiscardTarget,
  signal?: AbortSignal,
): Promise<DiscardPlan> {
  return invokeOperation<DiscardPlan>("plan_discard", {
    machineId,
    request: { repositoryPath, worktreePath, target },
  }, { signal });
}

/** Execute a reviewed discard; the engine refuses unless the working copy still matches `plan.fingerprint`. */
export function discardChanges(
  machineId: string,
  repositoryPath: string,
  worktreePath: string,
  plan: DiscardPlan,
): Promise<DiscardResult> {
  return invokeOperation<DiscardResult>("discard_changes", {
    machineId,
    request: { repositoryPath, worktreePath, target: plan.target, fingerprint: plan.fingerprint },
  });
}

export function loadRecoveryPoints(
  machineId: string,
  repositoryPath: string,
  worktreePath: string,
  signal?: AbortSignal,
): Promise<RecoveryPointList> {
  return invokeOperation<RecoveryPointList>("load_recovery_points", {
    machineId,
    request: { repositoryPath, worktreePath },
  }, { signal });
}

export function planRecoveryRestore(
  machineId: string,
  repositoryPath: string,
  worktreePath: string,
  point: RecoveryPointReference,
  signal?: AbortSignal,
): Promise<RecoveryRestorePlan> {
  return invokeOperation<RecoveryRestorePlan>("plan_recovery_restore", {
    machineId,
    request: { repositoryPath, worktreePath, point },
  }, { signal });
}

/** Restore a reviewed recovery point; whatever it replaces is saved as a new recovery point first. */
export function restoreRecoveryPoint(
  machineId: string,
  repositoryPath: string,
  worktreePath: string,
  plan: RecoveryRestorePlan,
): Promise<RecoveryRestoreResult> {
  return invokeOperation<RecoveryRestoreResult>("restore_recovery_point", {
    machineId,
    request: {
      repositoryPath,
      worktreePath,
      point: { id: plan.point.id, oid: plan.point.oid },
      fingerprint: plan.fingerprint,
    },
  });
}

export function loadRecoveryFileDiff(
  machineId: string,
  repositoryPath: string,
  worktreePath: string,
  point: RecoveryPointReference,
  path: GitPath,
  signal?: AbortSignal,
): Promise<RecoveryFileDiff> {
  return invokeOperation<RecoveryFileDiff>("load_recovery_file_diff", {
    machineId,
    request: { repositoryPath, worktreePath, point, path },
  }, { signal });
}

/** Delete the reviewed recovery points in one transaction. */
export function deleteRecoveryPoints(
  machineId: string,
  repositoryPath: string,
  worktreePath: string,
  points: RecoveryPointReference[],
): Promise<void> {
  return invokeOperation<void>("delete_recovery_points", {
    machineId,
    request: { repositoryPath, worktreePath, points },
  });
}

export function mutateRepositoryOperation(
  machineId: string,
  repositoryPath: string,
  worktreePath: string,
  action: RepositoryOperationAction,
  expectedOperation: RepositoryOperation,
  expectedHead: string | null,
): Promise<RepositoryOperationMutationResult> {
  return invokeOperation<RepositoryOperationMutationResult>("mutate_repository_operation", {
    machineId,
    request: { repositoryPath, worktreePath, action, expectedOperation, expectedHead },
  });
}

export function commitWorkingCopy(
  machineId: string,
  request: CommitRequest,
): Promise<CommitResult> {
  return invokeOperation<CommitResult>("commit_working_copy", { machineId, request });
}

export function generateCommitMessage(
  machineId: string,
  request: GenerateCommitMessageRequest,
  signal?: AbortSignal,
): Promise<GeneratedCommitMessage> {
  return invokeOperation<GeneratedCommitMessage>("generate_commit_message", {
    machineId,
    request,
  }, { signal });
}

export function loadTextGenerationStatus(machineId: string, provider: TextGenerationProvider | null, signal?: AbortSignal): Promise<TextGenerationStatus> {
  return invokeOperation<TextGenerationStatus>("text_generation_status", { machineId, provider }, { signal });
}

export function undoLatestCommit(
  machineId: string,
  repositoryPath: string,
  worktreePath: string,
  expectedHead: string,
): Promise<UndoCommitResult> {
  return invokeOperation<UndoCommitResult>("undo_commit", {
    machineId,
    request: { repositoryPath, worktreePath, expectedHead },
  });
}

export function loadHistory(
  machineId: string,
  repositoryPath: string,
  worktreePath: string,
  cursor: string | null,
  query: string | null = null,
  comparisonBase: string | null = null,
  signal?: AbortSignal,
): Promise<HistoryPage> {
  return invokeOperation<HistoryPage>("load_history", {
    machineId,
    request: { repositoryPath, worktreePath, cursor, query, comparisonBase, limit: 50 },
  }, { signal });
}

export function loadReflog(
  machineId: string,
  repositoryPath: string,
  worktreePath: string,
  signal?: AbortSignal,
): Promise<ReflogEntry[]> {
  return invokeOperation<ReflogEntry[]>("load_reflog", {
    machineId,
    request: { repositoryPath, worktreePath, limit: 200 },
  }, { signal });
}

export function mutateHistory(
  machineId: string,
  snapshot: WorkingCopySnapshot,
  kind: HistoryMutationKind,
  target: string,
): Promise<HistoryMutationResult> {
  return invokeOperation<HistoryMutationResult>("mutate_history", {
    machineId,
    request: {
      repositoryPath: snapshot.repositoryPath,
      worktreePath: snapshot.worktreePath,
      kind,
      target,
      expectedHead: snapshot.head,
      expectedChanges: snapshot.changes
        .filter((change) => !change.ignored)
        .map((change) => ({
          pathToken: change.path.token,
          previousPathToken: change.previousPath?.token ?? null,
          indexStatus: change.indexStatus,
          worktreeStatus: change.worktreeStatus,
        })),
    },
  });
}

export function loadTags(
  machineId: string,
  repositoryPath: string,
  worktreePath: string,
  signal?: AbortSignal,
): Promise<TagInfo[]> {
  return invokeOperation<TagInfo[]>("load_tags", {
    machineId,
    request: { repositoryPath, worktreePath },
  }, { signal });
}

export function mutateTag(
  machineId: string,
  snapshot: WorkingCopySnapshot,
  kind: TagMutationKind,
  tag: string,
  target: string | null,
  message: string | null,
  expectedTagTarget: string | null,
): Promise<TagMutationResult> {
  return invokeOperation<TagMutationResult>("mutate_tag", {
    machineId,
    request: {
      repositoryPath: snapshot.repositoryPath,
      worktreePath: snapshot.worktreePath,
      kind,
      tag,
      target,
      message,
      expectedHead: snapshot.head,
      expectedTagTarget,
    },
  });
}

export function loadCommitFiles(
  machineId: string,
  repositoryPath: string,
  worktreePath: string,
  commit: string,
  signal?: AbortSignal,
): Promise<CommitChangedFile[]> {
  return invokeOperation<CommitChangedFile[]>("load_commit_files", {
    machineId,
    request: { repositoryPath, worktreePath, commit },
  }, { signal });
}

export function fetchCommitFileDiff(
  machineId: string,
  repositoryPath: string,
  worktreePath: string,
  commit: string,
  path: GitPath,
  signal?: AbortSignal,
): Promise<FileDiff> {
  return invokeOperation<FileDiff>("commit_file_diff", {
    machineId,
    request: { repositoryPath, worktreePath, commit, path },
  }, { signal });
}

export function synchronizeWorkingCopy(
  machineId: string,
  repositoryPath: string,
  worktreePath: string,
  kind: SyncKind,
  expectedHead: string | null,
  expectedUpstreamHead: string | null = null,
): Promise<SyncResult> {
  return invokeOperation<SyncResult>("synchronize_working_copy", {
    machineId,
    request: { repositoryPath, worktreePath, kind, expectedHead, expectedUpstreamHead },
  });
}

export function loadBranches(
  machineId: string,
  repositoryPath: string,
  worktreePath: string,
  signal?: AbortSignal,
): Promise<BranchInfo[]> {
  return invokeOperation<BranchInfo[]>("load_branches", {
    machineId,
    request: { repositoryPath, worktreePath },
  }, { signal });
}

export function mutateBranch(
  machineId: string,
  repositoryPath: string,
  worktreePath: string,
  kind: BranchMutationKind,
  branch: string,
  startPoint: string | null,
  expectedHead: string | null,
): Promise<BranchMutationResult> {
  return invokeOperation<BranchMutationResult>("mutate_branch", {
    machineId,
    request: { repositoryPath, worktreePath, kind, branch, startPoint, expectedHead },
  });
}

export function cloneRepository(
  machineId: string,
  source: string,
  destinationPath: string,
  signal?: AbortSignal,
): Promise<RepositoryOperationResult> {
  return invokeOperation<RepositoryOperationResult>("clone_repository", {
    machineId,
    request: { source, destinationPath },
  }, { signal });
}

export function createRepository(
  machineId: string,
  destinationPath: string,
  initialBranch: string,
  signal?: AbortSignal,
): Promise<RepositoryOperationResult> {
  return invokeOperation<RepositoryOperationResult>("create_repository", {
    machineId,
    request: { destinationPath, initialBranch },
  }, { signal });
}

export function createLinkedWorktree(
  machineId: string,
  request: {
    repositoryPath: string;
    sourceWorktreePath: string;
    destinationPath: string;
    branch: string;
    createBranch: boolean;
    startPoint: string | null;
    expectedHead: string | null;
  },
  signal?: AbortSignal,
): Promise<CreateWorktreeResult> {
  return invokeOperation<CreateWorktreeResult>("create_worktree", { machineId, request }, { signal });
}

export function loadStashes(
  machineId: string,
  repositoryPath: string,
  worktreePath: string,
  signal?: AbortSignal,
): Promise<StashEntry[]> {
  return invokeOperation<StashEntry[]>("load_stashes", {
    machineId,
    request: { repositoryPath, worktreePath },
  }, { signal });
}

export function mutateStash(
  machineId: string,
  request: {
    repositoryPath: string;
    worktreePath: string;
    kind: StashMutationKind;
    message: string | null;
    includeUntracked: boolean;
    paths: GitPath[];
    stashIndex: number | null;
    expectedStashOid: string | null;
    expectedHead: string | null;
  },
): Promise<StashMutationResult> {
  return invokeOperation<StashMutationResult>("mutate_stash", { machineId, request });
}

export function revealWorktree(path: string): Promise<void> {
  return revealItemInDir(path);
}

/** Show one changed file in the desktop file manager. The backend joins the
 * exact path from the Git token; only valid for the local machine. */
export function showFileInFileManager(machineId: string, worktreePath: string, pathToken: string): Promise<void> {
  return invoke("show_file_in_file_manager", { machineId, worktreePath, pathToken });
}

export function openExternalUrl(url: string): Promise<void> {
  return openUrl(url);
}

export async function pickRepositoryRoot(): Promise<string | null> {
  const selected = await open({
    directory: true,
    multiple: false,
    title: "Add Existing Repository",
  });
  return typeof selected === "string" ? selected : null;
}

const WORKTREE_CHANGED_EVENT = "repola://worktree-changed";

export function watchWorktree(machineId: string, worktreePath: string): Promise<WorktreeWatchResult> {
  return invoke<WorktreeWatchResult>("watch_worktree", { machineId, worktreePath });
}

export function unwatchWorktree(): Promise<void> {
  return invoke<void>("unwatch_worktree");
}

export async function onWorktreeChanged(handler: (event: WorktreeChangedEvent) => void): Promise<() => void> {
  return listen<WorktreeChangedEvent>(WORKTREE_CHANGED_EVENT, (event) => handler(event.payload));
}

export function revealAuditLog(path: string): Promise<void> {
  return revealItemInDir(path);
}

export function prepareWorktreeAction(
  machineId: string,
  kind: ActionKind,
  repositoryPath: string,
  worktreePath: string,
): Promise<ActionPlan> {
  return invokeOperation<ActionPlan>("prepare_worktree_action", {
    machineId,
    request: { kind, repositoryPath, worktreePath },
  });
}

export function prepareBranchDeletion(
  machineId: string,
  repositoryPath: string,
  branch: string,
): Promise<ActionPlan> {
  return invokeOperation<ActionPlan>("prepare_worktree_action", {
    machineId,
    request: { kind: "deleteBranch", repositoryPath, worktreePath: "", branch },
  });
}

export function executeWorktreeAction(machineId: string, plan: ActionPlan): Promise<ActionResult> {
  return invokeOperation<ActionResult>("execute_worktree_action", {
    machineId,
    request: {
      kind: plan.kind,
      repositoryPath: plan.repositoryPath,
      worktreePath: plan.worktreePath,
      expectedHead: plan.expectedHead,
      expectedBranch: plan.branch,
      expectedAffectedPaths: plan.affectedPaths,
    },
  });
}
