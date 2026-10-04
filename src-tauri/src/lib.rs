mod desktop;
pub mod settings;

use repola_engine::{host, operation, protocol, worktree};

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use operation::OperationToken;
use protocol::{AgentEvent, AgentInfo, AgentRequest, AgentResult, RequestEnvelope, ResponseBody};
use serde::{Deserialize, Serialize};
use settings::{
    AppPreferences, MachineKind, MachineProfile, MachineProfileInput, RepositoryRegistrationResult,
    WindowState, WorkspaceContext,
};
use tauri::ipc::Channel;
use tauri::{Emitter, Manager};
use worktree::{
    ActionExecutionRequest, ActionKind, ActionPlan, ActionRequest, ActionResult,
    ApplyPatchHunkRequest, BranchDeletionExecutionRequest, BranchDeletionPlan,
    BranchDeletionRequest, BranchDeletionResult, BranchInfo, BranchMutationRequest,
    BranchMutationResult, BranchRequest, CloneRepositoryRequest, CommitChangedFile,
    CommitFileDiffRequest, CommitFilesRequest, CommitRequest, CommitResult, ConflictFile,
    ConflictFileRequest, CreateRepositoryRequest, CreateWorktreeRequest, CreateWorktreeResult,
    DiscardAllRequest, DiscardFileRequest, FileDiff, FileDiffRequest, GenerateCommitMessageRequest,
    GeneratedCommitMessage, HistoryMutationRequest, HistoryMutationResult, HistoryPage,
    HistoryRequest, PullRequestEvidence, PullRequestMutationRequest, PullRequestMutationResult,
    ReflogEntry, ReflogRequest, RepositoryOperationMutationResult, RepositoryOperationRequest,
    RepositoryOperationResult, ResolveConflictRequest, ScanEvent, ScanRequest, ScanResult,
    SetFileStagingRequest, StashEntry, StashMutationRequest, StashMutationResult, StashRequest,
    SyncRequest, SyncResult, TagInfo, TagMutationRequest, TagMutationResult, TagRequest,
    TextGenerationStatus, UndoCommitRequest, UndoCommitResult, WorkingCopyRequest,
    WorkingCopySnapshot, WorktreeChanges, WorktreeWatcher,
};

const WORKTREE_CHANGED_EVENT: &str = "repola://worktree-changed";

#[derive(Default)]
struct WorktreeWatchState(Mutex<Option<WorktreeWatcher>>);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorktreeWatchResult {
    supported: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct WorktreeChangedPayload {
    machine_id: String,
    worktree_path: String,
}

#[tauri::command]
fn watch_worktree(
    app: tauri::AppHandle,
    watch: tauri::State<'_, WorktreeWatchState>,
    machine_id: String,
    worktree_path: String,
) -> Result<WorktreeWatchResult, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let mut active = watch
        .0
        .lock()
        .map_err(|_| "The worktree watcher is unavailable.".to_string())?;
    // Replace any previous watch before deciding whether the new one is supported.
    active.take();
    if machine.kind != MachineKind::Local {
        return Ok(WorktreeWatchResult { supported: false });
    }
    let emitter = app.clone();
    let watcher = WorktreeWatcher::start(&worktree_path, move |event| {
        let _ = emitter.emit(
            WORKTREE_CHANGED_EVENT,
            WorktreeChangedPayload {
                machine_id: machine_id.clone(),
                worktree_path: event.worktree_path,
            },
        );
    })?;
    *active = Some(watcher);
    Ok(WorktreeWatchResult { supported: true })
}

#[tauri::command]
fn unwatch_worktree(watch: tauri::State<'_, WorktreeWatchState>) -> Result<(), String> {
    let mut active = watch
        .0
        .lock()
        .map_err(|_| "The worktree watcher is unavailable.".to_string())?;
    active.take();
    Ok(())
}

#[tauri::command]
fn load_registered_repositories(
    app: tauri::AppHandle,
    machine_id: String,
) -> Result<Vec<String>, String> {
    settings::registered_repositories(&app, &machine_id).map_err(|error| error.to_string())
}

#[tauri::command]
fn load_app_preferences(app: tauri::AppHandle) -> Result<AppPreferences, String> {
    settings::app_preferences(&app).map_err(|error| error.to_string())
}

#[tauri::command]
fn save_app_preferences(
    app: tauri::AppHandle,
    preferences: AppPreferences,
) -> Result<AppPreferences, String> {
    settings::set_app_preferences(&app, preferences).map_err(|error| error.to_string())
}

#[tauri::command]
fn load_external_tools() -> worktree::ExternalToolAvailability {
    worktree::available_external_tools()
}

#[tauri::command]
fn load_window_state(app: tauri::AppHandle) -> Result<Option<WindowState>, String> {
    settings::window_state(&app).map_err(|error| error.to_string())
}

#[tauri::command]
fn save_window_state(app: tauri::AppHandle, state: WindowState) -> Result<WindowState, String> {
    settings::set_window_state(&app, state).map_err(|error| error.to_string())
}

#[tauri::command]
fn launch_worktree_tool(
    app: tauri::AppHandle,
    machine_id: String,
    path: String,
    tool: worktree::WorktreeTool,
) -> Result<String, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let preferences = settings::app_preferences(&app).map_err(|error| error.to_string())?;
    worktree::launch_worktree_tool(&machine, &preferences, &path, tool)
}

