//! Persistent app settings.
//!
//! Settings live in `settings.json` inside the platform app-data directory
//! (`%APPDATA%\<identifier>` on Windows, `~/Library/Application Support/<identifier>`
//! on macOS, `~/.config/<identifier>` on Linux) and are managed by
//! `tauri-plugin-store`. The store holds untyped JSON; this module is the only
//! place that knows the keys and the shapes behind them.
//!
//! The machine-profile and preference models, and their validation, live in
//! `repola_engine` so the agent and the desktop share one definition.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_store::{Store, StoreExt};

use repola_engine::machines::{
    move_machine as move_stored_machine, normalize_machine, reject_duplicate_connections,
    with_local_machine,
};
pub use repola_engine::machines::{
    MachineError, MachineKind, MachineProfile, MachineProfileInput, SshProfile, LOCAL_MACHINE_ID,
};
pub use repola_engine::preferences::AppPreferences;
use repola_engine::preferences::{
    migrate_app_preferences, normalize_tool_id, APP_PREFERENCES_VERSION,
};

pub const STORE_FILE: &str = "settings.json";
const REGISTERED_REPOSITORIES_BY_MACHINE_KEY: &str = "registeredRepositoriesByMachine";
const MACHINES_KEY: &str = "machines";
const WORKSPACE_CONTEXT_KEY: &str = "workspaceContext";
const APP_PREFERENCES_KEY: &str = "appPreferences";
const WINDOW_STATE_KEY: &str = "windowState";
const LEGACY_APP_IDENTIFIER: &str = "net.crapshack.git-in-here";
const MIGRATED_FILES: &[&str] = &[STORE_FILE, "actions.jsonl"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryRegistrationResult {
    pub repository_path: String,
    pub registered_repositories: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceContext {
    pub selected_machine_id: String,
    #[serde(default)]
    pub locations: BTreeMap<String, WorkspaceLocation>,
    #[serde(default)]
    pub filters: BTreeMap<String, WorkspaceFilters>,
    #[serde(default)]
    pub layout: WorkspaceLayout,
}

impl Default for WorkspaceContext {
    fn default() -> Self {
        Self {
            selected_machine_id: LOCAL_MACHINE_ID.into(),
            locations: BTreeMap::new(),
            filters: BTreeMap::new(),
            layout: WorkspaceLayout::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceLayout {
    pub inventory_sidebar_width: u16,
    pub details_width: u16,
}

impl Default for WorkspaceLayout {
    fn default() -> Self {
        Self {
            inventory_sidebar_width: 230,
            details_width: 360,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WorkspaceFilters {
    pub query: String,
    pub repository_path: String,
    pub age_days: u16,
    pub state: PersistedStateFilter,
}

impl Default for WorkspaceFilters {
    fn default() -> Self {
        Self {
            query: String::new(),
            repository_path: "all".into(),
            age_days: 0,
            state: PersistedStateFilter::All,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PersistedStateFilter {
    #[default]
    All,
    Clean,
    Changed,
    Attention,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceLocation {
    pub repository_path: Option<String>,
    pub worktree_path: Option<String>,
    pub view: WorkspaceView,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkspaceView {
    #[default]
    Changes,
    History,
    Worktrees,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowState {
    pub version: u16,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("Could not open the settings store: {0}")]
    Store(String),
    #[error("Repository paths must be absolute; rejected {0:?}.")]
    RelativePath(String),
    #[error("Could not migrate Repola application data: {0}")]
    Migration(String),
    #[error("Invalid setting: {0}")]
    Invalid(String),
    #[error(transparent)]
    Machine(#[from] MachineError),
}

/// Copy known user data from the pre-Repola identifier exactly once. Existing
/// Repola files always win, making the migration idempotent and safe after an
/// interrupted launch.
pub fn migrate_legacy_app_data<R: Runtime>(app: &AppHandle<R>) -> Result<(), SettingsError> {
    let current = app
        .path()
        .app_data_dir()
        .map_err(|error| SettingsError::Migration(error.to_string()))?;
    let Some(parent) = current.parent() else {
        return Err(SettingsError::Migration(format!(
            "{} has no parent directory",
            current.display()
        )));
    };
    migrate_known_files(&parent.join(LEGACY_APP_IDENTIFIER), &current)
}

fn migrate_known_files(legacy: &Path, current: &Path) -> Result<(), SettingsError> {
    if !legacy.is_dir() {
        return Ok(());
    }

    for file_name in MIGRATED_FILES {
        let source = legacy.join(file_name);
        let destination = current.join(file_name);
        if !source.is_file() || destination.exists() {
            continue;
        }
        std::fs::create_dir_all(current)
            .map_err(|error| SettingsError::Migration(error.to_string()))?;
        std::fs::copy(&source, &destination)
            .map_err(|error| SettingsError::Migration(error.to_string()))?;
    }
    Ok(())
}

pub fn registered_repositories<R: Runtime>(
    app: &AppHandle<R>,
    machine_id: &str,
) -> Result<Vec<String>, SettingsError> {
    let store = open_store(app)?;
    machine_by_id(&store, machine_id)?;
    Ok(read_registered_repositories(&store, machine_id))
}

pub fn add_registered_repository<R: Runtime>(
    app: &AppHandle<R>,
    machine_id: &str,
    repository_path: String,
) -> Result<RepositoryRegistrationResult, SettingsError> {
    let store = open_store(app)?;
    let machine = machine_by_id(&store, machine_id)?;
    let is_local = machine.kind == MachineKind::Local;
    let mut repositories = read_registered_repositories(&store, machine_id);
    repositories.push(repository_path.clone());
    let registered_repositories =
        write_registered_repositories(&store, machine_id, is_local, repositories)?;
    let repository_path = registered_repositories
        .iter()
        .find(|path| paths_equivalent(path, &repository_path, is_local))
        .cloned()
        .ok_or_else(|| SettingsError::Store("the resolved repository was not persisted".into()))?;
    Ok(RepositoryRegistrationResult {
        repository_path,
        registered_repositories,
    })
}

pub fn remove_registered_repository<R: Runtime>(
    app: &AppHandle<R>,
    machine_id: &str,
    repository_path: &str,
) -> Result<Vec<String>, SettingsError> {
    let store = open_store(app)?;
    let machine = machine_by_id(&store, machine_id)?;
    let is_local = machine.kind == MachineKind::Local;
    let repositories = read_registered_repositories(&store, machine_id)
        .into_iter()
        .filter(|path| !paths_equivalent(path, repository_path, is_local))
        .collect();
    write_registered_repositories(&store, machine_id, is_local, repositories)
}

pub fn machine<R: Runtime>(
    app: &AppHandle<R>,
    machine_id: &str,
) -> Result<MachineProfile, SettingsError> {
    let store = open_store(app)?;
    machine_by_id(&store, machine_id)
}

pub fn machines<R: Runtime>(app: &AppHandle<R>) -> Result<Vec<MachineProfile>, SettingsError> {
    let store = open_store(app)?;
    read_machines(&store)
}

pub fn workspace_context<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<WorkspaceContext, SettingsError> {
    let store = open_store(app)?;
    Ok(normalize_workspace_context(
        read_workspace_context(&store),
        &read_machines(&store)?,
    ))
}

pub fn app_preferences<R: Runtime>(app: &AppHandle<R>) -> Result<AppPreferences, SettingsError> {
    let store = open_store(app)?;
    Ok(read_app_preferences(&store))
}

pub fn set_app_preferences<R: Runtime>(
    app: &AppHandle<R>,
    mut preferences: AppPreferences,
) -> Result<AppPreferences, SettingsError> {
    preferences.version = APP_PREFERENCES_VERSION;
    preferences.editor_id = normalize_tool_id(preferences.editor_id);
    preferences.terminal_id = normalize_tool_id(preferences.terminal_id);
    let store = open_store(app)?;
    store.set(
        APP_PREFERENCES_KEY,
        serde_json::to_value(&preferences)
            .map_err(|error| SettingsError::Store(error.to_string()))?,
    );
    store
        .save()
        .map_err(|error| SettingsError::Store(error.to_string()))?;
    Ok(preferences)
}

pub fn window_state<R: Runtime>(app: &AppHandle<R>) -> Result<Option<WindowState>, SettingsError> {
    let store = open_store(app)?;
    Ok(store
        .get(WINDOW_STATE_KEY)
        .and_then(|value| serde_json::from_value(value).ok())
        .filter(valid_window_state))
}

pub fn set_window_state<R: Runtime>(
    app: &AppHandle<R>,
    mut state: WindowState,
) -> Result<WindowState, SettingsError> {
    state.version = 1;
    if !valid_window_state(&state) {
        return Err(SettingsError::Invalid(
            "window geometry is outside supported bounds".into(),
        ));
    }
    let store = open_store(app)?;
    store.set(
        WINDOW_STATE_KEY,
        serde_json::to_value(&state).map_err(|error| SettingsError::Store(error.to_string()))?,
    );
    store
        .save()
        .map_err(|error| SettingsError::Store(error.to_string()))?;
    Ok(state)
}

fn valid_window_state(state: &WindowState) -> bool {
    state.version == 1
        && (1_080..=16_384).contains(&state.width)
        && (680..=16_384).contains(&state.height)
        && (-100_000..=100_000).contains(&state.x)
        && (-100_000..=100_000).contains(&state.y)
}

pub fn set_workspace_context<R: Runtime>(
    app: &AppHandle<R>,
    context: WorkspaceContext,
) -> Result<WorkspaceContext, SettingsError> {
    let store = open_store(app)?;
    let context = normalize_workspace_context(context, &read_machines(&store)?);
    validate_workspace_context(&context)?;
    store.set(
        WORKSPACE_CONTEXT_KEY,
        serde_json::to_value(&context).map_err(|error| SettingsError::Store(error.to_string()))?,
    );
    store
        .save()
        .map_err(|error| SettingsError::Store(error.to_string()))?;
    Ok(context)
}

pub fn upsert_machine<R: Runtime>(
    app: &AppHandle<R>,
    input: MachineProfileInput,
) -> Result<Vec<MachineProfile>, SettingsError> {
    let store = open_store(app)?;
    let mut stored = read_stored_machines(&store);
    let profile = normalize_machine(input)?;
    if let Some(index) = stored.iter().position(|machine| machine.id == profile.id) {
        stored[index] = profile;
    } else {
        stored.push(profile);
    }
    reject_duplicate_connections(&stored)?;
    write_machines(&store, &stored)?;
    Ok(with_local_machine(stored))
}

pub fn remove_machine<R: Runtime>(
    app: &AppHandle<R>,
    machine_id: String,
) -> Result<Vec<MachineProfile>, SettingsError> {
    if machine_id == LOCAL_MACHINE_ID {
        return Err(
            MachineError::Invalid("the built-in local machine cannot be removed".into()).into(),
        );
    }
    let store = open_store(app)?;
    let mut stored = read_stored_machines(&store);
    let original_length = stored.len();
    stored.retain(|machine| machine.id != machine_id);
    if stored.len() == original_length {
        return Err(MachineError::NotFound(machine_id).into());
    }
    write_machines(&store, &stored)?;
    let mut repositories = registered_repositories_by_machine(&store);
    if repositories.remove(&machine_id).is_some() {
        store.set(
            REGISTERED_REPOSITORIES_BY_MACHINE_KEY,
            serde_json::to_value(repositories)
                .map_err(|error| SettingsError::Store(error.to_string()))?,
        );
        store
            .save()
            .map_err(|error| SettingsError::Store(error.to_string()))?;
    }
    Ok(with_local_machine(stored))
}

pub fn move_machine<R: Runtime>(
    app: &AppHandle<R>,
    machine_id: &str,
    delta: i8,
) -> Result<Vec<MachineProfile>, SettingsError> {
    if !matches!(delta, -1 | 1) {
        return Err(MachineError::Invalid(
            "machine order can only move one position at a time".into(),
        )
        .into());
    }
    let store = open_store(app)?;
    let mut stored = read_stored_machines(&store);
    move_stored_machine(&mut stored, machine_id, delta)?;
    write_machines(&store, &stored)?;
    Ok(with_local_machine(stored))
}

fn open_store<R: Runtime>(app: &AppHandle<R>) -> Result<Arc<Store<R>>, SettingsError> {
    app.store(STORE_FILE)
        .map_err(|error| SettingsError::Store(error.to_string()))
}

pub(crate) fn read_registered_repositories<R: Runtime>(
    store: &Store<R>,
    machine_id: &str,
) -> Vec<String> {
    registered_repositories_by_machine(store)
        .remove(machine_id)
        .unwrap_or_default()
}

pub(crate) fn write_registered_repositories<R: Runtime>(
    store: &Store<R>,
    machine_id: &str,
    is_local: bool,
    repositories: Vec<String>,
) -> Result<Vec<String>, SettingsError> {
    let repositories = if is_local {
        normalize_local_repository_paths(repositories)?
    } else {
        normalize_remote_repository_paths(repositories)?
    };
    let mut by_machine = registered_repositories_by_machine(store);
    by_machine.insert(machine_id.to_string(), repositories.clone());
    store.set(
        REGISTERED_REPOSITORIES_BY_MACHINE_KEY,
        serde_json::to_value(by_machine)
            .map_err(|error| SettingsError::Store(error.to_string()))?,
    );
    store
        .save()
        .map_err(|error| SettingsError::Store(error.to_string()))?;
    Ok(repositories)
}

fn registered_repositories_by_machine<R: Runtime>(
    store: &Store<R>,
) -> BTreeMap<String, Vec<String>> {
    store
        .get(REGISTERED_REPOSITORIES_BY_MACHINE_KEY)
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

fn machine_by_id<R: Runtime>(
    store: &Store<R>,
    machine_id: &str,
) -> Result<MachineProfile, SettingsError> {
    if machine_id == LOCAL_MACHINE_ID {
        return Ok(MachineProfile::local());
    }
    read_stored_machines(store)
        .into_iter()
        .find(|machine| machine.id == machine_id)
        .ok_or_else(|| MachineError::NotFound(machine_id.to_string()).into())
}

fn read_machines<R: Runtime>(store: &Store<R>) -> Result<Vec<MachineProfile>, SettingsError> {
    let stored = read_stored_machines(store);
    reject_duplicate_connections(&stored)?;
    Ok(with_local_machine(stored))
}

fn read_stored_machines<R: Runtime>(store: &Store<R>) -> Vec<MachineProfile> {
    store
        .get(MACHINES_KEY)
        .and_then(|value| serde_json::from_value::<Vec<MachineProfile>>(value).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|machine| machine.kind == MachineKind::Ssh && machine.id != LOCAL_MACHINE_ID)
        .collect()
}

fn read_workspace_context<R: Runtime>(store: &Store<R>) -> WorkspaceContext {
    store
        .get(WORKSPACE_CONTEXT_KEY)
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

fn read_app_preferences<R: Runtime>(store: &Store<R>) -> AppPreferences {
    store
        .get(APP_PREFERENCES_KEY)
        .and_then(|value| serde_json::from_value(value).ok())
        .map(migrate_app_preferences)
        .unwrap_or_default()
}

fn normalize_workspace_context(
    mut context: WorkspaceContext,
    machines: &[MachineProfile],
) -> WorkspaceContext {
    context
        .locations
        .retain(|machine_id, _| machines.iter().any(|machine| machine.id == *machine_id));
    context
        .filters
        .retain(|machine_id, _| machines.iter().any(|machine| machine.id == *machine_id));
    if !machines
        .iter()
        .any(|machine| machine.id == context.selected_machine_id && machine.enabled)
    {
        context.selected_machine_id = LOCAL_MACHINE_ID.into();
    }
    context
}

fn validate_workspace_context(context: &WorkspaceContext) -> Result<(), SettingsError> {
    if !(180..=420).contains(&context.layout.inventory_sidebar_width)
        || !(280..=560).contains(&context.layout.details_width)
    {
        return Err(SettingsError::Invalid(
            "workspace pane sizes are outside supported bounds".into(),
        ));
    }
    for location in context.locations.values() {
        for value in [&location.repository_path, &location.worktree_path]
            .into_iter()
            .flatten()
        {
            if value.len() > 32 * 1024 || value.contains('\0') {
                return Err(SettingsError::Invalid(
                    "workspace paths must be bounded and cannot contain NUL characters".into(),
                ));
            }
        }
    }
    for filters in context.filters.values() {
        if filters.query.chars().count() > 1_024
            || filters.repository_path.len() > 32 * 1024
            || filters.repository_path.contains('\0')
            || !matches!(filters.age_days, 0 | 30 | 90 | 180 | 365)
        {
            return Err(SettingsError::Invalid(
                "workspace filters are outside their supported bounds".into(),
            ));
        }
    }
    Ok(())
}

fn write_machines<R: Runtime>(
    store: &Store<R>,
    machines: &[MachineProfile],
) -> Result<(), SettingsError> {
    let value =
        serde_json::to_value(machines).map_err(|error| SettingsError::Store(error.to_string()))?;
    store.set(MACHINES_KEY, value);
    store
        .save()
        .map_err(|error| SettingsError::Store(error.to_string()))
}

pub(crate) fn normalize_local_repository_paths(
    repositories: Vec<String>,
) -> Result<Vec<String>, SettingsError> {
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut normalized = Vec::new();
    for raw in repositories {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let path = Path::new(trimmed);
        if !path.is_absolute() {
            return Err(SettingsError::RelativePath(raw));
        }
        let canonical = dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if seen.contains(&canonical) {
            continue;
        }
        seen.push(canonical.clone());
        normalized.push(canonical.to_string_lossy().into_owned());
    }
    Ok(normalized)
}

fn normalize_remote_repository_paths(
    repositories: Vec<String>,
) -> Result<Vec<String>, SettingsError> {
    let mut normalized = Vec::new();
    for raw in repositories {
        let path = raw.trim();
        if path.is_empty() {
            continue;
        }
        if path.len() > 32 * 1024 || path.chars().any(char::is_control) {
            return Err(SettingsError::Invalid(
                "repository paths must be bounded and cannot contain control characters".into(),
            ));
        }
        if !normalized.iter().any(|existing| existing == path) {
            normalized.push(path.to_string());
        }
    }
    Ok(normalized)
}

fn paths_equivalent(left: &str, right: &str, is_local: bool) -> bool {
    if !is_local {
        return left == right;
    }
    let canonical =
        |value: &str| dunce::canonicalize(value).unwrap_or_else(|_| PathBuf::from(value));
    canonical(left) == canonical(right)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_store(
        store_path: &Path,
    ) -> (
        tauri::App<tauri::test::MockRuntime>,
        Arc<Store<tauri::test::MockRuntime>>,
    ) {
        let app = tauri::test::mock_builder()
            .plugin(tauri_plugin_store::Builder::default().build())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("mock app");
        let store = app
            .handle()
            .store_builder(store_path)
            .build()
            .expect("store at temp path");
        (app, store)
    }

    #[test]
    fn registered_repositories_round_trip_through_the_store_file() {
        let temp = tempfile::tempdir().expect("temp dir");
        let location = temp.path().join("code");
        std::fs::create_dir(&location).expect("create location");
        let store_path = temp.path().join("settings.json");
        let (app, store) = mock_store(&store_path);

        assert!(read_registered_repositories(&store, LOCAL_MACHINE_ID).is_empty());

        let saved = write_registered_repositories(
            &store,
            LOCAL_MACHINE_ID,
            true,
            vec![location.to_string_lossy().into_owned()],
        )
        .expect("write repositories");
        assert_eq!(saved.len(), 1);
        assert!(store_path.is_file(), "settings must be persisted to disk");

        // A fresh store handle must observe the persisted value.
        let reloaded = app
            .handle()
            .store_builder(&store_path)
            .build()
            .expect("reload store");
        reloaded.reload().expect("reload from disk");
        assert_eq!(
            read_registered_repositories(&reloaded, LOCAL_MACHINE_ID),
            saved
        );
    }

    #[test]
    fn workspace_context_round_trips_and_drops_removed_machines() {
        let temp = tempfile::tempdir().expect("temp dir");
        let (_app, store) = mock_store(&temp.path().join("settings.json"));
        let context = WorkspaceContext {
            selected_machine_id: "gone".into(),
            filters: BTreeMap::new(),
            layout: WorkspaceLayout::default(),
            locations: BTreeMap::from([
                (
                    LOCAL_MACHINE_ID.into(),
                    WorkspaceLocation {
                        repository_path: Some("/code/project".into()),
                        worktree_path: Some("/code/project/feature".into()),
                        view: WorkspaceView::History,
                    },
                ),
                (
                    "gone".into(),
                    WorkspaceLocation {
                        repository_path: Some("/srv/project".into()),
                        worktree_path: None,
                        view: WorkspaceView::Changes,
                    },
                ),
            ]),
        };
        store.set(
            WORKSPACE_CONTEXT_KEY,
            serde_json::to_value(context).expect("serialize"),
        );
        store.save().expect("save");
        let normalized =
            normalize_workspace_context(read_workspace_context(&store), &[MachineProfile::local()]);
        assert_eq!(normalized.selected_machine_id, LOCAL_MACHINE_ID);
        assert_eq!(normalized.locations.len(), 1);
        assert_eq!(
            normalized.locations[LOCAL_MACHINE_ID].view,
            WorkspaceView::History
        );
    }

    #[test]
    fn repository_normalization_dedupes_existing_paths_and_keeps_missing_ones() {
        let temp = tempfile::tempdir().expect("temp dir");
        let existing = temp.path().join("code");
        std::fs::create_dir(&existing).expect("create dir");
        let missing = temp.path().join("not-here");

        let mut with_trailing = existing.to_string_lossy().into_owned();
        with_trailing.push(std::path::MAIN_SEPARATOR);
        let repositories = normalize_local_repository_paths(vec![
            format!("  {}  ", existing.display()),
            with_trailing,
            missing.to_string_lossy().into_owned(),
            String::new(),
        ])
        .expect("absolute paths normalize");

        let canonical_existing = dunce::canonicalize(&existing).expect("canonical");
        assert_eq!(
            repositories,
            vec![
                canonical_existing.to_string_lossy().into_owned(),
                missing.to_string_lossy().into_owned(),
            ]
        );
    }

    #[test]
    fn relative_paths_are_rejected() {
        let error = normalize_local_repository_paths(vec!["relative/code".to_string()])
            .expect_err("must reject");
        assert!(matches!(error, SettingsError::RelativePath(path) if path == "relative/code"));
    }

    #[test]
    fn malformed_and_legacy_discovery_values_never_register_repositories() {
        let temp = tempfile::tempdir().expect("temp dir");
        let (_app, store) = mock_store(&temp.path().join("settings.json"));
        store.set(
            REGISTERED_REPOSITORIES_BY_MACHINE_KEY,
            serde_json::json!("nope"),
        );
        store.set("scanRoots", serde_json::json!(["/Users/example/Developer"]));
        store.set(
            "scanRootsByMachine",
            serde_json::json!({ "local": ["/Users/example/Developer"] }),
        );
        assert!(read_registered_repositories(&store, LOCAL_MACHINE_ID).is_empty());
    }

    #[test]
    fn legacy_data_migration_never_overwrites_repola_data() {
        let temp = tempfile::tempdir().expect("temp dir");
        let legacy = temp.path().join("legacy");
        let current = temp.path().join("repola");
        std::fs::create_dir_all(&legacy).expect("legacy directory");
        std::fs::create_dir_all(&current).expect("current directory");
        std::fs::write(legacy.join(STORE_FILE), "legacy settings").expect("legacy settings");
        std::fs::write(legacy.join("actions.jsonl"), "legacy audit").expect("legacy audit");
        std::fs::write(current.join(STORE_FILE), "repola settings").expect("current settings");

        migrate_known_files(&legacy, &current).expect("first migration");
        migrate_known_files(&legacy, &current).expect("idempotent migration");

        assert_eq!(
            std::fs::read_to_string(current.join(STORE_FILE)).expect("read settings"),
            "repola settings"
        );
        assert_eq!(
            std::fs::read_to_string(current.join("actions.jsonl")).expect("read audit"),
            "legacy audit"
        );
    }

    #[test]
    fn machine_store_always_includes_the_local_machine() {
        let temp = tempfile::tempdir().expect("temp dir");
        let (_app, store) = mock_store(&temp.path().join("machines.json"));

        let machines = read_machines(&store).expect("machines");
        assert_eq!(machines, vec![MachineProfile::local()]);
    }

    #[test]
    fn remote_repositories_are_stored_per_machine_without_local_filesystem_guesses() {
        let temp = tempfile::tempdir().expect("temp dir");
        let (_app, store) = mock_store(&temp.path().join("remote-roots.json"));

        let saved = write_registered_repositories(
            &store,
            "remote-id",
            false,
            vec![" /srv/code ".into(), "/srv/code".into(), r"D:\code".into()],
        )
        .expect("save remote repositories");
        assert_eq!(saved.len(), 2);
        assert_eq!(read_registered_repositories(&store, "remote-id"), saved);
        assert!(read_registered_repositories(&store, "another-machine").is_empty());
    }
}
