use serde::{Deserialize, Serialize};

use crate::preferences::TextGenerationSelection;

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanRequest {
    #[serde(default)]
    pub repository_paths: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    pub scanned_at_ms: u64,
    pub repository_paths: Vec<String>,
    pub repositories: Vec<RepositorySummary>,
    pub worktrees: Vec<WorktreeRecord>,
    pub totals: ScanTotals,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositorySummary {
    pub id: String,
    pub name: String,
    pub path: String,
    pub remote_url: Option<String>,
    pub provider: RemoteProvider,
    pub worktree_count: usize,
    pub attention_count: usize,
    pub conflicted_count: usize,
    pub allocated_bytes: u64,
    pub allocation_incomplete: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RemoteProvider {
    GitHub,
    AzureDevOps,
    Other,
    None,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeRecord {
    pub id: String,
    pub repository_name: String,
    pub repository_path: String,
    pub path: String,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub detached: bool,
    pub is_primary: bool,
    pub exists: bool,
    pub created_at_ms: Option<u64>,
    pub head_commit_at_ms: Option<u64>,
    pub last_activity_at_ms: Option<u64>,
    pub head_subject: Option<String>,
    pub unpushed_commit_count: Option<u64>,
    pub size_bytes: Option<u64>,
    pub size_incomplete: bool,
    pub origin: WorktreeOrigin,
    pub status: ChangeSummary,
    pub registration: RegistrationState,
    pub integration: IntegrationEvidence,
    pub safety: SafetyAssessment,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeOrigin {
    pub kind: WorktreeOriginKind,
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WorktreeOriginKind {
    Primary,
    Agent,
    Unattributed,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeSummary {
    pub available: bool,
    pub total: usize,
    pub staged: usize,
    pub unstaged: usize,
    pub untracked: usize,
    pub conflicted: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistrationState {
    pub kind: RegistrationKind,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RegistrationKind {
    Healthy,
    BrokenLink,
    Locked,
    Missing,
    Prunable,
    Primary,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationEvidence {
    pub kind: IntegrationKind,
    pub target: Option<String>,
    pub summary: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum IntegrationKind {
    HeadContained,
    HeadNotContained,
    Unknown,
    NotApplicable,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SafetyAssessment {
    pub level: SafetyLevel,
    pub label: String,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SafetyLevel {
    Protected,
    Review,
    Repair,
    MetadataOnly,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanTotals {
    pub repository_count: usize,
    pub primary_count: usize,
    pub linked_count: usize,
    pub existing_linked_count: usize,
    pub clean_count: usize,
    pub dirty_count: usize,
    pub missing_count: usize,
    pub prunable_count: usize,
    pub broken_link_count: usize,
    pub linked_size_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ActionKind {
    Remove,
    Repair,
    Unlock,
    PruneRepository,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionRequest {
    pub kind: ActionKind,
    pub repository_path: String,
    pub worktree_path: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionExecutionRequest {
    pub kind: ActionKind,
    pub repository_path: String,
    pub worktree_path: String,
    pub expected_head: Option<String>,
    pub expected_branch: Option<String>,
    #[serde(default)]
    pub expected_affected_paths: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionPlan {
    pub kind: ActionKind,
    pub title: String,
    pub summary: String,
    pub repository_path: String,
    pub worktree_path: String,
    pub branch: Option<String>,
    pub expected_head: Option<String>,
    pub command_display: String,
    pub affected_paths: Vec<String>,
    pub warnings: Vec<String>,
    pub confirmation_text: String,
    pub destructive: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionResult {
    pub message: String,
    pub audit_path: Option<String>,
    pub audit_warning: Option<String>,
    pub follow_up: Option<FollowUpAction>,
}

/// A branch a removed worktree left behind, reviewed for deletion from
/// `worktree_path`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FollowUpAction {
    pub repository_path: String,
    pub worktree_path: String,
    pub branch: String,
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PullRequestFetchStatus {
    Fetched,
    CliMissing,
    NotAuthenticated,
    CliError,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestSummary {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub url: Option<String>,
    #[serde(default)]
    pub head_branch: String,
    #[serde(default)]
    pub base_branch: String,
    #[serde(default)]
    pub review_state: String,
    #[serde(default)]
    pub merge_state: String,
    #[serde(default)]
    pub checks_total: u32,
    #[serde(default)]
    pub checks_passed: u32,
    #[serde(default)]
    pub checks_pending: u32,
    #[serde(default)]
    pub checks_failed: u32,
    #[serde(default)]
    pub linked_issues: Vec<IssueSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueSummary {
    pub number: u64,
    pub title: String,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestEvidence {
    pub status: PullRequestFetchStatus,
    pub provider: RemoteProvider,
    pub branch: String,
    pub pulls: Vec<PullRequestSummary>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PullRequestMutationKind {
    Create,
    Checkout,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestMutationRequest {
    pub repository_path: String,
    pub worktree_path: String,
    pub kind: PullRequestMutationKind,
    pub branch: String,
    pub expected_head: Option<String>,
    pub expected_pull: Option<PullRequestSummary>,
    pub title: Option<String>,
    pub body: Option<String>,
    pub base_branch: Option<String>,
    #[serde(default)]
    pub draft: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestMutationResult {
    pub evidence: PullRequestEvidence,
    pub snapshot: Option<WorkingCopySnapshot>,
    pub created_url: Option<String>,
    pub output: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeChanges {
    pub available: bool,
    pub patch: String,
    pub truncated: bool,
    pub untracked: Vec<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDiffRequest {
    pub repository_path: String,
    pub worktree_path: String,
    pub path: GitPath,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDiff {
    pub patch: String,
    pub truncated: bool,
    pub binary: bool,
    pub submodule: bool,
    pub image: Option<ImageComparison>,
    pub hunks: Vec<PatchHunk>,
    pub staged_hunks: Vec<PatchHunk>,
    pub unstaged_hunks: Vec<PatchHunk>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageComparison {
    pub before: ImageVersion,
    pub after: ImageVersion,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", content = "preview", rename_all = "camelCase")]
pub enum ImageVersion {
    Missing,
    Unsupported,
    TooLarge,
    Preview(ImagePreview),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImagePreview {
    pub mime_type: String,
    pub base64: String,
    pub byte_length: u64,
    pub label: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchHunk {
    pub index: u32,
    pub header: String,
    pub patch: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PatchHunkAction {
    Stage,
    Unstage,
    Discard,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyPatchHunkRequest {
    pub repository_path: String,
    pub worktree_path: String,
    pub path: GitPath,
    pub action: PatchHunkAction,
    pub hunk_index: u32,
    pub expected_patch: String,
    pub expected_head: Option<String>,
    #[serde(default)]
    pub selected_line_indices: Vec<u32>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkingCopyRequest {
    pub repository_path: String,
    pub worktree_path: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkingCopySnapshot {
    pub repository_path: String,
    pub worktree_path: String,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub upstream_head: Option<String>,
    pub remote: Option<String>,
    pub ahead: u64,
    pub behind: u64,
    pub changes: Vec<FileChange>,
    pub operation: Option<RepositoryOperation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitPath {
    pub display: String,
    /// Exact bytes from Git's NUL-delimited porcelain output, hex encoded so
    /// filenames that are not Unicode can safely round-trip through JSON.
    pub token: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FileChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Unmerged,
    Untracked,
    Ignored,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FileModeChange {
    ExecutableBit,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    pub id: String,
    pub path: GitPath,
    pub previous_path: Option<GitPath>,
    pub kind: FileChangeKind,
    pub index_status: String,
    pub worktree_status: String,
    pub staged: bool,
    pub unstaged: bool,
    pub conflicted: bool,
    pub untracked: bool,
    pub ignored: bool,
    pub submodule: bool,
    pub head_mode: Option<String>,
    pub index_mode: Option<String>,
    pub worktree_mode: Option<String>,
    pub mode_change: Option<FileModeChange>,
    /// Object IDs reported by porcelain v2 (`None` for records without them,
    /// e.g. untracked and unmerged entries) and a stat stamp of the entry on
    /// disk. Together they identify the change's content, so a diff loaded
    /// for one snapshot can be proven still valid in the next. All three
    /// default to `None` when an older agent omits them, which callers must
    /// read as "content identity unknown".
    #[serde(default)]
    pub head_oid: Option<String>,
    #[serde(default)]
    pub index_oid: Option<String>,
    #[serde(default)]
    pub worktree_stamp: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RepositoryOperation {
    Merge,
    Rebase,
    CherryPick,
    Revert,
    Bisect,
    Sequencer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RepositoryOperationAction {
    Continue,
    Skip,
    Abort,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryOperationRequest {
    pub repository_path: String,
    pub worktree_path: String,
    pub action: RepositoryOperationAction,
    pub expected_operation: RepositoryOperation,
    pub expected_head: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryOperationMutationResult {
    pub snapshot: WorkingCopySnapshot,
    pub succeeded: bool,
    pub output: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum HistoryMutationKind {
    Merge,
    SquashMerge,
    Rebase,
    CherryPick,
    Revert,
    ResetSoft,
    ResetMixed,
    ResetHard,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryMutationRequest {
    pub repository_path: String,
    pub worktree_path: String,
    pub kind: HistoryMutationKind,
    pub target: String,
    pub expected_head: Option<String>,
    #[serde(default)]
    pub expected_changes: Vec<ReviewedFileChange>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryMutationResult {
    pub snapshot: WorkingCopySnapshot,
    pub succeeded: bool,
    pub output: String,
    pub conflicted: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagInfo {
    pub name: String,
    pub target: String,
    pub object: String,
    pub annotated: bool,
    pub subject: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TagMutationKind {
    CreateLightweight,
    CreateAnnotated,
    Delete,
    Push,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagRequest {
    pub repository_path: String,
    pub worktree_path: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagMutationRequest {
    pub repository_path: String,
    pub worktree_path: String,
    pub kind: TagMutationKind,
    pub tag: String,
    pub target: Option<String>,
    pub message: Option<String>,
    pub expected_head: Option<String>,
    pub expected_tag_target: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagMutationResult {
    pub tags: Vec<TagInfo>,
    pub snapshot: WorkingCopySnapshot,
    pub output: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateWorktreeRequest {
    pub repository_path: String,
    pub source_worktree_path: String,
    pub destination_path: String,
    pub branch: String,
    pub create_branch: bool,
    #[serde(default)]
    pub start_point: Option<String>,
    pub expected_head: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateWorktreeResult {
    pub repository_path: String,
    pub worktree_path: String,
    pub branch: String,
    pub output: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetFileStagingRequest {
    pub repository_path: String,
    pub worktree_path: String,
    pub paths: Vec<GitPath>,
    pub staged: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConflictResolutionKind {
    Ours,
    Theirs,
    Both,
    Manual,
    MarkResolved,
    Remove,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictFileRequest {
    pub repository_path: String,
    pub worktree_path: String,
    pub path: GitPath,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictFile {
    pub content: String,
    pub byte_length: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveConflictRequest {
    pub repository_path: String,
    pub worktree_path: String,
    pub path: GitPath,
    pub kind: ConflictResolutionKind,
    pub expected_head: Option<String>,
    #[serde(default)]
    pub expected_content: Option<String>,
    #[serde(default)]
    pub manual_content: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DiscardScope {
    Unstaged,
    All,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscardFileRequest {
    pub repository_path: String,
    pub worktree_path: String,
    pub path: GitPath,
    pub scope: DiscardScope,
    pub expected_head: Option<String>,
    pub expected_index_status: String,
    pub expected_worktree_status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewedFileChange {
    pub path_token: String,
    pub previous_path_token: Option<String>,
    pub index_status: String,
    pub worktree_status: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscardAllRequest {
    pub repository_path: String,
    pub worktree_path: String,
    pub expected_head: Option<String>,
    pub expected_changes: Vec<ReviewedFileChange>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitRequest {
    pub repository_path: String,
    pub worktree_path: String,
    pub expected_head: Option<String>,
    #[serde(default)]
    pub included_changes: Vec<CommitFileSelection>,
    pub summary: String,
    pub description: String,
    #[serde(default)]
    pub amend: bool,
    #[serde(default)]
    pub author: Option<CommitPerson>,
    #[serde(default)]
    pub co_authors: Vec<CommitPerson>,
    #[serde(default)]
    pub trailers: Vec<CommitTrailer>,
    #[serde(default)]
    pub signing: CommitSigning,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateCommitMessageRequest {
    pub repository_path: String,
    pub worktree_path: String,
    pub expected_head: Option<String>,
    #[serde(default)]
    pub included_changes: Vec<CommitFileSelection>,
    #[serde(default)]
    pub amend: bool,
    #[serde(default)]
    pub text_generation_selection: Option<TextGenerationSelection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedCommitMessage {
    pub subject: String,
    pub body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TextGenerationStatusKind {
    Ready,
    NotInstalled,
    SignedOut,
    UpdateRequired,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextGenerationStatus {
    pub status: TextGenerationStatusKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub version: Option<String>,
    #[serde(default)]
    pub models: Vec<TextGenerationModel>,
    pub recommended_selection: Option<TextGenerationSelection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextGenerationModel {
    pub model: String,
    pub display_name: String,
    pub description: String,
    pub is_default: bool,
    #[serde(default)]
    pub recommended_for_commit_messages: bool,
    #[serde(default)]
    pub upgrade: Option<String>,
    pub default_reasoning_effort: Option<String>,
    pub supported_reasoning_efforts: Vec<ReasoningEffort>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningEffort {
    pub reasoning_effort: String,
    pub description: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitFileSelection {
    pub path: GitPath,
    pub previous_path: Option<GitPath>,
    pub expected_index_status: String,
    pub expected_worktree_status: String,
    pub include_all: bool,
    #[serde(default)]
    pub hunks: Vec<CommitHunkSelection>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitHunkSelection {
    pub expected_patch: String,
    #[serde(default)]
    pub selected_line_indices: Vec<u32>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitPerson {
    pub name: String,
    pub email: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitTrailer {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CommitSigning {
    #[default]
    Default,
    Sign,
    DoNotSign,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitResult {
    pub commit: String,
    pub snapshot: WorkingCopySnapshot,
    pub hook_output: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoCommitRequest {
    pub repository_path: String,
    pub worktree_path: String,
    pub expected_head: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoCommitResult {
    pub commit: String,
    pub summary: String,
    pub description: String,
    pub snapshot: WorkingCopySnapshot,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRequest {
    pub repository_path: String,
    pub worktree_path: String,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub comparison_base: Option<String>,
    #[serde(default = "default_history_limit")]
    pub limit: u16,
}

fn default_history_limit() -> u16 {
    50
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPage {
    pub commits: Vec<CommitSummary>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReflogEntry {
    pub oid: String,
    pub selector: String,
    pub subject: String,
    pub committed_at: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReflogRequest {
    pub repository_path: String,
    pub worktree_path: String,
    pub limit: u16,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitFilesRequest {
    pub repository_path: String,
    pub worktree_path: String,
    pub commit: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitFileDiffRequest {
    pub repository_path: String,
    pub worktree_path: String,
    pub commit: String,
    pub path: GitPath,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitChangedFile {
    pub id: String,
    pub path: GitPath,
    pub previous_path: Option<GitPath>,
    pub kind: FileChangeKind,
    pub status: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitSummary {
    pub oid: String,
    pub parents: Vec<String>,
    pub author_name: String,
    pub author_email: String,
    pub authored_at: String,
    pub committed_at: String,
    pub signature: CommitSignature,
    pub subject: String,
    pub body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CommitSignature {
    Good,
    Bad,
    UnknownValidity,
    Expired,
    ExpiredKey,
    RevokedKey,
    MissingKey,
    Unsigned,
    Error,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncKind {
    Fetch,
    Pull,
    Push,
    Publish,
    ForcePush,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncRequest {
    pub repository_path: String,
    pub worktree_path: String,
    pub kind: SyncKind,
    pub expected_head: Option<String>,
    #[serde(default)]
    pub expected_upstream_head: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncResult {
    pub snapshot: WorkingCopySnapshot,
    pub output: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchRequest {
    pub repository_path: String,
    pub worktree_path: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchInfo {
    pub name: String,
    pub full_name: String,
    pub head: String,
    pub remote: bool,
    pub current: bool,
    pub upstream: Option<String>,
    pub ahead: u64,
    pub behind: u64,
    pub occupied_worktree_path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BranchMutationKind {
    Create,
    Checkout,
    Rename,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchMutationRequest {
    pub repository_path: String,
    pub worktree_path: String,
    pub kind: BranchMutationKind,
    pub branch: String,
    #[serde(default)]
    pub start_point: Option<String>,
    pub expected_head: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchMutationResult {
    pub branches: Vec<BranchInfo>,
    pub snapshot: WorkingCopySnapshot,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchDeletionRequest {
    pub repository_path: String,
    pub worktree_path: String,
    /// Full ref name: `refs/heads/<branch>` or `refs/remotes/<remote>/<branch>`.
    pub branch_ref: String,
    pub delete_local: bool,
    pub delete_remote: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MergeReferenceKind {
    /// The branch's configured upstream, which `git branch -d` checks first.
    Upstream,
    /// HEAD of the worktree that runs the deletion, used when no upstream resolves.
    Head,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BranchDeletionConfirmation {
    Confirm,
    TypeBranchName,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalBranchDeletion {
    pub name: String,
    pub tip: String,
    pub merge_reference: String,
    pub merge_reference_kind: MergeReferenceKind,
    pub merge_reference_oid: Option<String>,
    pub contained_in_merge_reference: bool,
    pub default_target: Option<String>,
    pub contained_in_default_target: Option<bool>,
    pub occupied_worktree_path: Option<String>,
    pub is_default_branch: bool,
    pub upstream: Option<String>,
    /// Commits reachable from this branch and from no ref that remains after the deletion.
    pub exclusive_commit_count: u64,
    pub exclusive_commit_count_capped: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteBranchDeletion {
    pub remote: String,
    pub remote_ref: String,
    pub tracking_ref: String,
    pub display_name: String,
    /// The remote-tracking ref's value; the push lease requires the remote to still match it.
    pub expected_oid: String,
    /// When the remote-tracking ref last changed (a fetch or push that moved it), in Unix seconds.
    pub tracking_ref_updated_at: Option<u64>,
    /// The most recent fetch from any worktree of this repository, in Unix seconds.
    pub last_fetched_at: Option<u64>,
    pub is_remote_default_branch: bool,
    pub tracked_by: Vec<String>,
    pub exclusive_commit_count: u64,
    pub exclusive_commit_count_capped: bool,
}

/// The reviewed state that execution must find unchanged before deleting anything.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchDeletionFingerprint {
    pub local_tip: Option<String>,
    pub merge_reference_oid: Option<String>,
    pub requires_force: bool,
    pub remote: Option<String>,
    pub remote_ref: Option<String>,
    pub remote_oid: Option<String>,
    /// The commits the local deletion would leave unreachable, as reviewed.
    pub local_exclusive_commits: Option<CommitCount>,
    /// The commits the remote deletion would leave unreachable, as reviewed.
    pub remote_exclusive_commits: Option<CommitCount>,
    pub confirmation: BranchDeletionConfirmation,
}

/// A commit count that stops at a limit, recording whether it got there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitCount {
    pub count: u64,
    pub capped: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchDeletionPlan {
    pub repository_path: String,
    pub worktree_path: String,
    pub branch_ref: String,
    pub branch_name: String,
    pub delete_local: bool,
    pub delete_remote: bool,
    pub local: Option<LocalBranchDeletion>,
    /// The remote branch that a remote deletion would target, reported even when not selected.
    pub remote: Option<RemoteBranchDeletion>,
    pub remote_unavailable_reason: Option<String>,
    pub requires_force: bool,
    pub confirmation: BranchDeletionConfirmation,
    pub commands: Vec<String>,
    pub warnings: Vec<String>,
    pub blockers: Vec<String>,
    pub fingerprint: BranchDeletionFingerprint,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchDeletionExecutionRequest {
    pub request: BranchDeletionRequest,
    pub force: bool,
    pub expected: BranchDeletionFingerprint,
    /// The branch name the user typed when the plan required it.
    pub typed_confirmation: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchDeletionStep {
    pub target: String,
    pub deleted_oid: String,
    pub succeeded: bool,
    pub output: String,
    /// Something left undone by a step that still succeeded.
    pub warning: Option<String>,
    pub recovery_command: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchDeletionResult {
    pub message: String,
    pub local: Option<BranchDeletionStep>,
    pub remote: Option<BranchDeletionStep>,
    pub audit_path: Option<String>,
    pub audit_warning: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloneRepositoryRequest {
    pub source: String,
    pub destination_path: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRepositoryRequest {
    pub destination_path: String,
    pub initial_branch: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryOperationResult {
    pub repository_path: String,
    pub output: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StashRequest {
    pub repository_path: String,
    pub worktree_path: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StashEntry {
    pub index: u64,
    pub oid: String,
    pub created_at: String,
    pub subject: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StashMutationKind {
    Push,
    Apply,
    Pop,
    Drop,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StashMutationRequest {
    pub repository_path: String,
    pub worktree_path: String,
    pub kind: StashMutationKind,
    pub message: Option<String>,
    #[serde(default)]
    pub include_untracked: bool,
    #[serde(default)]
    pub paths: Vec<GitPath>,
    pub stash_index: Option<u64>,
    pub expected_stash_oid: Option<String>,
    pub expected_head: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StashMutationResult {
    pub snapshot: WorkingCopySnapshot,
    pub stashes: Vec<StashEntry>,
    pub output: String,
    pub conflicted: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ScanEvent {
    Repositories {
        repository_paths: Vec<String>,
        repositories: Vec<RepositorySummary>,
        warnings: Vec<String>,
    },
    Worktree {
        record: Box<WorktreeRecord>,
    },
    Size {
        id: String,
        size_bytes: Option<u64>,
        size_incomplete: bool,
        last_activity_at_ms: Option<u64>,
    },
    Done {
        result: Box<ScanResult>,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct WorktreeSeed {
    pub path: String,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub detached: bool,
    pub is_primary: bool,
    pub locked_reason: Option<String>,
    pub prunable_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct RepositoryContext {
    pub name: String,
    pub path: std::path::PathBuf,
    pub git_dir: std::path::PathBuf,
    pub remote_url: Option<String>,
    pub provider: RemoteProvider,
    pub default_target: Option<String>,
}