#[tauri::command]
fn open_file_in_editor(
    app: tauri::AppHandle,
    machine_id: String,
    worktree_path: String,
    path_token: String,
    remote_os: Option<String>,
) -> Result<String, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let preferences = settings::app_preferences(&app).map_err(|error| error.to_string())?;
    worktree::open_file_in_editor(
        &machine,
        &preferences,
        &worktree_path,
        &path_token,
        remote_os.as_deref(),
    )
}

#[tauri::command]
fn show_file_in_file_manager(
    app: tauri::AppHandle,
    machine_id: String,
    worktree_path: String,
    path_token: String,
) -> Result<(), String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    if machine.kind != MachineKind::Local {
        return Err(
            "Files on a remote machine cannot be shown in this computer's file manager.".into(),
        );
    }
    let path = worktree::working_copy_entry_path(&worktree_path, &path_token)?;
    tauri_plugin_opener::reveal_item_in_dir(&path).map_err(|error| error.to_string())
}

#[tauri::command]
fn resolve_dropped_repository(path: String) -> Result<String, String> {
    let path = std::path::PathBuf::from(path);
    let directory = if path.is_dir() {
        path
    } else {
        path.parent()
            .map(std::path::Path::to_path_buf)
            .ok_or_else(|| "The dropped item has no containing folder.".to_string())?
    };
    worktree::resolve_repository(&directory.to_string_lossy())
        .map_err(|error| format!("The dropped item is not inside a Git working copy: {error}"))
}

#[tauri::command]
fn unregister_repository(
    app: tauri::AppHandle,
    machine_id: String,
    repository_path: String,
) -> Result<Vec<String>, String> {
    settings::remove_registered_repository(&app, &machine_id, &repository_path)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn load_machines(app: tauri::AppHandle) -> Result<Vec<MachineProfile>, String> {
    settings::machines(&app).map_err(|error| error.to_string())
}

#[tauri::command]
fn load_workspace_context(app: tauri::AppHandle) -> Result<WorkspaceContext, String> {
    settings::workspace_context(&app).map_err(|error| error.to_string())
}

#[tauri::command]
fn save_workspace_context(
    app: tauri::AppHandle,
    context: WorkspaceContext,
) -> Result<WorkspaceContext, String> {
    settings::set_workspace_context(&app, context).map_err(|error| error.to_string())
}

#[tauri::command]
fn upsert_machine(
    app: tauri::AppHandle,
    input: MachineProfileInput,
) -> Result<Vec<MachineProfile>, String> {
    settings::upsert_machine(&app, input).map_err(|error| error.to_string())
}

#[tauri::command]
fn remove_machine(
    app: tauri::AppHandle,
    machine_id: String,
) -> Result<Vec<MachineProfile>, String> {
    settings::remove_machine(&app, machine_id).map_err(|error| error.to_string())
}

#[tauri::command]
fn move_machine(
    app: tauri::AppHandle,
    machine_id: String,
    delta: i8,
) -> Result<Vec<MachineProfile>, String> {
    settings::move_machine(&app, &machine_id, delta).map_err(|error| error.to_string())
}

#[tauri::command]
async fn test_machine(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    operation_id: String,
) -> Result<AgentInfo, String> {
    let machine = settings::machines(&app)
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|machine| machine.id == machine_id)
        .ok_or_else(|| format!("Machine {machine_id:?} was not found."))?;
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        host::handshake_with_token(&machine, request_id, token).map_err(|error| error.to_string())
    })
    .await;
    operations.finish(&operation_id);
    result.map_err(|error| format!("Machine connection test failed: {error}"))?
}

#[tauri::command]
async fn scan_worktrees(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    operation_id: String,
    request: ScanRequest,
    on_event: Channel<ScanEvent>,
) -> Result<ScanResult, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let result = host::execute(
            &machine,
            RequestEnvelope::current(request_id, AgentRequest::ScanWorktrees { request }),
            |response| {
                if let ResponseBody::Event {
                    event: AgentEvent::Scan { event },
                } = response.body
                {
                    let _ = on_event.send(event);
                }
            },
            token,
        );
        match result {
            Ok(AgentResult::ScanWorktrees { result }) => Ok(result),
            Ok(_) => Err("The local Repola agent returned an unexpected response.".to_string()),
            Err(error) => Err(error.to_string()),
        }
    })
    .await;
    operations.finish(&operation_id);
    result.map_err(|error| format!("Worktree scan task failed: {error}"))?
}

#[tauri::command]
async fn register_repository(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    operation_id: String,
    path: String,
) -> Result<RepositoryRegistrationResult, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        match execute_on_machine(
            &machine,
            request_id,
            AgentRequest::ResolveRepository { path },
            token,
        )? {
            AgentResult::RepositoryResolved { repository_path } => Ok(repository_path),
            _ => Err("The Repola agent returned an unexpected response.".to_string()),
        }
    })
    .await;
    operations.finish(&operation_id);
    let repository_path =
        result.map_err(|error| format!("Repository validation task failed: {error}"))??;
    settings::add_registered_repository(&app, &machine_id, repository_path)
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn fetch_pull_requests(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    operation_id: String,
    repository_path: String,
    branch: String,
) -> Result<PullRequestEvidence, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        match execute_on_machine(
            &machine,
            request_id,
            AgentRequest::FetchPullRequests {
                repository_path,
                branch,
            },
            token,
        )? {
            AgentResult::PullRequests { evidence } => Ok(evidence),
            _ => Err("The local Repola agent returned an unexpected response.".to_string()),
        }
    })
    .await;
    operations.finish(&operation_id);
    result.map_err(|error| format!("Pull-request lookup task failed: {error}"))?
}

