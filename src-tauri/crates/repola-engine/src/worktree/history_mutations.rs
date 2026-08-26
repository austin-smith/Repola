use std::ffi::OsString;
use std::path::Path;

use super::command;
use super::models::{
    HistoryMutationKind, HistoryMutationRequest, HistoryMutationResult, ReviewedFileChange,
    WorkingCopyRequest, WorkingCopySnapshot,
};
use super::working_copy::working_copy_snapshot;

pub fn mutate_history(request: HistoryMutationRequest) -> Result<HistoryMutationResult, String> {
    validate_target(&request.target)?;
    let before = working_copy_snapshot(WorkingCopyRequest {
        repository_path: request.repository_path,
        worktree_path: request.worktree_path,
    })?;
    if before.head != request.expected_head {
        return Err(
            "The working-copy HEAD changed after this history action was reviewed. Refresh and try again."
                .into(),
        );
    }
    if before.operation.is_some() {
        return Err(
            "Finish or abort the in-progress Git operation before starting another history action."
                .into(),
        );
    }
    revalidate_changes(&before, request.expected_changes)?;
    if requires_clean_worktree(request.kind) && before.changes.iter().any(|change| !change.ignored)
    {
        return Err("Commit or stash local changes before this history action.".into());
    }
    if before.head.is_none() {
        return Err("Create the first commit before changing repository history.".into());
    }
    if before.branch.is_none()
        && matches!(
            request.kind,
            HistoryMutationKind::Merge
                | HistoryMutationKind::SquashMerge
                | HistoryMutationKind::Rebase
        )
    {
        return Err("Check out a branch before merging or rebasing.".into());
    }

    let worktree = Path::new(&before.worktree_path);
    let target = resolve_commit(worktree, &request.target)?;
    let args = arguments(request.kind, &target);
    let output = command::git_at(worktree, args).map_err(|error| error.to_string())?;
    let diagnostic = combined_output(&output.stdout, &output.stderr);
    let snapshot = working_copy_snapshot(WorkingCopyRequest {
        repository_path: before.repository_path,
        worktree_path: before.worktree_path,
    })?;
    let conflicted = snapshot.changes.iter().any(|change| change.conflicted);
    if !output.status.success() && !conflicted && snapshot.operation.is_none() {
        return Err(if diagnostic.is_empty() {
            "Git rejected the history action without diagnostic output.".into()
        } else {
            diagnostic
        });
    }
    Ok(HistoryMutationResult {
        snapshot,
        succeeded: output.status.success(),
        output: diagnostic,
        conflicted,
    })
}

fn requires_clean_worktree(kind: HistoryMutationKind) -> bool {
    matches!(
        kind,
        HistoryMutationKind::Merge
            | HistoryMutationKind::SquashMerge
            | HistoryMutationKind::Rebase
            | HistoryMutationKind::CherryPick
            | HistoryMutationKind::Revert
    )
}

fn revalidate_changes(
    snapshot: &WorkingCopySnapshot,
    mut reviewed: Vec<ReviewedFileChange>,
) -> Result<(), String> {
    reviewed.sort_by(|left, right| left.path_token.cmp(&right.path_token));
    if reviewed
        .windows(2)
        .any(|pair| pair[0].path_token == pair[1].path_token)
    {
        return Err("The reviewed history action contains a duplicate path.".into());
    }
    let mut current: Vec<_> = snapshot
        .changes
        .iter()
        .filter(|change| !change.ignored)
        .map(|change| ReviewedFileChange {
            path_token: change.path.token.clone(),
            previous_path_token: change.previous_path.as_ref().map(|path| path.token.clone()),
            index_status: change.index_status.clone(),
            worktree_status: change.worktree_status.clone(),
        })
        .collect();
    current.sort_by(|left, right| left.path_token.cmp(&right.path_token));
    if current != reviewed {
        return Err(
            "The working-copy changes no longer match the reviewed history action. Refresh and review it again."
                .into(),
        );
    }
    Ok(())
}

fn validate_target(target: &str) -> Result<(), String> {
    if target.is_empty()
        || target.len() > 1024
        || target.starts_with('-')
        || target.chars().any(char::is_control)
    {
        return Err("The selected history target is invalid.".into());
    }
    Ok(())
}

