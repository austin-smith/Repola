//! Discarding working-copy changes. A discard is planned first; execution
//! repeats the plan, refuses unless the working copy still matches the plan's
//! fingerprint, and saves everything it will change as a recovery point
//! before Git touches anything.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::Path;

use super::command;
use super::models::{
    DiscardEffect, DiscardPlan, DiscardPlanEntry, DiscardPlanRequest, DiscardRequest,
    DiscardResult, DiscardScope, DiscardTarget, FileChange, FileChangeKind, GitPath, KeptChange,
    KeptChangeReason, RecoveryPointKind, WorkingCopyRequest, WorkingCopySnapshot,
};
use super::recovery::{
    fingerprint, observe_index, observe_paths, remove_worktree_entry, store_recovery_point,
    stored_bytes, PathState,
};
use super::working_copy::{decode_path_token_bytes, ensure_success, working_copy_snapshot};

const STALE_PLAN: &str =
    "The working copy changed after this discard was reviewed. Nothing was discarded; review it again.";

/// What one discard changes, derived the same way when planning and when
/// executing so both phases agree on the exact paths.
struct Selection {
    entries: Vec<DiscardPlanEntry>,
    kept: Vec<KeptChange>,
    /// Every path whose index entries or working-tree content may change.
    touched: Vec<GitPath>,
    /// Tokens of the paths already selected, so a path Git reports twice is
    /// discarded and saved once.
    selected: HashSet<String>,
    /// Tracked paths whose index entries and working tree return to HEAD.
    reset: Vec<GitPath>,
    /// New files already gone from disk: only their index entries change, so
    /// only those are fingerprinted and saved. Whatever now occupies the path,
    /// such as a directory, is not part of the change and is never touched.
    unstage: Vec<GitPath>,
    /// Tracked paths whose working tree returns to the staged version.
    restore_from_index: Vec<GitPath>,
    /// Untracked files removed from the working tree.
    remove: Vec<GitPath>,
    kind: RecoveryPointKind,
    summary: String,
}

impl Selection {
    fn new(kind: RecoveryPointKind, summary: String) -> Self {
        Self {
            entries: Vec::new(),
            kept: Vec::new(),
            touched: Vec::new(),
            selected: HashSet::new(),
            reset: Vec::new(),
            unstage: Vec::new(),
            restore_from_index: Vec::new(),
            remove: Vec::new(),
            kind,
            summary,
        }
    }

    /// Adds one status record. Callers add tracked records first: a path
    /// removed from the index but still on disk is also reported as
    /// untracked, and returning it to HEAD already replaces that file.
    fn add(&mut self, change: &FileChange, scope: DiscardScope, unborn: bool) {
        if !self.selected.insert(change.path.token.clone()) {
            return;
        }
        // Rename and copy records report HEAD's mode for the original path, so
        // only ordinary records can name a path HEAD lacks.
        let gone_addition = change.worktree_status == "D"
            && (unborn
                || (!matches!(change.index_status.as_str(), "R" | "C")
                    && change.head_mode.as_deref() == Some("000000")));
        let effect = if change.untracked {
            DiscardEffect::Remove
        } else if scope == DiscardScope::Unstaged {
            DiscardEffect::RestoreStaged
        } else if gone_addition {
            DiscardEffect::Unstage
        } else if unborn
            || matches!(change.kind, FileChangeKind::Added | FileChangeKind::Copied)
            || change.head_mode.as_deref() == Some("000000")
        {
            DiscardEffect::Remove
        } else {
            DiscardEffect::RestoreCommitted
        };
        self.entries.push(DiscardPlanEntry {
            path: change.path.clone(),
            previous_path: change.previous_path.clone(),
            kind: change.kind,
            effect,
        });
        if effect == DiscardEffect::Unstage {
            self.unstage.push(change.path.clone());
            return;
        }
        self.touched.push(change.path.clone());
        if change.untracked {
            self.remove.push(change.path.clone());
        } else if scope == DiscardScope::Unstaged {
            self.restore_from_index.push(change.path.clone());
        } else {
            self.reset.push(change.path.clone());
            // Discarding a rename restores its original path as well, even
            // when the new name has since been deleted from disk.
            if change.index_status == "R" {
                if let Some(previous) = &change.previous_path {
                    if self.selected.insert(previous.token.clone()) {
                        self.reset.push(previous.clone());
                        self.touched.push(previous.clone());
                    }
                }
            }
        }
    }
}

fn is_submodule(change: &FileChange) -> bool {
    change.submodule
        || [&change.head_mode, &change.index_mode, &change.worktree_mode]
            .into_iter()
            .any(|mode| mode.as_deref() == Some("160000"))
}

/// Git reports an untracked repository nested in the working tree as one
/// directory entry whose path ends with a slash.
fn is_nested_repository(change: &FileChange) -> bool {
    change.untracked
        && decode_path_token_bytes(&change.path.token)
            .is_ok_and(|bytes| bytes.last() == Some(&b'/'))
}

fn select(snapshot: &WorkingCopySnapshot, target: &DiscardTarget) -> Result<Selection, String> {
    let unborn = snapshot.head.is_none();
    match target {
        DiscardTarget::File { path, scope } => {
            // A path removed from the index but still on disk has a tracked
            // and an untracked record. Its unstaged change is the file on
            // disk; discarding everything returns the path to HEAD.
            let records = || {
                snapshot
                    .changes
                    .iter()
                    .filter(|change| change.path.token == path.token)
            };
            let change = records()
                .find(|change| change.untracked == (*scope == DiscardScope::Unstaged))
                .or_else(|| records().next())
                .ok_or_else(|| {
                    "The selected change no longer exists. Refresh and try again.".to_string()
                })?;
            if change.conflicted {
                return Err("Resolve the conflict explicitly instead of discarding it.".into());
            }
            if change.ignored {
                return Err("Repola does not discard ignored files.".into());
            }
            if is_submodule(change) {
                return Err("Repola does not discard submodule changes because it cannot save them first. Commit, stash, or reset them inside the submodule.".into());
            }
            if is_nested_repository(change) {
                return Err("Repola does not discard nested repositories because it cannot save them first. Move or delete the folder yourself if you no longer need it.".into());
            }
            if *scope == DiscardScope::Unstaged && !change.unstaged && !change.untracked {
                return Err("The selected path has no unstaged changes to discard.".into());
            }
            let summary = if *scope == DiscardScope::Unstaged && !change.untracked {
                format!("Discarded unstaged changes to {}", change.path.display)
            } else {
                format!("Discarded changes to {}", change.path.display)
            };
            let mut selection = Selection::new(RecoveryPointKind::DiscardFile, summary);
            selection.add(change, *scope, unborn);
            Ok(selection)
        }
        DiscardTarget::All => {
            if snapshot.operation.is_some() {
                return Err(
                    "Abort or finish the in-progress Git operation before discarding every change."
                        .into(),
                );
            }
            let mut selection = Selection::new(RecoveryPointKind::DiscardAll, String::new());
            let (tracked, untracked): (Vec<&FileChange>, Vec<&FileChange>) = snapshot
                .changes
                .iter()
                .filter(|change| !change.ignored)
                .partition(|change| !change.untracked);
            for change in tracked.into_iter().chain(untracked) {
                if is_submodule(change) {
                    selection.kept.push(KeptChange {
                        path: change.path.clone(),
                        reason: KeptChangeReason::Submodule,
                    });
                } else if is_nested_repository(change) {
                    selection.kept.push(KeptChange {
                        path: change.path.clone(),
                        reason: KeptChangeReason::NestedRepository,
                    });
                } else {
                    selection.add(change, DiscardScope::All, unborn);
                }
            }
            if selection.entries.is_empty() {
                return Err(if selection.kept.is_empty() {
                    "There are no changes to discard.".into()
                } else {
                    "Every change here is in a submodule or nested repository, which Repola leaves alone.".into()
                });
            }
            let count = selection.entries.len();
            selection.summary = format!(
                "Discarded all changes ({count} file{})",
                if count == 1 { "" } else { "s" }
            );
            Ok(selection)
        }
    }
}