#[tauri::command]
async fn mutate_pull_request(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    operation_id: String,
    request: PullRequestMutationRequest,
) -> Result<PullRequestMutationResult, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        match execute_on_machine(
            &machine,
            request_id,
            AgentRequest::MutatePullRequest { request },
            token,
        )? {
            AgentResult::PullRequestMutation { result } => Ok(*result),
            _ => Err("The local Repola agent returned an unexpected response.".to_string()),
        }
    })
    .await;
    operations.finish(&operation_id);
    result.map_err(|error| format!("Pull-request action failed: {error}"))?
}

#[tauri::command]
async fn worktree_changes(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    operation_id: String,
    repository_path: String,
    worktree_path: String,
) -> Result<WorktreeChanges, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        match execute_on_machine(
            &machine,
            request_id,
            AgentRequest::WorktreeChanges {
                repository_path,
                worktree_path,
            },
            token,
        )? {
            AgentResult::WorktreeChanges { changes } => Ok(changes),
            _ => Err("The local Repola agent returned an unexpected response.".to_string()),
        }
    })
    .await;
    operations.finish(&operation_id);
    result.map_err(|error| format!("Change inspection task failed: {error}"))?
}

#[tauri::command]
async fn file_diff(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    operation_id: String,
    request: FileDiffRequest,
) -> Result<FileDiff, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        match execute_on_machine(
            &machine,
            request_id,
            AgentRequest::FileDiff { request },
            token,
        )? {
            AgentResult::FileDiff { diff } => Ok(diff),
            _ => Err("The local Repola agent returned an unexpected response.".to_string()),
        }
    })
    .await;
    operations.finish(&operation_id);
    result.map_err(|error| format!("File diff task failed: {error}"))?
}

#[tauri::command]
async fn working_copy_snapshot(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    operation_id: String,
    request: WorkingCopyRequest,
) -> Result<WorkingCopySnapshot, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        match execute_on_machine(
            &machine,
            request_id,
            AgentRequest::WorkingCopySnapshot { request },
            token,
        )? {
            AgentResult::WorkingCopySnapshot { snapshot } => Ok(snapshot),
            _ => Err("The Repola agent returned an unexpected response.".to_string()),
        }
    })
    .await;
    operations.finish(&operation_id);
    result.map_err(|error| format!("Working-copy inspection task failed: {error}"))?
}

#[tauri::command]
async fn set_file_staging(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    operation_id: String,
    request: SetFileStagingRequest,
) -> Result<WorkingCopySnapshot, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        match execute_on_machine(
            &machine,
            request_id,
            AgentRequest::SetFileStaging { request },
            token,
        )? {
            AgentResult::WorkingCopyUpdated { snapshot } => Ok(snapshot),
            _ => Err("The Repola agent returned an unexpected response.".to_string()),
        }
    })
    .await;
    operations.finish(&operation_id);
    result.map_err(|error| format!("Staging task failed: {error}"))?
}

#[tauri::command]
async fn resolve_conflict(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    operation_id: String,
    request: ResolveConflictRequest,
) -> Result<WorkingCopySnapshot, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        match execute_on_machine(
            &machine,
            request_id,
            AgentRequest::ResolveConflict { request },
            token,
        )? {
            AgentResult::WorkingCopyUpdated { snapshot } => Ok(snapshot),
            _ => Err("The Repola agent returned an unexpected response.".to_string()),
        }
    })
    .await;
    operations.finish(&operation_id);
    result.map_err(|error| format!("Conflict-resolution task failed: {error}"))?
}

#[tauri::command]
async fn load_conflict_file(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    operation_id: String,
    request: ConflictFileRequest,
) -> Result<ConflictFile, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        match execute_on_machine(
            &machine,
            request_id,
            AgentRequest::ConflictFile { request },
            token,
        )? {
            AgentResult::ConflictFile { file } => Ok(file),
            _ => Err("The Repola agent returned an unexpected response.".to_string()),
        }
    })
    .await;
    operations.finish(&operation_id);
    result.map_err(|error| format!("Conflict-file loading task failed: {error}"))?
}

#[tauri::command]
async fn discard_file(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    operation_id: String,
    request: DiscardFileRequest,
) -> Result<WorkingCopySnapshot, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        match execute_on_machine(
            &machine,
            request_id,
            AgentRequest::DiscardFile { request },
            token,
        )? {
            AgentResult::WorkingCopyUpdated { snapshot } => Ok(snapshot),
            _ => Err("The Repola agent returned an unexpected response.".to_string()),
        }
    })
    .await;
    operations.finish(&operation_id);
    result.map_err(|error| format!("Discard task failed: {error}"))?
}

#[tauri::command]
async fn discard_all(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    operation_id: String,
    request: DiscardAllRequest,
) -> Result<WorkingCopySnapshot, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        match execute_on_machine(
            &machine,
            request_id,
            AgentRequest::DiscardAll { request },
            token,
        )? {
            AgentResult::WorkingCopyUpdated { snapshot } => Ok(snapshot),
            _ => Err("The Repola agent returned an unexpected response.".to_string()),
        }
    })
    .await;
    operations.finish(&operation_id);
    result.map_err(|error| format!("Discard-all task failed: {error}"))?
}

