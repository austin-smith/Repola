use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use notify::event::{AccessKind, AccessMode, EventKind};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use super::command;

/// Quiet period that must elapse after the last relevant filesystem event
/// before the change callback fires.
const DEBOUNCE: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeChangeEvent {
    pub worktree_path: String,
}

/// How paths under a watched directory are classified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetKind {
    /// A working tree: everything counts except noise inside its `.git` entry.
    Worktree,
    /// A bare Git directory (linked-worktree gitdir or common dir): only
    /// status-affecting metadata counts.
    GitDir,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WatchTarget {
    root: PathBuf,
    kind: TargetKind,
}

/// Watches a worktree root recursively (plus its Git directory when that
/// lives elsewhere, as in linked worktrees) and reports debounced changes.
///
/// Dropping the watcher (or calling [`WorktreeWatcher::stop`]) tears down the
/// OS watches and the debounce thread.
pub struct WorktreeWatcher {
    watcher: Option<RecommendedWatcher>,
    stop: mpsc::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl WorktreeWatcher {
    pub fn start<F>(worktree_path: &str, on_change: F) -> Result<Self, String>
    where
        F: Fn(WorktreeChangeEvent) + Send + 'static,
    {
        let root = dunce::canonicalize(worktree_path)
            .map_err(|error| format!("Could not resolve the worktree path: {error}"))?;
        if !root.is_dir() {
            return Err("The worktree path is not a directory.".into());
        }
        let (events_tx, events_rx) = mpsc::channel::<notify::Result<notify::Event>>();
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = events_tx.send(event);
        })
        .map_err(|error| format!("Could not create the filesystem watcher: {error}"))?;
        watcher
            .watch(&root, RecursiveMode::Recursive)
            .map_err(|error| format!("Could not watch the worktree: {error}"))?;
        let mut targets = vec![WatchTarget {
            root: root.clone(),
            kind: TargetKind::Worktree,
        }];

        // Linked worktrees keep HEAD/index under `<common>/.git/worktrees/<name>`,
        // outside the working tree; watch that directory (and the shared refs)
        // too. Failure here degrades to the working-tree-only watch.
        for (directory, mode) in external_git_directories(&root) {
            if watcher.watch(&directory, mode).is_ok() {
                targets.push(WatchTarget {
                    root: directory,
                    kind: TargetKind::GitDir,
                });
            }
        }

        let event = WorktreeChangeEvent {
            worktree_path: root.to_string_lossy().into_owned(),
        };
        let thread = thread::Builder::new()
            .name("repola-worktree-watch".into())
            .spawn(move || debounce_loop(&targets, &events_rx, &stop_rx, &event, &on_change))
            .map_err(|error| format!("Could not start the watcher thread: {error}"))?;
        Ok(Self {
            watcher: Some(watcher),
            stop: stop_tx,
            thread: Some(thread),
        })
    }

    pub fn stop(&mut self) {
        // Dropping the OS watcher closes the event channel; the stop signal
        // wakes the debounce thread even if it is mid-quiet-period.
        self.watcher.take();
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for WorktreeWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Resolves the Git directories that live outside `root` and must be watched
/// separately: the worktree's own gitdir, and — for linked worktrees — the
/// common dir's `refs/` tree and `packed-refs` (branch updates from siblings).
fn external_git_directories(root: &Path) -> Vec<(PathBuf, RecursiveMode)> {
    let Some(git_dir) = resolve_git_dir(root) else {
        return Vec::new();
    };
    if git_dir.starts_with(root) {
        return Vec::new();
    }
    let mut directories = vec![(git_dir.clone(), RecursiveMode::Recursive)];
    let is_linked = git_dir
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name == "worktrees");
    if is_linked {
        if let Some(common) = git_dir.parent().and_then(Path::parent) {
            if !common.starts_with(root) {
                // Non-recursive on the common dir picks up `packed-refs`
                // without subscribing to `objects/**`.
                directories.push((common.to_path_buf(), RecursiveMode::NonRecursive));
                directories.push((common.join("refs"), RecursiveMode::Recursive));
            }
        }
    }
    directories
}

fn resolve_git_dir(root: &Path) -> Option<PathBuf> {
    let output = command::git_at(root, ["rev-parse", "--absolute-git-dir"]).ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    dunce::canonicalize(text).ok()
}

