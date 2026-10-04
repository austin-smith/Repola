//! Discarding working-copy changes. A discard is planned first; execution
//! plans again, refuses unless the result matches the reviewed plan's
//! fingerprint, and saves everything it will change as a recovery point
//! before anything is touched.
//!
//! What happens to each path is decided from what the last commit, the index,
//! and the disk hold there, never from how status pairs files into renames,
//! and is carried out with plumbing that touches exactly the listed paths.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::command;
use super::models::{
    DiscardEffect, DiscardPlan, DiscardPlanEntry, DiscardPlanRequest, DiscardRequest,
    DiscardResult, DiscardScope, DiscardTarget, FileChange, GitPath, KeptChange, KeptChangeReason,
    RecoveryPointKind, WorkingCopyRequest, WorkingCopySnapshot,
};
use super::recovery::{
    create_recovery_point, file_folder_conflict, fingerprint, head_entries, observe_paths,
    prepare_recovery_point, push_index_info, remove_empty_directory, remove_worktree_entry,
    store_contents, stored_bytes, HeadEntry, PathState, Removals, CHANGED_WHILE_SAVING,
};
use super::working_copy::{decode_path_token_bytes, ensure_success, working_copy_snapshot};

const STALE_PLAN: &str =
    "The working copy changed after this discard was reviewed. Nothing was discarded; review it again.";

/// One path a discard covers, and how far back it goes.
struct Target {
    path: GitPath,
    scope: DiscardScope,
}

/// The paths a discard covers, chosen from the working copy's status.
struct Selection {
    targets: Vec<Target>,
    kept: Vec<KeptChange>,
    kind: RecoveryPointKind,
    /// The recovery point's description; Discard All counts its paths later.
    summary: Option<String>,
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

/// Status reports a rename as one record naming both paths. Discarding it
/// covers both, unless only the working tree's side of a staged rename is
/// being discarded.
fn rename_partner(change: &FileChange, scope: DiscardScope) -> Option<&GitPath> {
    let renamed =
        change.worktree_status == "R" || change.index_status == "R" && scope == DiscardScope::All;
    renamed.then_some(change.previous_path.as_ref()).flatten()
}

fn select(snapshot: &WorkingCopySnapshot, target: &DiscardTarget) -> Result<Selection, String> {
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
                return Err(SUBMODULE_REFUSAL.into());
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
            let mut targets = vec![Target {
                path: change.path.clone(),
                scope: *scope,
            }];
            if let Some(partner) = rename_partner(change, *scope) {
                targets.push(Target {
                    path: partner.clone(),
                    scope: *scope,
                });
            }
            Ok(Selection {
                targets,
                kept: Vec::new(),
                kind: RecoveryPointKind::DiscardFile,
                summary: Some(summary),
            })
        }
        DiscardTarget::All => {
            if snapshot.operation.is_some() {
                return Err(
                    "Abort or finish the in-progress Git operation before discarding every change."
                        .into(),
                );
            }
            let mut targets = Vec::new();
            let mut kept = Vec::new();
            let mut seen = HashSet::new();
            for change in snapshot.changes.iter().filter(|change| !change.ignored) {
                if is_submodule(change) {
                    kept.push(KeptChange {
                        path: change.path.clone(),
                        reason: KeptChangeReason::Submodule,
                    });
                } else if is_nested_repository(change) {
                    kept.push(KeptChange {
                        path: change.path.clone(),
                        reason: KeptChangeReason::NestedRepository,
                    });
                } else {
                    let paths = std::iter::once(&change.path)
                        .chain(rename_partner(change, DiscardScope::All));
                    for path in paths {
                        if seen.insert(path.token.clone()) {
                            targets.push(Target {
                                path: path.clone(),
                                scope: DiscardScope::All,
                            });
                        }
                    }
                }
            }
            Ok(Selection {
                targets,
                kept,
                kind: RecoveryPointKind::DiscardAll,
                summary: None,
            })
        }
    }
}

const SUBMODULE_REFUSAL: &str = "Repola does not discard submodule changes because it cannot save them first. Commit, stash, or reset them inside the submodule.";

/// Everything a discard will do, derived the same way when planning and when
/// executing.
struct Planned {
    entries: Vec<DiscardPlanEntry>,
    kept: Vec<KeptChange>,
    /// The state of every path the discard changes, which the recovery point
    /// saves.
    states: Vec<PathState>,
    kind: RecoveryPointKind,
    summary: String,
    fingerprint: String,
    /// Folders, emptied by the removals, where files are written back.
    empty_folders: Vec<GitPath>,
    /// Files removed from disk.
    remove: Vec<GitPath>,
    /// Input for `update-index --index-info`: removals, then writes.
    index_removals: Vec<u8>,
    index_writes: Vec<u8>,
    /// Paths `checkout-index` writes from the index, NUL-terminated.
    checkout: Vec<u8>,
}