#[tauri::command]
async fn apply_patch_hunk(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    operation_id: String,
    request: ApplyPatchHunkRequest,
) -> Result<WorkingCopySnapshot, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        match execute_on_machine(
            &machine,
            request_id,
            AgentRequest::ApplyPatchHunk { request },
            token,
        )? {
            AgentResult::WorkingCopyUpdated { snapshot } => Ok(snapshot),
            _ => Err("The Repola agent returned an unexpected response.".to_string()),
        }
    })
    .await;
    operations.finish(&operation_id);
    result.map_err(|error| format!("Partial staging task failed: {error}"))?
}

#[tauri::command]
async fn mutate_repository_operation(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    operation_id: String,
    request: RepositoryOperationRequest,
) -> Result<RepositoryOperationMutationResult, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        match execute_on_machine(
            &machine,
            request_id,
            AgentRequest::MutateRepositoryOperation { request },
            token,
        )? {
            AgentResult::RepositoryOperationMutation { result } => Ok(result),
            _ => Err("The Repola agent returned an unexpected response.".to_string()),
        }
    })
    .await;
    operations.finish(&operation_id);
    result.map_err(|error| format!("Git operation task failed: {error}"))?
}

#[tauri::command]
async fn commit_working_copy(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    operation_id: String,
    request: CommitRequest,
) -> Result<CommitResult, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        match execute_on_machine(
            &machine,
            request_id,
            AgentRequest::Commit { request },
            token,
        )? {
            AgentResult::Commit { result } => Ok(result),
            _ => Err("The Repola agent returned an unexpected response.".to_string()),
        }
    })
    .await;
    operations.finish(&operation_id);
    result.map_err(|error| format!("Commit task failed: {error}"))?
}

#[tauri::command]
async fn generate_commit_message(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    operation_id: String,
    mut request: GenerateCommitMessageRequest,
) -> Result<GeneratedCommitMessage, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let preferences = settings::app_preferences(&app).map_err(|error| error.to_string())?;
    request.text_generation_selection = preferences
        .text_generation_selections
        .get(&machine_id)
        .map(|settings| settings.selection());
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        match execute_on_machine(
            &machine,
            request_id,
            AgentRequest::GenerateCommitMessage { request },
            token,
        )? {
            AgentResult::GeneratedCommitMessage { message } => Ok(message),
            _ => Err("The Repola agent returned an unexpected response.".to_string()),
        }
    })
    .await;
    operations.finish(&operation_id);
    result.map_err(|error| format!("Commit-message task failed: {error}"))?
}

#[tauri::command]
async fn text_generation_status(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    provider: Option<repola_engine::preferences::TextGenerationProvider>,
    operation_id: String,
) -> Result<TextGenerationStatus, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        match execute_on_machine(
            &machine,
            request_id,
            AgentRequest::TextGenerationStatus { provider },
            token,
        )? {
            AgentResult::TextGenerationStatus { status } => Ok(status),
            _ => Err("The Repola agent returned an unexpected response.".to_string()),
        }
    })
    .await;
    operations.finish(&operation_id);
    result.map_err(|error| format!("Text generation status task failed: {error}"))?
}

#[tauri::command]
async fn undo_commit(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    operation_id: String,
    request: UndoCommitRequest,
) -> Result<UndoCommitResult, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        match execute_on_machine(
            &machine,
            request_id,
            AgentRequest::UndoCommit { request },
            token,
        )? {
            AgentResult::CommitUndone { result } => Ok(*result),
            _ => Err("The Repola agent returned an unexpected response.".to_string()),
        }
    })
    .await;
    operations.finish(&operation_id);
    result.map_err(|error| format!("Commit undo task failed: {error}"))?
}

#[tauri::command]
async fn load_history(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    operation_id: String,
    request: HistoryRequest,
) -> Result<HistoryPage, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        match execute_on_machine(
            &machine,
            request_id,
            AgentRequest::History { request },
            token,
        )? {
            AgentResult::History { page } => Ok(page),
            _ => Err("The Repola agent returned an unexpected response.".to_string()),
        }
    })
    .await;
    operations.finish(&operation_id);
    result.map_err(|error| format!("History task failed: {error}"))?
}

#[tauri::command]
async fn load_reflog(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    operation_id: String,
    request: ReflogRequest,
) -> Result<Vec<ReflogEntry>, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        match execute_on_machine(
            &machine,
            request_id,
            AgentRequest::Reflog { request },
            token,
        )? {
            AgentResult::Reflog { entries } => Ok(entries),
            _ => Err("The Repola agent returned an unexpected response.".to_string()),
        }
    })
    .await;
    operations.finish(&operation_id);
    result.map_err(|error| format!("Reflog task failed: {error}"))?
}

#[tauri::command]
async fn mutate_history(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    operation_id: String,
    request: HistoryMutationRequest,
) -> Result<HistoryMutationResult, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        match execute_on_machine(
            &machine,
            request_id,
            AgentRequest::MutateHistory { request },
            token,
        )? {
            AgentResult::HistoryMutation { result } => Ok(result),
            _ => Err("The Repola agent returned an unexpected response.".to_string()),
        }
    })
    .await;
    operations.finish(&operation_id);
    result.map_err(|error| format!("History mutation task failed: {error}"))?
}

#[tauri::command]
async fn load_tags(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    operation_id: String,
    request: TagRequest,
) -> Result<Vec<TagInfo>, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        match execute_on_machine(&machine, request_id, AgentRequest::Tags { request }, token)? {
            AgentResult::Tags { tags } => Ok(tags),
            _ => Err("The Repola agent returned an unexpected response.".to_string()),
        }
    })
    .await;
    operations.finish(&operation_id);
    result.map_err(|error| format!("Tag loading task failed: {error}"))?
}

