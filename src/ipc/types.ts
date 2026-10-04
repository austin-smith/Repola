export type RemoteProvider = "gitHub" | "azureDevOps" | "other" | "none";
export type WorktreeOriginKind = "primary" | "agent" | "unattributed";
export type RegistrationKind = "healthy" | "brokenLink" | "locked" | "missing" | "prunable" | "primary";
export type IntegrationKind = "headContained" | "headNotContained" | "unknown" | "notApplicable";
export type SafetyLevel = "protected" | "review" | "repair" | "metadataOnly";
export type MachineKind = "local" | "ssh";

export interface SshProfile {
  host: string;
  user: string | null;
  port: number | null;
}

export interface MachineProfile {
  id: string;
  name: string;
  kind: MachineKind;
  enabled: boolean;
  ssh: SshProfile | null;
}

export interface MachineProfileInput {
  id: string | null;
  name: string;
  enabled: boolean;
  host: string;
  user: string | null;
  port: number | null;
}

export interface AgentInfo {
  agentVersion: string;
  protocolVersion: number;
  operatingSystem: string;
  architecture: string;
  gitVersion: string | null;
  maximumFrameBytes: number;
  capabilities: ("repositoryDiscovery" | "worktreeInventory" | "workingCopy" | "history" | "synchronization" | "branches" | "repositoryManagement" | "worktreeManagement" | "stashes" | "providerIntegration" | "textGeneration")[];
}

export type WorkspaceView = "changes" | "history" | "worktrees";

export interface AppPreferences {
  version: number;
  editorId: string | null;
  terminalId: string | null;
  defaultSignCommits: boolean;
  textGenerationSelections: Record<string, TextGenerationPreferences>;
}

export interface TextGenerationPreferences {
  provider: TextGenerationProvider;
  selections: Partial<Record<TextGenerationProvider, Omit<TextGenerationSelection, "provider">>>;
}

export interface ExternalTool {
  id: string;
  label: string;
  supportsRemoteWorkspaces: boolean;
}

export interface ExternalToolAvailability {
  editors: ExternalTool[];
  terminals: ExternalTool[];
}

export interface WindowState {
  version: number;
  x: number;
  y: number;
  width: number;
  height: number;
  maximized: boolean;
}

export interface WorkspaceLocation {
  repositoryPath: string | null;
  worktreePath: string | null;
  view: WorkspaceView;
}

export interface WorkspaceContext {
  selectedMachineId: string;
  locations: Record<string, WorkspaceLocation>;
  filters: Record<string, WorkspaceFilters>;
  layout: WorkspaceLayout;
}

export interface WorkspaceLayout {
  inventorySidebarWidth: number;
  detailsWidth: number;
}

export interface WorkspaceFilters {
  query: string;
  repositoryPath: string;
  ageDays: 0 | 30 | 90 | 180 | 365;
  state: "all" | "clean" | "changed" | "attention";
}

export interface ChangeSummary {
  available: boolean;
  total: number;
  staged: number;
  unstaged: number;
  untracked: number;
  conflicted: number;
}

export interface WorktreeOrigin {
  kind: WorktreeOriginKind;
  id: string;
  label: string;
}

export interface WorktreeRecord {
  id: string;
  repositoryName: string;
  repositoryPath: string;
  path: string;
  branch: string | null;
  head: string | null;
  detached: boolean;
  isPrimary: boolean;
  exists: boolean;
  createdAtMs: number | null;
  headCommitAtMs: number | null;
  lastActivityAtMs: number | null;
  headSubject: string | null;
  unpushedCommitCount: number | null;
  sizeBytes: number | null;
  sizeIncomplete: boolean;
  origin: WorktreeOrigin;
  status: ChangeSummary;
  registration: { kind: RegistrationKind; reason: string | null };
  integration: { kind: IntegrationKind; target: string | null; summary: string };
  safety: { level: SafetyLevel; label: string; reasons: string[] };
}

