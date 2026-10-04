use serde::{Deserialize, Serialize};

use crate::worktree::{
    ActionExecutionRequest, ActionPlan, ActionRequest, ActionResult, ApplyPatchHunkRequest,
    BranchInfo, BranchMutationRequest, BranchMutationResult, BranchRequest, CloneRepositoryRequest,
    CommitChangedFile, CommitFileDiffRequest, CommitFilesRequest, CommitRequest, CommitResult,
    ConflictFile, ConflictFileRequest, CreateRepositoryRequest, CreateWorktreeRequest,
    CreateWorktreeResult, DeleteRecoveryPointsRequest, DiscardPlan, DiscardPlanRequest,
    DiscardRequest, DiscardResult, FileDiff, FileDiffRequest, GenerateCommitMessageRequest,
    GeneratedCommitMessage, HistoryMutationRequest, HistoryMutationResult, HistoryPage,
    HistoryRequest, PullRequestEvidence, PullRequestMutationRequest, PullRequestMutationResult,
    RecoveryFileDiff, RecoveryFileDiffRequest, RecoveryPoint, RecoveryPointRequest,
    RecoveryRestorePlan, RecoveryRestoreRequest, RecoveryRestoreResult, ReflogEntry, ReflogRequest,
    RepositoryOperationMutationResult, RepositoryOperationRequest, RepositoryOperationResult,
    ResolveConflictRequest, ScanEvent, ScanRequest, ScanResult, SetFileStagingRequest, StashEntry,
    StashMutationRequest, StashMutationResult, StashRequest, SyncRequest, SyncResult, TagInfo,
    TagMutationRequest, TagMutationResult, TagRequest, TextGenerationStatus, UndoCommitRequest,
    UndoCommitResult, WorkingCopyRequest, WorkingCopySnapshot, WorktreeChanges,
};

pub const PROTOCOL_VERSION: u16 = 20;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestEnvelope {
    pub protocol_version: u16,
    pub request_id: String,
    pub request: AgentRequest,
}