#[tauri::command]
async fn mutate_tag(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    operation_id: String,
    request: TagMutationRequest,
) -> Result<TagMutationResult, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        match execute_on_machine(
            &machine,
            request_id,
            AgentRequest::MutateTag { request },
            token,
        )? {
            AgentResult::TagMutation { result } => Ok(result),
            _ => Err("The Repola agent returned an unexpected response.".to_string()),
        }
    })
    .await;
    operations.finish(&operation_id);
    result.map_err(|error| format!("Tag mutation task failed: {error}"))?
}

#[tauri::command]
async fn load_commit_files(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    operation_id: String,
    request: CommitFilesRequest,
) -> Result<Vec<CommitChangedFile>, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        match execute_on_machine(
            &machine,
            request_id,
            AgentRequest::CommitFiles { request },
            token,
        )? {
            AgentResult::CommitFiles { files } => Ok(files),
            _ => Err("The Repola agent returned an unexpected response.".to_string()),
        }
    })
    .await;
    operations.finish(&operation_id);
    result.map_err(|error| format!("Commit-file task failed: {error}"))?
}

#[tauri::command]
async fn commit_file_diff(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    operation_id: String,
    request: CommitFileDiffRequest,
) -> Result<FileDiff, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        match execute_on_machine(
            &machine,
            request_id,
            AgentRequest::CommitFileDiff { request },
            token,
        )? {
            AgentResult::CommitFileDiff { diff } => Ok(diff),
            _ => Err("The Repola agent returned an unexpected response.".to_string()),
        }
    })
    .await;
    operations.finish(&operation_id);
    result.map_err(|error| format!("Commit-diff task failed: {error}"))?
}

#[tauri::command]
async fn synchronize_working_copy(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    operation_id: String,
    request: SyncRequest,
) -> Result<SyncResult, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        match execute_on_machine(
            &machine,
            request_id,
            AgentRequest::Synchronize { request },
            token,
        )? {
            AgentResult::Synchronize { result } => Ok(result),
            _ => Err("The Repola agent returned an unexpected response.".to_string()),
        }
    })
    .await;
    operations.finish(&operation_id);
    result.map_err(|error| format!("Synchronization task failed: {error}"))?
}

#[tauri::command]
async fn load_branches(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    operation_id: String,
    request: BranchRequest,
) -> Result<Vec<BranchInfo>, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        match execute_on_machine(
            &machine,
            request_id,
            AgentRequest::Branches { request },
            token,
        )? {
            AgentResult::Branches { branches } => Ok(branches),
            _ => Err("The Repola agent returned an unexpected response.".to_string()),
        }
    })
    .await;
    operations.finish(&operation_id);
    result.map_err(|error| format!("Branch-list task failed: {error}"))?
}

#[tauri::command]
async fn mutate_branch(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    operation_id: String,
    request: BranchMutationRequest,
) -> Result<BranchMutationResult, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        match execute_on_machine(
            &machine,
            request_id,
            AgentRequest::MutateBranch { request },
            token,
        )? {
            AgentResult::BranchMutation { result } => Ok(result),
            _ => Err("The Repola agent returned an unexpected response.".to_string()),
        }
    })
    .await;
    operations.finish(&operation_id);
    result.map_err(|error| format!("Branch task failed: {error}"))?
}

#[tauri::command]
async fn clone_repository(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    operation_id: String,
    request: CloneRepositoryRequest,
) -> Result<RepositoryOperationResult, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        match execute_on_machine(
            &machine,
            request_id,
            AgentRequest::CloneRepository { request },
            token,
        )? {
            AgentResult::RepositoryOperation { result } => Ok(result),
            _ => Err("The Repola agent returned an unexpected response.".to_string()),
        }
    })
    .await;
    operations.finish(&operation_id);
    result.map_err(|error| format!("Repository clone task failed: {error}"))?
}

#[tauri::command]
async fn create_repository(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    operation_id: String,
    request: CreateRepositoryRequest,
) -> Result<RepositoryOperationResult, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        match execute_on_machine(
            &machine,
            request_id,
            AgentRequest::CreateRepository { request },
            token,
        )? {
            AgentResult::RepositoryOperation { result } => Ok(result),
            _ => Err("The Repola agent returned an unexpected response.".to_string()),
        }
    })
    .await;
    operations.finish(&operation_id);
    result.map_err(|error| format!("Repository creation task failed: {error}"))?
}

#[tauri::command]
async fn create_worktree(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    operation_id: String,
    request: CreateWorktreeRequest,
) -> Result<CreateWorktreeResult, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        match execute_on_machine(
            &machine,
            request_id,
            AgentRequest::CreateWorktree { request },
            token,
        )? {
            AgentResult::WorktreeCreated { result } => Ok(result),
            _ => Err("The Repola agent returned an unexpected response.".to_string()),
        }
    })
    .await;
    operations.finish(&operation_id);
    result.map_err(|error| format!("Worktree creation task failed: {error}"))?
}

#[tauri::command]
async fn load_stashes(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    operation_id: String,
    request: StashRequest,
) -> Result<Vec<StashEntry>, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        match execute_on_machine(
            &machine,
            request_id,
            AgentRequest::Stashes { request },
            token,
        )? {
            AgentResult::Stashes { stashes } => Ok(stashes),
            _ => Err("The Repola agent returned an unexpected response.".to_string()),
        }
    })
    .await;
    operations.finish(&operation_id);
    result.map_err(|error| format!("Stash-list task failed: {error}"))?
}

