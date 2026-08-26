use std::collections::HashSet;
use std::ffi::OsString;
use std::path::Path;

use super::command;
use super::models::{
    StashEntry, StashMutationKind, StashMutationRequest, StashMutationResult, StashRequest,
    WorkingCopyRequest,
};
use super::working_copy::{path_from_token, working_copy_snapshot};

const MAX_STASH_MESSAGE_BYTES: usize = 998;

pub fn list_stashes(request: StashRequest) -> Result<Vec<StashEntry>, String> {
    let snapshot = working_copy_snapshot(WorkingCopyRequest {
        repository_path: request.repository_path,
        worktree_path: request.worktree_path,
    })?;
    let output = command::successful_git_at(
        Path::new(&snapshot.worktree_path),
        [
            "stash",
            "list",
            "-z",
            "--format=%gd%x00%H%x00%cI%x00%gs%x00",
        ],
    )
    .map_err(|error| error.to_string())?;
    parse_stashes(&output.stdout)
}

pub fn mutate_stash(request: StashMutationRequest) -> Result<StashMutationResult, String> {
    let before = working_copy_snapshot(WorkingCopyRequest {
        repository_path: request.repository_path.clone(),
        worktree_path: request.worktree_path.clone(),
    })?;
    if before.head != request.expected_head {
        return Err("The working-copy HEAD changed after the stash action was reviewed.".into());
    }
    if before.operation.is_some() {
        return Err(
            "Finish or abort the in-progress Git operation before changing stashes.".into(),
        );
    }
    let worktree = Path::new(&before.worktree_path);
    let mut args = vec![OsString::from("stash")];
    match request.kind {
        StashMutationKind::Push => {
            if !before.changes.iter().any(|change| !change.ignored) {
                return Err("There are no local changes to stash.".into());
            }
            if request.paths.is_empty() {
                return Err("Select at least one changed file to stash.".into());
            }
            let available: HashSet<&str> = before
                .changes
                .iter()
                .filter(|change| !change.ignored)
                .map(|change| change.path.token.as_str())
                .collect();
            let mut selected = Vec::with_capacity(request.paths.len());
            let mut unique = HashSet::with_capacity(request.paths.len());
            for path in &request.paths {
                if !unique.insert(path.token.as_str()) {
                    return Err("The stash selection contains a duplicate path.".into());
                }
                if !available.contains(path.token.as_str()) {
                    return Err(format!(
                        "{:?} is no longer a changed file. Refresh and review the stash selection again.",
                        path.display
                    ));
                }
                selected.push(path_from_token(&path.token)?);
            }
            let message = request
                .message
                .as_deref()
                .map(str::trim)
                .filter(|message| !message.is_empty());
            if let Some(message) = message {
                validate_message(message)?;
            }
            args.push(OsString::from("push"));
            if request.include_untracked {
                args.push(OsString::from("--include-untracked"));
            }
            if let Some(message) = message {
                args.push(OsString::from("--message"));
                args.push(OsString::from(message));
            }
            args.push(OsString::from("--"));
            args.extend(selected);
        }
        StashMutationKind::Apply | StashMutationKind::Pop | StashMutationKind::Drop => {
            let (reference, _) = checked_stash(&request, worktree)?;
            if request.kind != StashMutationKind::Drop
                && before.changes.iter().any(|change| !change.ignored)
            {
                return Err(
                    "Commit or stash current changes before restoring another stash.".into(),
                );
            }
            args.push(OsString::from(match request.kind {
                StashMutationKind::Apply => "apply",
                StashMutationKind::Pop => "pop",
                StashMutationKind::Drop => "drop",
                StashMutationKind::Push => unreachable!(),
            }));
            if request.kind != StashMutationKind::Drop {
                args.push(OsString::from("--index"));
            }
            args.push(OsString::from(reference));
        }
    }

    let output = command::git_at(worktree, args).map_err(|error| error.to_string())?;
    let diagnostic = combined_output(&output.stdout, &output.stderr);
    let snapshot = working_copy_snapshot(WorkingCopyRequest {
        repository_path: before.repository_path.clone(),
        worktree_path: before.worktree_path.clone(),
    })?;
    let conflicted = snapshot.changes.iter().any(|change| change.conflicted);
    if !output.status.success()
        && !((request.kind == StashMutationKind::Apply || request.kind == StashMutationKind::Pop)
            && conflicted)
    {
        return Err(if diagnostic.is_empty() {
            "Git could not complete the stash action and returned no diagnostic output.".into()
        } else {
            diagnostic
        });
    }
    let stashes = list_stashes(StashRequest {
        repository_path: before.repository_path,
        worktree_path: before.worktree_path,
    })?;
    Ok(StashMutationResult {
        snapshot,
        stashes,
        output: diagnostic,
        conflicted,
    })
}

