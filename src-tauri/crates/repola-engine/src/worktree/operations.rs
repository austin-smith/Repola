use super::command;
use super::models::{
    RepositoryOperation, RepositoryOperationAction, RepositoryOperationMutationResult,
    RepositoryOperationRequest, WorkingCopyRequest,
};
use super::working_copy::working_copy_snapshot;
use std::path::Path;

pub fn mutate_operation(
    request: RepositoryOperationRequest,
) -> Result<RepositoryOperationMutationResult, String> {
    let before = working_copy_snapshot(WorkingCopyRequest {
        repository_path: request.repository_path.clone(),
        worktree_path: request.worktree_path.clone(),
    })?;
    if before.head != request.expected_head {
        return Err(
            "The working-copy HEAD changed after this action was reviewed. Refresh and try again."
                .into(),
        );
    }
    if before.operation != Some(request.expected_operation) {
        return Err("The in-progress Git operation changed after this action was reviewed. Refresh and try again.".into());
    }

    let args = operation_args(request.expected_operation, request.action)?;
    let output = command::git_at(Path::new(&before.worktree_path), args)
        .map_err(|error| error.to_string())?;
    let succeeded = output.status.success();
    let message = combined_output(&output.stdout, &output.stderr);
    let snapshot = working_copy_snapshot(WorkingCopyRequest {
        repository_path: before.repository_path,
        worktree_path: before.worktree_path,
    })?;

    // A failed continuation commonly means Git found another conflict. That is
    // a valid state transition and the refreshed snapshot is more useful than
    // hiding it behind an exception. Failures that leave no operation to
    // recover are surfaced as errors with Git's original diagnostic.
    if !succeeded && snapshot.operation.is_none() {
        return Err(if message.is_empty() {
            "Git could not update the operation and did not report a reason.".into()
        } else {
            message
        });
    }

    Ok(RepositoryOperationMutationResult {
        snapshot,
        succeeded,
        output: message,
    })
}

fn operation_args(
    operation: RepositoryOperation,
    action: RepositoryOperationAction,
) -> Result<Vec<&'static str>, String> {
    use RepositoryOperation as Operation;
    use RepositoryOperationAction as Action;

    let command = match (operation, action) {
        (Operation::Merge, Action::Continue) => vec!["-c", "core.editor=true", "merge", "--continue"],
        (Operation::Merge, Action::Abort) => vec!["merge", "--abort"],
        (Operation::Rebase, Action::Continue) => vec!["-c", "core.editor=true", "rebase", "--continue"],
        (Operation::Rebase, Action::Skip) => vec!["rebase", "--skip"],
        (Operation::Rebase, Action::Abort) => vec!["rebase", "--abort"],
        (Operation::CherryPick, Action::Continue) => vec!["-c", "core.editor=true", "cherry-pick", "--continue"],
        (Operation::CherryPick, Action::Skip) => vec!["cherry-pick", "--skip"],
        (Operation::CherryPick, Action::Abort) => vec!["cherry-pick", "--abort"],
        (Operation::Revert, Action::Continue) => vec!["-c", "core.editor=true", "revert", "--continue"],
        (Operation::Revert, Action::Skip) => vec!["revert", "--skip"],
        (Operation::Revert, Action::Abort) => vec!["revert", "--abort"],
        (Operation::Bisect, Action::Skip) => vec!["bisect", "skip"],
        (Operation::Bisect, Action::Abort) => vec!["bisect", "reset"],
        (_, Action::Skip) => return Err("This Git operation cannot skip the current step.".into()),
        (_, Action::Continue) => return Err("This Git operation needs a more specific decision before it can continue.".into()),
        (Operation::Sequencer, Action::Abort) => return Err("Repola cannot safely infer which sequencer command to abort. Use Git to finish this operation.".into()),
    };
    Ok(command)
}

fn combined_output(stdout: &[u8], stderr: &[u8]) -> String {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    [stdout.trim(), stderr.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn repository() -> tempfile::TempDir {
        let directory = tempfile::tempdir().expect("temp repository");
        command::successful_git_at(directory.path(), ["init"]).expect("initialize");
        command::successful_git_at(directory.path(), ["config", "user.name", "Repola Test"])
            .expect("name");
        command::successful_git_at(
            directory.path(),
            ["config", "user.email", "repola@example.invalid"],
        )
        .expect("email");
        directory
    }

    fn commit(repository: &std::path::Path, contents: &str, message: &str) {
        fs::write(repository.join("file.txt"), contents).expect("write");
        command::successful_git_at(repository, ["add", "file.txt"]).expect("add");
        command::successful_git_at(repository, ["commit", "-m", message]).expect("commit");
    }

    #[test]
    fn continues_and_aborts_reviewed_merge_operations() {
        let repository = repository();
        commit(repository.path(), "base\n", "base");
        command::successful_git_at(repository.path(), ["switch", "-c", "feature"])
            .expect("feature");
        commit(repository.path(), "feature\n", "feature");
        command::successful_git_at(repository.path(), ["switch", "-"]).expect("main");
        commit(repository.path(), "main\n", "main");
        let merge = command::git_at(repository.path(), ["merge", "feature"]).expect("merge");
        assert!(!merge.status.success());

        let conflicted = working_copy_snapshot(WorkingCopyRequest {
            repository_path: repository.path().to_string_lossy().into_owned(),
            worktree_path: repository.path().to_string_lossy().into_owned(),
        })
        .expect("snapshot");
        let aborted = mutate_operation(RepositoryOperationRequest {
            repository_path: conflicted.repository_path.clone(),
            worktree_path: conflicted.worktree_path.clone(),
            action: RepositoryOperationAction::Abort,
            expected_operation: RepositoryOperation::Merge,
            expected_head: conflicted.head,
        })
        .expect("abort");
        assert!(aborted.succeeded);
        assert_eq!(aborted.snapshot.operation, None);

        let merge = command::git_at(repository.path(), ["merge", "feature"]).expect("merge again");
        assert!(!merge.status.success());
        fs::write(repository.path().join("file.txt"), "resolved\n").expect("resolve");
        command::successful_git_at(repository.path(), ["add", "file.txt"]).expect("stage");
        let resolved = working_copy_snapshot(WorkingCopyRequest {
            repository_path: repository.path().to_string_lossy().into_owned(),
            worktree_path: repository.path().to_string_lossy().into_owned(),
        })
        .expect("resolved snapshot");
        let continued = mutate_operation(RepositoryOperationRequest {
            repository_path: resolved.repository_path.clone(),
            worktree_path: resolved.worktree_path.clone(),
            action: RepositoryOperationAction::Continue,
            expected_operation: RepositoryOperation::Merge,
            expected_head: resolved.head,
        })
        .expect("continue");
        assert!(continued.succeeded);
        assert_eq!(continued.snapshot.operation, None);
    }
}