#[tauri::command]
async fn mutate_stash(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    operation_id: String,
    request: StashMutationRequest,
) -> Result<StashMutationResult, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        match execute_on_machine(
            &machine,
            request_id,
            AgentRequest::MutateStash { request },
            token,
        )? {
            AgentResult::StashMutation { result } => Ok(result),
            _ => Err("The Repola agent returned an unexpected response.".to_string()),
        }
    })
    .await;
    operations.finish(&operation_id);
    result.map_err(|error| format!("Stash task failed: {error}"))?
}

#[tauri::command]
async fn prepare_worktree_action(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    operation_id: String,
    request: ActionRequest,
) -> Result<ActionPlan, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        match execute_on_machine(
            &machine,
            request_id,
            AgentRequest::PrepareWorktreeAction { request },
            token,
        )? {
            AgentResult::WorktreeActionPlan { plan } => Ok(plan),
            _ => Err("The local Repola agent returned an unexpected response.".to_string()),
        }
    })
    .await;
    operations.finish(&operation_id);
    result.map_err(|error| format!("Action review task failed: {error}"))?
}

#[tauri::command]
async fn execute_worktree_action(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    operation_id: String,
    request: ActionExecutionRequest,
) -> Result<ActionResult, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let audit_request = request.clone();
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        match execute_on_machine(
            &machine,
            request_id,
            AgentRequest::ExecuteWorktreeAction { request },
            token,
        )? {
            AgentResult::WorktreeActionResult { result } => Ok(result),
            _ => Err("The local Repola agent returned an unexpected response.".to_string()),
        }
    })
    .await;
    operations.finish(&operation_id);
    let outcome = outcome.map_err(|error| format!("Action task failed: {error}"))?;
    let (succeeded, message) = match &outcome {
        Ok(result) => (true, result.message.clone()),
        Err(error) => (false, error.clone()),
    };
    let audit = append_audit_entry(
        &app,
        &AuditEntry::worktree_action(&machine_id, &audit_request, succeeded, message),
    );
    match outcome {
        Ok(mut result) => {
            match audit {
                Ok(path) => result.audit_path = Some(path),
                Err(error) => result.audit_warning = Some(error),
            }
            Ok(result)
        }
        Err(error) => Err(error.to_string()),
    }
}

#[tauri::command]
async fn prepare_branch_deletion(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    operation_id: String,
    request: BranchDeletionRequest,
) -> Result<BranchDeletionPlan, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        match execute_on_machine(
            &machine,
            request_id,
            AgentRequest::PrepareBranchDeletion { request },
            token,
        )? {
            AgentResult::BranchDeletionPlan { plan } => Ok(*plan),
            _ => Err("The Repola agent returned an unexpected response.".to_string()),
        }
    })
    .await;
    operations.finish(&operation_id);
    result.map_err(|error| format!("Branch deletion review failed: {error}"))?
}

#[tauri::command]
async fn execute_branch_deletion(
    app: tauri::AppHandle,
    operations: tauri::State<'_, OperationRegistry>,
    machine_id: String,
    operation_id: String,
    request: BranchDeletionExecutionRequest,
) -> Result<BranchDeletionResult, String> {
    let machine = settings::machine(&app, &machine_id).map_err(|error| error.to_string())?;
    let audit_request = request.clone();
    let token = operations.begin(&operation_id)?;
    let request_id = operation_id.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        match execute_on_machine(
            &machine,
            request_id,
            AgentRequest::ExecuteBranchDeletion { request },
            token,
        )? {
            AgentResult::BranchDeletion { result } => Ok(*result),
            _ => Err("The Repola agent returned an unexpected response.".to_string()),
        }
    })
    .await;
    operations.finish(&operation_id);
    let outcome = outcome.map_err(|error| format!("Branch deletion failed: {error}"))?;
    let (succeeded, message) = match &outcome {
        Ok(result) => {
            let steps = [&result.local, &result.remote].into_iter().flatten();
            (
                steps.clone().all(|step| step.succeeded),
                std::iter::once(result.message.as_str())
                    .chain(steps.filter_map(|step| step.warning.as_deref()))
                    .collect::<Vec<_>>()
                    .join(" "),
            )
        }
        Err(error) => (false, error.clone()),
    };
    let audit = append_audit_entry(
        &app,
        &AuditEntry::branch_deletion(&machine_id, &audit_request, succeeded, message),
    );
    let mut result = outcome?;
    match audit {
        Ok(path) => result.audit_path = Some(path),
        Err(error) => result.audit_warning = Some(error),
    }
    Ok(result)
}

fn execute_on_machine(
    machine: &MachineProfile,
    operation_id: String,
    request: AgentRequest,
    token: OperationToken,
) -> Result<AgentResult, String> {
    host::execute(
        machine,
        RequestEnvelope::current(operation_id, request),
        |_| {},
        token,
    )
    .map_err(|error| error.to_string())
}

#[derive(Default)]
struct OperationRegistry(Mutex<HashMap<String, OperationToken>>);

impl OperationRegistry {
    fn begin(&self, operation_id: &str) -> Result<OperationToken, String> {
        if operation_id.is_empty()
            || operation_id.len() > 128
            || operation_id.chars().any(char::is_control)
        {
            return Err("The operation ID is invalid.".into());
        }
        let token = OperationToken::new();
        let mut active = self
            .0
            .lock()
            .map_err(|_| "The operation registry is unavailable.".to_string())?;
        match active.entry(operation_id.to_string()) {
            Entry::Vacant(entry) => {
                entry.insert(token.clone());
            }
            Entry::Occupied(_) => {
                return Err("An operation with this ID is already active.".into());
            }
        }
        Ok(token)
    }