/// A target's effect, decided before any write is checked against what the
/// rest of the plan removes.
struct Candidate<'a> {
    target: &'a Target,
    bytes: Vec<u8>,
    effect: DiscardEffect,
    head: Option<&'a HeadEntry>,
    state: PathState,
}

fn plan(snapshot: &WorkingCopySnapshot, target: &DiscardTarget) -> Result<Planned, String> {
    let selection = select(snapshot, target)?;
    let single = matches!(target, DiscardTarget::File { .. });
    let worktree = Path::new(&snapshot.worktree_path);
    let head = snapshot.head.as_deref();
    let paths: Vec<GitPath> = selection
        .targets
        .iter()
        .map(|target| target.path.clone())
        .collect();
    let observed = observe_paths(worktree, head, &paths, false)?;
    let heads = match head {
        Some(head) => head_entries(worktree, head, &paths)?,
        None => HashMap::new(),
    };
    let mut kept = selection.kept;
    let mut keep_or_refuse = |target: &Target, reason, refusal: String| {
        if single {
            return Err(refusal);
        }
        kept.push(KeptChange {
            path: target.path.clone(),
            reason,
        });
        Ok(())
    };

    let mut candidates = Vec::new();
    for (target, state) in selection.targets.iter().zip(observed) {
        let bytes = decode_path_token_bytes(&target.path.token)?;
        let head_entry = heads.get(&bytes);
        if head_entry.is_some_and(|entry| entry.kind == "commit")
            || state.index.iter().any(|entry| entry.mode == "160000")
        {
            keep_or_refuse(
                target,
                KeptChangeReason::Submodule,
                SUBMODULE_REFUSAL.into(),
            )?;
            continue;
        }
        if target.scope == DiscardScope::Unstaged && state.index.iter().any(|entry| entry.stage > 0)
        {
            return Err("Resolve the conflict explicitly instead of discarding it.".into());
        }
        if let Some(effect) = effect_of(target.scope, head_entry, &state) {
            candidates.push(Candidate {
                target,
                bytes,
                effect,
                head: head_entry,
                state,
            });
        }
    }

    // A write may replace only what the plan itself removes first: a file in
    // the way of its folder, or index entries its own entry would displace.
    let mut removals = Removals::default();
    for candidate in &candidates {
        if candidate.effect == DiscardEffect::Remove {
            removals.files.insert(&candidate.bytes);
        }
        if matches!(
            candidate.effect,
            DiscardEffect::Remove | DiscardEffect::Unstage
        ) && !candidate.state.index.is_empty()
        {
            removals.entries.insert(&candidate.bytes);
        }
    }
    let mut planned = Planned {
        entries: Vec::new(),
        kept: Vec::new(),
        states: Vec::new(),
        kind: selection.kind,
        summary: String::new(),
        fingerprint: String::new(),
        empty_folders: Vec::new(),
        remove: Vec::new(),
        index_removals: Vec::new(),
        index_writes: Vec::new(),
        checkout: Vec::new(),
    };
    for candidate in &candidates {
        let state = &candidate.state;
        let writes = matches!(
            candidate.effect,
            DiscardEffect::RestoreCommitted | DiscardEffect::RestoreStaged
        );
        if writes {
            if state.directory && !removals.empties_folder(worktree, &state.path)? {
                keep_or_refuse(
                    candidate.target,
                    KeptChangeReason::FileFolderConflict,
                    format!(
                        "{} is now a directory in the working copy, so Repola will not discard over it. Move it aside and review the discard again.",
                        state.path.display
                    ),
                )?;
                continue;
            }
            let writes_index = candidate.effect == DiscardEffect::RestoreCommitted;
            let crosses = removals.crossed_by(&candidate.bytes, state, true, writes_index);
            if crosses {
                keep_or_refuse(
                    candidate.target,
                    KeptChangeReason::FileFolderConflict,
                    file_folder_conflict(&state.path.display),
                )?;
                continue;
            }
            if state.directory {
                planned.empty_folders.push(state.path.clone());
            }
        }
        add_effect(&mut planned, candidate)?;
        planned.entries.push(DiscardPlanEntry {
            path: candidate.target.path.clone(),
            effect: candidate.effect,
            on_disk: state.worktree.is_some(),
            tracked: !state.index.is_empty(),
        });
        planned.states.push(candidate.state.clone());
    }
    planned.kept = kept;

    if planned.entries.is_empty() {
        return Err(if single {
            "The selected change no longer exists. Refresh and try again.".into()
        } else if planned.kept.is_empty() {
            "There are no changes to discard.".into()
        } else {
            "Every change here is one Repola leaves alone; see why for each below.".into()
        });
    }
    let count = planned.entries.len();
    planned.summary = selection.summary.unwrap_or_else(|| {
        format!(
            "Discarded all changes ({count} file{})",
            if count == 1 { "" } else { "s" }
        )
    });
    planned.fingerprint = fingerprint(
        head,
        snapshot.operation,
        &planned.states,
        &(&planned.entries, &planned.kept),
    )?;
    Ok(planned)
}