/// Plans a discard without changing anything.
pub fn plan_discard(request: DiscardPlanRequest) -> Result<DiscardPlan, String> {
    let snapshot = working_copy_snapshot(WorkingCopyRequest {
        repository_path: request.repository_path,
        worktree_path: request.worktree_path,
    })?;
    let selection = select(&snapshot, &request.target)?;
    let states = observe(
        Path::new(&snapshot.worktree_path),
        snapshot.head.as_deref(),
        &selection,
        false,
    )?;
    let fingerprint = plan_fingerprint(&snapshot, &selection, &states)?;
    Ok(DiscardPlan {
        target: request.target,
        entries: selection.entries,
        kept: selection.kept,
        backup_bytes: stored_bytes(&states),
        fingerprint,
    })
}

/// Covers the reviewed effects as well as the observed paths: if the index
/// changes while a plan is made, the paths it observed can select a different
/// effect when the discard runs.
fn plan_fingerprint(
    snapshot: &WorkingCopySnapshot,
    selection: &Selection,
    states: &[PathState],
) -> Result<String, String> {
    fingerprint(
        snapshot.head.as_deref(),
        snapshot.operation,
        states,
        &selection.entries,
    )
}

/// Executes a reviewed discard. The content it removes is saved as a
/// recovery point first; the returned point names where it went.
pub fn discard_changes(request: DiscardRequest) -> Result<DiscardResult, String> {
    let snapshot = working_copy_snapshot(WorkingCopyRequest {
        repository_path: request.repository_path,
        worktree_path: request.worktree_path,
    })?;
    let selection = select(&snapshot, &request.target)?;
    let worktree = Path::new(&snapshot.worktree_path);
    // Hashing with `store` writes the very bytes being fingerprinted, so the
    // recovery point holds exactly the reviewed content.
    let states = observe(worktree, snapshot.head.as_deref(), &selection, true)?;
    if plan_fingerprint(&snapshot, &selection, &states)? != request.fingerprint {
        return Err(STALE_PLAN.into());
    }
    let recovery_point = store_recovery_point(
        worktree,
        selection.kind,
        selection.summary.clone(),
        snapshot.head.as_deref(),
        &states,
    )?;
    execute(worktree, snapshot.head.as_deref(), &selection).map_err(|error| {
        format!(
            "{error}\n\nEverything this discard was changing is saved in recovery point {}. Restore it from Discarded Changes.",
            recovery_point.id
        )
    })?;
    let snapshot = working_copy_snapshot(WorkingCopyRequest {
        repository_path: snapshot.repository_path,
        worktree_path: snapshot.worktree_path,
    })?;
    Ok(DiscardResult {
        snapshot,
        recovery_point,
    })
}

/// Observes every path the selection changes, in the same order whenever it
/// is called, so a plan's fingerprint and an execution's agree.
fn observe(
    worktree: &Path,
    head: Option<&str>,
    selection: &Selection,
    store: bool,
) -> Result<Vec<PathState>, String> {
    let mut states = observe_paths(worktree, head, &selection.touched, store)?;
    // Git replaces a directory in the way of a restored file together with
    // everything inside it, so a discard never runs over one.
    if let Some(state) = states.iter().find(|state| state.directory) {
        return Err(format!(
            "{} is now a directory in the working copy, so Repola will not discard over it. Move it aside and review the discard again.",
            state.path.display
        ));
    }
    states.extend(observe_index(worktree, head, &selection.unstage)?);
    Ok(states)
}

fn execute(worktree: &Path, head: Option<&str>, selection: &Selection) -> Result<(), String> {
    reset_to_head(worktree, head, &selection.reset, true)?;
    reset_to_head(worktree, head, &selection.unstage, false)?;
    if !selection.restore_from_index.is_empty() {
        let output = command::git_at_with_input(
            worktree,
            [
                "restore",
                "--worktree",
                "--pathspec-from-file=-",
                "--pathspec-file-nul",
            ],
            &pathspec_input(&selection.restore_from_index)?,
        )
        .map_err(|error| error.to_string())?;
        ensure_success(output, "restore the selected working-tree paths")?;
    }
    for path in &selection.remove {
        remove_worktree_entry(worktree, path)?;
    }
    Ok(())
}

/// Returns `paths` to HEAD in the index and, with `working_tree`, on disk.
fn reset_to_head(
    worktree: &Path,
    head: Option<&str>,
    paths: &[GitPath],
    working_tree: bool,
) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    let mut args = Vec::new();
    match head {
        Some(head) => {
            args.extend([
                OsString::from("restore"),
                OsString::from(format!("--source={head}")),
                OsString::from("--staged"),
            ]);
            if working_tree {
                args.push(OsString::from("--worktree"));
            }
        }
        // An unborn branch has nothing to restore; drop the paths from the
        // index and, unless only the index changes, the working tree.
        None => {
            args.extend(["rm", "--force", "--quiet", "--ignore-unmatch"].map(OsString::from));
            if !working_tree {
                args.push(OsString::from("--cached"));
            }
        }
    }
    args.extend(["--pathspec-from-file=-", "--pathspec-file-nul"].map(OsString::from));
    let output = command::git_at_with_input(worktree, args, &pathspec_input(paths)?)
        .map_err(|error| error.to_string())?;
    ensure_success(
        output,
        if working_tree {
            "restore the selected tracked paths"
        } else {
            "unstage the selected new files"
        },
    )
}