fn resolve_commit(worktree: &Path, target: &str) -> Result<String, String> {
    let revision = format!("{target}^{{commit}}");
    let output = command::git_at(
        worktree,
        [
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &revision,
        ],
    )
    .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(
            "The selected branch or commit no longer exists. Refresh and try again.".into(),
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn arguments(kind: HistoryMutationKind, target: &str) -> Vec<OsString> {
    let values: Vec<&str> = match kind {
        HistoryMutationKind::Merge => vec!["-c", "core.editor=true", "merge", "--no-edit", target],
        HistoryMutationKind::SquashMerge => vec!["merge", "--squash", target],
        HistoryMutationKind::Rebase => vec![
            "-c",
            "core.editor=true",
            "-c",
            "sequence.editor=true",
            "rebase",
            target,
        ],
        HistoryMutationKind::CherryPick => {
            vec!["-c", "core.editor=true", "cherry-pick", target]
        }
        HistoryMutationKind::Revert => {
            vec!["-c", "core.editor=true", "revert", "--no-edit", target]
        }
        HistoryMutationKind::ResetSoft => vec!["reset", "--soft", target],
        HistoryMutationKind::ResetMixed => vec!["reset", "--mixed", target],
        HistoryMutationKind::ResetHard => vec!["reset", "--hard", "--recurse-submodules", target],
    };
    values.into_iter().map(OsString::from).collect()
}

fn combined_output(stdout: &[u8], stderr: &[u8]) -> String {
    [stdout, stderr]
        .into_iter()
        .map(|bytes| String::from_utf8_lossy(bytes).trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository() -> tempfile::TempDir {
        let repository = tempfile::tempdir().expect("repository");
        command::successful_git_at(repository.path(), ["init", "-b", "main"]).expect("init");
        command::successful_git_at(
            repository.path(),
            ["config", "user.name", "History Actions"],
        )
        .expect("name");
        command::successful_git_at(
            repository.path(),
            ["config", "user.email", "history-actions@example.invalid"],
        )
        .expect("email");
        std::fs::write(repository.path().join("base.txt"), "base\n").expect("base");
        command::successful_git_at(repository.path(), ["add", "."]).expect("add");
        command::successful_git_at(repository.path(), ["commit", "-m", "base"]).expect("commit");
        repository
    }

    fn request(
        snapshot: &WorkingCopySnapshot,
        kind: HistoryMutationKind,
        target: String,
    ) -> HistoryMutationRequest {
        HistoryMutationRequest {
            repository_path: snapshot.repository_path.clone(),
            worktree_path: snapshot.worktree_path.clone(),
            kind,
            target,
            expected_head: snapshot.head.clone(),
            expected_changes: snapshot
                .changes
                .iter()
                .filter(|change| !change.ignored)
                .map(|change| ReviewedFileChange {
                    path_token: change.path.token.clone(),
                    previous_path_token: change
                        .previous_path
                        .as_ref()
                        .map(|path| path.token.clone()),
                    index_status: change.index_status.clone(),
                    worktree_status: change.worktree_status.clone(),
                })
                .collect(),
        }
    }

    fn snapshot(repository: &tempfile::TempDir) -> WorkingCopySnapshot {
        let path = repository.path().to_string_lossy().into_owned();
        working_copy_snapshot(WorkingCopyRequest {
            repository_path: path.clone(),
            worktree_path: path,
        })
        .expect("snapshot")
    }

    #[test]
    fn cherry_picks_reverts_and_revalidates_reset_state() {
        let repository = repository();
        let base = snapshot(&repository).head.expect("base");
        command::successful_git_at(repository.path(), ["switch", "-c", "feature"])
            .expect("feature");
        std::fs::write(repository.path().join("feature.txt"), "feature\n").expect("feature file");
        command::successful_git_at(repository.path(), ["add", "."]).expect("add feature");
        command::successful_git_at(repository.path(), ["commit", "-m", "feature"])
            .expect("feature commit");
        let feature = snapshot(&repository).head.expect("feature head");
        command::successful_git_at(repository.path(), ["switch", "main"]).expect("main");

        let picked = mutate_history(request(
            &snapshot(&repository),
            HistoryMutationKind::CherryPick,
            feature,
        ))
        .expect("cherry-pick");
        assert!(picked.succeeded);
        let picked_head = picked.snapshot.head.clone().expect("picked head");
        let reverted = mutate_history(request(
            &picked.snapshot,
            HistoryMutationKind::Revert,
            picked_head.clone(),
        ))
        .expect("revert");
        assert!(reverted.succeeded);

        let softened = mutate_history(request(
            &reverted.snapshot,
            HistoryMutationKind::ResetSoft,
            picked_head,
        ))
        .expect("soft reset");
        assert!(softened.snapshot.changes.iter().any(|change| change.staged));
        let hardened = mutate_history(request(
            &softened.snapshot,
            HistoryMutationKind::ResetHard,
            base.clone(),
        ))
        .expect("hard reset");
        assert_eq!(hardened.snapshot.head.as_deref(), Some(base.as_str()));
        assert!(hardened.snapshot.changes.is_empty());
    }

    #[test]
    fn every_history_mutation_uses_a_fixed_git_operation() {
        let target = "0123456789012345678901234567890123456789";
        for kind in [
            HistoryMutationKind::Merge,
            HistoryMutationKind::SquashMerge,
            HistoryMutationKind::Rebase,
            HistoryMutationKind::CherryPick,
            HistoryMutationKind::Revert,
            HistoryMutationKind::ResetSoft,
            HistoryMutationKind::ResetMixed,
            HistoryMutationKind::ResetHard,
        ] {
            let args = arguments(kind, target);
            assert!(args.iter().any(|argument| argument == target));
            assert!(!args.iter().any(|argument| argument.is_empty()));
        }
    }
}