export interface RepositorySummary {
  id: string;
  name: string;
  path: string;
  remoteUrl: string | null;
  provider: RemoteProvider;
  worktreeCount: number;
  attentionCount: number;
  conflictedCount: number;
  allocatedBytes: number;
  allocationIncomplete: boolean;
}

export interface RepositoryRegistrationResult {
  repositoryPath: string;
  registeredRepositories: string[];
}

export interface ScanResult {
  scannedAtMs: number;
  repositoryPaths: string[];
  repositories: RepositorySummary[];
  worktrees: WorktreeRecord[];
  totals: {
    repositoryCount: number;
    primaryCount: number;
    linkedCount: number;
    existingLinkedCount: number;
    cleanCount: number;
    dirtyCount: number;
    missingCount: number;
    prunableCount: number;
    brokenLinkCount: number;
    linkedSizeBytes: number;
  };
  warnings: string[];
}

export type ActionKind = "remove" | "repair" | "unlock" | "pruneRepository";

export interface ActionPlan {
  kind: ActionKind;
  title: string;
  summary: string;
  repositoryPath: string;
  worktreePath: string;
  branch: string | null;
  expectedHead: string | null;
  commandDisplay: string;
  affectedPaths: string[];
  warnings: string[];
  confirmationText: string;
  destructive: boolean;
}

export interface ActionResult {
  message: string;
  auditPath: string | null;
  auditWarning: string | null;
  followUp: FollowUpAction | null;
}

/** A branch a removed worktree left behind, reviewed for deletion from `worktreePath`. */
export interface FollowUpAction {
  repositoryPath: string;
  worktreePath: string;
  branch: string;
  description: string;
}

export type PullRequestFetchStatus = "fetched" | "cliMissing" | "notAuthenticated" | "cliError" | "unsupported";

export interface PullRequestSummary {
  number: number;
  title: string;
  state: string;
  url: string | null;
  headBranch: string;
  baseBranch: string;
  reviewState: string;
  mergeState: string;
  checksTotal: number;
  checksPassed: number;
  checksPending: number;
  checksFailed: number;
  linkedIssues: IssueSummary[];
}

export interface IssueSummary {
  number: number;
  title: string;
  url: string | null;
}

export interface PullRequestEvidence {
  status: PullRequestFetchStatus;
  provider: RemoteProvider;
  branch: string;
  pulls: PullRequestSummary[];
  detail: string | null;
}

export type PullRequestMutationKind = "create" | "checkout";

export interface PullRequestMutationResult {
  evidence: PullRequestEvidence;
  snapshot: WorkingCopySnapshot | null;
  createdUrl: string | null;
  output: string;
}

export interface WorktreeWatchResult {
  supported: boolean;
}

export interface WorktreeChangedEvent {
  machineId: string;
  worktreePath: string;
}

export interface WorktreeChanges {
  available: boolean;
  patch: string;
  truncated: boolean;
  untracked: string[];
  reason: string | null;
}

export interface FileDiff {
  patch: string;
  truncated: boolean;
  binary: boolean;
  submodule: boolean;
  image: ImageComparison | null;
  hunks: PatchHunk[];
  stagedHunks: PatchHunk[];
  unstagedHunks: PatchHunk[];
}

export interface ImageComparison {
  before: ImageVersion;
  after: ImageVersion;
}

export type ImageVersion =
  | { kind: "preview"; preview: ImagePreview }
  | { kind: "missing" | "unsupported" | "tooLarge" };

export interface ImagePreview {
  mimeType: string;
  base64: string;
  byteLength: number;
  label: string;
}

export interface PatchHunk {
  index: number;
  header: string;
  patch: string;
}

export type PatchHunkAction = "stage" | "unstage" | "discard";

export type FileChangeKind = "added" | "modified" | "deleted" | "renamed" | "copied" | "typeChanged" | "unmerged" | "untracked" | "ignored" | "unknown";
export type FileModeChange = "executableBit" | "symlink" | "other";
export type RepositoryOperation = "merge" | "rebase" | "cherryPick" | "revert" | "bisect" | "sequencer";
export type RepositoryOperationAction = "continue" | "skip" | "abort";