fn checked_stash(
    request: &StashMutationRequest,
    worktree: &Path,
) -> Result<(String, String), String> {
    let index = request
        .stash_index
        .ok_or_else(|| "Select a stash before running this action.".to_string())?;
    let expected = request
        .expected_stash_oid
        .as_deref()
        .ok_or_else(|| "The selected stash has no state fingerprint.".to_string())?;
    if expected.len() < 40
        || expected.len() > 64
        || !expected.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("The selected stash fingerprint is invalid.".into());
    }
    let reference = format!("stash@{{{index}}}");
    let output = command::git_at(worktree, ["rev-parse", "--verify", reference.as_str()])
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err("The selected stash no longer exists. Refresh and try again.".into());
    }
    let actual = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if actual != expected {
        return Err(
            "The stash list changed after this action was reviewed. Refresh and try again.".into(),
        );
    }
    Ok((reference, actual))
}

fn validate_message(message: &str) -> Result<(), String> {
    if message.len() > MAX_STASH_MESSAGE_BYTES || message.contains(['\0', '\n', '\r']) {
        return Err("The stash description must be one line and no more than 998 bytes.".into());
    }
    Ok(())
}

fn parse_stashes(bytes: &[u8]) -> Result<Vec<StashEntry>, String> {
    let fields: Vec<&[u8]> = bytes
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .collect();
    if !fields.len().is_multiple_of(4) {
        return Err("Git returned malformed stash metadata.".into());
    }
    fields
        .as_chunks::<4>()
        .0
        .iter()
        .map(|fields| {
            let reference = String::from_utf8_lossy(fields[0]);
            let index = reference
                .strip_prefix("stash@{")
                .and_then(|value| value.strip_suffix('}'))
                .and_then(|value| value.parse().ok())
                .ok_or_else(|| "Git returned an invalid stash reference.".to_string())?;
            Ok(StashEntry {
                index,
                oid: String::from_utf8_lossy(fields[1]).into_owned(),
                created_at: String::from_utf8_lossy(fields[2]).into_owned(),
                subject: String::from_utf8_lossy(fields[3]).into_owned(),
            })
        })
        .collect()
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
        command::successful_git_at(repository.path(), ["init"]).expect("init");
        command::successful_git_at(repository.path(), ["config", "user.name", "Stash Test"])
            .expect("name");
        command::successful_git_at(
            repository.path(),
            ["config", "user.email", "stash@example.invalid"],
        )
        .expect("email");
        std::fs::write(repository.path().join("tracked.txt"), "initial\n").expect("file");
        command::successful_git_at(repository.path(), ["add", "--", "tracked.txt"]).expect("stage");
        command::successful_git_at(repository.path(), ["commit", "-m", "initial"]).expect("commit");
        repository
    }

    #[test]
    fn pushes_lists_applies_and_pops_a_real_stash() {
        let repository = repository();
        let path = repository.path().to_string_lossy().into_owned();
        std::fs::write(repository.path().join("tracked.txt"), "changed\n").expect("change");
        std::fs::write(repository.path().join("untracked.txt"), "new\n").expect("untracked");
        let before = working_copy_snapshot(WorkingCopyRequest {
            repository_path: path.clone(),
            worktree_path: path.clone(),
        })
        .expect("snapshot");
        let pushed = mutate_stash(StashMutationRequest {
            repository_path: path.clone(),
            worktree_path: path.clone(),
            kind: StashMutationKind::Push,
            message: Some("work in progress".into()),
            include_untracked: true,
            paths: before
                .changes
                .iter()
                .filter(|change| !change.ignored)
                .map(|change| change.path.clone())
                .collect(),
            stash_index: None,
            expected_stash_oid: None,
            expected_head: before.head,
        })
        .expect("push");
        assert_eq!(pushed.stashes.len(), 1);
        assert!(pushed.snapshot.changes.is_empty());
        let stash = pushed.stashes[0].clone();
        let applied = mutate_stash(StashMutationRequest {
            repository_path: path.clone(),
            worktree_path: path.clone(),
            kind: StashMutationKind::Apply,
            message: None,
            include_untracked: false,
            paths: Vec::new(),
            stash_index: Some(stash.index),
            expected_stash_oid: Some(stash.oid.clone()),
            expected_head: pushed.snapshot.head.clone(),
        })
        .expect("apply");
        assert!(!applied.snapshot.changes.is_empty());
        assert_eq!(applied.stashes.len(), 1);
        command::successful_git_at(repository.path(), ["reset", "--hard", "HEAD"]).expect("reset");
        command::successful_git_at(repository.path(), ["clean", "-fd"]).expect("clean");
        let clean = working_copy_snapshot(WorkingCopyRequest {
            repository_path: path.clone(),
            worktree_path: path.clone(),
        })
        .expect("clean snapshot");
        let popped = mutate_stash(StashMutationRequest {
            repository_path: path.clone(),
            worktree_path: path,
            kind: StashMutationKind::Pop,
            message: None,
            include_untracked: false,
            paths: Vec::new(),
            stash_index: Some(stash.index),
            expected_stash_oid: Some(stash.oid),
            expected_head: clean.head,
        })
        .expect("pop");
        assert!(popped.stashes.is_empty());
        assert!(!popped.snapshot.changes.is_empty());
    }

    #[test]
    fn stashes_only_the_reviewed_exact_paths() {
        let repository = repository();
        let path = repository.path().to_string_lossy().into_owned();
        std::fs::write(repository.path().join("tracked.txt"), "selected\n").expect("change");
        std::fs::write(repository.path().join("keep.txt"), "keep\n").expect("keep");
        command::successful_git_at(repository.path(), ["add", "--", "keep.txt"]).expect("add keep");
        command::successful_git_at(repository.path(), ["commit", "-m", "add keep"])
            .expect("commit keep");
        std::fs::write(repository.path().join("keep.txt"), "remaining\n").expect("remaining");
        std::fs::write(repository.path().join("untracked.txt"), "also remaining\n")
            .expect("untracked");
        let before = working_copy_snapshot(WorkingCopyRequest {
            repository_path: path.clone(),
            worktree_path: path.clone(),
        })
        .expect("snapshot");
        let tracked = before
            .changes
            .iter()
            .find(|change| change.path.display == "tracked.txt")
            .expect("tracked change")
            .path
            .clone();
        let result = mutate_stash(StashMutationRequest {
            repository_path: path.clone(),
            worktree_path: path,
            kind: StashMutationKind::Push,
            message: Some("selected file".into()),
            include_untracked: true,
            paths: vec![tracked],
            stash_index: None,
            expected_stash_oid: None,
            expected_head: before.head,
        })
        .expect("selected stash");
        assert_eq!(result.stashes.len(), 1);
        assert!(result
            .snapshot
            .changes
            .iter()
            .any(|change| change.path.display == "keep.txt"));
        assert!(result
            .snapshot
            .changes
            .iter()
            .any(|change| change.path.display == "untracked.txt"));
        assert!(!result
            .snapshot
            .changes
            .iter()
            .any(|change| change.path.display == "tracked.txt"));
    }
}