    fn cancel(&self, operation_id: &str) -> Result<bool, String> {
        let active = self
            .0
            .lock()
            .map_err(|_| "The operation registry is unavailable.".to_string())?;
        let Some(token) = active.get(operation_id) else {
            return Ok(false);
        };
        token.cancel();
        Ok(true)
    }

    fn finish(&self, operation_id: &str) {
        if let Ok(mut active) = self.0.lock() {
            active.remove(operation_id);
        }
    }
}

#[tauri::command]
fn cancel_operation(
    operations: tauri::State<'_, OperationRegistry>,
    operation_id: String,
) -> Result<bool, String> {
    operations.cancel(&operation_id)
}

/// One line of `actions.jsonl`. Fields added later are optional so entries
/// written by earlier versions keep decoding.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuditEntry {
    timestamp_ms: u64,
    action: AuditAction,
    machine_id: String,
    repository_path: String,
    worktree_path: String,
    expected_head: Option<String>,
    expected_branch: Option<String>,
    affected_paths: Vec<String>,
    succeeded: bool,
    outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    branch_deletion: Option<BranchDeletionAudit>,
}

/// Worktree actions keep their `ActionKind` names. Branch deletions, including
/// those earlier versions ran as a worktree action, are `deleteBranch`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum AuditAction {
    Remove,
    Repair,
    Unlock,
    PruneRepository,
    DeleteBranch,
}

impl From<ActionKind> for AuditAction {
    fn from(kind: ActionKind) -> Self {
        match kind {
            ActionKind::Remove => Self::Remove,
            ActionKind::Repair => Self::Repair,
            ActionKind::Unlock => Self::Unlock,
            ActionKind::PruneRepository => Self::PruneRepository,
        }
    }
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BranchDeletionAudit {
    branch_ref: String,
    delete_local: bool,
    delete_remote: bool,
    force: bool,
    remote: Option<String>,
    remote_ref: Option<String>,
    remote_oid: Option<String>,
}

impl AuditEntry {
    fn worktree_action(
        machine_id: &str,
        request: &ActionExecutionRequest,
        succeeded: bool,
        outcome: String,
    ) -> Self {
        Self {
            timestamp_ms: now_ms(),
            action: request.kind.into(),
            machine_id: machine_id.to_string(),
            repository_path: request.repository_path.clone(),
            worktree_path: request.worktree_path.clone(),
            expected_head: request.expected_head.clone(),
            expected_branch: request.expected_branch.clone(),
            affected_paths: request.expected_affected_paths.clone(),
            succeeded,
            outcome,
            branch_deletion: None,
        }
    }