export interface GitPath {
  display: string;
  token: string;
}

export interface FileChange {
  id: string;
  path: GitPath;
  previousPath: GitPath | null;
  kind: FileChangeKind;
  indexStatus: string;
  worktreeStatus: string;
  staged: boolean;
  unstaged: boolean;
  conflicted: boolean;
  untracked: boolean;
  ignored: boolean;
  submodule: boolean;
  headMode: string | null;
  indexMode: string | null;
  worktreeMode: string | null;
  modeChange: FileModeChange | null;
  /**
   * Content identity: the HEAD/index object IDs from porcelain v2 (null for
   * records without them, e.g. untracked and unmerged entries) and a stat
   * stamp of the entry on disk. Older agents omit all three, so `undefined`
   * (as opposed to null) means the content identity is unknown.
   */
  headOid?: string | null;
  indexOid?: string | null;
  worktreeStamp?: string | null;
}

export type ConflictResolutionKind = "ours" | "theirs" | "both" | "manual" | "markResolved" | "remove";

export interface ConflictFile {
  content: string;
  byteLength: number;
}
export type DiscardScope = "unstaged" | "all";

export interface WorkingCopySnapshot {
  repositoryPath: string;
  worktreePath: string;
  head: string | null;
  branch: string | null;
  upstream: string | null;
  upstreamHead: string | null;
  remote: string | null;
  ahead: number;
  behind: number;
  changes: FileChange[];
  operation: RepositoryOperation | null;
}

export interface RepositoryOperationMutationResult {
  snapshot: WorkingCopySnapshot;
  succeeded: boolean;
  output: string;
}

export type HistoryMutationKind =
  | "merge"
  | "squashMerge"
  | "rebase"
  | "cherryPick"
  | "revert"
  | "resetSoft"
  | "resetMixed"
  | "resetHard";

export interface HistoryMutationResult {
  snapshot: WorkingCopySnapshot;
  succeeded: boolean;
  output: string;
  conflicted: boolean;
}

export interface TagInfo {
  name: string;
  target: string;
  object: string;
  annotated: boolean;
  subject: string;
}

export type TagMutationKind = "createLightweight" | "createAnnotated" | "delete" | "push";

export interface TagMutationResult {
  tags: TagInfo[];
  snapshot: WorkingCopySnapshot;
  output: string;
}

export interface CreateWorktreeResult {
  repositoryPath: string;
  worktreePath: string;
  branch: string;
  output: string;
}

export interface CommitRequest {
  repositoryPath: string;
  worktreePath: string;
  expectedHead: string | null;
  includedChanges: CommitFileSelectionRequest[];
  summary: string;
  description: string;
  amend: boolean;
  author: CommitPerson | null;
  coAuthors: CommitPerson[];
  trailers: CommitTrailer[];
  signing: CommitSigning;
}

export interface GenerateCommitMessageRequest {
  repositoryPath: string;
  worktreePath: string;
  expectedHead: string | null;
  includedChanges: CommitFileSelectionRequest[];
  amend: boolean;
}

export interface GeneratedCommitMessage {
  subject: string;
  body: string;
}

export type TextGenerationStatusKind = "ready" | "notInstalled" | "signedOut" | "updateRequired" | "unavailable";

export interface TextGenerationStatus {
  status: TextGenerationStatusKind;
  detail?: string | null;
  version: string | null;
  models: TextGenerationModel[];
  recommendedSelection: TextGenerationSelection | null;
}

export interface TextGenerationSelection {
  provider: TextGenerationProvider;
  model: string | null;
  reasoningEffort: string | null;
}

export type TextGenerationProvider = "codex" | "claude";

