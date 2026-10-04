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

export type ActionKind = "remove" | "repair" | "unlock" | "pruneRepository" | "deleteBranch";

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

export interface FollowUpAction {
  kind: ActionKind;
  repositoryPath: string;
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

/** What a discard covers. Every discard is planned, then executed against the plan's fingerprint. */
export type DiscardTarget =
  | { kind: "file"; path: GitPath; scope: DiscardScope }
  | { kind: "all" };

export type DiscardEffect = "restoreCommitted" | "restoreStaged" | "remove" | "unstage" | "restoreCommittedStaged";

/** One path a discard changes. A rename is two entries: its new name and its original one. */
export interface DiscardPlanEntry {
  path: GitPath;
  effect: DiscardEffect;
  /** A file is at the path now. */
  onDisk: boolean;
  /** The index has an entry for the path now. */
  tracked: boolean;
}

export type KeptChangeReason = "submodule" | "nestedRepository" | "fileFolderConflict";

/** A change a discard leaves alone because it cannot save everything discarding it would replace. */
export interface KeptChange {
  path: GitPath;
  reason: KeptChangeReason;
}

export interface DiscardPlan {
  target: DiscardTarget;
  /** Paths the discard changes beyond `entries`, which lists as many as fit in one response. */
  omitted: number;
  /** Paths left alone beyond `kept`. */
  keptOmitted: number;
  entries: DiscardPlanEntry[];
  kept: KeptChange[];
  /** Working-tree bytes the recovery point adds to the repository's object store. */
  backupBytes: number;
  fingerprint: string;
}

export interface DiscardResult {
  recoveryPoint: RecoveryPoint;
}

export type RecoveryPointKind = "discardFile" | "discardAll" | "restore";

/** Content Repola saved under `refs/repola/discarded/` before discarding or overwriting it. */
export interface RecoveryPoint {
  /** The full reference name. */
  id: string;
  /** The tree the reference points at. */
  oid: string;
  kind: RecoveryPointKind;
  summary: string;
  /** RFC 3339 UTC timestamp. */
  createdAt: string;
  worktreePath: string;
  head: string | null;
  pathCount: number;
  /** The first saved paths, for display; `pathCount` is authoritative. */
  paths: GitPath[];
  storedBytes: number;
}

/** The newest recovery points that fit in one response, and how many older ones were left out. */
export interface RecoveryPointList {
  points: RecoveryPoint[];
  omitted: number;
}

export interface RecoveryPointReference {
  id: string;
  oid: string;
}

export type RestoreEffect = "unchanged" | "create" | "replace" | "remove" | "createFolder";

export interface RecoveryRestoreEntry {
  path: GitPath;
  worktree: RestoreEffect;
  indexChanges: boolean;
}

export interface RecoveryRestorePlan {
  point: RecoveryPoint;
  /** As many entries as fit in one response. */
  entries: RecoveryRestoreEntry[];
  /** Saved paths the restore covers beyond `entries`. */
  omitted: number;
  fingerprint: string;
}

export interface RecoveryRestoreResult {
  /** The recovery point holding what the restore replaced, when there was anything. */
  replaced: RecoveryPoint | null;
}

/** The change restoring one saved path would make to the working tree. */
export interface RecoveryFileDiff {
  patch: string;
  binary: boolean;
  truncated: boolean;
}

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
