mod actions;
mod agents;
mod branches;
mod claude;
mod codex;
pub(crate) mod command;
mod create_worktree;
mod discovery;
mod external_tools;
mod history;
mod history_mutations;
mod identity;
mod inspection;
mod model_catalog;
mod models;
mod operations;
mod providers;
mod repository;
mod stash;
mod sync;
mod tags;
mod text_generation;
mod watch;
mod working_copy;

pub use crate::preferences::{TextGenerationProvider, TextGenerationSelection};
pub use actions::{execute_action, prepare_action};
pub use branches::{branches, mutate_branch};
pub use create_worktree::create_worktree;
pub use discovery::{resolve_repository, scan, scan_streaming, ScanError};
pub use external_tools::{
    available_external_tools, launch_worktree_tool, open_file_in_editor, working_copy_entry_path,
    ExternalToolAvailability, WorktreeTool,
};
pub use history::{commit_file_diff, commit_files, history, reflog};
pub use history_mutations::mutate_history;
pub use inspection::worktree_changes;
pub use models::{
    ActionExecutionRequest, ActionKind, ActionPlan, ActionRequest, ActionResult,
    ApplyPatchHunkRequest, BranchInfo, BranchMutationRequest, BranchMutationResult, BranchRequest,
    CloneRepositoryRequest, CommitChangedFile, CommitFileDiffRequest, CommitFilesRequest,
    CommitRequest, CommitResult, ConflictFile, ConflictFileRequest, CreateRepositoryRequest,
    CreateWorktreeRequest, CreateWorktreeResult, DiscardAllRequest, DiscardFileRequest,
    DiscardScope, FileDiff, FileDiffRequest, GenerateCommitMessageRequest, GeneratedCommitMessage,
    HistoryMutationKind, HistoryMutationRequest, HistoryMutationResult, HistoryPage,
    HistoryRequest, PatchHunk, PatchHunkAction, PullRequestEvidence, PullRequestMutationKind,
    PullRequestMutationRequest, PullRequestMutationResult, ReasoningEffort, ReflogEntry,
    ReflogRequest, RepositoryOperationAction, RepositoryOperationMutationResult,
    RepositoryOperationRequest, RepositoryOperationResult, ResolveConflictRequest,
    ReviewedFileChange, ScanEvent, ScanRequest, ScanResult, SetFileStagingRequest, StashEntry,
    StashMutationRequest, StashMutationResult, StashRequest, SyncRequest, SyncResult, TagInfo,
    TagMutationKind, TagMutationRequest, TagMutationResult, TagRequest, TextGenerationModel,
    TextGenerationStatus, TextGenerationStatusKind, UndoCommitRequest, UndoCommitResult,
    WorkingCopyRequest, WorkingCopySnapshot, WorktreeChanges,
};
pub use operations::mutate_operation;
pub use providers::{fetch_pull_requests, mutate_pull_request};
pub use repository::{clone_repository, create_repository};
pub use stash::{list_stashes, mutate_stash};
pub use sync::synchronize;
pub use tags::{mutate_tag, tags};
pub(crate) use text_generation::{commit_message_provider_timeout, text_generation_status_timeout};
pub use text_generation::{generate_commit_message, text_generation_status};
pub use watch::{WorktreeChangeEvent, WorktreeWatcher};
pub use working_copy::{
    apply_patch_hunk, commit, conflict_file, discard_all, discard_file, file_diff,
    resolve_conflict, set_file_staging, undo_commit, working_copy_snapshot,
};

pub(crate) fn git_version() -> Result<String, String> {
    let output = command::output("git", ["--version"]).map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests;