export interface TextGenerationModel {
  model: string;
  displayName: string;
  description: string;
  isDefault: boolean;
  recommendedForCommitMessages: boolean;
  upgrade: string | null;
  defaultReasoningEffort: string | null;
  supportedReasoningEfforts: ReasoningEffort[];
}

export interface ReasoningEffort {
  reasoningEffort: string;
  description: string;
}

export interface CommitFileSelectionRequest {
  path: GitPath;
  previousPath: GitPath | null;
  expectedIndexStatus: string;
  expectedWorktreeStatus: string;
  includeAll: boolean;
  hunks: CommitHunkSelectionRequest[];
}

export interface CommitHunkSelectionRequest {
  expectedPatch: string;
  selectedLineIndices: number[];
}

export interface CommitPerson {
  name: string;
  email: string;
}

export interface CommitTrailer {
  key: string;
  value: string;
}

export type CommitSigning = "default" | "sign" | "doNotSign";

export interface CommitResult {
  commit: string;
  snapshot: WorkingCopySnapshot;
  hookOutput: string;
}

export interface UndoCommitResult {
  commit: string;
  summary: string;
  description: string;
  snapshot: WorkingCopySnapshot;
}

export type CommitSignature = "good" | "bad" | "unknownValidity" | "expired" | "expiredKey" | "revokedKey" | "missingKey" | "unsigned" | "error" | "unknown";

export interface CommitSummary {
  oid: string;
  parents: string[];
  authorName: string;
  authorEmail: string;
  authoredAt: string;
  committedAt: string;
  signature: CommitSignature;
  subject: string;
  body: string;
}

export interface HistoryPage {
  commits: CommitSummary[];
  nextCursor: string | null;
}

export interface ReflogEntry {
  oid: string;
  selector: string;
  subject: string;
  committedAt: string;
}

export interface CommitChangedFile {
  id: string;
  path: GitPath;
  previousPath: GitPath | null;
  kind: FileChangeKind;
  status: string;
}

export type SyncKind = "fetch" | "pull" | "push" | "publish" | "forcePush";

export interface SyncResult {
  snapshot: WorkingCopySnapshot;
  output: string;
}

export interface BranchInfo {
  name: string;
  fullName: string;
  head: string;
  remote: boolean;
  current: boolean;
  upstream: string | null;
  ahead: number;
  behind: number;
  occupiedWorktreePath: string | null;
}

export type BranchMutationKind = "create" | "checkout" | "rename";

export interface BranchMutationResult {
  branches: BranchInfo[];
  snapshot: WorkingCopySnapshot;
}

export interface BranchDeletionRequest {
  repositoryPath: string;
  worktreePath: string;
  /** Full ref name: `refs/heads/<branch>` or `refs/remotes/<remote>/<branch>`. */
  branchRef: string;
  deleteLocal: boolean;
  deleteRemote: boolean;
}

export type MergeReferenceKind = "upstream" | "head";

export type BranchDeletionConfirmation = "confirm" | "typeBranchName";

export interface LocalBranchDeletion {
  name: string;
  tip: string;
  mergeReference: string;
  mergeReferenceKind: MergeReferenceKind;
  mergeReferenceOid: string | null;
  containedInMergeReference: boolean;
  defaultTarget: string | null;
  containedInDefaultTarget: boolean | null;
  occupiedWorktreePath: string | null;
  isDefaultBranch: boolean;
  upstream: string | null;
  exclusiveCommitCount: number;
  exclusiveCommitCountCapped: boolean;
}

export interface RemoteBranchDeletion {
  remote: string;
  remoteRef: string;
  trackingRef: string;
  displayName: string;
  expectedOid: string;
  /** Where the deletion pushes, with any credentials redacted. */
  pushUrl: string;
  /** Open pull requests using this branch as their source or target; asked only when the remote branch is selected. */
  pullRequests: BranchPullRequests | null;
  /** Unix seconds. */
  trackingRefUpdatedAt: number | null;
  /** Unix seconds. */
  lastFetchedAt: number | null;
  isRemoteDefaultBranch: boolean;
  trackedBy: string[];
  exclusiveCommitCount: number;
  exclusiveCommitCountCapped: boolean;
}