impl RequestEnvelope {
    pub fn current(request_id: impl Into<String>, request: AgentRequest) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            request_id: request_id.into(),
            request,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AgentRequest {
    Handshake {
        client_version: String,
        minimum_protocol_version: u16,
        maximum_protocol_version: u16,
    },
    ScanWorktrees {
        request: ScanRequest,
    },
    ResolveRepository {
        path: String,
    },
    FetchPullRequests {
        repository_path: String,
        branch: String,
    },
    MutatePullRequest {
        request: PullRequestMutationRequest,
    },
    WorktreeChanges {
        repository_path: String,
        worktree_path: String,
    },
    FileDiff {
        request: FileDiffRequest,
    },
    WorkingCopySnapshot {
        request: WorkingCopyRequest,
    },
    SetFileStaging {
        request: SetFileStagingRequest,
    },
    Commit {
        request: CommitRequest,
    },
    GenerateCommitMessage {
        request: GenerateCommitMessageRequest,
    },
    TextGenerationStatus {
        provider: Option<crate::preferences::TextGenerationProvider>,
    },
    UndoCommit {
        request: UndoCommitRequest,
    },
    History {
        request: HistoryRequest,
    },
    Reflog {
        request: ReflogRequest,
    },
    CommitFiles {
        request: CommitFilesRequest,
    },
    CommitFileDiff {
        request: CommitFileDiffRequest,
    },
    MutateHistory {
        request: HistoryMutationRequest,
    },
    Tags {
        request: TagRequest,
    },
    MutateTag {
        request: TagMutationRequest,
    },
    Synchronize {
        request: SyncRequest,
    },
    Branches {
        request: BranchRequest,
    },
    MutateBranch {
        request: BranchMutationRequest,
    },
    CloneRepository {
        request: CloneRepositoryRequest,
    },
    CreateRepository {
        request: CreateRepositoryRequest,
    },
    CreateWorktree {
        request: CreateWorktreeRequest,
    },
    Stashes {
        request: StashRequest,
    },
    MutateStash {
        request: StashMutationRequest,
    },
    ResolveConflict {
        request: ResolveConflictRequest,
    },
    ConflictFile {
        request: ConflictFileRequest,
    },
    PlanDiscard {
        request: DiscardPlanRequest,
    },
    Discard {
        request: DiscardRequest,
    },
    RecoveryPoints {
        request: WorkingCopyRequest,
    },
    PlanRecoveryRestore {
        request: RecoveryPointRequest,
    },
    RestoreRecoveryPoint {
        request: RecoveryRestoreRequest,
    },
    RecoveryFileDiff {
        request: RecoveryFileDiffRequest,
    },
    DeleteRecoveryPoints {
        request: DeleteRecoveryPointsRequest,
    },
    ApplyPatchHunk {
        request: ApplyPatchHunkRequest,
    },
    MutateRepositoryOperation {
        request: RepositoryOperationRequest,
    },
    PrepareWorktreeAction {
        request: ActionRequest,
    },
    ExecuteWorktreeAction {
        request: ActionExecutionRequest,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseEnvelope {
    pub protocol_version: u16,
    pub request_id: String,
    #[serde(flatten)]
    pub body: ResponseBody,
}

impl ResponseEnvelope {
    pub fn new(request_id: impl Into<String>, body: ResponseBody) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            request_id: request_id.into(),
            body,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum ResponseBody {
    Event { event: AgentEvent },
    Success { result: AgentResult },
    Failure { error: AgentError },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AgentEvent {
    Scan { event: ScanEvent },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AgentResult {
    Handshake {
        agent: AgentInfo,
    },
    ScanWorktrees {
        result: ScanResult,
    },
    RepositoryResolved {
        repository_path: String,
    },
    PullRequests {
        evidence: PullRequestEvidence,
    },
    PullRequestMutation {
        result: Box<PullRequestMutationResult>,
    },
    WorktreeChanges {
        changes: WorktreeChanges,
    },
    FileDiff {
        diff: FileDiff,
    },
    ConflictFile {
        file: ConflictFile,
    },
    WorkingCopySnapshot {
        snapshot: WorkingCopySnapshot,
    },
    WorkingCopyUpdated {
        snapshot: WorkingCopySnapshot,
    },
    Commit {
        result: CommitResult,
    },
    GeneratedCommitMessage {
        message: GeneratedCommitMessage,
    },
    TextGenerationStatus {
        status: TextGenerationStatus,
    },
    CommitUndone {
        result: Box<UndoCommitResult>,
    },
    History {
        page: HistoryPage,
    },
    Reflog {
        entries: Vec<ReflogEntry>,
    },
    CommitFiles {
        files: Vec<CommitChangedFile>,
    },
    CommitFileDiff {
        diff: FileDiff,
    },
    HistoryMutation {
        result: HistoryMutationResult,
    },
    Tags {
        tags: Vec<TagInfo>,
    },
    TagMutation {
        result: TagMutationResult,
    },
    Synchronize {
        result: SyncResult,
    },
    Branches {
        branches: Vec<BranchInfo>,
    },
    BranchMutation {
        result: BranchMutationResult,
    },
    RepositoryOperation {
        result: RepositoryOperationResult,
    },
    RepositoryOperationMutation {
        result: RepositoryOperationMutationResult,
    },
    WorktreeCreated {
        result: CreateWorktreeResult,
    },
    Stashes {
        stashes: Vec<StashEntry>,
    },
    StashMutation {
        result: StashMutationResult,
    },
    DiscardPlan {
        plan: DiscardPlan,
    },
    Discarded {
        result: Box<DiscardResult>,
    },
    RecoveryPoints {
        points: Vec<RecoveryPoint>,
    },
    RecoveryRestorePlan {
        plan: RecoveryRestorePlan,
    },
    RecoveryRestored {
        result: Box<RecoveryRestoreResult>,
    },
    RecoveryFileDiff {
        diff: RecoveryFileDiff,
    },
    WorktreeActionPlan {
        plan: ActionPlan,
    },
    WorktreeActionResult {
        result: ActionResult,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    pub agent_version: String,
    pub protocol_version: u16,
    pub operating_system: String,
    pub architecture: String,
    pub git_version: Option<String>,
    pub maximum_frame_bytes: usize,
    pub capabilities: Vec<AgentCapability>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentCapability {
    RepositoryDiscovery,
    WorktreeInventory,
    WorkingCopy,
    History,
    Synchronization,
    Branches,
    RepositoryManagement,
    WorktreeManagement,
    Stashes,
    ProviderIntegration,
    TextGeneration,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentError {
    pub kind: AgentErrorKind,
    pub summary: String,
    pub detail: Option<String>,
    pub retryable: bool,
}

impl AgentError {
    pub fn protocol(summary: impl Into<String>) -> Self {
        Self {
            kind: AgentErrorKind::Protocol,
            summary: summary.into(),
            detail: None,
            retryable: false,
        }
    }

    pub fn operation(summary: impl Into<String>) -> Self {
        Self {
            kind: AgentErrorKind::Operation,
            summary: summary.into(),
            detail: None,
            retryable: false,
        }
    }

    pub fn cancelled() -> Self {
        Self {
            kind: AgentErrorKind::Cancelled,
            summary: "The operation was cancelled.".into(),
            detail: None,
            retryable: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentErrorKind {
    Protocol,
    Operation,
    Transport,
    Cancelled,
    Timeout,
    Internal,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preferences::TextGenerationProvider;

    #[test]
    fn provider_discovery_supports_automatic_and_explicit_remote_requests() {
        for (provider, expected) in [
            (None, serde_json::Value::Null),
            (
                Some(TextGenerationProvider::Codex),
                serde_json::json!("codex"),
            ),
            (
                Some(TextGenerationProvider::Claude),
                serde_json::json!("claude"),
            ),
        ] {
            let value =
                serde_json::to_value(AgentRequest::TextGenerationStatus { provider }).unwrap();
            assert_eq!(
                value,
                serde_json::json!({ "type": "textGenerationStatus", "provider": expected })
            );
            let decoded: AgentRequest = serde_json::from_value(value).unwrap();
            let AgentRequest::TextGenerationStatus { provider: actual } = decoded else {
                panic!("wrong request type")
            };
            assert_eq!(actual, provider);
        }
    }

    #[test]
    fn handshake_request_has_a_stable_golden_json_contract() {
        let request = RequestEnvelope::current(
            "golden-1",
            AgentRequest::Handshake {
                client_version: "0.1.0".into(),
                minimum_protocol_version: PROTOCOL_VERSION,
                maximum_protocol_version: PROTOCOL_VERSION,
            },
        );
        let json = serde_json::to_string(&request).expect("serialize request");
        assert_eq!(
            json,
            r#"{"protocolVersion":20,"requestId":"golden-1","request":{"type":"handshake","clientVersion":"0.1.0","minimumProtocolVersion":20,"maximumProtocolVersion":20}}"#
        );

        let with_future_field = json.replace(
            "\"requestId\":\"golden-1\"",
            "\"requestId\":\"golden-1\",\"futureEnvelopeField\":true",
        );
        let decoded: RequestEnvelope =
            serde_json::from_str(&with_future_field).expect("ignore compatible future field");
        assert_eq!(decoded.request_id, "golden-1");
    }

    #[test]
    fn discard_requests_have_a_stable_golden_json_contract() {
        let file = RequestEnvelope::current(
            "golden-2",
            AgentRequest::Discard {
                request: crate::worktree::DiscardRequest {
                    repository_path: "/repo".into(),
                    worktree_path: "/repo".into(),
                    target: crate::worktree::DiscardTarget::File {
                        path: crate::worktree::GitPath {
                            display: "a.txt".into(),
                            token: "612e747874".into(),
                        },
                        scope: crate::worktree::DiscardScope::Unstaged,
                    },
                    fingerprint: "f".repeat(64),
                },
            },
        );
        assert_eq!(
            serde_json::to_string(&file).expect("serialize discard"),
            format!(
                r#"{{"protocolVersion":20,"requestId":"golden-2","request":{{"type":"discard","request":{{"repositoryPath":"/repo","worktreePath":"/repo","target":{{"kind":"file","path":{{"display":"a.txt","token":"612e747874"}},"scope":"unstaged"}},"fingerprint":"{}"}}}}}}"#,
                "f".repeat(64)
            )
        );
        let all = AgentRequest::PlanDiscard {
            request: crate::worktree::DiscardPlanRequest {
                repository_path: "/repo".into(),
                worktree_path: "/repo".into(),
                target: crate::worktree::DiscardTarget::All,
            },
        };
        assert_eq!(
            serde_json::to_string(&all).expect("serialize plan"),
            r#"{"type":"planDiscard","request":{"repositoryPath":"/repo","worktreePath":"/repo","target":{"kind":"all"}}}"#
        );
    }
}