    fn branch_deletion(
        machine_id: &str,
        execution: &BranchDeletionExecutionRequest,
        succeeded: bool,
        outcome: String,
    ) -> Self {
        let request = &execution.request;
        let expected = &execution.expected;
        let mut affected_paths = Vec::new();
        if request.delete_local {
            affected_paths.push(request.branch_ref.clone());
        }
        if let (true, Some(remote), Some(remote_ref)) = (
            request.delete_remote,
            expected.remote.as_deref(),
            expected.remote_ref.as_deref(),
        ) {
            affected_paths.push(format!("{remote}:{remote_ref}"));
        }
        Self {
            timestamp_ms: now_ms(),
            action: AuditAction::DeleteBranch,
            machine_id: machine_id.to_string(),
            repository_path: request.repository_path.clone(),
            worktree_path: request.worktree_path.clone(),
            expected_head: expected.local_tip.clone(),
            expected_branch: Some(request.branch_ref.clone()),
            affected_paths,
            succeeded,
            outcome,
            branch_deletion: Some(BranchDeletionAudit {
                branch_ref: request.branch_ref.clone(),
                delete_local: request.delete_local,
                delete_remote: request.delete_remote,
                force: execution.force,
                remote: expected.remote.clone(),
                remote_ref: expected.remote_ref.clone(),
                remote_oid: expected.remote_oid.clone(),
            }),
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn append_audit_entry(app: &tauri::AppHandle, entry: &AuditEntry) -> Result<String, String> {
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("Could not resolve the audit directory: {error}"))?;
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("Could not create the audit directory: {error}"))?;
    let path = directory.join("actions.jsonl");
    let mut line = serde_json::to_vec(entry)
        .map_err(|error| format!("Could not serialize the audit entry: {error}"))?;
    line.push(b'\n');
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| format!("Could not open the audit log: {error}"))?;
    file.write_all(&line)
        .map_err(|error| format!("Could not write the audit log: {error}"))?;
    Ok(path.to_string_lossy().into_owned())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(OperationRegistry::default())
        .manage(WorktreeWatchState::default())
        .menu(desktop::application_menu)
        .on_menu_event(|app, event| {
            let _ = app.emit("repola://menu-action", event.id().as_ref());
        })
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .setup(|app| {
            settings::migrate_legacy_app_data(app.handle())?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            cancel_operation,
            apply_patch_hunk,
            clone_repository,
            text_generation_status,
            commit_working_copy,
            generate_commit_message,
            commit_file_diff,
            create_repository,
            create_worktree,
            discard_all,
            discard_file,
            execute_branch_deletion,
            execute_worktree_action,
            file_diff,
            fetch_pull_requests,
            launch_worktree_tool,
            open_file_in_editor,
            show_file_in_file_manager,
            load_app_preferences,
            load_external_tools,
            load_machines,
            load_history,
            load_reflog,
            load_commit_files,
            load_branches,
            load_conflict_file,
            load_registered_repositories,
            load_stashes,
            load_tags,
            load_workspace_context,
            load_window_state,
            move_machine,
            mutate_history,
            mutate_pull_request,
            mutate_repository_operation,
            mutate_branch,
            mutate_stash,
            mutate_tag,
            prepare_branch_deletion,
            prepare_worktree_action,
            remove_machine,
            register_repository,
            resolve_conflict,
            resolve_dropped_repository,
            save_app_preferences,
            save_workspace_context,
            save_window_state,
            scan_worktrees,
            set_file_staging,
            synchronize_working_copy,
            test_machine,
            unregister_repository,
            undo_commit,
            unwatch_worktree,
            upsert_machine,
            watch_worktree,
            worktree_changes,
            working_copy_snapshot,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Repola");
}

#[cfg(test)]
mod coordinator_tests {
    use super::*;

    #[test]
    fn duplicate_ids_do_not_replace_the_running_operation() {
        let registry = OperationRegistry::default();
        let original = registry.begin("same-id").expect("first operation");
        assert!(registry.begin("same-id").is_err());
        assert!(registry.cancel("same-id").expect("cancel operation"));
        assert!(original.is_cancelled());
        registry.finish("same-id");
        assert!(!registry.cancel("same-id").expect("finished operation"));
    }

    #[test]
    fn operation_ids_are_bounded_and_safe_for_diagnostics() {
        let registry = OperationRegistry::default();
        assert!(registry.begin("").is_err());
        assert!(registry.begin("line\nbreak").is_err());
        assert!(registry.begin(&"x".repeat(129)).is_err());
    }
}

#[cfg(test)]
mod audit_tests {
    use super::*;
    use worktree::{BranchDeletionConfirmation, BranchDeletionFingerprint};

    /// A line exactly as earlier versions wrote it, before branch-deletion details existed.
    const LEGACY_ENTRY: &str = r#"{"timestampMs":1700000000000,"action":"remove","machineId":"local","repositoryPath":"/work/repository","worktreePath":"/work/linked","expectedHead":"0123456789abcdef0123456789abcdef01234567","expectedBranch":"feature","affectedPaths":["/work/linked"],"succeeded":true,"outcome":"Worktree removed. Its Git branch was left intact."}"#;

    #[test]
    fn entries_written_by_earlier_versions_still_decode() {
        let entry: AuditEntry = serde_json::from_str(LEGACY_ENTRY).expect("legacy audit entry");
        assert_eq!(entry.action, AuditAction::Remove);
        assert_eq!(entry.expected_branch.as_deref(), Some("feature"));
        assert_eq!(entry.affected_paths, vec!["/work/linked".to_string()]);
        assert!(entry.branch_deletion.is_none());
        assert_eq!(
            serde_json::to_string(&entry).expect("serialize"),
            LEGACY_ENTRY,
            "worktree actions keep writing the same line format"
        );
    }

    #[test]
    fn branch_deletions_written_as_worktree_actions_still_decode() {
        let line = r#"{"timestampMs":1700000000000,"action":"deleteBranch","machineId":"local","repositoryPath":"/work/repository","worktreePath":"","expectedHead":"0123456789abcdef0123456789abcdef01234567","expectedBranch":"feature","affectedPaths":["refs/heads/feature"],"succeeded":true,"outcome":"Branch feature deleted with git branch -d."}"#;
        let entry: AuditEntry = serde_json::from_str(line).expect("legacy branch deletion entry");
        assert_eq!(entry.action, AuditAction::DeleteBranch);
        assert!(entry.branch_deletion.is_none());
    }

    #[test]
    fn branch_deletions_record_their_scope_and_round_trip() {
        let execution = BranchDeletionExecutionRequest {
            request: BranchDeletionRequest {
                repository_path: "/work/repository".into(),
                worktree_path: "/work/repository".into(),
                branch_ref: "refs/heads/feature".into(),
                delete_local: true,
                delete_remote: true,
            },
            force: true,
            expected: BranchDeletionFingerprint {
                local_tip: Some("a".repeat(40)),
                merge_reference_oid: Some("b".repeat(40)),
                requires_force: true,
                remote: Some("origin".into()),
                remote_ref: Some("refs/heads/feature".into()),
                remote_oid: Some("c".repeat(40)),
                local_exclusive_commits: None,
                remote_exclusive_commits: None,
                confirmation: BranchDeletionConfirmation::TypeBranchName,
            },
            typed_confirmation: Some("feature".into()),
        };
        let entry = AuditEntry::branch_deletion("local", &execution, false, "partial".into());
        assert_eq!(entry.action, AuditAction::DeleteBranch);
        assert_eq!(
            entry.affected_paths,
            vec![
                "refs/heads/feature".to_string(),
                "origin:refs/heads/feature".to_string()
            ]
        );
        let details = entry
            .branch_deletion
            .as_ref()
            .expect("branch deletion details");
        assert!(details.force);
        let line = serde_json::to_string(&entry).expect("serialize");
        let decoded: AuditEntry = serde_json::from_str(&line).expect("decode");
        assert_eq!(decoded, entry);
    }
}