/// Exact paths for `--pathspec-from-file`, each marked literal so glob
/// characters in a name never widen it.
fn pathspec_input(paths: &[GitPath]) -> Result<Vec<u8>, String> {
    let mut input = Vec::new();
    for path in paths {
        input.extend_from_slice(b":(literal)");
        input.extend(decode_path_token_bytes(&path.token)?);
        input.push(0);
    }
    Ok(input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worktree::models::{
        RecoveryPoint, RecoveryPointReference, RecoveryPointRequest, RecoveryRestoreRequest,
        RestoreEffect,
    };
    use crate::worktree::recovery::{
        list_recovery_points, plan_recovery_restore, restore_recovery_point,
    };
    use std::path::PathBuf;

    fn git(path: &Path, args: &[&str]) -> String {
        let output = command::successful_git_at(path, args).expect("git");
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn repository() -> tempfile::TempDir {
        let directory = tempfile::tempdir().expect("temp repository");
        let path = directory.path();
        git(path, &["init", "--initial-branch=main"]);
        git(path, &["config", "core.autocrlf", "false"]);
        git(path, &["config", "user.name", "Repola Test"]);
        git(path, &["config", "user.email", "repola@example.invalid"]);
        directory
    }

    fn root(directory: &tempfile::TempDir) -> PathBuf {
        dunce::canonicalize(directory.path()).expect("canonical repository")
    }

    fn request(path: &Path) -> WorkingCopyRequest {
        WorkingCopyRequest {
            repository_path: path.to_string_lossy().into_owned(),
            worktree_path: path.to_string_lossy().into_owned(),
        }
    }

    fn write(path: &Path, name: &str, contents: &[u8]) {
        let target = path.join(name);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).expect("parent");
        }
        std::fs::write(target, contents).expect("write");
    }

    fn path_of(path: &Path, display: &str) -> GitPath {
        working_copy_snapshot(request(path))
            .expect("snapshot")
            .changes
            .into_iter()
            .find(|change| change.path.display == display)
            .unwrap_or_else(|| panic!("{display} is not changed"))
            .path
    }

    fn plan(path: &Path, target: DiscardTarget) -> DiscardPlan {
        let request = request(path);
        plan_discard(DiscardPlanRequest {
            repository_path: request.repository_path,
            worktree_path: request.worktree_path,
            target,
        })
        .expect("plan")
    }

    fn discard(path: &Path, target: DiscardTarget) -> DiscardResult {
        let plan = plan(path, target.clone());
        let request = request(path);
        discard_changes(DiscardRequest {
            repository_path: request.repository_path,
            worktree_path: request.worktree_path,
            target,
            fingerprint: plan.fingerprint,
        })
        .expect("discard")
    }

    fn restore(
        path: &Path,
        point: &RecoveryPoint,
    ) -> crate::worktree::models::RecoveryRestoreResult {
        let request = request(path);
        let reference = RecoveryPointReference {
            id: point.id.clone(),
            oid: point.oid.clone(),
        };
        let plan = plan_recovery_restore(RecoveryPointRequest {
            repository_path: request.repository_path.clone(),
            worktree_path: request.worktree_path.clone(),
            point: reference.clone(),
        })
        .expect("restore plan");
        restore_recovery_point(RecoveryRestoreRequest {
            repository_path: request.repository_path,
            worktree_path: request.worktree_path,
            point: reference,
            fingerprint: plan.fingerprint,
        })
        .expect("restore")
    }

    /// Everything a recovery must reproduce: status, exact index entries, and
    /// every changed file's bytes on disk.
    #[derive(Debug, PartialEq, Eq)]
    struct ExactState {
        status: String,
        index: String,
        files: Vec<(String, Option<Vec<u8>>)>,
    }

    fn exact_state(path: &Path) -> ExactState {
        let status = git(
            path,
            &["status", "--porcelain=v2", "-z", "--untracked-files=all"],
        );
        // `-v` tags each entry with its assume-unchanged and skip-worktree flags.
        let index = git(path, &["ls-files", "--stage", "-v", "-z"]);
        let mut files: Vec<(String, Option<Vec<u8>>)> = working_copy_snapshot(request(path))
            .expect("snapshot")
            .changes
            .iter()
            .flat_map(|change| {
                std::iter::once(change.path.display.clone()).chain(
                    change
                        .previous_path
                        .iter()
                        .map(|previous| previous.display.clone()),
                )
            })
            .map(|name| {
                let bytes = std::fs::read(path.join(&name)).ok();
                (name, bytes)
            })
            .collect();
        files.sort();
        ExactState {
            status,
            index,
            files,
        }
    }

    fn base_commit(path: &Path) {
        write(path, "modified.txt", b"one\ntwo\n");
        write(path, "staged.txt", b"staged base\n");
        write(path, "deleted.txt", b"delete me\n");
        write(path, "rename-me.txt", b"rename\n");
        write(path, "crlf.txt", b"first\r\nsecond\r\n");
        git(path, &["add", "."]);
        git(path, &["commit", "-m", "base"]);
    }

    #[test]
    fn discard_all_saves_every_path_and_restores_the_exact_index_and_working_tree() {
        let directory = repository();
        let path = root(&directory);
        base_commit(&path);
        write(&path, "modified.txt", b"one\nchanged\n");
        write(&path, "staged.txt", b"staged edit\n");
        git(&path, &["add", "staged.txt"]);
        write(&path, "staged.txt", b"staged edit\nthen unstaged\n");
        std::fs::remove_file(path.join("deleted.txt")).expect("delete");
        git(&path, &["mv", "rename-me.txt", "renamed.txt"]);
        write(&path, "crlf.txt", b"first\r\nmixed\nsecond\r\n");
        write(&path, "added.txt", b"new and staged\n");
        git(&path, &["add", "added.txt"]);
        write(&path, "nested/dir/untracked.bin", &[0, 159, 146, 150, 0xff]);
        let before = exact_state(&path);

        let plan = plan(&path, DiscardTarget::All);
        assert!(plan.kept.is_empty());
        assert_eq!(plan.entries.len(), 7);
        let effect = |name: &str| {
            plan.entries
                .iter()
                .find(|entry| entry.path.display == name)
                .expect("entry")
                .effect
        };
        assert_eq!(effect("added.txt"), DiscardEffect::Remove);
        assert_eq!(effect("nested/dir/untracked.bin"), DiscardEffect::Remove);
        assert_eq!(effect("renamed.txt"), DiscardEffect::RestoreCommitted);
        assert!(plan.backup_bytes > 0);

        let result = discard(&path, DiscardTarget::All);
        assert!(result.snapshot.changes.is_empty());
        assert_eq!(result.recovery_point.kind, RecoveryPointKind::DiscardAll);
        // Seven changes plus the original path of the rename.
        assert_eq!(result.recovery_point.path_count, 8);
        assert!(path.join("rename-me.txt").is_file());
        assert!(!path.join("nested/dir/untracked.bin").exists());

        let restored = restore(&path, &result.recovery_point);
        assert!(
            restored.replaced.is_some(),
            "the committed content the restore replaced is saved"
        );
        assert_eq!(exact_state(&path), before);
    }

    #[test]
    fn file_discards_cover_each_scope_and_restore_exactly() {
        let directory = repository();
        let path = root(&directory);
        base_commit(&path);
        write(&path, "staged.txt", b"staged edit\n");
        git(&path, &["add", "staged.txt"]);
        write(&path, "staged.txt", b"staged edit\nthen unstaged\n");
        git(&path, &["mv", "rename-me.txt", "renamed.txt"]);
        write(&path, "untracked.txt", b"temporary\n");
        let before = exact_state(&path);

        let unstaged = discard(
            &path,
            DiscardTarget::File {
                path: path_of(&path, "staged.txt"),
                scope: DiscardScope::Unstaged,
            },
        );
        assert_eq!(
            std::fs::read(path.join("staged.txt")).expect("read"),
            b"staged edit\n"
        );
        assert!(unstaged
            .snapshot
            .changes
            .iter()
            .any(|change| change.path.display == "staged.txt"
                && change.staged
                && !change.unstaged));

        let renamed = discard(
            &path,
            DiscardTarget::File {
                path: path_of(&path, "renamed.txt"),
                scope: DiscardScope::All,
            },
        );
        assert!(path.join("rename-me.txt").is_file());
        assert!(!path.join("renamed.txt").exists());
        assert_eq!(renamed.recovery_point.path_count, 2);

        let untracked = discard(
            &path,
            DiscardTarget::File {
                path: path_of(&path, "untracked.txt"),
                scope: DiscardScope::Unstaged,
            },
        );
        assert!(!path.join("untracked.txt").exists());

        for point in [
            &untracked.recovery_point,
            &renamed.recovery_point,
            &unstaged.recovery_point,
        ] {
            restore(&path, point);
        }
        assert_eq!(exact_state(&path), before);
        // Three discards, plus the committed content two of the restores
        // replaced; restoring the removed untracked file replaced nothing.
        assert_eq!(
            list_recovery_points(request(&path))
                .expect("list")
                .points
                .len(),
            5
        );
    }

    #[test]
    fn a_stale_review_discards_nothing_and_saves_nothing() {
        let directory = repository();
        let path = root(&directory);
        base_commit(&path);
        write(&path, "modified.txt", b"reviewed\n");
        let file = DiscardTarget::File {
            path: path_of(&path, "modified.txt"),
            scope: DiscardScope::All,
        };
        let reviewed = plan(&path, file.clone());
        // Same length, so only the content fingerprint can notice.
        write(&path, "modified.txt", b"replaced\n");
        let error = discard_changes(DiscardRequest {
            repository_path: request(&path).repository_path,
            worktree_path: request(&path).worktree_path,
            target: file,
            fingerprint: reviewed.fingerprint,
        })
        .expect_err("stale file review");
        assert_eq!(error, STALE_PLAN);
        assert_eq!(
            std::fs::read(path.join("modified.txt")).expect("read"),
            b"replaced\n"
        );

        let reviewed = plan(&path, DiscardTarget::All);
        write(&path, "later.txt", b"not reviewed\n");
        let error = discard_changes(DiscardRequest {
            repository_path: request(&path).repository_path,
            worktree_path: request(&path).worktree_path,
            target: DiscardTarget::All,
            fingerprint: reviewed.fingerprint,
        })
        .expect_err("stale discard-all review");
        assert_eq!(error, STALE_PLAN);
        assert!(path.join("later.txt").is_file());
        assert!(list_recovery_points(request(&path))
            .expect("list")
            .points
            .is_empty());
    }

    #[test]
    fn glob_characters_in_file_names_never_widen_a_discard() {
        // Read as a glob, each `[c].txt` would also match its neighbour `c.txt`.
        for unborn in [false, true] {
            let directory = repository();
            let path = root(&directory);
            let mut targets = vec![
                ("[a].txt", DiscardScope::All),
                ("[g].txt", DiscardScope::All),
                ("[u].txt", DiscardScope::Unstaged),
            ];
            if !unborn {
                for name in ["[x].txt", "x.txt", "old.txt"] {
                    write(&path, name, b"original\n");
                }
                git(&path, &["add", "."]);
                git(&path, &["commit", "-m", "base"]);
                for name in ["[x].txt", "x.txt"] {
                    write(&path, name, b"staged\n");
                    git(&path, &["add", "--", name]);
                    write(&path, name, b"unstaged\n");
                }
                git(&path, &["mv", "old.txt", "[n].txt"]);
                write(&path, "n.txt", b"keep\n");
                targets.extend([
                    ("[x].txt", DiscardScope::Unstaged),
                    ("[x].txt", DiscardScope::All),
                    ("[n].txt", DiscardScope::All),
                ]);
            }
            for name in ["[a].txt", "a.txt", "[g].txt", "g.txt"] {
                write(&path, name, b"added\n");
                git(&path, &["add", "--", name]);
            }
            // Deleted after staging, so discarding it only unstages it.
            std::fs::remove_file(path.join("[g].txt")).expect("delete after staging");
            write(&path, "[u].txt", b"reviewed\n");
            write(&path, "u.txt", b"keep\n");
            let before = exact_state(&path);

            let mut points = Vec::new();
            for (name, scope) in targets {
                let target = DiscardTarget::File {
                    path: path_of(&path, name),
                    scope,
                };
                points.push(discard(&path, target).recovery_point);
            }
            assert!(!path.join("[a].txt").exists());
            assert!(!path.join("[u].txt").exists());
            assert_eq!(git(&path, &["show", ":a.txt"]), "added\n");
            assert_eq!(git(&path, &["show", ":g.txt"]), "added\n");
            assert_eq!(std::fs::read(path.join("u.txt")).expect("read"), b"keep\n");
            if !unborn {
                assert_eq!(
                    std::fs::read(path.join("[x].txt")).expect("read"),
                    b"original\n"
                );
                assert_eq!(git(&path, &["show", ":x.txt"]), "staged\n");
                assert_eq!(
                    std::fs::read(path.join("x.txt")).expect("read"),
                    b"unstaged\n"
                );
                assert!(path.join("old.txt").is_file());
                assert!(!path.join("[n].txt").exists());
                assert_eq!(std::fs::read(path.join("n.txt")).expect("read"), b"keep\n");
            }

            for point in points.iter().rev() {
                restore(&path, point);
            }
            assert_eq!(exact_state(&path), before, "unborn: {unborn}");
        }
    }

    #[test]
    fn autocrlf_files_are_saved_as_their_exact_bytes() {
        let directory = repository();
        let path = root(&directory);
        git(&path, &["config", "core.autocrlf", "true"]);
        write(&path, "text.txt", b"one\r\ntwo\r\n");
        git(&path, &["add", "."]);
        git(&path, &["commit", "-m", "base"]);
        // Mixed endings do not survive Git's clean and smudge filters.
        let edited: &[u8] = b"one\r\nlone\ntwo\r\n";
        write(&path, "text.txt", edited);
        let result = discard(
            &path,
            DiscardTarget::File {
                path: path_of(&path, "text.txt"),
                scope: DiscardScope::All,
            },
        );
        assert_eq!(
            std::fs::read(path.join("text.txt")).expect("read"),
            b"one\r\ntwo\r\n"
        );
        restore(&path, &result.recovery_point);
        assert_eq!(std::fs::read(path.join("text.txt")).expect("read"), edited);
    }

    #[test]
    fn unborn_branch_discards_are_recoverable() {
        let directory = repository();
        let path = root(&directory);
        write(&path, "first.txt", b"staged on an unborn branch\n");
        write(&path, "gone.txt", b"staged, then deleted\n");
        git(&path, &["add", "first.txt", "gone.txt"]);
        std::fs::remove_file(path.join("gone.txt")).expect("delete after staging");
        write(&path, "scratch.txt", b"untracked\n");
        let before = exact_state(&path);
        assert_eq!(
            effect_of(&plan(&path, DiscardTarget::All), "gone.txt"),
            DiscardEffect::Unstage
        );
        let result = discard(&path, DiscardTarget::All);
        assert!(result.snapshot.changes.is_empty());
        assert!(!path.join("first.txt").exists());
        restore(&path, &result.recovery_point);
        assert_eq!(exact_state(&path), before);
    }

    #[test]
    fn a_conflicted_index_is_saved_with_every_stage() {
        let directory = repository();
        let path = root(&directory);
        write(&path, "conflict.txt", b"base\n");
        git(&path, &["add", "."]);
        git(&path, &["commit", "-m", "base"]);
        write(&path, "conflict.txt", b"stashed\n");
        git(&path, &["stash"]);
        write(&path, "conflict.txt", b"committed\n");
        git(&path, &["commit", "-am", "other"]);
        // A conflicting stash apply leaves stages 1-3 without an operation.
        assert!(!command::git_at(&path, ["stash", "apply"])
            .expect("apply")
            .status
            .success());
        let before = exact_state(&path);
        assert!(before.index.contains(" 3\tconflict.txt"));

        let result = discard(&path, DiscardTarget::All);
        assert!(result.snapshot.changes.is_empty());
        restore(&path, &result.recovery_point);
        assert_eq!(exact_state(&path), before);
    }

    #[test]
    fn submodules_and_nested_repositories_are_kept() {
        let directory = repository();
        let path = root(&directory);
        let library = repository();
        write(library.path(), "lib.txt", b"library\n");
        git(library.path(), &["add", "."]);
        git(library.path(), &["commit", "-m", "library"]);
        write(&path, "tracked.txt", b"tracked\n");
        git(&path, &["add", "."]);
        git(&path, &["commit", "-m", "base"]);
        let library_path = library.path().to_string_lossy().into_owned();
        git(
            &path,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "--quiet",
                &library_path,
                "module",
            ],
        );
        git(&path, &["commit", "-m", "add submodule"]);
        write(&path, "module/lib.txt", b"edited inside the submodule\n");
        write(&path, "nested/inner.txt", b"nested work\n");
        git(&path.join("nested"), &["init", "--quiet"]);
        write(&path, "tracked.txt", b"changed\n");

        let plan = plan(&path, DiscardTarget::All);
        let mut kept: Vec<(String, KeptChangeReason)> = plan
            .kept
            .iter()
            .map(|kept| (kept.path.display.clone(), kept.reason))
            .collect();
        kept.sort_by(|left, right| left.0.cmp(&right.0));
        assert_eq!(
            kept,
            vec![
                ("module".to_string(), KeptChangeReason::Submodule),
                ("nested/".to_string(), KeptChangeReason::NestedRepository),
            ]
        );
        assert_eq!(plan.entries.len(), 1);

        discard(&path, DiscardTarget::All);
        assert_eq!(
            std::fs::read(path.join("tracked.txt")).expect("read"),
            b"tracked\n"
        );
        assert_eq!(
            std::fs::read(path.join("module/lib.txt")).expect("read"),
            b"edited inside the submodule\n"
        );
        assert!(path.join("nested/inner.txt").is_file());

        for display in ["module", "nested/"] {
            let error = plan_discard(DiscardPlanRequest {
                repository_path: request(&path).repository_path,
                worktree_path: request(&path).worktree_path,
                target: DiscardTarget::File {
                    path: path_of(&path, display),
                    scope: DiscardScope::All,
                },
            })
            .expect_err("kept paths cannot be discarded individually");
            assert!(error.contains("cannot save them first"), "{error}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn unix_modes_and_links_restore_exactly() {
        use std::os::unix::fs::PermissionsExt;
        let directory = repository();
        let path = root(&directory);
        write(&path, "tool.sh", b"#!/bin/sh\n");
        git(&path, &["add", "."]);
        git(&path, &["commit", "-m", "base"]);
        let mut permissions = std::fs::metadata(path.join("tool.sh"))
            .expect("meta")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path.join("tool.sh"), permissions).expect("chmod");
        std::os::unix::fs::symlink("tool.sh", path.join("alias")).expect("symlink");
        let before = exact_state(&path);

        let result = discard(&path, DiscardTarget::All);
        assert!(result.snapshot.changes.is_empty());
        assert_eq!(
            std::fs::metadata(path.join("tool.sh"))
                .expect("meta")
                .permissions()
                .mode()
                & 0o111,
            0
        );
        restore(&path, &result.recovery_point);
        assert_eq!(exact_state(&path), before);
        assert_ne!(
            std::fs::metadata(path.join("tool.sh"))
                .expect("meta")
                .permissions()
                .mode()
                & 0o111,
            0
        );
        assert_eq!(
            std::fs::read_link(path.join("alias")).expect("link"),
            Path::new("tool.sh")
        );
    }

    // Only Linux filesystems accept file names that are not valid Unicode.
    #[cfg(target_os = "linux")]
    #[test]
    fn non_unicode_file_names_restore_exactly() {
        use std::os::unix::ffi::OsStrExt;
        let directory = repository();
        let path = root(&directory);
        let odd = std::ffi::OsStr::from_bytes(b"caf\xe9.txt");
        std::fs::write(path.join(odd), b"latin-1 name\n").expect("odd name");
        let before = exact_state(&path);
        let result = discard(&path, DiscardTarget::All);
        assert!(!path.join(odd).exists());
        restore(&path, &result.recovery_point);
        assert_eq!(exact_state(&path), before);
        assert_eq!(
            std::fs::read(path.join(odd)).expect("odd"),
            b"latin-1 name\n"
        );
    }

    #[test]
    fn untracked_folders_leave_no_empty_directories_and_restore_exactly() {
        let directory = repository();
        let path = root(&directory);
        write(&path, ".gitignore", b"*.log\n");
        write(&path, "keep/tracked.txt", b"tracked\n");
        git(&path, &["add", "."]);
        git(&path, &["commit", "-m", "base"]);
        write(&path, "deep/a/b/file.bin", &[0, 1, 2, 0xfe, 0xff]);
        write(&path, "deep/a/other.txt", b"other\n");
        write(&path, "keep/sub/new.txt", b"new\n");
        write(&path, "logs/new.txt", b"untracked beside an ignored file\n");
        write(&path, "logs/run.log", b"ignored\n");
        std::fs::create_dir(path.join("empty")).expect("unrelated empty directory");
        let before = exact_state(&path);

        let result = discard(&path, DiscardTarget::All);
        assert!(result.snapshot.changes.is_empty());
        assert!(
            !path.join("deep").exists(),
            "an emptied folder tree is removed"
        );
        assert!(!path.join("keep/sub").exists());
        assert!(
            path.join("keep/tracked.txt").is_file(),
            "a folder with tracked files stays"
        );
        assert!(
            path.join("logs/run.log").is_file(),
            "a folder with ignored files stays"
        );
        assert!(
            path.join("empty").is_dir(),
            "directories no file was removed from stay"
        );

        restore(&path, &result.recovery_point);
        assert_eq!(exact_state(&path), before);
        assert_eq!(
            std::fs::read(path.join("deep/a/b/file.bin")).expect("read"),
            [0, 1, 2, 0xfe, 0xff]
        );

        discard(
            &path,
            DiscardTarget::File {
                path: path_of(&path, "deep/a/b/file.bin"),
                scope: DiscardScope::Unstaged,
            },
        );
        assert!(!path.join("deep/a/b").exists());
        assert!(path.join("deep/a/other.txt").is_file());
    }

    #[test]
    fn a_plan_whose_index_changed_mid_review_never_runs_a_different_effect() {
        let directory = repository();
        let path = root(&directory);
        base_commit(&path);
        write(&path, "modified.txt", b"edited\n");
        let target = DiscardTarget::File {
            path: path_of(&path, "modified.txt"),
            scope: DiscardScope::Unstaged,
        };
        // Planning as `plan_discard` does, with the file leaving the index
        // between the snapshot and the observation: the plan shows restoring
        // the staged version, but its paths now select removing the file.
        let snapshot = working_copy_snapshot(request(&path)).expect("snapshot");
        let selection = select(&snapshot, &target).expect("select");
        assert_eq!(selection.entries[0].effect, DiscardEffect::RestoreStaged);
        git(&path, &["rm", "--cached", "--quiet", "modified.txt"]);
        let worktree = Path::new(&snapshot.worktree_path);
        let states =
            observe(worktree, snapshot.head.as_deref(), &selection, false).expect("observe");
        let error = discard_changes(DiscardRequest {
            repository_path: request(&path).repository_path,
            worktree_path: request(&path).worktree_path,
            target,
            fingerprint: plan_fingerprint(&snapshot, &selection, &states).expect("fingerprint"),
        })
        .expect_err("changed review");
        assert_eq!(error, STALE_PLAN);
        assert_eq!(
            std::fs::read(path.join("modified.txt")).expect("read"),
            b"edited\n"
        );
    }

    #[test]
    fn a_case_only_rename_is_refused_where_case_is_ignored() {
        let directory = repository();
        let path = root(&directory);
        base_commit(&path);
        git(&path, &["config", "core.ignorecase", "true"]);
        git(&path, &["mv", "rename-me.txt", "Rename-Me.txt"]);
        let before = exact_state(&path);
        for target in [file(&path, "Rename-Me.txt"), DiscardTarget::All] {
            let error = plan_error(&path, target);
            assert!(error.contains("differ only in letter case"), "{error}");
        }
        assert_eq!(exact_state(&path), before);
    }

    fn effect_of(plan: &DiscardPlan, display: &str) -> DiscardEffect {
        plan.entries
            .iter()
            .find(|entry| entry.path.display == display)
            .unwrap_or_else(|| panic!("{display} is not planned"))
            .effect
    }

    fn file(path: &Path, display: &str) -> DiscardTarget {
        DiscardTarget::File {
            path: path_of(path, display),
            scope: DiscardScope::All,
        }
    }

    fn plan_error(path: &Path, target: DiscardTarget) -> String {
        plan_discard(DiscardPlanRequest {
            repository_path: request(path).repository_path,
            worktree_path: request(path).worktree_path,
            target,
        })
        .expect_err("plan refused")
    }

    #[test]
    fn staged_new_files_are_discarded_however_they_changed_after_staging() {
        let directory = repository();
        let path = root(&directory);
        write(&path, "base.txt", b"base\n");
        git(&path, &["add", "."]);
        git(&path, &["commit", "-m", "base"]);
        write(&path, "edited.txt", b"staged\n");
        write(&path, "removed.txt", b"staged\n");
        git(&path, &["add", "edited.txt", "removed.txt"]);
        write(&path, "edited.txt", b"staged\nedited\n");
        std::fs::remove_file(path.join("removed.txt")).expect("remove after staging");
        let before = exact_state(&path);

        let plan = plan(&path, DiscardTarget::All);
        assert_eq!(effect_of(&plan, "edited.txt"), DiscardEffect::Remove);
        assert_eq!(effect_of(&plan, "removed.txt"), DiscardEffect::Unstage);
        let all = discard(&path, DiscardTarget::All);
        assert!(all.snapshot.changes.is_empty());
        assert!(!path.join("edited.txt").exists());
        restore(&path, &all.recovery_point);
        assert_eq!(exact_state(&path), before);

        let mut points = Vec::new();
        for name in ["edited.txt", "removed.txt"] {
            points.push(discard(&path, file(&path, name)).recovery_point);
        }
        assert!(working_copy_snapshot(request(&path))
            .expect("snapshot")
            .changes
            .is_empty());
        for point in points.iter().rev() {
            restore(&path, point);
        }
        assert_eq!(exact_state(&path), before);
    }

    #[test]
    fn discarding_a_staged_addition_touches_only_the_reviewed_file() {
        let directory = repository();
        let path = root(&directory);
        write(&path, ".gitignore", b"*.log\n");
        git(&path, &["add", "."]);
        git(&path, &["commit", "-m", "base"]);
        for name in ["replaced", "[x].txt", "forced.log"] {
            write(&path, name, b"staged\n");
            git(&path, &["add", "-f", "--", name]);
        }
        // A directory takes over a deleted addition's path; its file is a
        // change of its own.
        std::fs::remove_file(path.join("replaced")).expect("remove addition");
        write(&path, "replaced/unreviewed.txt", b"keep\n");
        // A literal name that also reads as a glob matching an untracked neighbour.
        write(&path, "[x].txt", b"staged\nedited\n");
        write(&path, "x.txt", b"keep\n");
        // A force-added file stays covered by its ignore rule once unstaged.
        write(&path, "forced.log", b"staged\nedited\n");
        let before = exact_state(&path);

        let mut points = Vec::new();
        for name in ["replaced", "[x].txt", "forced.log"] {
            points.push(discard(&path, file(&path, name)).recovery_point);
        }
        assert_eq!(
            std::fs::read(path.join("replaced/unreviewed.txt")).expect("unreviewed file kept"),
            b"keep\n"
        );
        assert_eq!(
            std::fs::read(path.join("x.txt")).expect("neighbour kept"),
            b"keep\n"
        );
        assert!(!path.join("[x].txt").exists());
        assert!(!path.join("forced.log").exists());
        assert_eq!(git(&path, &["ls-files"]), ".gitignore\n");

        // The directory is still in the way, and the addition comes back into
        // the index without touching it.
        for point in points.iter().rev() {
            restore(&path, point);
        }
        assert_eq!(exact_state(&path), before);
        assert_eq!(
            std::fs::read(path.join("forced.log")).expect("restored"),
            b"staged\nedited\n"
        );
    }

    #[test]
    fn a_directory_in_place_of_a_committed_file_is_never_replaced() {
        let directory = repository();
        let path = root(&directory);
        write(&path, "swapped", b"committed\n");
        write(&path, "edited.txt", b"committed\n");
        git(&path, &["add", "."]);
        git(&path, &["commit", "-m", "base"]);
        std::fs::remove_file(path.join("swapped")).expect("remove file");
        write(&path, "swapped/unreviewed.txt", b"keep\n");

        for target in [file(&path, "swapped"), DiscardTarget::All] {
            let error = plan_error(&path, target);
            assert!(error.contains("swapped is now a directory"), "{error}");
        }

        // A directory that appears after the review invalidates it.
        std::fs::remove_dir_all(path.join("swapped")).expect("remove directory");
        write(&path, "edited.txt", b"reviewed\n");
        let reviewed = plan(&path, file(&path, "edited.txt"));
        std::fs::remove_file(path.join("edited.txt")).expect("remove file");
        write(&path, "edited.txt/unreviewed.txt", b"keep\n");
        let error = discard_changes(DiscardRequest {
            repository_path: request(&path).repository_path,
            worktree_path: request(&path).worktree_path,
            target: file(&path, "edited.txt"),
            fingerprint: reviewed.fingerprint,
        })
        .expect_err("directory after review");
        assert!(error.contains("edited.txt is now a directory"), "{error}");
        assert_eq!(
            std::fs::read(path.join("edited.txt/unreviewed.txt")).expect("kept"),
            b"keep\n"
        );
        assert!(list_recovery_points(request(&path))
            .expect("list")
            .points
            .is_empty());

        // A restore does not replace a directory with the saved file either.
        std::fs::remove_dir_all(path.join("edited.txt")).expect("remove directory");
        write(&path, "edited.txt", b"discarded\n");
        let discarded = discard(&path, file(&path, "edited.txt"));
        std::fs::remove_file(path.join("edited.txt")).expect("remove file");
        write(&path, "edited.txt/unreviewed.txt", b"keep\n");
        let error = plan_recovery_restore(RecoveryPointRequest {
            repository_path: request(&path).repository_path,
            worktree_path: request(&path).worktree_path,
            point: RecoveryPointReference {
                id: discarded.recovery_point.id.clone(),
                oid: discarded.recovery_point.oid.clone(),
            },
        })
        .expect_err("restore over a directory");
        assert!(error.contains("edited.txt is now a directory"), "{error}");
        assert_eq!(
            std::fs::read(path.join("edited.txt/unreviewed.txt")).expect("kept"),
            b"keep\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn discarding_never_follows_a_parent_replaced_by_a_link() {
        use std::os::unix::fs::symlink;
        let directory = repository();
        let path = root(&directory);
        let outside = tempfile::tempdir().expect("outside directory");
        write(outside.path(), "file.txt", b"outside\n");
        git(&path, &["commit", "--allow-empty", "-m", "base"]);
        write(&path, "parent/file.txt", b"staged\n");
        git(&path, &["add", "parent/file.txt"]);
        write(&path, "parent/file.txt", b"staged\nedited\n");
        let reviewed = plan(&path, file(&path, "parent/file.txt"));
        std::fs::remove_dir_all(path.join("parent")).expect("remove parent");
        symlink(outside.path(), path.join("parent")).expect("linked parent");

        let error = discard_changes(DiscardRequest {
            repository_path: request(&path).repository_path,
            worktree_path: request(&path).worktree_path,
            target: reviewed.target,
            fingerprint: reviewed.fingerprint,
        })
        .expect_err("parent replaced after review");
        assert_eq!(error, STALE_PLAN);

        // Reviewed again, Git sees the file gone, so only its index entry goes.
        let plan = plan(&path, file(&path, "parent/file.txt"));
        assert_eq!(effect_of(&plan, "parent/file.txt"), DiscardEffect::Unstage);
        discard(&path, file(&path, "parent/file.txt"));
        assert_eq!(git(&path, &["ls-files"]), "");
        assert_eq!(
            std::fs::read(outside.path().join("file.txt")).expect("outside file kept"),
            b"outside\n"
        );
        assert!(std::fs::symlink_metadata(path.join("parent"))
            .expect("link kept")
            .file_type()
            .is_symlink());
    }

    #[test]
    fn a_newly_added_submodule_is_kept() {
        let directory = repository();
        let path = root(&directory);
        write(&path, "base.txt", b"base\n");
        git(&path, &["add", "."]);
        git(&path, &["commit", "-m", "base"]);
        let nested = path.join("nested");
        std::fs::create_dir(&nested).expect("nested directory");
        git(&nested, &["init", "--quiet"]);
        git(
            &nested,
            &[
                "-c",
                "user.name=Repola Test",
                "-c",
                "user.email=repola@example.invalid",
                "commit",
                "--allow-empty",
                "-m",
                "nested",
            ],
        );
        git(&path, &["add", "nested"]);
        write(&path, "base.txt", b"changed\n");

        let error = plan_error(&path, file(&path, "nested"));
        assert!(error.contains("cannot save them first"), "{error}");
        let plan = plan(&path, DiscardTarget::All);
        assert_eq!(plan.kept.len(), 1);
        assert_eq!(plan.kept[0].reason, KeptChangeReason::Submodule);
        discard(&path, DiscardTarget::All);
        assert!(nested.join(".git").exists());
        assert_eq!(git(&path, &["diff", "--cached", "--name-only"]), "nested\n");
    }

    #[test]
    fn a_path_git_reports_twice_is_discarded_and_saved_once() {
        let directory = repository();
        let path = root(&directory);
        write(&path, "unindexed.txt", b"committed\n");
        write(&path, "moved.txt", b"original\n");
        git(&path, &["add", "."]);
        git(&path, &["commit", "-m", "base"]);
        // Each path below is reported by a tracked record and an untracked one.
        git(&path, &["rm", "--quiet", "--cached", "unindexed.txt"]);
        write(&path, "unindexed.txt", b"edited on disk\n");
        git(&path, &["mv", "moved.txt", "renamed.txt"]);
        write(&path, "moved.txt", b"recreated\n");
        let before = exact_state(&path);

        let plan = plan(&path, DiscardTarget::All);
        let mut planned: Vec<&str> = plan
            .entries
            .iter()
            .map(|entry| entry.path.display.as_str())
            .collect();
        planned.sort_unstable();
        assert_eq!(planned, ["renamed.txt", "unindexed.txt"]);
        let all = discard(&path, DiscardTarget::All);
        assert!(all.snapshot.changes.is_empty());
        assert_eq!(
            std::fs::read(path.join("unindexed.txt")).expect("read"),
            b"committed\n"
        );
        assert_eq!(
            std::fs::read(path.join("moved.txt")).expect("read"),
            b"original\n"
        );
        restore(&path, &all.recovery_point);
        assert_eq!(exact_state(&path), before);

        // Its unstaged change is the file on disk; all of it returns to HEAD.
        let unstaged = discard(
            &path,
            DiscardTarget::File {
                path: path_of(&path, "unindexed.txt"),
                scope: DiscardScope::Unstaged,
            },
        );
        assert!(!path.join("unindexed.txt").exists());
        assert_eq!(git(&path, &["ls-files", "--", "unindexed.txt"]), "");
        let committed = discard(&path, file(&path, "unindexed.txt"));
        assert_eq!(
            std::fs::read(path.join("unindexed.txt")).expect("read"),
            b"committed\n"
        );
        for point in [&committed.recovery_point, &unstaged.recovery_point] {
            restore(&path, point);
        }
        assert_eq!(exact_state(&path), before);
    }

    #[test]
    fn intent_to_add_entries_restore_exactly() {
        for unborn in [false, true] {
            let directory = repository();
            let path = root(&directory);
            write(&path, ".gitignore", b"*.log\n");
            if !unborn {
                git(&path, &["add", "."]);
                git(&path, &["commit", "-m", "base"]);
            }
            write(&path, "intended.txt", b"not staged yet\n");
            write(&path, "forced.log", b"ignored, but intended\n");
            write(&path, "nested/gone.txt", b"deleted after git add -N\n");
            write(&path, "made/for/it.txt", b"deleted with its folders\n");
            git(
                &path,
                &[
                    "add",
                    "--intent-to-add",
                    "intended.txt",
                    "nested/gone.txt",
                    "made/for/it.txt",
                ],
            );
            git(&path, &["add", "--force", "--intent-to-add", "forced.log"]);
            std::fs::remove_file(path.join("nested/gone.txt")).expect("delete");
            std::fs::remove_dir_all(path.join("made")).expect("delete folders");
            // The same index entry, made by staging an empty file.
            write(&path, "empty.txt", b"");
            git(&path, &["add", "empty.txt"]);
            write(&path, "empty.txt", b"edited after staging\n");
            let before = exact_state(&path);

            let all = discard(&path, DiscardTarget::All);
            assert!(
                all.snapshot.changes.iter().all(|change| change.untracked),
                "unborn: {unborn}"
            );
            // An empty folder made since the discard is not the restore's to
            // remove, and the restore makes no folders for deleted entries.
            std::fs::create_dir_all(path.join("nested")).expect("folder");
            restore(&path, &all.recovery_point);
            assert_eq!(exact_state(&path), before, "unborn: {unborn}");
            assert!(!path.join("nested/gone.txt").exists(), "unborn: {unborn}");
            assert!(path.join("nested").is_dir(), "unborn: {unborn}");
            assert!(!path.join("made").exists(), "unborn: {unborn}");
        }
    }

    #[test]
    fn index_flags_restore_exactly() {
        let directory = repository();
        let path = root(&directory);
        for name in ["assumed.txt", "sparse.txt"] {
            write(&path, name, b"committed\n");
        }
        git(&path, &["add", "."]);
        git(&path, &["commit", "-m", "base"]);
        for name in ["assumed.txt", "sparse.txt"] {
            write(&path, name, b"staged\n");
            git(&path, &["add", name]);
        }
        git(
            &path,
            &["update-index", "--assume-unchanged", "assumed.txt"],
        );
        git(&path, &["update-index", "--skip-worktree", "sparse.txt"]);
        let before = exact_state(&path);

        let all = discard(&path, DiscardTarget::All);
        assert!(all.snapshot.changes.is_empty());
        restore(&path, &all.recovery_point);
        assert_eq!(exact_state(&path), before);
    }

    #[test]
    fn restoring_saves_what_it_replaces() {
        let directory = repository();
        let path = root(&directory);
        base_commit(&path);
        write(&path, "modified.txt", b"discarded edit\n");
        let discarded = discard(
            &path,
            DiscardTarget::File {
                path: path_of(&path, "modified.txt"),
                scope: DiscardScope::All,
            },
        );
        write(&path, "modified.txt", b"later edit\n");
        let reference = RecoveryPointReference {
            id: discarded.recovery_point.id.clone(),
            oid: discarded.recovery_point.oid.clone(),
        };
        let plan = plan_recovery_restore(RecoveryPointRequest {
            repository_path: request(&path).repository_path,
            worktree_path: request(&path).worktree_path,
            point: reference.clone(),
        })
        .expect("plan");
        assert_eq!(plan.entries.len(), 1);
        assert_eq!(plan.entries[0].worktree, RestoreEffect::Replace);
        assert!(!plan.entries[0].index_changes);

        write(&path, "modified.txt", b"newer edit\n");
        let stale = restore_recovery_point(RecoveryRestoreRequest {
            repository_path: request(&path).repository_path,
            worktree_path: request(&path).worktree_path,
            point: reference,
            fingerprint: plan.fingerprint,
        })
        .expect_err("stale restore review");
        assert!(
            stale.contains("changed after this restore was reviewed"),
            "{stale}"
        );

        let restored = restore(&path, &discarded.recovery_point);
        assert_eq!(
            std::fs::read(path.join("modified.txt")).expect("read"),
            b"discarded edit\n"
        );
        let replaced = restored.replaced.expect("replaced content is saved");
        assert_eq!(replaced.kind, RecoveryPointKind::Restore);
        restore(&path, &replaced);
        assert_eq!(
            std::fs::read(path.join("modified.txt")).expect("read"),
            b"newer edit\n"
        );
    }
}