/** What the provider hosting a remote branch said about the open pull requests that use it as their source or target. */
export type BranchPullRequests =
  | {
    status: "checked";
    provider: RemoteProvider;
    pulls: OpenPullRequest[];
    /** The provider has more than Repola lists. */
    moreThanListed: boolean;
  }
  | { status: "unsupported" }
  | { status: "unavailable"; reason: string };

export interface OpenPullRequest {
  /** The repository the pull request belongs to, which numbers it. */
  repository: string;
  number: number;
  title: string;
  url: string | null;
  /** Whether it merges from the branch or into it. */
  relation: "source" | "target";
  /** The repository and branch it merges from. */
  from: string;
  /** The repository and branch it merges into. */
  into: string;
}

export interface BranchDeletionFingerprint {
  localTip: string | null;
  mergeReferenceOid: string | null;
  requiresForce: boolean;
  remote: string | null;
  remoteRef: string | null;
  remoteOid: string | null;
  /** The open pull requests the review listed for the remote branch, as `repository#number`, or null when the provider could not be asked. */
  pullRequests: string[] | null;
  /** A digest of the exact URL the remote deletion pushes to, as reviewed. */
  pushDestination: string | null;
  /** A digest of exactly which commits the local deletion would leave unreachable, as reviewed. */
  localReachability: string | null;
  /** A digest of exactly which commits the remote deletion would leave unreachable, as reviewed. */
  remoteReachability: string | null;
  /** The commands the review displayed, which execution runs. */
  commands: string[];
  /** The warnings the review displayed. */
  warnings: string[];
  confirmation: BranchDeletionConfirmation;
}

export interface BranchDeletionPlan {
  repositoryPath: string;
  worktreePath: string;
  branchRef: string;
  branchName: string;
  deleteLocal: boolean;
  deleteRemote: boolean;
  local: LocalBranchDeletion | null;
  remote: RemoteBranchDeletion | null;
  remoteUnavailableReason: string | null;
  requiresForce: boolean;
  confirmation: BranchDeletionConfirmation;
  commands: string[];
  warnings: string[];
  blockers: string[];
  fingerprint: BranchDeletionFingerprint;
}

export interface BranchDeletionStep {
  target: string;
  deletedOid: string;
  succeeded: boolean;
  /** The step was interrupted after it started, so it may have happened. */
  unconfirmed: boolean;
  output: string;
  /** Something left undone by a step that still succeeded. */
  warning: string | null;
  /** The commands that finish what the warning says was left undone. */
  finishCommands: string[];
  /** The commands that restore what a successful step deleted, in order. */
  recoveryCommands: string[];
}

/**
 * Why a deletion returned no result. A refusal deletes nothing; otherwise the deletion was
 * interrupted after it started and may have completed anyway.
 */
export interface BranchDeletionFailure {
  message: string;
  outcomeKnown: boolean;
}

export interface BranchDeletionResult {
  message: string;
  local: BranchDeletionStep | null;
  remote: BranchDeletionStep | null;
  auditPath: string | null;
  auditWarning: string | null;
}

export interface RepositoryOperationResult {
  repositoryPath: string;
  output: string;
}

export interface StashEntry {
  index: number;
  oid: string;
  createdAt: string;
  subject: string;
}

export type StashMutationKind = "push" | "apply" | "pop" | "drop";

export interface StashMutationResult {
  snapshot: WorkingCopySnapshot;
  stashes: StashEntry[];
  output: string;
  conflicted: boolean;
}

export type ScanEvent =
  | { type: "repositories"; repositoryPaths: string[]; repositories: RepositorySummary[]; warnings: string[] }
  | { type: "worktree"; record: WorktreeRecord }
  | { type: "size"; id: string; sizeBytes: number | null; sizeIncomplete: boolean; lastActivityAtMs: number | null }
  | { type: "done"; result: ScanResult };