/// What discarding `state` does: back to the last commit for everything, or
/// back to the staged version for unstaged changes. A path the last commit
/// lacks is removed, as is anything not staged at all; an intent-to-add entry
/// stages nothing, so all of its changes are unstaged ones. `None` means
/// there is nothing at the path to discard.
fn effect_of(
    scope: DiscardScope,
    head: Option<&HeadEntry>,
    state: &PathState,
) -> Option<DiscardEffect> {
    let staged = state.index.iter().any(|entry| !entry.intent_to_add);
    if scope == DiscardScope::Unstaged && staged {
        return Some(DiscardEffect::RestoreStaged);
    }
    if scope == DiscardScope::All && head.is_some_and(|entry| entry.kind == "blob") {
        return Some(DiscardEffect::RestoreCommitted);
    }
    if state.worktree.is_some() {
        Some(DiscardEffect::Remove)
    } else if !state.index.is_empty() {
        Some(DiscardEffect::Unstage)
    } else {
        None
    }
}

fn add_effect(planned: &mut Planned, candidate: &Candidate) -> Result<(), String> {
    let path = candidate.bytes.as_slice();
    let unstage = |planned: &mut Planned| {
        if let Some(entry) = candidate.state.index.first() {
            // A zero mode removes every stage of the path.
            push_index_info(
                &mut planned.index_removals,
                "0",
                &"0".repeat(entry.oid.len()),
                path,
            );
        }
    };
    match candidate.effect {
        DiscardEffect::RestoreCommitted => {
            let head = candidate
                .head
                .ok_or_else(|| "The last commit no longer has this path.".to_string())?;
            // A stage-zero entry replaces every conflict stage of the path.
            push_index_info(&mut planned.index_writes, &head.mode, &head.oid, path);
            planned.checkout.extend_from_slice(path);
            planned.checkout.push(0);
        }
        DiscardEffect::RestoreStaged => {
            planned.checkout.extend_from_slice(path);
            planned.checkout.push(0);
        }
        DiscardEffect::Remove => {
            unstage(planned);
            planned.remove.push(candidate.state.path.clone());
        }
        DiscardEffect::Unstage => unstage(planned),
    }
    Ok(())
}

/// Plans a discard without changing anything.
pub fn plan_discard(request: DiscardPlanRequest) -> Result<DiscardPlan, String> {
    let snapshot = working_copy_snapshot(WorkingCopyRequest {
        repository_path: request.repository_path,
        worktree_path: request.worktree_path,
    })?;
    let planned = plan(&snapshot, &request.target)?;
    Ok(DiscardPlan {
        target: request.target,
        backup_bytes: stored_bytes(&planned.states),
        entries: planned.entries,
        kept: planned.kept,
        fingerprint: planned.fingerprint,
    })
}

/// Executes a reviewed discard. The content it removes is saved as a
/// recovery point first; the returned point names where it went.
pub fn discard_changes(request: DiscardRequest) -> Result<DiscardResult, String> {
    let snapshot = working_copy_snapshot(WorkingCopyRequest {
        repository_path: request.repository_path,
        worktree_path: request.worktree_path,
    })?;
    let worktree = Path::new(&snapshot.worktree_path);
    let planned = plan(&snapshot, &request.target)?;
    if planned.fingerprint != request.fingerprint {
        return Err(STALE_PLAN.into());
    }
    store_contents(worktree, &planned.states)?;
    let prepared = prepare_recovery_point(
        worktree,
        planned.kind,
        planned.summary.clone(),
        snapshot.head.as_deref(),
        &planned.states,
    )?;
    // The last look before changing anything, at HEAD and the in-progress
    // operation as well as the paths: only naming the recovery point and the
    // discard itself follow.
    let latest = working_copy_snapshot(WorkingCopyRequest {
        repository_path: snapshot.repository_path.clone(),
        worktree_path: snapshot.worktree_path.clone(),
    })?;
    if plan(&latest, &request.target)?.fingerprint != planned.fingerprint {
        return Err(CHANGED_WHILE_SAVING.into());
    }
    let recovery_point = create_recovery_point(worktree, prepared)?;
    execute(worktree, &planned).map_err(|error| {
        format!(
            "{error}\n\nEverything this discard was changing is saved in recovery point {}. Restore it from Discarded Changes.",
            recovery_point.id
        )
    })?;
    Ok(DiscardResult { recovery_point })
}