fn debounce_loop<F>(
    targets: &[WatchTarget],
    events: &mpsc::Receiver<notify::Result<notify::Event>>,
    stop: &mpsc::Receiver<()>,
    event: &WorktreeChangeEvent,
    on_change: &F,
) where
    F: Fn(WorktreeChangeEvent),
{
    let mut pending: Option<Instant> = None;
    loop {
        if stop.try_recv().is_ok() {
            return;
        }
        let timeout = match pending {
            Some(since) => DEBOUNCE.saturating_sub(since.elapsed()),
            None => Duration::from_millis(250),
        };
        match events.recv_timeout(timeout) {
            Ok(Ok(fs_event)) => {
                if is_relevant_event(targets, &fs_event) {
                    pending = Some(Instant::now());
                }
            }
            Ok(Err(_)) => {
                // Watcher errors (for example overflow) may hide changes; refresh.
                pending = Some(Instant::now());
            }
            Err(RecvTimeoutError::Timeout) => {
                if pending.is_some_and(|since| since.elapsed() >= DEBOUNCE) {
                    pending = None;
                    on_change(event.clone());
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn is_relevant_event(targets: &[WatchTarget], event: &notify::Event) -> bool {
    match event.kind {
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => {}
        EventKind::Access(_) => return false,
        _ => {}
    }
    if event.paths.is_empty() {
        // Rescan / overflow notices carry no paths; treat them as changes.
        return true;
    }
    event.paths.iter().any(|path| {
        // The most specific target wins: a linked gitdir also sits under the
        // common dir, and both must use the gitdir rule.
        let target = targets
            .iter()
            .filter(|target| path.starts_with(&target.root))
            .max_by_key(|target| target.root.as_os_str().len());
        match target {
            Some(WatchTarget {
                root,
                kind: TargetKind::Worktree,
            }) => is_relevant_path(root, path),
            Some(WatchTarget {
                root,
                kind: TargetKind::GitDir,
            }) => is_relevant_git_dir_path(root, path),
            // Unknown origin: refresh rather than risk dropping a change.
            None => true,
        }
    })
}

fn normal_components(path: &Path) -> Vec<&str> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect()
}

/// Decides whether a changed path inside a working tree `root` should refresh
/// the changes pane. Everything outside `.git` counts; inside `.git` the
/// gitdir rule applies.
pub fn is_relevant_path(root: &Path, path: &Path) -> bool {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let components = normal_components(relative);
    let Some((first, rest)) = components.split_first() else {
        return true;
    };
    if *first != ".git" {
        return true;
    }
    if rest.is_empty() {
        // The `.git` entry itself (a gitdir pointer file in linked worktrees).
        return true;
    }
    is_relevant_git_entry(rest)
}

/// Decides whether a changed path inside a Git directory `git_dir` (the
/// worktree's gitdir or the common dir) should refresh the changes pane.
pub fn is_relevant_git_dir_path(git_dir: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(git_dir) else {
        return true;
    };
    let components = normal_components(relative);
    if components.is_empty() {
        return false;
    }
    is_relevant_git_entry(&components)
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

    fn relevant(relative: &str) -> bool {
        let root = Path::new("/repo");
        is_relevant_path(root, &root.join(relative))
    }

    #[test]
    fn working_tree_files_are_relevant() {
        assert!(relevant("src/main.rs"));
        assert!(relevant("README.md"));
        assert!(relevant("nested/deep/file.lock"));
        assert!(relevant(".gitignore"));
        assert!(relevant(".git"));
    }

    #[test]
    fn git_metadata_that_affects_status_is_relevant() {
        for path in [
            ".git/HEAD",
            ".git/ORIG_HEAD",
            ".git/index",
            ".git/packed-refs",
            ".git/MERGE_HEAD",
            ".git/REBASE_HEAD",
            ".git/CHERRY_PICK_HEAD",
            ".git/REVERT_HEAD",
            ".git/BISECT_LOG",
            ".git/refs/heads/main",
            ".git/refs/remotes/origin/main",
            ".git/rebase-merge/done",
            ".git/rebase-apply/next",
            ".git/sequencer/todo",
        ] {
            assert!(relevant(path), "{path} should be relevant");
        }
    }

    #[test]
    fn git_noise_is_ignored() {
        for path in [
            ".git/objects/ab/cdef",
            ".git/objects/pack/pack-1.idx",
            ".git/index.lock",
            ".git/HEAD.lock",
            ".git/refs/heads/main.lock",
            ".git/packed-refs.lock",
            ".git/logs/HEAD",
            ".git/config",
            ".git/FETCH_HEAD",
            ".git/COMMIT_EDITMSG",
            ".git/hooks/pre-commit",
            ".git/info/exclude",
            ".git/worktrees/other/HEAD",
            ".git/HEAD/unexpected",
        ] {
            assert!(!relevant(path), "{path} should be ignored");
        }
    }

    #[test]
    fn unclassifiable_paths_default_to_refreshing() {
        // A path outside the root cannot be matched against the `.git` rules,
        // so it is treated as a change rather than silently dropped.
        assert!(is_relevant_path(
            Path::new("/repo"),
            Path::new("/elsewhere/.git/objects/x")
        ));
        assert!(is_relevant_path(Path::new("/repo"), Path::new("/repo")));
    }

    #[test]
    fn linked_worktree_git_dir_uses_the_git_rules_at_its_root() {
        let git_dir = Path::new("/repo/.git/worktrees/feature");
        let relevant = |relative: &str| is_relevant_git_dir_path(git_dir, &git_dir.join(relative));
        for path in [
            "HEAD",
            "ORIG_HEAD",
            "index",
            "MERGE_HEAD",
            "REBASE_HEAD",
            "CHERRY_PICK_HEAD",
            "REVERT_HEAD",
            "BISECT_LOG",
            "packed-refs",
            "refs/heads/feature",
            "rebase-merge/done",
            "rebase-apply/next",
            "sequencer/todo",
        ] {
            assert!(relevant(path), "{path} should be relevant");
        }
        for path in [
            "index.lock",
            "HEAD.lock",
            "objects/ab/cdef",
            "logs/HEAD",
            "commondir",
            "gitdir",
            "COMMIT_EDITMSG",
            "HEAD/unexpected",
        ] {
            assert!(!relevant(path), "{path} should be ignored");
        }
        // The directory itself is not a change; paths outside it default to refreshing.
        assert!(!is_relevant_git_dir_path(git_dir, git_dir));
        assert!(is_relevant_git_dir_path(git_dir, Path::new("/elsewhere/x")));
    }

    #[test]
    fn common_dir_refs_are_relevant_but_objects_are_not() {
        let common = Path::new("/repo/.git");
        assert!(is_relevant_git_dir_path(
            common,
            &common.join("packed-refs")
        ));
        assert!(is_relevant_git_dir_path(
            common,
            &common.join("refs/heads/main")
        ));
        assert!(!is_relevant_git_dir_path(
            common,
            &common.join("objects/ab/cd")
        ));
        assert!(!is_relevant_git_dir_path(
            common,
            &common.join("packed-refs.lock")
        ));
    }

    fn targets() -> Vec<WatchTarget> {
        vec![
            WatchTarget {
                root: PathBuf::from("/work/feature"),
                kind: TargetKind::Worktree,
            },
            WatchTarget {
                root: PathBuf::from("/repo/.git/worktrees/feature"),
                kind: TargetKind::GitDir,
            },
            WatchTarget {
                root: PathBuf::from("/repo/.git"),
                kind: TargetKind::GitDir,
            },
        ]
    }

    fn modify(path: &str) -> notify::Event {
        let mut event = notify::Event::new(EventKind::Modify(notify::event::ModifyKind::Any));
        event.paths.push(PathBuf::from(path));
        event
    }

    #[test]
    fn events_are_classified_by_their_most_specific_target() {
        let targets = targets();
        assert!(is_relevant_event(
            &targets,
            &modify("/work/feature/src/main.rs")
        ));
        assert!(is_relevant_event(
            &targets,
            &modify("/repo/.git/worktrees/feature/index")
        ));
        assert!(!is_relevant_event(
            &targets,
            &modify("/repo/.git/worktrees/feature/index.lock")
        ));
        assert!(is_relevant_event(
            &targets,
            &modify("/repo/.git/refs/heads/main")
        ));
        assert!(is_relevant_event(
            &targets,
            &modify("/repo/.git/packed-refs")
        ));
        assert!(!is_relevant_event(
            &targets,
            &modify("/repo/.git/objects/ab/cd")
        ));
        // Another worktree's gitdir under the common dir is noise for this one.
        assert!(!is_relevant_event(
            &targets,
            &modify("/repo/.git/worktrees/other/index")
        ));
        assert!(is_relevant_event(&targets, &modify("/unrelated/file")));
    }

    #[test]
    fn access_events_other_than_close_write_are_dropped() {
        let targets = vec![WatchTarget {
            root: PathBuf::from("/repo"),
            kind: TargetKind::Worktree,
        }];
        let mut event = notify::Event::new(EventKind::Access(AccessKind::Read));
        event.paths.push(PathBuf::from("/repo/src/main.rs"));
        assert!(!is_relevant_event(&targets, &event));
        let mut event = notify::Event::new(EventKind::Access(AccessKind::Close(AccessMode::Write)));
        event.paths.push(PathBuf::from("/repo/src/main.rs"));
        assert!(is_relevant_event(&targets, &event));
        assert!(!is_relevant_event(
            &targets,
            &modify("/repo/.git/objects/ab/cd")
        ));
        assert!(is_relevant_event(
            &targets,
            &notify::Event::new(EventKind::Other)
        ));
    }
}
