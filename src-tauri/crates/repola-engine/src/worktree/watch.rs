use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use notify::event::{AccessKind, AccessMode, CreateKind, EventKind, RemoveKind};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};

use super::git_metadata::metadata_line;

const DEBOUNCE: Duration = Duration::from_millis(300);
const MAX_DELAY: Duration = Duration::from_secs(2);
const MAX_WATCH_DIRECTORIES: usize = 4_096;
const MAX_SETUP_TIME: Duration = Duration::from_secs(2);
const MAX_ENUMERATION_TIME: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceWatchTarget {
    pub repository_path: String,
    pub worktree_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkspaceChangeKind {
    Files,
    Git,
    Inventory,
    Overflow,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceChangeEvent {
    pub repository_paths: Vec<String>,
    pub worktree_paths: Vec<String>,
    pub kind: WorkspaceChangeKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchCoverageError {
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetKind {
    Worktree,
    GitDir,
    CommonDir,
}

#[derive(Debug, Clone)]
struct WatchTarget {
    root: PathBuf,
    kind: TargetKind,
    owners: Vec<WorkspaceWatchTarget>,
}

#[derive(Default)]
struct ChangeSet {
    repositories: BTreeSet<String>,
    worktrees: BTreeSet<String>,
}

#[derive(Default)]
struct PendingChanges {
    changes: BTreeMap<WorkspaceChangeKind, ChangeSet>,
    first: Option<Instant>,
    last: Option<Instant>,
}

impl PendingChanges {
    fn add(&mut self, kind: WorkspaceChangeKind, owners: &[WorkspaceWatchTarget], now: Instant) {
        let changes = self.changes.entry(kind).or_default();
        for owner in owners {
            changes.repositories.insert(owner.repository_path.clone());
            changes.worktrees.insert(owner.worktree_path.clone());
        }
        self.first.get_or_insert(now);
        self.last = Some(now);
    }

    fn deadline(&self) -> Option<Instant> {
        Some((self.last? + DEBOUNCE).min(self.first? + MAX_DELAY))
    }

    fn drain(&mut self) -> Vec<WorkspaceChangeEvent> {
        self.first = None;
        self.last = None;
        std::mem::take(&mut self.changes)
            .into_iter()
            .map(|(kind, paths)| WorkspaceChangeEvent {
                repository_paths: paths.repositories.into_iter().collect(),
                worktree_paths: paths.worktrees.into_iter().collect(),
                kind,
            })
            .collect()
    }
}

/// One native watcher and one sleeping debounce worker for the entire local
/// inventory. Raw events are immediately classified into bounded dirty sets;
/// the wake channel retains at most one signal, never an event backlog.
pub struct WorkspaceWatcher {
    watcher: Option<RecommendedWatcher>,
    wake: mpsc::SyncSender<()>,
    stopped: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl WorkspaceWatcher {
    pub fn start<F>(
        requested: &[WorkspaceWatchTarget],
        on_change: F,
    ) -> Result<(Self, Vec<WatchCoverageError>), String>
    where
        F: Fn(WorkspaceChangeEvent) + Send + 'static,
    {
        let started = Instant::now();
        let (targets, candidates, mut errors) = resolve_targets(requested);
        let worktree_roots = targets
            .iter()
            .filter(|target| target.kind == TargetKind::Worktree)
            .map(|target| target.root.clone())
            .collect();
        let (roots, budget_errors) = bounded_watch_roots(
            &candidates,
            &worktree_roots,
            MAX_WATCH_DIRECTORIES,
            started + MAX_ENUMERATION_TIME,
        );
        errors.extend(budget_errors);
        let watched_directories = roots.clone();
        let mut roots: Vec<_> = roots.into_iter().collect();
        roots.sort_by_key(|root| {
            let metadata = targets.iter().any(|target| {
                target.kind != TargetKind::Worktree && root.starts_with(&target.root)
            });
            (!metadata, root.components().count())
        });
        let owners = requested.to_vec();
        let pending = Arc::new(Mutex::new(PendingChanges::default()));
        let stopped = Arc::new(AtomicBool::new(false));
        let (wake, receiver) = mpsc::sync_channel(1);
        let callback_pending = pending.clone();
        let callback_wake = wake.clone();
        let callback_stopped = stopped.clone();
        let mut watcher = notify::recommended_watcher(move |event| {
            if callback_stopped.load(Ordering::Acquire) {
                return;
            }
            let Ok(mut pending) = callback_pending.lock() else {
                return;
            };
            let was_empty = pending.first.is_none();
            classify_event_with_directories(
                &targets,
                &owners,
                &watched_directories,
                event,
                &mut pending,
            );
            if was_empty && pending.first.is_some() {
                // While dirty, the worker checks the latest quiet deadline at
                // its existing timeout. Event storms do not wake it per event.
                let _ = callback_wake.try_send(());
            }
        })
        .map_err(|error| format!("Could not create the filesystem watcher: {error}"))?;
        for root in roots {
            if started.elapsed() >= MAX_SETUP_TIME {
                errors.push(WatchCoverageError {
                    path: root.to_string_lossy().into_owned(),
                    message: "Watcher setup reached its time budget; reconciliation will cover remaining directories.".into(),
                });
                break;
            }
            if let Err(error) = watcher.watch(&root, RecursiveMode::NonRecursive) {
                errors.push(WatchCoverageError {
                    path: root.to_string_lossy().into_owned(),
                    message: format!("Could not watch this directory: {error}"),
                });
            }
        }
        let worker_stopped = stopped.clone();
        let thread = thread::Builder::new()
            .name("repola-workspace-watch".into())
            .spawn(move || debounce_loop(pending, receiver, worker_stopped, on_change))
            .map_err(|error| format!("Could not start the watcher thread: {error}"))?;
        Ok((
            Self {
                watcher: Some(watcher),
                wake,
                stopped,
                thread: Some(thread),
            },
            errors,
        ))
    }

    pub fn stop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.watcher.take();
        let _ = self.wake.try_send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for WorkspaceWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}

fn debounce_loop<F>(
    pending: Arc<Mutex<PendingChanges>>,
    receiver: mpsc::Receiver<()>,
    stopped: Arc<AtomicBool>,
    on_change: F,
) where
    F: Fn(WorkspaceChangeEvent),
{
    loop {
        if stopped.load(Ordering::Acquire) {
            return;
        }
        let (deadline, ready) = match pending.lock() {
            Ok(mut pending) => {
                let deadline = pending.deadline();
                if deadline.is_some_and(|deadline| deadline <= Instant::now()) {
                    (None, pending.drain())
                } else {
                    (deadline, Vec::new())
                }
            }
            Err(_) => return,
        };
        if !ready.is_empty() {
            for event in ready {
                if stopped.load(Ordering::Acquire) {
                    return;
                }
                on_change(event);
            }
            continue;
        }
        let signal = match deadline {
            Some(deadline) => {
                receiver.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            }
            None => receiver
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected),
        };
        match signal {
            Ok(()) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let events = match pending.lock() {
                    Ok(mut pending) => {
                        // An event may have advanced the quiet deadline while
                        // recv_timeout was expiring. The maximum still applies.
                        if pending
                            .deadline()
                            .is_some_and(|deadline| deadline <= Instant::now())
                        {
                            pending.drain()
                        } else {
                            Vec::new()
                        }
                    }
                    Err(_) => return,
                };
                for event in events {
                    if stopped.load(Ordering::Acquire) {
                        return;
                    }
                    on_change(event);
                }
            }
        }
    }
}

fn add_target(
    targets: &mut Vec<WatchTarget>,
    roots: &mut BTreeMap<PathBuf, RecursiveMode>,
    root: PathBuf,
    kind: TargetKind,
    mode: RecursiveMode,
    owner: &WorkspaceWatchTarget,
) {
    roots
        .entry(root.clone())
        .and_modify(|existing| {
            if mode == RecursiveMode::Recursive {
                *existing = mode;
            }
        })
        .or_insert(mode);
    if let Some(target) = targets
        .iter_mut()
        .find(|target| target.root == root && target.kind == kind)
    {
        if !target.owners.contains(owner) {
            target.owners.push(owner.clone());
        }
    } else {
        targets.push(WatchTarget {
            root,
            kind,
            owners: vec![owner.clone()],
        });
    }
}

fn resolve_targets(
    requested: &[WorkspaceWatchTarget],
) -> (
    Vec<WatchTarget>,
    BTreeMap<PathBuf, RecursiveMode>,
    Vec<WatchCoverageError>,
) {
    let mut targets = Vec::new();
    let mut roots = BTreeMap::new();
    let mut errors = Vec::new();
    for owner in requested {
        let root = match dunce::canonicalize(&owner.worktree_path) {
            Ok(root) if root.is_dir() => root,
            result => {
                errors.push(WatchCoverageError {
                    path: owner.worktree_path.clone(),
                    message: match result {
                        Err(error) => format!("Could not resolve the worktree: {error}"),
                        Ok(_) => "The worktree directory is unavailable.".into(),
                    },
                });
                // A missing working directory can still have live registration
                // metadata under the explicitly registered repository.
                if let Some(common) = read_git_directory(Path::new(&owner.repository_path)) {
                    add_common_targets(&mut targets, &mut roots, common, owner);
                }
                continue;
            }
        };
        add_target(
            &mut targets,
            &mut roots,
            root.clone(),
            TargetKind::Worktree,
            RecursiveMode::Recursive,
            owner,
        );
        match read_git_directory(&root) {
            Some(git_dir) => {
                add_target(
                    &mut targets,
                    &mut roots,
                    git_dir.clone(),
                    TargetKind::GitDir,
                    RecursiveMode::NonRecursive,
                    owner,
                );
                for entry in ["refs", "rebase-apply", "rebase-merge", "sequencer"] {
                    let directory = git_dir.join(entry);
                    if directory.is_dir() {
                        roots.insert(directory, RecursiveMode::Recursive);
                    }
                }
                let common = read_common_directory(&git_dir).unwrap_or(git_dir);
                add_common_targets(&mut targets, &mut roots, common, owner);
            }
            None => errors.push(WatchCoverageError {
                path: owner.worktree_path.clone(),
                message: "The worktree's Git directory is unavailable.".into(),
            }),
        }
    }
    (targets, roots, errors)
}

/// Enumerate only explicit working directories and known Git administration
/// trees. Native watches are all nonrecursive, so their resource use is capped
/// even when a repository contains a very large dependency/build tree.
fn bounded_watch_roots(
    candidates: &BTreeMap<PathBuf, RecursiveMode>,
    worktree_roots: &BTreeSet<PathBuf>,
    limit: usize,
    deadline: Instant,
) -> (BTreeSet<PathBuf>, Vec<WatchCoverageError>) {
    let mut directories = BTreeSet::new();
    let mut errors = Vec::new();
    // Give Git metadata coverage priority over potentially huge working trees.
    let mut candidates: Vec<_> = candidates.iter().collect();
    candidates.sort_by_key(|(root, mode)| {
        (
            worktree_roots.contains(*root),
            **mode == RecursiveMode::Recursive,
        )
    });
    for (root, mode) in candidates {
        let mut stack = vec![root.clone()];
        while let Some(directory) = stack.pop() {
            if directories.contains(&directory) {
                continue;
            }
            if directories.len() >= limit || Instant::now() >= deadline {
                errors.push(WatchCoverageError {
                    path: root.to_string_lossy().into_owned(),
                    message: "Watcher coverage reached its directory or time budget; reconciliation will cover remaining directories.".into(),
                });
                return (directories, errors);
            }
            directories.insert(directory.clone());
            if *mode == RecursiveMode::NonRecursive {
                continue;
            }
            let entries = match fs::read_dir(&directory) {
                Ok(entries) => entries,
                Err(error) => {
                    errors.push(WatchCoverageError {
                        path: directory.to_string_lossy().into_owned(),
                        message: format!("Could not enumerate watcher directories: {error}"),
                    });
                    continue;
                }
            };
            for entry in entries {
                if Instant::now() >= deadline || directories.len() + stack.len() >= limit {
                    errors.push(WatchCoverageError {
                        path: root.to_string_lossy().into_owned(),
                        message: "Watcher coverage reached its directory or time budget; reconciliation will cover remaining directories.".into(),
                    });
                    return (directories, errors);
                }
                match entry {
                    Ok(entry) => match entry.file_type() {
                        Ok(kind) if kind.is_dir() && entry.file_name() != ".git" => {
                            let path = entry.path();
                            if !directories.contains(&path) {
                                stack.push(path);
                            }
                        }
                        Ok(_) => {}
                        Err(error) => errors.push(WatchCoverageError {
                            path: entry.path().to_string_lossy().into_owned(),
                            message: format!("Could not inspect watcher directory: {error}"),
                        }),
                    },
                    Err(error) => errors.push(WatchCoverageError {
                        path: directory.to_string_lossy().into_owned(),
                        message: format!("Could not enumerate watcher directory: {error}"),
                    }),
                }
            }
        }
    }
    (directories, errors)
}

fn add_common_targets(
    targets: &mut Vec<WatchTarget>,
    roots: &mut BTreeMap<PathBuf, RecursiveMode>,
    common: PathBuf,
    owner: &WorkspaceWatchTarget,
) {
    add_target(
        targets,
        roots,
        common.clone(),
        TargetKind::CommonDir,
        RecursiveMode::NonRecursive,
        owner,
    );
    for entry in ["refs", "worktrees", "info", "reftable"] {
        let directory = common.join(entry);
        if directory.is_dir() {
            // These targets use the common-directory classifier at their parent.
            roots.insert(directory, RecursiveMode::Recursive);
        }
    }
}

fn read_git_directory(root: &Path) -> Option<PathBuf> {
    let entry = root.join(".git");
    if entry.is_dir() {
        return dunce::canonicalize(entry).ok();
    }
    let text = read_small_metadata(&entry)?;
    let path = PathBuf::from(metadata_line(&text).strip_prefix("gitdir: ")?);
    dunce::canonicalize(if path.is_absolute() {
        path
    } else {
        root.join(path)
    })
    .ok()
}

fn read_common_directory(git_dir: &Path) -> Option<PathBuf> {
    let text = read_small_metadata(&git_dir.join("commondir"))?;
    let path = PathBuf::from(metadata_line(&text));
    dunce::canonicalize(if path.is_absolute() {
        path
    } else {
        git_dir.join(path)
    })
    .ok()
}

fn read_small_metadata(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut text = String::new();
    fs::File::open(path)
        .ok()?
        .take(32 * 1024 + 1)
        .read_to_string(&mut text)
        .ok()?;
    (text.len() <= 32 * 1024).then_some(text)
}

#[cfg(test)]
fn classify_event(
    targets: &[WatchTarget],
    owners: &[WorkspaceWatchTarget],
    event: notify::Result<notify::Event>,
    pending: &mut PendingChanges,
) {
    classify_event_with_directories(targets, owners, &BTreeSet::new(), event, pending);
}

fn classify_event_with_directories(
    targets: &[WatchTarget],
    owners: &[WorkspaceWatchTarget],
    watched_directories: &BTreeSet<PathBuf>,
    event: notify::Result<notify::Event>,
    pending: &mut PendingChanges,
) {
    let now = Instant::now();
    let event = match event {
        Ok(event) => event,
        Err(_) => {
            pending.add(WorkspaceChangeKind::Overflow, owners, now);
            return;
        }
    };
    if matches!(event.kind, EventKind::Access(_))
        && !matches!(
            event.kind,
            EventKind::Access(AccessKind::Close(AccessMode::Write))
        )
    {
        return;
    }
    if event.paths.is_empty() || event.need_rescan() {
        pending.add(WorkspaceChangeKind::Overflow, owners, now);
        return;
    }
    for path in &event.paths {
        let longest = targets
            .iter()
            .filter(|target| path.starts_with(&target.root))
            .map(|target| target.root.as_os_str().len())
            .max();
        let mut matched = false;
        for target in targets.iter().filter(|target| {
            Some(target.root.as_os_str().len()) == longest && path.starts_with(&target.root)
        }) {
            matched = true;
            if let Some(kind) = classify_path(target, path) {
                let directory_topology = matches!(
                    event.kind,
                    EventKind::Create(CreateKind::Folder) | EventKind::Remove(RemoveKind::Folder)
                ) || watched_directories.contains(path)
                    || fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir());
                pending.add(
                    if directory_topology {
                        WorkspaceChangeKind::Inventory
                    } else {
                        kind
                    },
                    &target.owners,
                    now,
                );
            }
        }
        if !matched {
            pending.add(WorkspaceChangeKind::Overflow, owners, now);
        }
    }
}

fn classify_path(target: &WatchTarget, path: &Path) -> Option<WorkspaceChangeKind> {
    let relative = path.strip_prefix(&target.root).ok()?;
    let components = normal_components(relative);
    match target.kind {
        TargetKind::Worktree if components.is_empty() => Some(WorkspaceChangeKind::Inventory),
        TargetKind::Worktree if components.first().is_some_and(|name| *name != ".git") => Some(
            if components
                .last()
                .is_some_and(|name| *name == ".gitattributes")
            {
                WorkspaceChangeKind::Git
            } else {
                WorkspaceChangeKind::Files
            },
        ),
        TargetKind::Worktree => classify_git_entry(&components[1..]),
        TargetKind::GitDir => classify_git_entry(&components),
        TargetKind::CommonDir => match components.first().copied() {
            Some(
                "refs" | "packed-refs" | "config" | "info" | "worktrees" | "reftable" | "shallow",
            )
            | None => classify_git_entry(&components),
            _ => None,
        },
    }
}

fn classify_git_entry(components: &[&str]) -> Option<WorkspaceChangeKind> {
    let Some((head, tail)) = components.split_first() else {
        return Some(WorkspaceChangeKind::Inventory);
    };
    if components.last().is_some_and(|name| {
        Path::new(name)
            .extension()
            .is_some_and(|extension| extension == "lock")
    }) {
        return None;
    }
    match *head {
        "index" if tail.is_empty() => Some(WorkspaceChangeKind::Files),
        "worktrees" => Some(WorkspaceChangeKind::Inventory),
        "commondir" | "gitdir" | "locked" => Some(WorkspaceChangeKind::Inventory),
        "config" | "config.worktree" | "info" | "reftable" | "shallow" => {
            Some(WorkspaceChangeKind::Git)
        }
        _ if is_relevant_git_entry(components) => Some(WorkspaceChangeKind::Git),
        ".git" if tail.is_empty() => Some(WorkspaceChangeKind::Inventory),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeChangeEvent {
    pub worktree_path: String,
}

/// Compatibility adapter for a single selected worktree. Workspace consumers
/// use WorkspaceWatcher directly so adding trees does not add worker threads.
pub struct WorktreeWatcher(WorkspaceWatcher);

impl WorktreeWatcher {
    pub fn start<F>(worktree_path: &str, on_change: F) -> Result<Self, String>
    where
        F: Fn(WorktreeChangeEvent) + Send + 'static,
    {
        let path = dunce::canonicalize(worktree_path)
            .map_err(|error| format!("Could not resolve the worktree path: {error}"))?;
        if !path.is_dir() {
            return Err("The worktree path is not a directory.".into());
        }
        let path = path.to_string_lossy().into_owned();
        let target = WorkspaceWatchTarget {
            repository_path: path.clone(),
            worktree_path: path.clone(),
        };
        let (watcher, _) = WorkspaceWatcher::start(&[target], move |_| {
            on_change(WorktreeChangeEvent {
                worktree_path: path.clone(),
            });
        })?;
        Ok(Self(watcher))
    }

    pub fn stop(&mut self) {
        self.0.stop();
    }
}
fn normal_components(path: &Path) -> Vec<&str> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect()
}

/// The shared rule for entries relative to a Git directory: only the files
/// that describe HEAD, the index, refs, and in-progress operations count;
/// `objects/**`, lock files, logs, hooks, config and other worktrees are noise.
fn is_relevant_git_entry(components: &[&str]) -> bool {
    let Some((head, tail)) = components.split_first() else {
        return false;
    };
    if components
        .last()
        .is_some_and(|name| Path::new(name).extension().is_some_and(|ext| ext == "lock"))
    {
        return false;
    }
    match *head {
        "HEAD" | "ORIG_HEAD" | "index" | "packed-refs" | "MERGE_HEAD" | "REBASE_HEAD"
        | "CHERRY_PICK_HEAD" | "REVERT_HEAD" | "BISECT_LOG" => tail.is_empty(),
        "refs" | "rebase-merge" | "rebase-apply" | "sequencer" => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner(name: &str) -> WorkspaceWatchTarget {
        WorkspaceWatchTarget {
            repository_path: "repo".into(),
            worktree_path: name.into(),
        }
    }

    fn target(root: &str, kind: TargetKind, owners: Vec<WorkspaceWatchTarget>) -> WatchTarget {
        WatchTarget {
            root: PathBuf::from(root),
            kind,
            owners,
        }
    }

    fn modified(path: &str) -> notify::Result<notify::Event> {
        Ok(
            notify::Event::new(EventKind::Modify(notify::event::ModifyKind::Any))
                .add_path(PathBuf::from(path)),
        )
    }

    #[test]
    fn working_files_and_git_noise_are_classified_separately() {
        let target = target("repo", TargetKind::Worktree, vec![owner("repo")]);
        assert_eq!(
            classify_path(&target, Path::new("repo/src/file.lock")),
            Some(WorkspaceChangeKind::Files)
        );
        assert_eq!(
            classify_path(&target, Path::new("repo/.git/index")),
            Some(WorkspaceChangeKind::Files)
        );
        assert_eq!(
            classify_path(&target, Path::new("repo/nested/.gitattributes")),
            Some(WorkspaceChangeKind::Git)
        );
        assert_eq!(
            classify_path(&target, Path::new("repo/.git/info/attributes")),
            Some(WorkspaceChangeKind::Git)
        );
        assert_eq!(
            classify_path(&target, Path::new("repo/.git/config")),
            Some(WorkspaceChangeKind::Git)
        );
        assert_eq!(
            classify_path(&target, Path::new("repo/.git/info/exclude")),
            Some(WorkspaceChangeKind::Git)
        );
        assert_eq!(
            classify_path(&target, Path::new("repo/.git/worktrees/new/gitdir")),
            Some(WorkspaceChangeKind::Inventory)
        );
        for path in [
            "repo/.git/objects/ab/cd",
            "repo/.git/logs/HEAD",
            "repo/.git/index.lock",
        ] {
            assert_eq!(classify_path(&target, Path::new(path)), None);
        }
    }

    #[test]
    fn shared_refs_refresh_siblings_but_a_linked_index_only_refreshes_its_owner() {
        let owners = vec![owner("repo"), owner("linked")];
        let targets = vec![
            target("repo", TargetKind::Worktree, vec![owners[0].clone()]),
            target("repo/.git", TargetKind::GitDir, vec![owners[0].clone()]),
            target("repo/.git", TargetKind::CommonDir, owners.clone()),
            target(
                "repo/.git/worktrees/linked",
                TargetKind::GitDir,
                vec![owners[1].clone()],
            ),
        ];
        let mut pending = PendingChanges::default();
        classify_event(
            &targets,
            &owners,
            modified("repo/.git/refs/heads/main"),
            &mut pending,
        );
        let events = pending.drain();
        assert_eq!(events[0].worktree_paths, ["linked", "repo"]);
        assert_eq!(events[0].kind, WorkspaceChangeKind::Git);
        classify_event(
            &targets,
            &owners,
            modified("repo/.git/worktrees/linked/index"),
            &mut pending,
        );
        let events = pending.drain();
        assert_eq!(events[0].worktree_paths, ["linked"]);
        assert_eq!(events[0].kind, WorkspaceChangeKind::Files);
        classify_event(&targets, &owners, modified("repo/.git/index"), &mut pending);
        let events = pending.drain();
        assert_eq!(events[0].worktree_paths, ["repo"]);
        assert_eq!(events[0].kind, WorkspaceChangeKind::Files);
    }

    #[test]
    fn event_storm_retains_only_dirty_identities_and_obeys_the_maximum_delay() {
        let mut pending = PendingChanges::default();
        let owners = vec![owner("repo")];
        let first = Instant::now();
        for milliseconds in 0..10_000 {
            pending.add(
                WorkspaceChangeKind::Files,
                &owners,
                first + Duration::from_millis(milliseconds),
            );
        }
        assert_eq!(pending.deadline(), Some(first + MAX_DELAY));
        let events = pending.drain();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].worktree_paths, ["repo"]);
        assert_eq!(pending.deadline(), None);
    }

    #[test]
    fn overdue_changes_flush_even_with_a_queued_wake_signal() {
        let pending = Arc::new(Mutex::new(PendingChanges::default()));
        {
            let mut changes = pending.lock().unwrap();
            changes.add(
                WorkspaceChangeKind::Files,
                &[owner("repo")],
                Instant::now() - MAX_DELAY,
            );
            changes.last = Some(Instant::now());
        }
        let stopped = Arc::new(AtomicBool::new(false));
        let (wake, receiver) = mpsc::sync_channel(1);
        wake.try_send(()).unwrap();
        let (emitted, received) = mpsc::channel();
        let worker_stopped = stopped.clone();
        let worker = thread::spawn(move || {
            debounce_loop(pending, receiver, worker_stopped, move |event| {
                emitted.send(event).unwrap();
            })
        });
        let result = received.recv_timeout(Duration::from_secs(2));
        stopped.store(true, Ordering::Release);
        let _ = wake.try_send(());
        worker.join().unwrap();
        assert_eq!(result.unwrap().worktree_paths, ["repo"]);
    }

    #[test]
    fn watcher_errors_and_rescan_notices_invalidate_all_known_owners() {
        let owners = vec![owner("repo"), owner("linked")];
        let mut pending = PendingChanges::default();
        classify_event(
            &[],
            &owners,
            Err(notify::Error::generic("overflow")),
            &mut pending,
        );
        let event = pending.drain().pop().unwrap();
        assert_eq!(event.kind, WorkspaceChangeKind::Overflow);
        assert_eq!(event.worktree_paths.len(), 2);
        classify_event(
            &[],
            &owners,
            Ok(notify::Event::new(EventKind::Other)),
            &mut pending,
        );
        assert_eq!(pending.drain()[0].kind, WorkspaceChangeKind::Overflow);
    }

    #[test]
    fn access_events_are_ignored_and_metadata_resolution_spawns_no_git_processes() {
        let directory = tempfile::tempdir().unwrap();
        let primary = directory.path().join("primary");
        let linked = directory.path().join("linked");
        let git_dir = primary.join(".git");
        let admin_dir = git_dir.join("worktrees").join("linked");
        fs::create_dir_all(&admin_dir).unwrap();
        fs::create_dir_all(git_dir.join("refs")).unwrap();
        fs::create_dir_all(&linked).unwrap();
        fs::write(
            linked.join(".git"),
            format!("gitdir: {}\n", admin_dir.display()),
        )
        .unwrap();
        fs::write(admin_dir.join("commondir"), "../..\n").unwrap();
        let owners = vec![
            WorkspaceWatchTarget {
                repository_path: primary.to_string_lossy().into_owned(),
                worktree_path: primary.to_string_lossy().into_owned(),
            },
            WorkspaceWatchTarget {
                repository_path: primary.to_string_lossy().into_owned(),
                worktree_path: linked.to_string_lossy().into_owned(),
            },
        ];
        let (targets, roots, errors) = resolve_targets(&owners);
        assert!(errors.is_empty());
        let worktree_roots = targets
            .iter()
            .filter(|target| target.kind == TargetKind::Worktree)
            .map(|target| target.root.clone())
            .collect();
        let (directories, errors) = bounded_watch_roots(
            &roots,
            &worktree_roots,
            MAX_WATCH_DIRECTORIES,
            Instant::now() + MAX_SETUP_TIME,
        );
        assert!(errors.is_empty());
        assert!(directories.contains(&dunce::canonicalize(&primary).unwrap()));
        assert!(directories.contains(&dunce::canonicalize(&linked).unwrap()));
        assert!(targets
            .iter()
            .any(|target| target.kind == TargetKind::CommonDir && target.owners.len() == 2));
        let mut pending = PendingChanges::default();
        classify_event(
            &targets,
            &owners,
            Ok(notify::Event::new(EventKind::Access(AccessKind::Read))
                .add_path(primary.join("file"))),
            &mut pending,
        );
        assert!(pending.drain().is_empty());
    }

    #[test]
    fn native_directory_registration_is_bounded_and_excludes_git_objects() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repo");
        let git_dir = root.join(".git");
        fs::create_dir_all(git_dir.join("objects/pack/deep")).unwrap();
        fs::create_dir_all(git_dir.join("refs/heads")).unwrap();
        for index in 0..20 {
            fs::create_dir_all(root.join(format!("dependencies/{index}/nested"))).unwrap();
        }
        let owners = [WorkspaceWatchTarget {
            repository_path: root.to_string_lossy().into_owned(),
            worktree_path: root.to_string_lossy().into_owned(),
        }];
        let (targets, candidates, _) = resolve_targets(&owners);
        let worktree_roots = targets
            .iter()
            .filter(|target| target.kind == TargetKind::Worktree)
            .map(|target| target.root.clone())
            .collect();
        let (bounded, errors) = bounded_watch_roots(
            &candidates,
            &worktree_roots,
            8,
            Instant::now() + MAX_SETUP_TIME,
        );
        assert!(bounded.len() <= 8);
        assert!(!errors.is_empty());
        assert!(bounded.contains(&dunce::canonicalize(&git_dir).unwrap()));
        let (all, errors) = bounded_watch_roots(
            &candidates,
            &worktree_roots,
            MAX_WATCH_DIRECTORIES,
            Instant::now() + MAX_SETUP_TIME,
        );
        assert!(errors.is_empty());
        let objects = dunce::canonicalize(git_dir.join("objects")).unwrap();
        assert!(!all.iter().any(|path| path.starts_with(&objects)));
        let (expired, errors) = bounded_watch_roots(
            &candidates,
            &worktree_roots,
            MAX_WATCH_DIRECTORIES,
            Instant::now(),
        );
        assert!(expired.is_empty());
        assert!(!errors.is_empty());
    }

    #[test]
    fn folder_creation_and_removal_request_a_subscription_rebuild() {
        let owners = [owner("repo")];
        let targets = [target("repo", TargetKind::Worktree, owners.to_vec())];
        let mut pending = PendingChanges::default();
        classify_event(
            &targets,
            &owners,
            Ok(notify::Event::new(EventKind::Create(CreateKind::Folder))
                .add_path(PathBuf::from("repo/new"))),
            &mut pending,
        );
        assert_eq!(pending.drain()[0].kind, WorkspaceChangeKind::Inventory);
        let directories = BTreeSet::from([PathBuf::from("repo/removed")]);
        classify_event_with_directories(
            &targets,
            &owners,
            &directories,
            modified("repo/removed"),
            &mut pending,
        );
        assert_eq!(pending.drain()[0].kind, WorkspaceChangeKind::Inventory);
    }

    #[cfg(unix)]
    #[test]
    fn directory_enumeration_does_not_follow_symlinks() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repo");
        let outside = directory.path().join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(outside.join("deep")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        let candidates = BTreeMap::from([(root.clone(), RecursiveMode::Recursive)]);
        let (roots, errors) = bounded_watch_roots(
            &candidates,
            &BTreeSet::from([root.clone()]),
            MAX_WATCH_DIRECTORIES,
            Instant::now() + MAX_SETUP_TIME,
        );
        assert!(errors.is_empty());
        assert_eq!(roots, BTreeSet::from([root]));
    }

    fn receive_kind(
        receiver: &mpsc::Receiver<WorkspaceChangeEvent>,
        kind: WorkspaceChangeKind,
        worktree_path: &str,
    ) -> WorkspaceChangeEvent {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let event = receiver
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("The native watcher should report the filesystem change");
            if event.kind == kind
                && event
                    .worktree_paths
                    .iter()
                    .any(|path| path == worktree_path)
            {
                return event;
            }
        }
    }

    #[test]
    fn native_watch_reports_files_shared_refs_and_rebuilt_directory_coverage() {
        let directory = tempfile::tempdir().unwrap();
        let primary = directory.path().join("primary source");
        let linked = directory.path().join("linked source");
        let git_dir = primary.join(".git");
        let admin_dir = git_dir.join("worktrees/linked");
        fs::create_dir_all(&admin_dir).unwrap();
        fs::create_dir_all(git_dir.join("refs/heads")).unwrap();
        fs::create_dir_all(&linked).unwrap();
        fs::write(
            linked.join(".git"),
            format!("gitdir: {}\n", admin_dir.display()),
        )
        .unwrap();
        fs::write(admin_dir.join("commondir"), "../..\n").unwrap();
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(git_dir.join("refs/heads/main"), "first\n").unwrap();
        fs::write(primary.join("tracked.txt"), "first").unwrap();
        let primary_path = dunce::canonicalize(&primary)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let linked_path = dunce::canonicalize(&linked)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let targets = [
            WorkspaceWatchTarget {
                repository_path: primary_path.clone(),
                worktree_path: primary_path.clone(),
            },
            WorkspaceWatchTarget {
                repository_path: primary_path.clone(),
                worktree_path: linked_path.clone(),
            },
        ];
        let (sender, receiver) = mpsc::channel();
        let (mut watcher, errors) = WorkspaceWatcher::start(&targets, move |event| {
            let _ = sender.send(event);
        })
        .unwrap();
        assert!(errors.is_empty(), "{errors:?}");

        fs::write(primary.join("tracked.txt"), "second").unwrap();
        let files = receive_kind(&receiver, WorkspaceChangeKind::Files, &primary_path);
        assert_eq!(files.worktree_paths, std::slice::from_ref(&primary_path));
        fs::write(git_dir.join("refs/heads/main"), "second\n").unwrap();
        let refs = receive_kind(&receiver, WorkspaceChangeKind::Git, &linked_path);
        assert!(refs.worktree_paths.contains(&primary_path));
        assert!(refs.worktree_paths.contains(&linked_path));

        let nested = linked.join("new-directory");
        fs::create_dir(&nested).unwrap();
        receive_kind(&receiver, WorkspaceChangeKind::Inventory, &linked_path);
        watcher.stop();
        fs::write(nested.join("tracked.txt"), "first").unwrap();
        let (sender, receiver) = mpsc::channel();
        let (mut replacement, errors) = WorkspaceWatcher::start(&targets, move |event| {
            let _ = sender.send(event);
        })
        .unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        fs::write(nested.join("tracked.txt"), "second").unwrap();
        let nested_edit = receive_kind(&receiver, WorkspaceChangeKind::Files, &linked_path);
        assert_eq!(nested_edit.worktree_paths, [linked_path]);
        replacement.stop();
    }

    #[test]
    fn missing_tree_reports_degraded_coverage_without_failing_other_watches() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory
            .path()
            .join("missing")
            .to_string_lossy()
            .into_owned();
        let targets = [WorkspaceWatchTarget {
            repository_path: path.clone(),
            worktree_path: path,
        }];
        let (mut watcher, errors) = WorkspaceWatcher::start(&targets, |_| {}).unwrap();
        assert_eq!(errors.len(), 1);
        watcher.stop();
        watcher.stop();
    }
}