/// Clears what is in the way first, so a write never replaces anything the
/// recovery point did not save: files the discard removes, the folders they
/// leave empty where files go back, and index entries it removes, before any
/// index entry or file is written.
fn execute(worktree: &Path, planned: &Planned) -> Result<(), String> {
    for path in &planned.remove {
        remove_worktree_entry(worktree, path)?;
    }
    for folder in &planned.empty_folders {
        remove_empty_directory(worktree, folder)?;
    }
    let index_info = [planned.index_removals.as_slice(), &planned.index_writes].concat();
    if !index_info.is_empty() {
        let output = command::git_at_with_input(
            worktree,
            ["update-index", "-z", "--index-info"],
            &index_info,
        )
        .map_err(|error| error.to_string())?;
        ensure_success(output, "update the index")?;
    }
    if !planned.checkout.is_empty() {
        // Writes each listed index entry as Git would on checkout, applying
        // filters and line-ending settings. Without `-u` it leaves the index
        // alone, so a run cut short never leaves the index locked; the
        // refresh afterwards records the written files' stat data.
        let output = command::git_at_with_input(
            worktree,
            ["checkout-index", "--force", "-z", "--stdin"],
            &planned.checkout,
        )
        .map_err(|error| error.to_string())?;
        ensure_success(output, "write the restored files")?;
        let output = command::git_at(worktree, ["update-index", "-q", "--unmerged", "--refresh"])
            .map_err(|error| error.to_string())?;
        ensure_success(output, "refresh the index")?;
    }
    Ok(())
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

    fn changes(path: &Path) -> Vec<FileChange> {
        working_copy_snapshot(request(path))
            .expect("snapshot")
            .changes
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
        // A rename is its new name and its original one.
        assert_eq!(plan.entries.len(), 8);
        let effect = |name: &str| {
            plan.entries
                .iter()
                .find(|entry| entry.path.display == name)
                .expect("entry")
                .effect
        };
        assert_eq!(effect("added.txt"), DiscardEffect::Remove);
        assert_eq!(effect("nested/dir/untracked.bin"), DiscardEffect::Remove);
        assert_eq!(effect("renamed.txt"), DiscardEffect::Remove);
        assert_eq!(effect("rename-me.txt"), DiscardEffect::RestoreCommitted);
        assert!(plan.backup_bytes > 0);

        let result = discard(&path, DiscardTarget::All);
        assert!(changes(&path).is_empty());
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
        assert!(changes(&path)
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
        assert!(changes(&path).is_empty());
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
        assert!(changes(&path).is_empty());
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
        assert!(changes(&path).is_empty());
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
        assert!(changes(&path).is_empty());
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
    fn a_plan_reports_the_effect_its_observation_selects() {
        let directory = repository();
        let path = root(&directory);
        base_commit(&path);
        write(&path, "modified.txt", b"edited\n");
        let target = DiscardTarget::File {
            path: path_of(&path, "modified.txt"),
            scope: DiscardScope::Unstaged,
        };
        // The file leaves the index between the snapshot and the observation.
        // Status still called it a tracked file with unstaged edits, but the
        // plan reports removing the file, which is what would run.
        let snapshot = working_copy_snapshot(request(&path)).expect("snapshot");
        git(&path, &["rm", "--cached", "--quiet", "modified.txt"]);
        let planned = super::plan(&snapshot, &target).expect("plan");
        assert_eq!(planned.entries[0].effect, DiscardEffect::Remove);
        assert!(!planned.entries[0].tracked);
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

    fn kept_paths(plan: &DiscardPlan) -> Vec<&str> {
        plan.kept
            .iter()
            .map(|kept| kept.path.display.as_str())
            .collect()
    }

    #[test]
    fn a_file_where_a_restored_file_needs_its_folder_is_never_replaced() {
        let directory = repository();
        let path = root(&directory);
        write(&path, "d/x", b"committed\n");
        write(&path, ".gitignore", b"build\n");
        write(&path, "build/out", b"committed\n");
        git(&path, &["add", "--force", "."]);
        git(&path, &["commit", "-m", "base"]);
        // The folders are gone and files took their names, one of them ignored.
        std::fs::remove_dir_all(path.join("d")).expect("remove folder");
        write(&path, "d", b"precious\n");
        std::fs::remove_dir_all(path.join("build")).expect("remove folder");
        write(&path, "build", b"ignored but precious\n");
        let before = exact_state(&path);

        for scope in [DiscardScope::All, DiscardScope::Unstaged] {
            let error = plan_error(
                &path,
                DiscardTarget::File {
                    path: path_of(&path, "d/x"),
                    scope,
                },
            );
            assert!(
                error.contains("a file in one place and a folder"),
                "{error}"
            );
        }
        // Discarding everything saves and removes the file `d` first, so `d/x`
        // can come back; the ignored file is not part of the discard, so it
        // stays and `build/out` is left alone.
        let all = plan(&path, DiscardTarget::All);
        assert_eq!(kept_paths(&all), ["build/out"]);
        assert_eq!(effect_of(&all, "d"), DiscardEffect::Remove);
        assert_eq!(effect_of(&all, "d/x"), DiscardEffect::RestoreCommitted);
        let discarded = discard(&path, DiscardTarget::All);
        assert_eq!(
            std::fs::read(path.join("d/x")).expect("restored"),
            b"committed\n"
        );
        assert_eq!(
            std::fs::read(path.join("build")).expect("ignored file kept"),
            b"ignored but precious\n"
        );
        restore(&path, &discarded.recovery_point);
        assert_eq!(exact_state(&path), before);
        assert_eq!(std::fs::read(path.join("d")).expect("saved"), b"precious\n");
    }

    #[cfg(unix)]
    #[test]
    fn a_link_where_a_restored_file_needs_its_folder_is_never_replaced() {
        let directory = repository();
        let path = root(&directory);
        write(&path, "d/x", b"committed\n");
        git(&path, &["add", "."]);
        git(&path, &["commit", "-m", "base"]);
        let outside = tempfile::tempdir().expect("outside");
        std::fs::remove_dir_all(path.join("d")).expect("remove folder");
        std::os::unix::fs::symlink(outside.path(), path.join("d")).expect("link");
        let error = plan_error(&path, file(&path, "d/x"));
        assert!(
            error.contains("a file in one place and a folder"),
            "{error}"
        );
        assert!(std::fs::symlink_metadata(path.join("d"))
            .expect("link kept")
            .file_type()
            .is_symlink());
    }

    #[test]
    fn a_path_whose_folder_is_staged_beneath_it_is_never_restored_over_it() {
        let directory = repository();
        let path = root(&directory);
        write(&path, "a", b"committed file\n");
        git(&path, &["add", "."]);
        git(&path, &["commit", "-m", "base"]);
        // `a` is deleted, and a folder of the same name was staged and then
        // deleted from disk.
        git(&path, &["rm", "--quiet", "a"]);
        write(&path, "a/x", b"staged beneath\n");
        git(&path, &["add", "a/x"]);
        std::fs::remove_dir_all(path.join("a")).expect("remove folder");
        let staged = git(&path, &["ls-files", "--stage"]);

        let error = plan_error(&path, file(&path, "a"));
        assert!(
            error.contains("a file in one place and a folder"),
            "{error}"
        );
        assert_eq!(git(&path, &["ls-files", "--stage"]), staged);
        // Discarding everything unstages `a/x` first, so `a` can come back.
        let before = exact_state(&path);
        let all = plan(&path, DiscardTarget::All);
        assert!(all.kept.is_empty());
        assert_eq!(effect_of(&all, "a/x"), DiscardEffect::Unstage);
        assert_eq!(effect_of(&all, "a"), DiscardEffect::RestoreCommitted);
        let discarded = discard(&path, DiscardTarget::All);
        assert!(changes(&path).is_empty());
        restore(&path, &discarded.recovery_point);
        assert_eq!(exact_state(&path), before);
    }

    #[test]
    fn removing_a_file_never_restores_the_folder_it_replaced() {
        let directory = repository();
        let path = root(&directory);
        write(&path, "a/x", b"committed\n");
        write(&path, "a/y", b"committed\n");
        git(&path, &["add", "."]);
        git(&path, &["commit", "-m", "base"]);
        git(&path, &["rm", "-r", "--quiet", "a"]);
        write(&path, "a", b"staged file\n");
        git(&path, &["add", "a"]);
        let before = exact_state(&path);

        let reviewed = plan(&path, file(&path, "a"));
        assert_eq!(reviewed.entries.len(), 1);
        assert_eq!(effect_of(&reviewed, "a"), DiscardEffect::Remove);
        let discarded = discard(&path, file(&path, "a"));
        assert!(!path.join("a").exists());
        // The staged deletions beneath were not part of the plan and stay.
        assert_eq!(git(&path, &["ls-files"]), "");
        restore(&path, &discarded.recovery_point);
        assert_eq!(exact_state(&path), before);
    }

    #[test]
    fn a_rename_git_sees_only_on_disk_is_undone_on_both_paths() {
        let directory = repository();
        let path = root(&directory);
        write(&path, "old.txt", b"content\n");
        git(&path, &["add", "."]);
        git(&path, &["commit", "-m", "base"]);
        std::fs::rename(path.join("old.txt"), path.join("new.txt")).expect("move");
        git(&path, &["add", "--intent-to-add", "new.txt"]);
        let before = exact_state(&path);
        assert!(changes(&path)
            .iter()
            .any(|change| change.worktree_status == "R"));

        for scope in [DiscardScope::Unstaged, DiscardScope::All] {
            let target = DiscardTarget::File {
                path: path_of(&path, "new.txt"),
                scope,
            };
            let reviewed = plan(&path, target.clone());
            assert_eq!(effect_of(&reviewed, "new.txt"), DiscardEffect::Remove);
            let discarded = discard(&path, target);
            assert!(changes(&path).is_empty(), "{scope:?}");
            assert_eq!(
                std::fs::read(path.join("old.txt")).expect("restored"),
                b"content\n"
            );
            restore(&path, &discarded.recovery_point);
            assert_eq!(exact_state(&path), before, "{scope:?}");
        }
    }

    #[test]
    fn an_intent_to_add_file_is_removed_not_emptied() {
        let directory = repository();
        let path = root(&directory);
        git(&path, &["commit", "--allow-empty", "-m", "base"]);
        write(&path, "intended.txt", b"not staged yet\n");
        git(&path, &["add", "--intent-to-add", "intended.txt"]);
        write(&path, "gone.txt", b"deleted after git add -N\n");
        git(&path, &["add", "--intent-to-add", "gone.txt"]);
        std::fs::remove_file(path.join("gone.txt")).expect("delete");

        // An intent-to-add entry stages nothing, so all of its changes are
        // unstaged ones.
        let target = DiscardTarget::File {
            path: path_of(&path, "intended.txt"),
            scope: DiscardScope::Unstaged,
        };
        assert_eq!(
            effect_of(&plan(&path, target.clone()), "intended.txt"),
            DiscardEffect::Remove
        );
        discard(&path, target);
        assert!(!path.join("intended.txt").exists());
        // Git reports the deleted entry with a committed mode; it is still
        // only an index entry to drop.
        assert_eq!(
            effect_of(&plan(&path, file(&path, "gone.txt")), "gone.txt"),
            DiscardEffect::Unstage
        );
        discard(&path, file(&path, "gone.txt"));
        assert!(changes(&path).is_empty());
    }

    #[test]
    fn a_conflict_the_last_commit_lacks_is_discarded_and_restored() {
        let directory = repository();
        let path = root(&directory);
        write(&path, "f", b"base\n");
        git(&path, &["add", "."]);
        git(&path, &["commit", "-m", "base"]);
        write(&path, "f", b"stashed\n");
        git(&path, &["stash", "--quiet"]);
        git(&path, &["rm", "--quiet", "f"]);
        git(&path, &["commit", "-m", "delete"]);
        // Applying the stash conflicts: deleted by us, modified by them.
        let output = command::git_at(&path, ["stash", "apply"]).expect("apply");
        assert!(!output.status.success());
        assert!(changes(&path).iter().any(|change| change.conflicted));
        let before = exact_state(&path);

        let all = plan(&path, DiscardTarget::All);
        assert_eq!(effect_of(&all, "f"), DiscardEffect::Remove);
        let discarded = discard(&path, DiscardTarget::All);
        assert!(changes(&path).is_empty());
        restore(&path, &discarded.recovery_point);
        assert_eq!(exact_state(&path), before);
    }

    #[cfg(unix)]
    #[test]
    fn an_unborn_branch_never_removes_through_a_linked_parent() {
        let directory = repository();
        let path = root(&directory);
        let outside = tempfile::tempdir().expect("outside");
        write(outside.path(), "f.txt", b"outside\n");
        write(&path, "p/f.txt", b"staged\n");
        git(&path, &["add", "p/f.txt"]);
        std::fs::remove_dir_all(path.join("p")).expect("remove folder");
        std::os::unix::fs::symlink(outside.path(), path.join("p")).expect("link");

        let all = plan(&path, DiscardTarget::All);
        assert_eq!(effect_of(&all, "p/f.txt"), DiscardEffect::Unstage);
        discard(&path, DiscardTarget::All);
        assert_eq!(
            std::fs::read(outside.path().join("f.txt")).expect("outside kept"),
            b"outside\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn permissions_and_folders_restore_exactly() {
        use std::os::unix::fs::PermissionsExt;
        let directory = repository();
        let path = root(&directory);
        write(&path, "old/dir/f.txt", b"committed\n");
        git(&path, &["add", "."]);
        git(&path, &["commit", "-m", "base"]);
        // Moving the file leaves its folders behind, empty.
        git(&path, &["mv", "old/dir/f.txt", "new.txt"]);
        write(&path, ".env.local", b"secret\n");
        std::fs::set_permissions(
            path.join(".env.local"),
            std::fs::Permissions::from_mode(0o600),
        )
        .expect("private");

        let discarded = discard(&path, DiscardTarget::All);
        restore(&path, &discarded.recovery_point);
        let mode = std::fs::metadata(path.join(".env.local"))
            .expect("restored")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        assert!(path.join("old/dir").is_dir());
    }

    #[test]
    fn discarding_more_paths_than_are_asked_about_one_by_one_restores_exactly() {
        let directory = repository();
        let path = root(&directory);
        for index in 0..600 {
            write(&path, &format!("files/{index}.txt"), b"committed\n");
        }
        git(&path, &["add", "."]);
        git(&path, &["commit", "-m", "base"]);
        for index in 0..600 {
            write(&path, &format!("files/{index}.txt"), b"edited\n");
        }
        write(&path, "files/0.txt", b"staged\n");
        git(&path, &["add", "files/0.txt"]);
        write(&path, "intended.txt", b"intent\n");
        git(&path, &["add", "--intent-to-add", "intended.txt"]);
        let before = exact_state(&path);

        let discarded = discard(&path, DiscardTarget::All);
        assert!(changes(&path).is_empty());
        restore(&path, &discarded.recovery_point);
        assert_eq!(exact_state(&path), before);
    }

    #[test]
    fn a_restore_never_displaces_staged_entries_across_a_file_and_a_folder() {
        let directory = repository();
        let path = root(&directory);
        git(&path, &["commit", "--allow-empty", "-m", "base"]);
        let restore_plan_error = |point: &RecoveryPoint| {
            plan_recovery_restore(RecoveryPointRequest {
                repository_path: request(&path).repository_path,
                worktree_path: request(&path).worktree_path,
                point: RecoveryPointReference {
                    id: point.id.clone(),
                    oid: point.oid.clone(),
                },
            })
            .expect_err("restore refused")
        };

        // A saved staged file `a`, while `a/x` is staged now.
        write(&path, "a", b"saved file\n");
        git(&path, &["add", "a"]);
        let file_point = discard(&path, file(&path, "a")).recovery_point;
        write(&path, "a/x", b"staged now\n");
        git(&path, &["add", "a/x"]);
        std::fs::remove_dir_all(path.join("a")).expect("remove folder");
        let staged = git(&path, &["ls-files", "--stage"]);
        let error = restore_plan_error(&file_point);
        assert!(
            error.contains("a file in one place and a folder"),
            "{error}"
        );
        assert_eq!(git(&path, &["ls-files", "--stage"]), staged);

        // A saved staged `b/x`, while a file `b` is staged now.
        write(&path, "b/x", b"saved beneath\n");
        git(&path, &["add", "b/x"]);
        std::fs::remove_dir_all(path.join("b")).expect("remove folder");
        let beneath_point = discard(&path, file(&path, "b/x")).recovery_point;
        write(&path, "b", b"staged now\n");
        git(&path, &["add", "b"]);
        let staged = git(&path, &["ls-files", "--stage"]);
        let error = restore_plan_error(&beneath_point);
        assert!(
            error.contains("a file in one place and a folder"),
            "{error}"
        );
        assert_eq!(git(&path, &["ls-files", "--stage"]), staged);
    }

    #[test]
    fn a_file_discards_beside_an_unrelated_conflict() {
        let directory = repository();
        let path = root(&directory);
        write(&path, "conflicted.txt", b"base\n");
        write(&path, "edited.txt", b"committed\n");
        git(&path, &["add", "."]);
        git(&path, &["commit", "-m", "base"]);
        git(&path, &["checkout", "--quiet", "-b", "other"]);
        write(&path, "conflicted.txt", b"theirs\n");
        git(&path, &["commit", "--quiet", "-am", "theirs"]);
        git(&path, &["checkout", "--quiet", "-"]);
        write(&path, "conflicted.txt", b"ours\n");
        git(&path, &["commit", "--quiet", "-am", "ours"]);
        let output = command::git_at(&path, ["merge", "other"]).expect("merge");
        assert!(!output.status.success());
        write(&path, "edited.txt", b"edited\n");

        discard(&path, file(&path, "edited.txt"));
        assert_eq!(
            std::fs::read(path.join("edited.txt")).expect("read"),
            b"committed\n"
        );
        assert!(changes(&path)
            .iter()
            .any(|change| change.conflicted && change.path.display == "conflicted.txt"));
    }

    #[test]
    fn a_discard_cut_short_names_its_recovery_point_and_restores_exactly() {
        let directory = repository();
        let path = root(&directory);
        write(&path, ".gitattributes", b"*.dat filter=strict\n");
        git(&path, &["config", "filter.strict.clean", "cat"]);
        git(&path, &["config", "filter.strict.smudge", "cat"]);
        git(&path, &["config", "filter.strict.required", "true"]);
        write(&path, "a.dat", b"committed\n");
        write(&path, "b.txt", b"committed\n");
        git(&path, &["add", "."]);
        git(&path, &["commit", "-m", "base"]);
        write(&path, "a.dat", b"edited\n");
        write(&path, "b.txt", b"edited\n");
        write(&path, "new.txt", b"untracked\n");
        let before = exact_state(&path);
        let reviewed = plan(&path, DiscardTarget::All);
        // Writing the committed file back now fails partway through.
        git(&path, &["config", "filter.strict.smudge", "false"]);

        let error = discard_changes(DiscardRequest {
            repository_path: request(&path).repository_path,
            worktree_path: request(&path).worktree_path,
            target: DiscardTarget::All,
            fingerprint: reviewed.fingerprint,
        })
        .expect_err("filter fails");
        let points = list_recovery_points(request(&path)).expect("list").points;
        assert_eq!(points.len(), 1);
        assert!(error.contains(&points[0].id), "{error}");
        git(&path, &["config", "filter.strict.smudge", "cat"]);
        restore(&path, &points[0]);
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
        assert!(changes(&path).is_empty());
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

        let error = plan_error(&path, file(&path, "swapped"));
        assert!(error.contains("swapped is now a directory"), "{error}");
        // Discarding everything removes the untracked file inside, which is
        // saved, so the folder is empty when the committed file comes back.
        let before = exact_state(&path);
        let all = plan(&path, DiscardTarget::All);
        assert!(all.kept.is_empty());
        assert_eq!(effect_of(&all, "swapped"), DiscardEffect::RestoreCommitted);
        let discarded = discard(&path, DiscardTarget::All);
        assert_eq!(
            std::fs::read(path.join("swapped")).expect("restored"),
            b"committed\n"
        );
        restore(&path, &discarded.recovery_point);
        assert_eq!(exact_state(&path), before);

        // A directory that appears after the review invalidates it, and
        // nothing is saved.
        let saved_points = list_recovery_points(request(&path))
            .expect("list")
            .points
            .len();
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
        assert_eq!(
            list_recovery_points(request(&path))
                .expect("list")
                .points
                .len(),
            saved_points
        );

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
        // The rename's original path is restored and its new name removed.
        assert_eq!(planned, ["moved.txt", "renamed.txt", "unindexed.txt"]);
        let all = discard(&path, DiscardTarget::All);
        assert!(changes(&path).is_empty());
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
                write(&path, "committed.txt", b"committed\n");
                write(&path, "committed-empty.txt", b"");
                git(&path, &["add", "."]);
                git(&path, &["commit", "-m", "base"]);
                // Intent-to-add entries for paths HEAD also has.
                for name in ["committed.txt", "committed-empty.txt"] {
                    git(&path, &["rm", "--cached", "--quiet", name]);
                    git(&path, &["add", "--intent-to-add", name]);
                }
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
                changes(&path).iter().all(|change| change.untracked),
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
        assert!(changes(&path).is_empty());
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
